//! Exposes the fader and the screen over HTTP or a serial port. JSON responses. Same fader API as controller.py.
//!
//! Usage: controller [--mode http]   [--host 0.0.0.0] [--port 8080]
//!        controller  --mode serial  [--device /dev/serial0] [--baud 115200]
//!
//! Serial: one request per line, `GET <path>` or `POST <path> <len>` followed by len bytes of body.
//! Response: one line `<code> <json>`.
//!
//!   /state                          position + motor state
//!   /position                       position only, 0.0-1.0
//!   /move?to=0.3                    closed loop to the position (blocks, max TIMEOUT)
//!   /drive?dir=forward&speed=0.5&ms=200   raw push, stops by itself after ms
//!   /stop                           stops the motor
//!   POST /screen?threshold=128&dither=false&invert=false   body = SVG, drawn centered
//!
//! Serial only (physical access; over HTTP it would mean replacing firmware unauthenticated from the network):
//!   POST /update?name=controller|tac-firmware|service&crc=<crc32> <len>   replaces the installed file
//!   /restart                        daemon-reload and exit; systemd relaunches tac-firmware
//!   /console                        hands the port to a login (agetty); the firmware returns when the session ends
//!   /subscribe?events=manual,motor,position,screen&hz=100   `@ <json>` events, only when something changes
//!   /unsubscribe                    no events (the state at boot and after the other end hangs up)

#[allow(dead_code)]
#[path = "screen/bitmap.rs"]
mod bitmap;
mod fader;
#[allow(dead_code)]
#[path = "screen/oled.rs"]
mod oled;
#[path = "screen/svg.rs"]
mod svg;

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fader::{Adc, Error, Motor};
use oled::{HEIGHT, Oled, WIDTH};
use rppal::uart::{Parity, Uart};

const MAX_DRIVE_MS: u64 = 1000;
const MAX_BODY: usize = 64 * 1024;
const MAX_UPDATE: usize = 8 * 1024 * 1024;
const UPDATE_TARGETS: [(&str, &str, u32); 3] = [
    ("controller", "/usr/local/bin/controller", 0o755),
    ("tac-firmware", "/usr/local/bin/tac-firmware", 0o755),
    ("service", "/etc/systemd/system/tac-firmware.service", 0o644),
];

const EVENTS: [&str; 4] = ["manual", "motor", "position", "screen"];
const DEFAULT_HZ: u32 = 100;
const MAX_HZ: u32 = 200;
const SAMPLE: Duration = Duration::from_millis(5);
// Above ADC noise.
const POS_EPS: f64 = 0.005;
// Inertia after the motor stops: without this margin it would read as a hand.
const MOTOR_GRACE: Duration = Duration::from_millis(50);
const GESTURE_IDLE: Duration = Duration::from_millis(150);

#[derive(Default)]
struct Subs {
    events: Vec<String>,
    hz: u32,
}

