//! Demo SSD1306.
//!
//! Uso: screen              cara que mira hacia el fader + barra de posicion (Ctrl+C para salir)
//!      screen hola mundo   pinta el texto centrado y sale; la pantalla lo conserva
//!
//! Sin ADS1115 conectado la cara barre sola.

#[allow(dead_code)]
#[path = "../fader.rs"]
mod fader;
#[allow(dead_code)]
mod oled;

use std::thread;
use std::time::{Duration, Instant};

use fader::{Adc, Error};
use oled::{HEIGHT, Oled, WIDTH};

const FPS: u64 = 30;
const BLINK_EVERY: Duration = Duration::from_millis(3500);
const BLINK_FOR: Duration = Duration::from_millis(140);

fn main() -> Result<(), Error> {
    let mut o = Oled::new()?;
    let words: Vec<String> = std::env::args().skip(1).collect();
    if !words.is_empty() {
        draw_text(&mut o, &words.join(" "));
        return o.flush();
    }

    let mut adc = Adc::new().inspect_err(|e| eprintln!("sin ADC ({e}), modo barrido")).ok();
    let t0 = Instant::now();
    loop {
        let t = t0.elapsed();
        let pos = match adc.as_mut() {
            Some(a) => a.read()?,
            None => (t.as_secs_f64() * 1.2).sin() * 0.5 + 0.5,
        };
        let blinking = t.as_millis() % BLINK_EVERY.as_millis() < BLINK_FOR.as_millis();
        o.clear();
        draw_face(&mut o, pos, blinking);
        draw_bar(&mut o, pos);
        o.flush()?;
        thread::sleep(Duration::from_millis(1000 / FPS));
    }
}

fn draw_face(o: &mut Oled, look: f64, blinking: bool) {
    let dx = ((look - 0.5) * 12.0).round() as i32;
    for cx in [40, 88] {
        let cy = 22;
        if blinking {
            o.fill_rect(cx - 12, cy - 1, 25, 3, true);
            continue;
        }
        o.fill_circle(cx, cy, 12, true);
        o.fill_circle(cx + dx, cy, 5, false);
    }
    o.line(56, 42, 64, 45, true);
    o.line(64, 45, 72, 42, true);
}

fn draw_bar(o: &mut Oled, pos: f64) {
    let (x, y, w, h) = (2, HEIGHT - 12, 96, 10);
    o.rect(x, y, w, h, true);
    o.fill_rect(x + 2, y + 2, ((w - 4) as f64 * pos).round() as i32, h - 4, true);
    let label = format!("{:3.0}%", pos * 100.0);
    o.text(WIDTH - Oled::text_width(&label, 1) - 1, y + 2, &label, 1, true);
}

/// Parte por palabras; escala 2 si cabe en pantalla, si no escala 1.
fn draw_text(o: &mut Oled, s: &str) {
    let scale = if wrap(s, 2).len() as i32 * 16 <= HEIGHT { 2 } else { 1 };
    let lines = wrap(s, scale);
    let line_h = 8 * scale;
    let mut y = (HEIGHT - lines.len() as i32 * line_h) / 2;
    for l in &lines {
        o.text((WIDTH - Oled::text_width(l, scale)) / 2, y, l, scale, true);
        y += line_h;
    }
}

fn wrap(s: &str, scale: i32) -> Vec<String> {
    let max = (WIDTH / (6 * scale)) as usize;
    let mut lines = vec![String::new()];
    for w in s.split_whitespace() {
        let cur = lines.last_mut().unwrap();
        if !cur.is_empty() && cur.len() + 1 + w.len() > max {
            lines.push(w.to_string());
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        }
    }
    lines
}
