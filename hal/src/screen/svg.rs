//! SVG subset for icons -> 0.0-1.0 gray fitted to the maximum size.
//!
//! Supports svg, g, path, rect, circle, ellipse, line, polyline and polygon with inherited fill, stroke,
//! stroke-width, stroke-linecap/linejoin, fill-rule, color and transform.
//! No CSS, gradients, use/defs, masks or text: ignored with a warning.

use roxmltree::{Document, Node};
use zeno::{Angle, ArcSize, ArcSweep, Cap, Command, Fill, Join, Mask, PathBuilder, Stroke, Transform};

use crate::bitmap::luma;
use crate::fader::Error;

pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

#[derive(Clone)]
struct Paint {
    fill: Option<f32>,
    stroke: Option<f32>,
    width: f32,
    rule: Fill,
    cap: Cap,
    join: Join,
    color: f32,
    transform: Transform,
}

pub fn render(src: &str, max_w: i32, max_h: i32) -> Result<Raster, Error> {
    let doc = Document::parse(src)?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err("root is not <svg>".into());
    }
    let [vx, vy, vw, vh] = view_box(root)?;
    let scale = (max_w as f32 / vw).min(max_h as f32 / vh);
    let w = ((vw * scale).round() as usize).max(1);
    let h = ((vh * scale).round() as usize).max(1);
    let base = Paint {
        fill: Some(0.0),
        stroke: None,
        width: 1.0,
        rule: Fill::NonZero,
        cap: Cap::Butt,
        join: Join::Miter,
        color: 0.0,
        transform: then(Transform::translation(-vx, -vy), Transform::scale(scale, scale)),
    };
    let mut out = Raster { w, h, px: vec![0.0; w * h] };
    let mut mask = vec![0u8; w * h];
    walk(root, &base, &mut out, &mut mask)?;
    Ok(out)
}

fn view_box(root: Node) -> Result<[f32; 4], Error> {
    if let Some(v) = root.attribute("viewBox") {
        let n = numbers(v)?;
        return match n[..] {
            [x, y, w, h] if w > 0.0 && h > 0.0 => Ok([x, y, w, h]),
            _ => Err(format!("invalid viewBox: {v}").into()),
        };
    }
    match (root.attribute("width"), root.attribute("height")) {
        (Some(w), Some(h)) => Ok([0.0, 0.0, num(w)?, num(h)?]),
        _ => Err("svg needs viewBox or width/height".into()),
    }
}

fn walk(node: Node, parent: &Paint, out: &mut Raster, mask: &mut [u8]) -> Result<(), Error> {
    let p = inherit(node, parent)?;
    let name = node.tag_name().name();
    if name == "svg" || name == "g" {
        for child in node.children().filter(Node::is_element) {
            walk(child, &p, out, mask)?;
        }
        return Ok(());
    }
    match shape(node, name)? {
        Some(path) => draw(&path, &p, out, mask),
        None => eprintln!("svg: <{name}> unsupported, ignored"),
    }
    Ok(())
}

fn inherit(n: Node, parent: &Paint) -> Result<Paint, Error> {
    let mut p = parent.clone();
    // color first: fill/stroke="currentColor" on the same element read it.
    if let Some(v) = n.attribute("color") {
        p.color = color(v, p.color)?.unwrap_or(p.color);
    }
    if let Some(v) = n.attribute("fill") {
        p.fill = color(v, p.color)?;
    }
    if let Some(v) = n.attribute("stroke") {
        p.stroke = color(v, p.color)?;
    }
    if let Some(v) = n.attribute("stroke-width") {
        p.width = num(v)?;
    }
    if let Some(v) = n.attribute("fill-rule") {
        p.rule = if v == "evenodd" { Fill::EvenOdd } else { Fill::NonZero };
    }
    if let Some(v) = n.attribute("stroke-linecap") {
        p.cap = match v {
            "round" => Cap::Round,
            "square" => Cap::Square,
            _ => Cap::Butt,
        };
    }
    if let Some(v) = n.attribute("stroke-linejoin") {
        p.join = match v {
            "round" => Join::Round,
            "bevel" => Join::Bevel,
            _ => Join::Miter,
        };
    }
    if let Some(v) = n.attribute("transform") {
        p.transform = then(transform(v)?, p.transform);
    }
    Ok(p)
}