impl Subs {
    fn json(&self) -> String {
        let events: Vec<String> = self.events.iter().map(|e| format!(r#""{e}""#)).collect();
        format!(r#"{{"events":[{}],"hz":{}}}"#, events.join(","), self.hz)
    }
}

struct State {
    adc: Mutex<Adc>,
    // One motor command at a time; the second requester gets 409.
    motor: Mutex<Motor>,
    // Each drive/stop bumps the generation; the auto-stop timer only fires if its generation is still current.
    drive_gen: AtomicU64,
    // "Motor running" mirror for the sampler: a try_lock from it would give spurious 409s to /move and /drive.
    running: AtomicBool,
    // None with no screen attached: the fader keeps serving.
    oled: Option<Mutex<Oled>>,
    // Sole writer of the serial port: responses and events never interleave mid-line. None over HTTP.
    out: Mutex<Option<File>>,
    subs: Mutex<Subs>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

impl State {
    fn position(&self) -> Result<f64, Error> {
        self.adc.lock().unwrap().read()
    }

    fn send(&self, line: &str) -> io::Result<()> {
        match self.out.lock().unwrap().as_mut() {
            Some(f) => writeln!(f, "{line}"),
            None => Ok(()),
        }
    }

    fn emit(&self, kind: &str, fields: &str) {
        if !self.subs.lock().unwrap().events.iter().any(|e| e == kind) {
            return;
        }
        if let Err(e) = self.send(&format!(r#"@ {{"event":"{kind}",{fields},"t":{}}}"#, now())) {
            eprintln!("emit {kind}: {e}");
        }
    }

    fn motor_start(&self, fields: &str) {
        self.running.store(true, Ordering::SeqCst);
        self.emit("motor", &format!(r#""phase":"start",{fields}"#));
    }

    fn motor_end(&self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        match self.position() {
            Ok(p) => self.emit("motor", &format!(r#""phase":"end","position":{p}"#)),
            Err(_) => self.emit("motor", r#""phase":"end""#),
        }
    }

    fn state_json(&self) -> Result<String, Error> {
        let pos = self.position()?;
        let (value, busy) = match self.motor.try_lock() {
            Ok(m) => (m.value(), false),
            Err(_) => (f64::NAN, true),
        };
        let direction = if busy || value == 0.0 {
            if busy { "moving" } else { "stopped" }
        } else if value > 0.0 {
            "forward"
        } else {
            "backward"
        };
        let speed = if busy { 0.0 } else { value.abs() };
        let t = now();
        Ok(format!(
            r#"{{"position":{pos},"motor":{{"direction":"{direction}","speed":{speed},"busy":{busy}}},"t":{t}}}"#
        ))
    }

    fn stop(&self, m: &mut Motor) -> Result<(), Error> {
        self.drive_gen.fetch_add(1, Ordering::SeqCst);
        m.stop()?;
        self.motor_end();
        Ok(())
    }
}

type Reply = (u16, String);

fn err(code: u16, msg: &str) -> Reply {
    (code, format!(r#"{{"error":"{}"}}"#, msg.replace('"', "'")))
}

fn param<T: std::str::FromStr>(q: &[(String, String)], key: &str, default: Option<T>) -> Result<T, String> {
    match q.iter().rev().find(|(k, _)| k == key) {
        Some((_, v)) => v.parse().map_err(|_| format!("bad param: {key}={v}")),
        None => default.ok_or_else(|| format!("missing param: {key}")),
    }
}

fn route(st: &Arc<State>, path: &str, q: &[(String, String)]) -> Result<Reply, Error> {
    match path {
        "/" | "/state" => Ok((200, st.state_json()?)),
        "/position" => Ok((200, format!(r#"{{"position":{}}}"#, st.position()?))),
        "/stop" => {
            {
                let mut m = st.motor.lock().unwrap();
                st.stop(&mut m)?;
            }
            Ok((200, st.state_json()?))
        }
        "/move" => {
            let target: f64 = match param(q, "to", None) {
                Ok(v) => v,
                Err(e) => return Ok(err(400, &e)),
            };
            let Ok(mut m) = st.motor.try_lock() else {
                return Ok(err(409, "motor busy"));
            };
            st.stop(&mut m)?;
            st.motor_start(&format!(r#""cmd":"move","target":{target}"#));
            let reached = m.move_to(target, || st.position());
            st.motor_end();
            let reached = reached?;
            drop(m);
            let body = st.state_json()?;
            Ok((200, format!(r#"{{"target":{target},"reached":{reached},{}"#, &body[1..])))
        }
        "/drive" => {
            let dir: String = param(q, "dir", Some("forward".into())).unwrap();
            if dir != "forward" && dir != "backward" {
                return Ok(err(400, "bad param: dir must be forward|backward"));
            }
            let (speed, ms): (f64, u64) = match (param(q, "speed", Some(0.5)), param(q, "ms", Some(200))) {
                (Ok(s), Ok(ms)) => (s, ms),
                (Err(e), _) | (_, Err(e)) => return Ok(err(400, &e)),
            };
            let ms = ms.clamp(1, MAX_DRIVE_MS);
            let Ok(mut m) = st.motor.try_lock() else {
                return Ok(err(409, "motor busy"));
            };
            // Close the previous drive so every start gets its end.
            st.stop(&mut m)?;
            let generation = st.drive_gen.fetch_add(1, Ordering::SeqCst) + 1;
            if dir == "forward" { m.forward(speed)? } else { m.backward(speed)? }
            st.motor_start(&format!(r#""cmd":"drive","direction":"{dir}","speed":{},"ms":{ms}"#, m.value().abs()));
            drop(m);
            let st2 = Arc::clone(st);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                let mut m = st2.motor.lock().unwrap();
                if st2.drive_gen.load(Ordering::SeqCst) == generation {
                    let _ = m.stop();
                    st2.motor_end();
                }
            });
            Ok((200, st.state_json()?))
        }
        _ => Ok(err(404, "not found")),
    }
}

fn screen(st: &State, q: &[(String, String)], body: &[u8]) -> Result<Reply, Error> {
    let Some(oled) = &st.oled else {
        return Ok(err(503, "no screen"));
    };
    let Ok(src) = std::str::from_utf8(body) else {
        return Ok(err(400, "body must be an utf-8 svg"));
    };
    let (threshold, dither, invert): (u8, bool, bool) =
        match (param(q, "threshold", Some(128)), param(q, "dither", Some(false)), param(q, "invert", Some(false))) {
            (Ok(t), Ok(d), Ok(i)) => (t, d, i),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return Ok(err(400, &e)),
        };
    let threshold = if dither { None } else { Some(threshold) };
    let img = match bitmap::load_svg(src, WIDTH, HEIGHT, threshold, invert) {
        Ok(img) => img,
        Err(e) => return Ok(err(400, &e.to_string())),
    };
    let mut o = oled.lock().unwrap();
    o.clear();
    let (x0, y0) = ((WIDTH - img.w) / 2, (HEIGHT - img.h) / 2);
    for y in 0..img.h {
        for x in 0..img.w {
            o.pixel(x0 + x, y0 + y, img.get(x, y));
        }
    }
    o.flush()?;
    drop(o);
    let mut frame = vec![0u8; (WIDTH * HEIGHT / 8) as usize];
    for y in 0..img.h {
        for x in 0..img.w {
            if img.get(x, y) {
                let i = ((y0 + y) * WIDTH + x0 + x) as usize;
                frame[i / 8] |= 0x80 >> (i % 8);
            }
        }
    }
    st.emit("screen", &format!(r#""w":{WIDTH},"h":{HEIGHT},"bitmap":"{}""#, base64(&frame)));
    Ok((200, format!(r#"{{"w":{},"h":{}}}"#, img.w, img.h)))
}

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for (k, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if k <= c.len() { ABC[(n >> shift & 63) as usize] as char } else { '=' });
        }
    }
    out
}

struct Request {
    method: String,
    path: String,
    q: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn new(method: &str, target: &str, body: Vec<u8>) -> Self {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let q = query
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (k.to_string(), v.to_string())
            })
            .collect();
        Request { method: method.to_string(), path: path.to_string(), q, body }
    }
}

fn dispatch(st: &Arc<State>, req: &Result<Request, Reply>) -> Reply {
    let result = match req {
        Err(reply) => Ok(reply.clone()),
        Ok(r) if r.method == "POST" && r.path == "/screen" => screen(st, &r.q, &r.body),
        Ok(r) if r.method == "GET" => route(st, &r.path, &r.q),
        Ok(_) => Ok(err(404, "not found")),
    };
    result.unwrap_or_else(|e| err(500, &e.to_string()))
}

fn check_method(method: &str) -> Result<(), Reply> {
    if method != "GET" && method != "POST" {
        return Err(err(405, "method not allowed"));
    }
    Ok(())
}

fn read_body(reader: &mut impl Read, len: usize, max: usize) -> Result<Vec<u8>, Reply> {
    if len > max {
        return Err(err(413, "body too large"));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).map_err(|_| err(400, "bad request"))?;
    Ok(body)
}

fn parse_http(stream: &TcpStream) -> Result<Request, Reply> {
    let bad = |_| err(400, "bad request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(bad)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(err(400, "bad request"));
    };
    check_method(method)?;
    let mut len = 0;
    // Drain headers so we never close with unread data (RST on some clients).
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).map_err(bad)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().map_err(|_| err(400, "bad content-length"))?;
        }
    }
    let body = read_body(&mut reader, len, MAX_BODY)?;
    Ok(Request::new(method, target, body))
}

fn handle_http(st: Arc<State>, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let req = parse_http(&stream);
    let (code, body) = dispatch(&st, &req);
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Content Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let method = req.as_ref().map_or("?", |r| r.method.as_str());
    println!("{} {method} {code}", stream.peer_addr().map(|a| a.to_string()).unwrap_or_default());
}

fn serve_http(st: Arc<State>, host: &str, port: u16) -> Result<(), Error> {
    let listener = TcpListener::bind((host, port))?;
    println!("controller on http://{host}:{port}  (Ctrl+C to quit)");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let st = Arc::clone(&st);
                thread::spawn(move || handle_http(st, s));
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    Ok(())
}

struct Serial(Uart);

impl Read for Serial {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).map_err(io::Error::other)
    }
}

fn subscribe(st: &State, q: &[(String, String)]) -> Reply {
    let (events, hz): (String, u32) = match (param(q, "events", Some(EVENTS.join(","))), param(q, "hz", Some(DEFAULT_HZ))) {
        (Ok(e), Ok(h)) => (e, h),
        (Err(e), _) | (_, Err(e)) => return err(400, &e),
    };
    let mut subs = Subs { events: Vec::new(), hz: hz.clamp(1, MAX_HZ) };
    for e in events.split(',').filter(|e| !e.is_empty()) {
        if !EVENTS.contains(&e) {
            return err(400, &format!("bad event: {e} (manual|motor|position|screen)"));
        }
        subs.events.push(e.to_string());
    }
    let body = subs.json();
    *st.subs.lock().unwrap() = subs;
    (200, body)
}

/// Emits on change, never on a clock: a still fader sends nothing. Compares against the last value sent,
/// so the hz limit never swallows the final position.
fn sample(st: Arc<State>) {
    let mut anchor = f64::NAN;
    let mut sent = f64::NAN;
    let mut sent_at: Option<Instant> = None;
    let mut last_move = Instant::now();
    let mut last_motor: Option<Instant> = None;
    let mut gesture = false;
    loop {
        thread::sleep(SAMPLE);
        let Ok(p) = st.position() else { continue };
        let now = Instant::now();
        if st.running.load(Ordering::SeqCst) {
            last_motor = Some(now);
        }
        let motor = last_motor.is_some_and(|t| now - t < MOTOR_GRACE);
        if anchor.is_nan() {
            (anchor, sent) = (p, p);
            continue;
        }
        let moved = (p - anchor).abs() > POS_EPS;
        if moved {
            (anchor, last_move) = (p, now);
        }
        if gesture && (motor || now - last_move > GESTURE_IDLE) {
            gesture = false;
            st.emit("manual", &format!(r#""phase":"end","position":{p}"#));
        } else if !gesture && moved && !motor {
            gesture = true;
            st.emit("manual", &format!(r#""phase":"start","position":{p}"#));
        }
        let interval = Duration::from_secs(1) / st.subs.lock().unwrap().hz.max(1);
        if (p - sent).abs() > POS_EPS && sent_at.is_none_or(|t| now - t >= interval) {
            (sent, sent_at) = (p, Some(now));
            let source = if motor { "motor" } else { "manual" };
            st.emit("position", &format!(r#""position":{p},"source":"{source}""#));
        }
    }
}

fn parse_serial(reader: &mut impl BufRead, line: &str) -> Result<Request, Reply> {
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(err(400, "bad request"));
    };
    check_method(method)?;
    let len = match parts.next() {
        Some(n) => n.parse().map_err(|_| err(400, "bad length"))?,
        None => 0,
    };
    let max = if target.starts_with("/update") { MAX_UPDATE } else { MAX_BODY };
    let body = read_body(reader, len, max)?;
    Ok(Request::new(method, target, body))
}

/// Writes alongside and renames: an interrupted upload leaves the previous file, not a truncated one.
fn update(r: &Request) -> Result<Reply, Error> {
    let (name, crc): (String, u32) = match (param(&r.q, "name", None), param(&r.q, "crc", None)) {
        (Ok(n), Ok(c)) => (n, c),
        (Err(e), _) | (_, Err(e)) => return Ok(err(400, &e)),
    };
    let Some(&(_, path, mode)) = UPDATE_TARGETS.iter().find(|(n, ..)| *n == name) else {
        return Ok(err(400, "bad param: name must be controller|tac-firmware|service"));
    };
    let got = crc32fast::hash(&r.body);
    if got != crc {
        return Ok(err(400, &format!("crc mismatch: got {got}")));
    }
    let tmp = format!("{path}.new");
    let mut f = File::create(&tmp)?;
    f.write_all(&r.body)?;
    f.set_permissions(Permissions::from_mode(mode))?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok((200, format!(r#"{{"name":"{name}","bytes":{}}}"#, r.body.len())))
}

/// exit skips drop, so the pins are never released: stop the motor by hand first.
fn restart(st: &State) -> ! {
    if let Ok(mut m) = st.motor.lock() {
        let _ = st.stop(&mut m);
    }
    let _ = Command::new("systemctl").arg("daemon-reload").status();
    std::process::exit(0);
}

/// agetty becomes the service process: it exits when the session ends and Restart=always brings the firmware back.
fn console(st: &State, device: &str, baud: u32) -> Error {
    if let Ok(mut m) = st.motor.lock() {
        let _ = st.stop(&mut m);
    }
    let tty = device.strip_prefix("/dev/").unwrap_or(device);
    Command::new("/sbin/agetty").args(["-L", tty, &baud.to_string(), "vt100"]).exec().into()
}

fn open_serial(st: &State, device: &str, baud: u32) -> Result<BufReader<Serial>, Error> {
    let mut uart = Uart::with_path(device, baud, Parity::None, 8, 1)?;
    uart.set_read_mode(1, Duration::ZERO)?;
    // Second fd so several threads can write. O_NOCTTY: without it the tty becomes the controlling terminal
    // and a host hangup sends SIGHUP to the service.
    let out = OpenOptions::new().write(true).custom_flags(libc::O_NOCTTY).open(device)?;
    *st.out.lock().unwrap() = Some(out);
    Ok(BufReader::new(Serial(uart)))
}

fn serve_serial(st: Arc<State>, device: &str, baud: u32) -> Result<(), Error> {
    let mut reader = open_serial(&st, device, baud)?;
    let st2 = Arc::clone(&st);
    thread::spawn(move || sample(st2));
    println!("controller on {device} @ {baud}  (Ctrl+C to quit)");
    let mut raw = Vec::new();
    loop {
        raw.clear();
        // 0 bytes = hangup (USB host gone, a getty's vhangup): that fd never recovers, so reopen.
        if reader.read_until(b'\n', &mut raw)? == 0 {
            *st.subs.lock().unwrap() = Subs::default();
            thread::sleep(Duration::from_millis(200));
            reader = open_serial(&st, device, baud)?;
            println!("reopened {device}");
            continue;
        }
        // Lossy: line noise must not kill the loop.
        let line = String::from_utf8_lossy(&raw);
        if line.trim().is_empty() {
            continue;
        }
        let req = parse_serial(&mut reader, &line);
        let path = req.as_ref().map_or("", |r| r.path.as_str());
        let (code, body) = match &req {
            Ok(r) if r.method == "POST" && path == "/update" => update(r).unwrap_or_else(|e| err(500, &e.to_string())),
            Ok(_) if path == "/restart" => (200, r#"{"restarting":true}"#.to_string()),
            Ok(_) if path == "/console" => (200, r#"{"console":true}"#.to_string()),
            Ok(r) if path == "/subscribe" => subscribe(&st, &r.q),
            Ok(_) if path == "/unsubscribe" => {
                *st.subs.lock().unwrap() = Subs::default();
                (200, Subs::default().json())
            }
            _ => dispatch(&st, &req),
        };
        st.send(&format!("{code} {body}"))?;
        let method = req.as_ref().map_or("?", |r| r.method.as_str());
        println!("serial {method} {path} {code}");
        if path == "/restart" || path == "/console" {
            let _ = reader.get_ref().0.drain();
        }
        if path == "/restart" {
            restart(&st);
        }
        if path == "/console" {
            println!("console on {device}");
            return Err(console(&st, device, baud));
        }
    }
}

fn main() -> Result<(), Error> {
    let mut mode = "http".to_string();
    let mut host = "0.0.0.0".to_string();
    let mut port: u16 = 8080;
    let mut device = "/dev/serial0".to_string();
    let mut baud: u32 = 115_200;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--mode" => mode = args.next().ok_or("--mode needs a value")?,
            "--host" => host = args.next().ok_or("--host needs a value")?,
            "--port" => port = args.next().ok_or("--port needs a value")?.parse()?,
            "--device" => device = args.next().ok_or("--device needs a value")?,
            "--baud" => baud = args.next().ok_or("--baud needs a value")?.parse()?,
            _ => return Err(format!("unknown arg {a}").into()),
        }
    }
    if mode != "http" && mode != "serial" {
        return Err(format!("bad --mode {mode}: http|serial").into());
    }
    let st = Arc::new(State {
        adc: Mutex::new(Adc::new()?),
        motor: Mutex::new(Motor::new()?),
        drive_gen: AtomicU64::new(0),
        running: AtomicBool::new(false),
        oled: Oled::new().inspect_err(|e| eprintln!("no screen ({e}), /screen answers 503")).ok().map(Mutex::new),
        out: Mutex::new(None),
        subs: Mutex::new(Subs::default()),
    });
    if mode == "serial" { serve_serial(st, &device, baud) } else { serve_http(st, &host, port) }
}
