//! Machine boot: logo, netplan apply while sweeping the fader until there is an IP, IP on screen with the
//! fader at 0.5; pulling it down to 0 clears the screen and replaces itself (exec) with the controller.
//! With `--mode serial` among the controller args there is no network to wait for: logo and straight exec.
//!
//! Usage: tac-firmware [controller [args...]]    (default /usr/local/bin/controller)

#[allow(dead_code)]
#[path = "../../hal/src/screen/bitmap.rs"]
mod bitmap;
#[allow(dead_code)]
#[path = "../../hal/src/fader.rs"]
mod fader;
#[allow(dead_code)]
#[path = "../../hal/src/screen/oled.rs"]
mod oled;
#[path = "../../hal/src/screen/svg.rs"]
mod svg;

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use fader::{Adc, Error, Motor};
use oled::{HEIGHT, Oled, WIDTH};

const LOGO: &str = include_str!("../assets/creators.svg");
const CONTROLLER: &str = "/usr/local/bin/controller";

const SWEEP_MIN: f64 = 0.3;
const SWEEP_MAX: f64 = 0.7;
// Measured with motor.rs. Cap per stroke: if the ADC misses the limit, don't push against the end stop.
const FULL_TRAVEL: Duration = Duration::from_millis(900);
// Reversing abruptly at full speed is a current spike on the L293D.
const DWELL: Duration = Duration::from_millis(120);
const PARK: f64 = 0.5;
const RELEASE_AT: f64 = 0.02;

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let mut controller = Command::new(args.next().unwrap_or_else(|| CONTROLLER.into()));
    let args: Vec<String> = args.collect();
    let serial = args.windows(2).any(|w| w[0] == "--mode" && w[1] == "serial");
    controller.args(&args);

    let mut oled = Oled::new().inspect_err(|e| eprintln!("no screen ({e})")).ok();
    if let Some(o) = &mut oled {
        draw_logo(o)?;
    }
    if serial {
        drop(oled);
        println!("exec {controller:?}");
        return Err(controller.exec().into());
    }

    let mut adc = Adc::new()?;
    let mut motor = Motor::new()?;
    let ip = sweep_until_ip(&mut motor, &mut adc)?;
    println!("ip {ip}");
    if let Some(o) = &mut oled {
        draw_ip(o, &ip)?;
    }
    drive_to(&mut motor, &mut adc, PARK)?;

    while adc.read()? > RELEASE_AT {
        thread::sleep(Duration::from_millis(20));
    }
    if let Some(o) = &mut oled {
        o.clear();
        o.flush()?;
    }

    drop((motor, adc, oled));
    println!("exec {controller:?}");
    Err(controller.exec().into())
}

/// netplan apply restarts networking: an IP seen before it finishes may be the old one.
fn sweep_until_ip(motor: &mut Motor, adc: &mut Adc) -> Result<String, Error> {
    let apply = thread::spawn(|| match Command::new("netplan").arg("apply").status() {
        Ok(s) if s.success() => println!("netplan apply ok"),
        Ok(s) => eprintln!("netplan apply: {s}"),
        Err(e) => eprintln!("netplan apply: {e}"),
    });

    let mut target = SWEEP_MAX;
    loop {
        drive_to(motor, adc, target)?;
        if apply.is_finished()
            && let Some(ip) = ip()
        {
            return Ok(ip);
        }
        thread::sleep(DWELL);
        target = if target == SWEEP_MAX { SWEEP_MIN } else { SWEEP_MAX };
    }
}

/// Always at speed 1.0: with the L293D at 5V, software PWM falls short and the motor won't start.
fn drive_to(motor: &mut Motor, adc: &mut Adc, target: f64) -> Result<f64, Error> {
    let forward = target > adc.read()?;
    if forward { motor.forward(1.0)? } else { motor.backward(1.0)? }
    let t0 = Instant::now();
    let pos = loop {
        let pos = adc.read()?;
        if forward == (pos >= target) {
            break pos;
        }
        if t0.elapsed() >= FULL_TRAVEL {
            eprintln!("drive_to {target}: timeout at {pos:.3}");
            break pos;
        }
        thread::sleep(Duration::from_millis(1));
    };
    motor.stop()?;
    Ok(pos)
}

fn ip() -> Option<String> {
    let out = Command::new("ip").args(["-4", "-o", "addr", "show", "scope", "global"]).output().ok()?;
    let out = String::from_utf8(out.stdout).ok()?;
    let cidr = out.split_whitespace().skip_while(|w| *w != "inet").nth(1)?;
    cidr.split('/').next().map(str::to_string)
}

fn draw_logo(o: &mut Oled) -> Result<(), Error> {
    let img = bitmap::load_svg(LOGO, WIDTH, HEIGHT, Some(128), false)?;
    let (x0, y0) = ((WIDTH - img.w) / 2, (HEIGHT - img.h) / 2);
    o.clear();
    for y in 0..img.h {
        for x in 0..img.w {
            o.pixel(x0 + x, y0 + y, img.get(x, y));
        }
    }
    o.flush()
}

/// Two octets per line: on a single line any IP over 10 characters drops to scale 1.
fn draw_ip(o: &mut Oled, ip: &str) -> Result<(), Error> {
    let mut dots = ip.match_indices('.').map(|(i, _)| i);
    let lines = match dots.nth(1) {
        Some(i) => vec![&ip[..i], &ip[i + 1..]],
        None => vec![ip],
    };
    let block_h = |s: i32| lines.len() as i32 * 9 * s - 2 * s;
    let scale = (1..=8)
        .rev()
        .find(|&s| block_h(s) <= HEIGHT && lines.iter().all(|l| Oled::text_width(l, s) <= WIDTH))
        .unwrap_or(1);

    o.clear();
    let mut y = (HEIGHT - block_h(scale)) / 2;
    for l in lines {
        o.text((WIDTH - Oled::text_width(l, scale)) / 2, y, l, scale, true);
        y += 9 * scale;
    }
    o.flush()
}
