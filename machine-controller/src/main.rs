//! Servidor HTTP que expone el fader y la pantalla. Respuestas JSON. Misma API de fader que controller.py.
//!
//! Uso: controller [--host 0.0.0.0] [--port 8080]
//!
//!   /state                          posicion + estado del motor
//!   /position                       solo posicion 0.0-1.0
//!   /move?to=0.3                    bucle cerrado hasta la posicion (bloquea, max TIMEOUT)
//!   /drive?dir=forward&speed=0.5&ms=200   empuje crudo, se para solo a los ms
//!   /stop                           para el motor
//!   POST /screen?threshold=128&dither=false&invert=false   body = SVG, lo pinta centrado

#[allow(dead_code)]
#[path = "screen/bitmap.rs"]
mod bitmap;
mod fader;
#[allow(dead_code)]
#[path = "screen/oled.rs"]
mod oled;
#[path = "screen/svg.rs"]
mod svg;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fader::{Adc, Error, Motor};
use oled::{HEIGHT, Oled, WIDTH};

const MAX_DRIVE_MS: u64 = 1000;
const MAX_BODY: usize = 64 * 1024;

struct State {
    adc: Mutex<Adc>,
    // Un solo mando sobre el motor a la vez; el segundo peticionario recibe 409.
    motor: Mutex<Motor>,
    // Cada drive/stop avanza la generacion; el timer de auto-stop solo actua si sigue vigente.
    drive_gen: AtomicU64,
    // None sin pantalla conectada: el fader sigue sirviendo.
    oled: Option<Mutex<Oled>>,
}

impl State {
    fn position(&self) -> Result<f64, Error> {
        self.adc.lock().unwrap().read()
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
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        Ok(format!(
            r#"{{"position":{pos},"motor":{{"direction":"{direction}","speed":{speed},"busy":{busy}}},"t":{t}}}"#
        ))
    }

    fn stop(&self, m: &mut Motor) -> Result<(), Error> {
        self.drive_gen.fetch_add(1, Ordering::SeqCst);
        m.stop()
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
            let reached = m.move_to(target, || st.position())?;
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
            let generation = st.drive_gen.fetch_add(1, Ordering::SeqCst) + 1;
            if dir == "forward" { m.forward(speed)? } else { m.backward(speed)? }
            drop(m);
            let st2 = Arc::clone(st);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                let mut m = st2.motor.lock().unwrap();
                if st2.drive_gen.load(Ordering::SeqCst) == generation {
                    let _ = m.stop();
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
    Ok((200, format!(r#"{{"w":{},"h":{}}}"#, img.w, img.h)))
}

struct Request {
    method: String,
    path: String,
    q: Vec<(String, String)>,
    body: Vec<u8>,
}

fn parse_request(stream: &TcpStream) -> Result<Request, Reply> {
    let bad = |_| err(400, "bad request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(bad)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(err(400, "bad request"));
    };
    if method != "GET" && method != "POST" {
        return Err(err(405, "method not allowed"));
    }
    let mut len = 0;
    // Drena cabeceras para no cerrar con datos sin leer (RST en algunos clientes).
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
    if len > MAX_BODY {
        return Err(err(413, "body too large"));
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).map_err(bad)?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), v.to_string())
        })
        .collect();
    Ok(Request { method: method.to_string(), path: path.to_string(), q, body })
}

fn handle(st: Arc<State>, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let req = parse_request(&stream);
    let result = match &req {
        Err(reply) => Ok(reply.clone()),
        Ok(r) if r.method == "POST" && r.path == "/screen" => screen(&st, &r.q, &r.body),
        Ok(r) if r.method == "GET" => route(&st, &r.path, &r.q),
        Ok(_) => Ok(err(404, "not found")),
    };
    let (code, body) = result.unwrap_or_else(|e| err(500, &e.to_string()));
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

fn main() -> Result<(), Error> {
    let mut host = "0.0.0.0".to_string();
    let mut port: u16 = 8080;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--host" => host = args.next().ok_or("--host needs a value")?,
            "--port" => port = args.next().ok_or("--port needs a value")?.parse()?,
            _ => return Err(format!("unknown arg {a}").into()),
        }
    }
    let st = Arc::new(State {
        adc: Mutex::new(Adc::new()?),
        motor: Mutex::new(Motor::new()?),
        drive_gen: AtomicU64::new(0),
        oled: Oled::new().inspect_err(|e| eprintln!("sin pantalla ({e}), /screen responde 503")).ok().map(Mutex::new),
    });
    let listener = TcpListener::bind((host.as_str(), port))?;
    println!("controller en http://{host}:{port}  (Ctrl+C para salir)");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let st = Arc::clone(&st);
                thread::spawn(move || handle(st, s));
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    Ok(())
}
