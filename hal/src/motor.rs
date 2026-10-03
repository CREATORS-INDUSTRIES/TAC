//! Prueba del motor sin ADC. L293D: IN1 GPIO19, IN2 GPIO26, EN GPIO13 (alto fijo, sin PWM).
//!
//! Uso: cargo run --bin motor -- 20    (0 = izquierda del todo, 100 = derecha del todo)
//!
//! Sin ADC la posicion es por tiempo: primero va a la izquierda hasta el tope y luego
//! a la derecha pct% de FULL_TRAVEL_MS. Ajusta FULL_TRAVEL_MS midiendo el recorrido real.

use std::thread;
use std::time::Duration;

use rppal::gpio::Gpio;

const HOMING_MS: u64 = 1200;
const FULL_TRAVEL_MS: u64 = 900;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pct: u64 = std::env::args().nth(1).ok_or("uso: motor <0-100>")?.parse()?;
    if pct > 100 {
        return Err("uso: motor <0-100>".into());
    }

    let gpio = Gpio::new()?;
    let mut in1 = gpio.get(19)?.into_output_low();
    let mut in2 = gpio.get(26)?.into_output_low();
    let mut en = gpio.get(13)?.into_output_low();

    println!("izquierda {HOMING_MS}ms");
    in1.set_low();
    in2.set_high();
    en.set_high();
    thread::sleep(Duration::from_millis(HOMING_MS));
    en.set_low();
    thread::sleep(Duration::from_millis(150));

    let ms = FULL_TRAVEL_MS * pct / 100;
    println!("derecha {ms}ms (~{pct}%)");
    in2.set_low();
    in1.set_high();
    en.set_high();
    thread::sleep(Duration::from_millis(ms));

    en.set_low();
    in1.set_low();
    in2.set_low();
    println!("stop");
    Ok(())
}
