//! Motorized fader: ADS1115 on I2C-1 (A0) + L293D over GPIO.
//!
//! Pi Zero 2W pins (BCM / physical):
//!   GPIO2  / 3   SDA1   -> ADS1115 SDA
//!   GPIO3  / 5   SCL1   -> ADS1115 SCL
//!   GPIO13 / 33  PWM/EN -> L293D 1-2EN (pin 1)
//!   GPIO19 / 35  IN1    -> L293D 1A    (pin 2)  forward
//!   GPIO26 / 37  IN2    -> L293D 2A    (pin 7)  backward

use std::thread;
use std::time::{Duration, Instant};

use rppal::gpio::{Gpio, OutputPin};
use rppal::i2c::I2c;

const ADDR: u16 = 0x48;
const REG_CONV: u8 = 0x00;
const REG_CONFIG: u8 = 0x01;
// A0 single-ended, +-4.096V, continuous, 860 SPS, comparator off
const CONFIG: u16 = 0xC2E3;
const FS_VOLTS: f64 = 4.096;
const VREF: f64 = 3.3;

// Fraction of VREF the wiper reads at each end stop; measure on the real fader
const POS_MIN: f64 = 0.005;
const POS_MAX: f64 = 0.985;

const PWM_HZ: f64 = 10_000.0;
const KP: f64 = 6.0;
const DEADBAND: f64 = 0.008;
const MIN_PWM: f64 = 0.35;
pub const TIMEOUT: Duration = Duration::from_millis(1500);

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct Adc {
    i2c: I2c,
}

impl Adc {
    pub fn new() -> Result<Self, Error> {
        let mut i2c = I2c::with_bus(1)?;
        i2c.set_slave_address(ADDR)?;
        i2c.write(&[REG_CONFIG, (CONFIG >> 8) as u8, CONFIG as u8])?;
        thread::sleep(Duration::from_millis(50));
        Ok(Self { i2c })
    }

    /// Normalized position 0.0-1.0.
    pub fn read(&mut self) -> Result<f64, Error> {
        let mut buf = [0u8; 2];
        self.i2c.write_read(&[REG_CONV], &mut buf)?;
        let raw = i16::from_be_bytes(buf) as f64;
        let v = raw * FS_VOLTS / 32768.0 / VREF;
        Ok(((v - POS_MIN) / (POS_MAX - POS_MIN)).clamp(0.0, 1.0))
    }
}

pub struct Motor {
    fwd: OutputPin,
    bwd: OutputPin,
    en: OutputPin,
    /// Signed speed, like gpiozero Motor.value: >0 forward, <0 backward, 0 stopped.
    value: f64,
}

impl Motor {
    pub fn new() -> Result<Self, Error> {
        Self::with_pins(19, 26, 13)
    }

    pub fn with_pins(fwd: u8, bwd: u8, en: u8) -> Result<Self, Error> {
        let gpio = Gpio::new()?;
        let mut m = Self {
            fwd: gpio.get(fwd)?.into_output_low(),
            bwd: gpio.get(bwd)?.into_output_low(),
            en: gpio.get(en)?.into_output_low(),
            value: 0.0,
        };
        m.stop()?;
        Ok(m)
    }

    /// Speed 1.0 skips software PWM: a fixed level, free of rppal thread jitter.
    fn enable(&mut self, speed: f64) -> Result<(), Error> {
        if speed >= 1.0 {
            self.en.clear_pwm()?;
            self.en.set_high();
        } else {
            self.en.set_pwm_frequency(PWM_HZ, speed)?;
        }
        Ok(())
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn forward(&mut self, speed: f64) -> Result<(), Error> {
        let speed = speed.clamp(0.0, 1.0);
        self.bwd.set_low();
        self.fwd.set_high();
        self.enable(speed)?;
        self.value = speed;
        Ok(())
    }

    pub fn backward(&mut self, speed: f64) -> Result<(), Error> {
        let speed = speed.clamp(0.0, 1.0);
        self.fwd.set_low();
        self.bwd.set_high();
        self.enable(speed)?;
        self.value = -speed;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        self.en.clear_pwm()?;
        self.en.set_low();
        self.fwd.set_low();
        self.bwd.set_low();
        self.value = 0.0;
        Ok(())
    }

    /// Closed P loop to `target` or TIMEOUT. `read` is called per iteration so the ADC is not
    /// held while the motor moves.
    pub fn move_to(&mut self, target: f64, mut read: impl FnMut() -> Result<f64, Error>) -> Result<f64, Error> {
        let target = target.clamp(0.0, 1.0);
        let t0 = Instant::now();
        while t0.elapsed() < TIMEOUT {
            let err = target - read()?;
            if err.abs() < DEADBAND {
                break;
            }
            let speed = (err.abs() * KP).clamp(MIN_PWM, 1.0);
            if err > 0.0 {
                self.forward(speed)?;
            } else {
                self.backward(speed)?;
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.stop()?;
        read()
    }
}

impl Drop for Motor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