fn shape(n: Node, name: &str) -> Result<Option<Vec<Command>>, Error> {
    let a = |k: &str| n.attribute(k).map_or(Ok(0.0), num);
    let mut path = Vec::new();
    match name {
        "path" => path = path_data(n.attribute("d").unwrap_or(""))?,
        "rect" => {
            let (x, y, w, h) = (a("x")?, a("y")?, a("width")?, a("height")?);
            let (rx, ry) = match (n.attribute("rx"), n.attribute("ry")) {
                (None, None) => (0.0, 0.0),
                (Some(r), None) | (None, Some(r)) => (num(r)?, num(r)?),
                (Some(rx), Some(ry)) => (num(rx)?, num(ry)?),
            };
            if rx > 0.0 || ry > 0.0 {
                path.add_round_rect([x, y], w, h, rx, ry);
            } else {
                path.add_rect([x, y], w, h);
            }
        }
        "circle" => {
            path.add_circle([a("cx")?, a("cy")?], a("r")?);
        }
        "ellipse" => {
            path.add_ellipse([a("cx")?, a("cy")?], a("rx")?, a("ry")?);
        }
        "line" => {
            path.move_to([a("x1")?, a("y1")?]).line_to([a("x2")?, a("y2")?]);
        }
        "polyline" | "polygon" => {
            let pts = numbers(n.attribute("points").unwrap_or(""))?;
            let mut it = pts.chunks_exact(2);
            if let Some(first) = it.next() {
                path.move_to([first[0], first[1]]);
                it.for_each(|p| {
                    path.line_to([p[0], p[1]]);
                });
                if name == "polygon" {
                    path.close();
                }
            }
        }
        _ => return Ok(None),
    }
    Ok(Some(path))
}

/// Own parser: zeno 0.3.3's reflects the S/T control point wrongly after M and on implicit repetition.
fn path_data(d: &str) -> Result<Vec<Command>, Error> {
    let mut t = Tokens { s: d.as_bytes(), i: 0 };
    let mut path = Vec::new();
    let (mut cur, mut start) = ([0.0f32; 2], [0.0f32; 2]);
    let mut last: Option<(u8, [f32; 2])> = None;
    let mut cmd = None;
    while let Some(c) = t.peek() {
        if c.is_ascii_alphabetic() {
            cmd = Some(c);
            t.i += 1;
        }
        let c = cmd.ok_or_else(|| format!("path: expected command at {}", t.i))?;
        let rel = c.is_ascii_lowercase();
        let base = if rel { cur } else { [0.0, 0.0] };
        let pt = move |t: &mut Tokens| -> Result<[f32; 2], Error> { Ok([t.num()? + base[0], t.num()? + base[1]]) };
        let mut ctrl = None;
        match c.to_ascii_uppercase() {
            b'M' => {
                cur = pt(&mut t)?;
                start = cur;
                path.move_to(cur);
                cmd = Some(if rel { b'l' } else { b'L' });
            }
            b'L' => {
                cur = pt(&mut t)?;
                path.line_to(cur);
            }
            b'H' => {
                cur[0] = t.num()? + base[0];
                path.line_to(cur);
            }
            b'V' => {
                cur[1] = t.num()? + base[1];
                path.line_to(cur);
            }
            b'C' | b'S' => {
                let c1 = if c.to_ascii_uppercase() == b'C' { pt(&mut t)? } else { reflect(last, b'C', cur) };
                let c2 = pt(&mut t)?;
                cur = pt(&mut t)?;
                path.curve_to(c1, c2, cur);
                ctrl = Some((b'C', c2));
            }
            b'Q' | b'T' => {
                let q = if c.to_ascii_uppercase() == b'Q' { pt(&mut t)? } else { reflect(last, b'Q', cur) };
                cur = pt(&mut t)?;
                path.quad_to(q, cur);
                ctrl = Some((b'Q', q));
            }
            b'A' => {
                let (rx, ry, rot) = (t.num()?, t.num()?, t.num()?);
                let size = if t.flag()? { ArcSize::Large } else { ArcSize::Small };
                let sweep = if t.flag()? { ArcSweep::Positive } else { ArcSweep::Negative };
                cur = pt(&mut t)?;
                path.arc_to(rx.abs(), ry.abs(), Angle::from_degrees(rot), size, sweep, cur);
            }
            b'Z' => {
                path.close();
                cur = start;
                // No implicit repetition after Z: a stray number is an error, not a loop.
                cmd = None;
            }
            _ => return Err(format!("path: unsupported command {}", c as char).into()),
        }
        last = ctrl;
    }
    Ok(path)
}

