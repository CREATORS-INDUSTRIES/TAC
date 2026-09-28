//! PNG -> bitmap 1 bit: escala manteniendo aspecto y binariza por umbral o Floyd-Steinberg.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::fader::Error;

pub struct Mono {
    pub w: i32,
    pub h: i32,
    pub px: Vec<bool>,
}

impl Mono {
    pub fn get(&self, x: i32, y: i32) -> bool {
        self.px[(y * self.w + x) as usize]
    }
}

struct Gray {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

/// `threshold` None = dithering. Transparente cuenta como negro (pixel apagado).
/// Sin `upscale` solo reduce: un icono pequeno queda a tamano nativo y se puede alinear.
pub fn load(
    path: impl AsRef<Path>,
    max_w: i32,
    max_h: i32,
    upscale: bool,
    threshold: Option<u8>,
    invert: bool,
) -> Result<Mono, Error> {
    let g = decode(path.as_ref())?;
    let fit = (max_w as f32 / g.w as f32).min(max_h as f32 / g.h as f32);
    let scale = if upscale { fit } else { fit.min(1.0) };
    let w = ((g.w as f32 * scale).round() as usize).max(1);
    let h = ((g.h as f32 * scale).round() as usize).max(1);
    let mut px = resize(&g, w, h);
    if invert {
        px.iter_mut().for_each(|v| *v = 1.0 - *v);
    }
    let bits = match threshold {
        Some(t) => px.iter().map(|&v| v * 255.0 >= t as f32).collect(),
        None => dither(&mut px, w, h),
    };
    Ok(Mono { w: w as i32, h: h as i32, px: bits })
}

fn decode(path: &Path) -> Result<Gray, Error> {
    let mut dec = png::Decoder::new(BufReader::new(File::open(path)?));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png demasiado grande")?];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    let px = buf
        .chunks(info.line_size)
        .take(h)
        .flat_map(|row| row.chunks(ch).take(w))
        .map(|p| {
            let luma = match ch {
                1 | 2 => p[0] as f32,
                _ => 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32,
            };
            let alpha = if ch == 2 || ch == 4 { p[ch - 1] as f32 / 255.0 } else { 1.0 };
            luma / 255.0 * alpha
        })
        .collect();
    Ok(Gray { w, h, px })
}

/// Media por area: al reducir, vecino mas cercano pierde lineas finas.
fn resize(g: &Gray, w: usize, h: usize) -> Vec<f32> {
    let (sx, sy) = (g.w as f32 / w as f32, g.h as f32 / h as f32);
    let span = |i: usize, s: f32, max: usize| {
        let a = (i as f32 * s) as usize;
        let b = (((i + 1) as f32 * s).ceil() as usize).clamp(a + 1, max);
        a..b
    };
    let mut out = Vec::with_capacity(w * h);
    for ty in 0..h {
        let ys = span(ty, sy, g.h);
        for tx in 0..w {
            let xs = span(tx, sx, g.w);
            let (mut sum, mut n) = (0.0, 0);
            for y in ys.clone() {
                for x in xs.clone() {
                    sum += g.px[y * g.w + x];
                    n += 1;
                }
            }
            out.push(sum / n as f32);
        }
    }
    out
}

fn dither(px: &mut [f32], w: usize, h: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let on = px[i] >= 0.5;
            out[i] = on;
            let err = px[i] - if on { 1.0 } else { 0.0 };
            let mut spread = |dx: isize, dy: usize, k: f32| {
                let nx = x as isize + dx;
                if nx >= 0 && (nx as usize) < w && y + dy < h {
                    px[(y + dy) * w + nx as usize] += err * k;
                }
            };
            spread(1, 0, 7.0 / 16.0);
            spread(-1, 1, 3.0 / 16.0);
            spread(0, 1, 5.0 / 16.0);
            spread(1, 1, 1.0 / 16.0);
        }
    }
    out
}
