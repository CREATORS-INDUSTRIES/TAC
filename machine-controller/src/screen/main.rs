//! Demo SSD1306.
//!
//! Uso: screen              cara que mira hacia el fader + barra de posicion (Ctrl+C para salir)
//!      screen hola mundo   pinta el texto y sale; la pantalla lo conserva
//!      screen --png logo.png [--fit] [--threshold 0-255] [--invert]
//!                          pinta el PNG y sale; sin --threshold usa dithering, sin --fit no amplia
//!      screen --svg '<svg ...>' [--threshold 0-255 | --dither] [--invert]
//!                          pinta el SVG ajustado a pantalla; "-" lo lee de stdin; umbral 128 por defecto
//!                          sin <text>: resvg va sin fuentes
//!
//!      --align top-left | top-center | ... | center | ... | bottom-right   (defecto center)
//!                          coloca PNG o bloque de texto; vale cualquier orden y "center" implicito
//!
//! Sin ADS1115 conectado la cara barre sola.

#[allow(dead_code)]
#[path = "../fader.rs"]
mod fader;
mod bitmap;
#[allow(dead_code)]
mod oled;

use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use fader::{Adc, Error};
use oled::{HEIGHT, Oled, WIDTH};

const FPS: u64 = 30;
const BLINK_EVERY: Duration = Duration::from_millis(3500);
const BLINK_FOR: Duration = Duration::from_millis(140);

fn main() -> Result<(), Error> {
    let (mut png, mut svg, mut threshold, mut dither, mut invert, mut fit, mut words) =
        (None, None, None, false, false, false, vec![]);
    let mut align = Align::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--png" => png = Some(args.next().ok_or("--png needs a path")?),
            "--svg" => svg = Some(args.next().ok_or("--svg needs an svg string or -")?),
            "--dither" => dither = true,
            "--threshold" => threshold = Some(args.next().ok_or("--threshold needs 0-255")?.parse::<u8>()?),
            "--invert" => invert = true,
            "--fit" => fit = true,
            "--align" => align = args.next().ok_or("--align needs e.g. bottom-center")?.parse()?,
            _ => words.push(a),
        }
    }

    let mut o = Oled::new()?;
    if let Some(path) = png {
        let img = bitmap::load_png(&path, WIDTH, HEIGHT, fit, threshold, invert)?;
        draw_bitmap(&mut o, &img, align);
        return o.flush();
    }
    if let Some(mut src) = svg {
        if src == "-" {
            src = std::io::read_to_string(std::io::stdin())?;
        }
        // Iconos planos: el dither ensucia los bordes antialias.
        let threshold = if dither { None } else { threshold.or(Some(128)) };
        let img = bitmap::load_svg(&src, WIDTH, HEIGHT, threshold, invert)?;
        draw_bitmap(&mut o, &img, align);
        return o.flush();
    }
    if !words.is_empty() {
        draw_text(&mut o, &words.join(" "), align);
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

#[derive(Clone, Copy, Default)]
enum Pos {
    Start,
    #[default]
    Center,
    End,
}

impl Pos {
    fn offset(self, free: i32) -> i32 {
        match self {
            Pos::Start => 0,
            Pos::Center => free / 2,
            Pos::End => free,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Align {
    v: Pos,
    h: Pos,
}

impl Align {
    fn place(self, w: i32, h: i32) -> (i32, i32) {
        (self.h.offset(WIDTH - w), self.v.offset(HEIGHT - h))
    }
}

impl FromStr for Align {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let mut a = Align::default();
        for part in s.split('-') {
            match part {
                "top" => a.v = Pos::Start,
                "bottom" => a.v = Pos::End,
                "left" => a.h = Pos::Start,
                "right" => a.h = Pos::End,
                "center" | "middle" => {}
                _ => return Err(format!("bad align: {s} (e.g. bottom-center, top-left)")),
            }
        }
        Ok(a)
    }
}

fn draw_bitmap(o: &mut Oled, img: &bitmap::Mono, align: Align) {
    let (x0, y0) = align.place(img.w, img.h);
    for y in 0..img.h {
        for x in 0..img.w {
            o.pixel(x0 + x, y0 + y, img.get(x, y));
        }
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
fn draw_text(o: &mut Oled, s: &str, align: Align) {
    let scale = if wrap(s, 2).len() as i32 * 16 <= HEIGHT { 2 } else { 1 };
    let lines = wrap(s, scale);
    let line_h = 8 * scale;
    // La celda deja 1px vacio abajo; sin descontarlo bottom queda despegado del borde.
    let (_, mut y) = align.place(0, lines.len() as i32 * line_h - scale);
    for l in &lines {
        let (x, _) = align.place(Oled::text_width(l, scale), 0);
        o.text(x, y, l, scale, true);
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
