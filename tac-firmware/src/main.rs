//! Arranque de la maquina: logo, netplan apply barriendo el fader hasta tener IP, IP en pantalla con el
//! fader en 0.5; al bajarlo a 0 limpia la pantalla y se reemplaza (exec) por el controller.
//!
//! Uso: tac-firmware [controller [args...]]    (defecto /usr/local/bin/controller)

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
const SWEEP_SPEED: f64 = 0.5;
// Sin esto, un ADC que no llega al limite deja el motor empujando contra el tope.
const LEG_TIMEOUT: Duration = Duration::from_secs(2);
const IP_POLL: Duration = Duration::from_millis(500);
const PARK: f64 = 0.5;
const RELEASE_AT: f64 = 0.02;

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let mut controller = Command::new(args.next().unwrap_or_else(|| CONTROLLER.into()));
    controller.args(args);

    let mut oled = Oled::new().inspect_err(|e| eprintln!("sin pantalla ({e})")).ok();
    let mut adc = Adc::new()?;
    let mut motor = Motor::new()?;

    if let Some(o) = &mut oled {
        draw_logo(o)?;
    }

    let ip = sweep_until_ip(&mut motor, &mut adc)?;
    println!("ip {ip}");
    if let Some(o) = &mut oled {
        draw_ip(o, &ip)?;
    }
    motor.move_to(PARK, || adc.read())?;

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

/// netplan apply reinicia la red: una IP vista antes de que termine puede ser la vieja.
fn sweep_until_ip(motor: &mut Motor, adc: &mut Adc) -> Result<String, Error> {
    let apply = thread::spawn(|| match Command::new("netplan").arg("apply").status() {
        Ok(s) if s.success() => println!("netplan apply ok"),
        Ok(s) => eprintln!("netplan apply: {s}"),
        Err(e) => eprintln!("netplan apply: {e}"),
    });

    motor.forward(SWEEP_SPEED)?;
    let mut leg = Instant::now();
    let mut polled: Option<Instant> = None;
    loop {
        let pos = adc.read()?;
        let forward = motor.value() > 0.0;
        let stale = leg.elapsed() >= LEG_TIMEOUT;
        if forward && (pos >= SWEEP_MAX || stale) {
            motor.backward(SWEEP_SPEED)?;
            leg = Instant::now();
        } else if !forward && (pos <= SWEEP_MIN || stale) {
            motor.forward(SWEEP_SPEED)?;
            leg = Instant::now();
        }

        if apply.is_finished() && polled.is_none_or(|t| t.elapsed() >= IP_POLL) {
            polled = Some(Instant::now());
            if let Some(ip) = ip() {
                motor.stop()?;
                return Ok(ip);
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
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

fn draw_ip(o: &mut Oled, ip: &str) -> Result<(), Error> {
    let scale = if Oled::text_width(ip, 2) <= WIDTH { 2 } else { 1 };
    o.clear();
    o.text((WIDTH - Oled::text_width(ip, scale)) / 2, (HEIGHT - 7 * scale) / 2, ip, scale, true);
    o.flush()
}
