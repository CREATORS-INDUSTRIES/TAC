//! Motor test without the ADC. L293D: IN1 GPIO19, IN2 GPIO26, EN GPIO13 (held high, no PWM).
//!
//! Usage: cargo run --bin motor -- 20    (0 = far left, 100 = far right)
//!
//! Without the ADC, position is by time: first left to the end stop, then right for
//! pct% of FULL_TRAVEL_MS. Tune FULL_TRAVEL_MS by measuring the real travel.

use std::thread;
use std::time::Duration;

use rppal::gpio::Gpio;

const HOMING_MS: u64 = 1200;
const FULL_TRAVEL_MS: u64 = 900;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pct: u64 = std::env::args().nth(1).ok_or("usage: motor <0-100>")?.parse()?;
    if pct > 100 {
        return Err("usage: motor <0-100>".into());
    }

    let gpio = Gpio::new()?;
    let mut in1 = gpio.get(19)?.into_output_low();
    let mut in2 = gpio.get(26)?.into_output_low();
    let mut en = gpio.get(13)?.into_output_low();

    println!("left {HOMING_MS}ms");
    in1.set_low();
    in2.set_high();
    en.set_high();
    thread::sleep(Duration::from_millis(HOMING_MS));
    en.set_low();
    thread::sleep(Duration::from_millis(150));

    let ms = FULL_TRAVEL_MS * pct / 100;
    println!("right {ms}ms (~{pct}%)");
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