fn reflect(last: Option<(u8, [f32; 2])>, kind: u8, cur: [f32; 2]) -> [f32; 2] {
    match last {
        Some((k, c)) if k == kind => [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]],
        _ => cur,
    }
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn peek(&mut self) -> Option<u8> {
        while self.s.get(self.i).is_some_and(|&c| c.is_ascii_whitespace() || c == b',') {
            self.i += 1;
        }
        self.s.get(self.i).copied()
    }

    /// Minified: "1.5.5" is two numbers, and so is "4-2".
    fn num(&mut self) -> Result<f32, Error> {
        self.peek();
        let (s, from) = (self.s, self.i);
        let digits = |i: &mut usize| {
            while s.get(*i).is_some_and(u8::is_ascii_digit) {
                *i += 1;
            }
        };
        let mut i = from;
        if matches!(s.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        digits(&mut i);
        if s.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i);
        }
        if matches!(s.get(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(s.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            digits(&mut i);
        }
        self.i = i;
        std::str::from_utf8(&s[from..i])?.parse().map_err(|_| format!("path: invalid number at {from}").into())
    }

    /// Arc flags may be packed together: "a1 1 0 00 1 1".
    fn flag(&mut self) -> Result<bool, Error> {
        let f = match self.peek() {
            Some(b'0') => false,
            Some(b'1') => true,
            _ => return Err(format!("path: invalid arc flag at {}", self.i).into()),
        };
        self.i += 1;
        Ok(f)
    }
}

/// Composites over what is already drawn: a white hole over a black fill stays visible.
fn draw(path: &[Command], p: &Paint, out: &mut Raster, mask: &mut [u8]) {
    let (w, h) = (out.w as u32, out.h as u32);
    let mut layer = |value: f32, style: zeno::Style| {
        mask.fill(0);
        Mask::new(path).size(w, h).transform(Some(p.transform)).style(style).render_into(mask, None);
        for (px, &m) in out.px.iter_mut().zip(mask.iter()) {
            let a = m as f32 / 255.0;
            *px = *px * (1.0 - a) + value * a;
        }
    };
    if let Some(v) = p.fill {
        layer(v, p.rule.into());
    }
    if let Some(v) = p.stroke {
        let mut s = Stroke::new(p.width);
        s.cap(p.cap).join(p.join);
        layer(v, s.into());
    }
}

/// None = "none". Only hex, white, black and currentColor: enough for icons.
fn color(v: &str, current: f32) -> Result<Option<f32>, Error> {
    let v = v.trim();
    let hex = |s: &str| u8::from_str_radix(s, 16);
    let rgb = match v {
        "none" | "transparent" => return Ok(None),
        "currentColor" => return Ok(Some(current)),
        "white" => [255, 255, 255],
        "black" => [0, 0, 0],
        _ if v.len() == 4 && v.starts_with('#') => {
            let d = |i: usize| hex(&v[i..i + 1]).map(|c| c * 17);
            [d(1)?, d(2)?, d(3)?]
        }
        _ if v.len() == 7 && v.starts_with('#') => [hex(&v[1..3])?, hex(&v[3..5])?, hex(&v[5..7])?],
        _ => return Err(format!("unsupported color: {v}").into()),
    };
    Ok(Some(luma(rgb[0], rgb[1], rgb[2]) / 255.0))
}

fn transform(v: &str) -> Result<Transform, Error> {
    let mut t = Transform::IDENTITY;
    for item in v.split(')').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, args) = item.split_once('(').ok_or_else(|| format!("invalid transform: {v}"))?;
        let n = numbers(args)?;
        let step = match (name.trim().trim_start_matches(','), &n[..]) {
            ("translate", &[x]) => Transform::translation(x, 0.0),
            ("translate", &[x, y]) => Transform::translation(x, y),
            ("scale", &[s]) => Transform::scale(s, s),
            ("scale", &[x, y]) => Transform::scale(x, y),
            ("rotate", &[a]) => Transform::rotation(Angle::from_degrees(a)),
            ("rotate", &[a, cx, cy]) => then(
                then(Transform::translation(-cx, -cy), Transform::rotation(Angle::from_degrees(a))),
                Transform::translation(cx, cy),
            ),
            ("matrix", &[a, b, c, d, e, f]) => Transform::new(a, b, c, d, e, f),
            _ => return Err(format!("unsupported transform: {item})").into()),
        };
        // "A B" applies B first.
        t = then(step, t);
    }
    Ok(t)
}

/// `a` then `b`. Not zeno's `Transform::then`: it composes rotation with non-uniform scale wrongly.
fn then(a: Transform, b: Transform) -> Transform {
    Transform::new(
        a.xx * b.xx + a.xy * b.yx,
        a.xx * b.xy + a.xy * b.yy,
        a.yx * b.xx + a.yy * b.yx,
        a.yx * b.xy + a.yy * b.yy,
        a.x * b.xx + a.y * b.yx + b.x,
        a.x * b.xy + a.y * b.yy + b.y,
    )
}

fn numbers(v: &str) -> Result<Vec<f32>, Error> {
    v.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(num).collect()
}

fn num(v: &str) -> Result<f32, Error> {
    let v = v.trim();
    v.strip_suffix("px").unwrap_or(v).parse().map_err(|_| format!("invalid number: {v}").into())
}
