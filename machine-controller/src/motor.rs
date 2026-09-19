//! Prueba del motor sin ADC, lazo abierto. Mismos pines que fader.rs (L293D en 13/19/26).
//!
//! Uso: cargo run --bin motor -- 20        (posicion 0-100)
//!      cargo run --bin motor -- forward 0.5 300   (empuje crudo: dir, speed, ms)
//!
//! Sin ADC la posicion es estimada: homing hacia atras HOMING_MS (tope inferior) y
//! luego forward pct% de FULL_TRAVEL_MS. Ajusta FULL_TRAVEL_MS midiendo el recorrido real.

#[allow(dead_code)]
#[path = "fader.rs"]
mod fader;

use std::thread;
use std::time::Duration;

use fader::{Error, Motor};

const HOMING_MS: u64 = 1200;
const FULL_TRAVEL_MS: u64 = 900;
const SPEED: f64 = 0.7;

fn usage() -> Error {
    "uso: motor <0-100> | motor <forward|backward> [speed 0-1] [ms]".into()
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut m = Motor::new()?;

    match args.first().map(String::as_str) {
        Some("forward") | Some("backward") => {
            let speed: f64 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(0.5);
            let ms: u64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(200);
            println!("{} speed={speed} {ms}ms", args[0]);
            if args[0] == "forward" { m.forward(speed)? } else { m.backward(speed)? }
            thread::sleep(Duration::from_millis(ms));
        }
        Some(p) => {
            let pct: u64 = p.parse().map_err(|_| usage())?;
            if pct > 100 {
                return Err(usage());
            }
            println!("homing backward {HOMING_MS}ms");
            m.backward(SPEED)?;
            thread::sleep(Duration::from_millis(HOMING_MS));
            m.stop()?;
            thread::sleep(Duration::from_millis(150));

            let ms = FULL_TRAVEL_MS * pct / 100;
            println!("forward {ms}ms (~{pct}%)");
            if ms > 0 {
                m.forward(SPEED)?;
                thread::sleep(Duration::from_millis(ms));
            }
        }
        None => return Err(usage()),
    }

    m.stop()?;
    println!("stop");
    Ok(())
}
