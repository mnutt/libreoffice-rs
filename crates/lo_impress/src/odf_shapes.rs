//! Bounded ODF enhanced geometry -> SVG artwork. See ODF 1.3 part 3,
//! draw:enhanced-path and draw:formula. Text remains a separate PDF text box.
use std::collections::BTreeMap;
use std::fmt::Write;
use lo_core::{ImageElement, Length, Rect, XmlNode};
use lo_core::style::{OdfStyles, odf_length};

const LIMIT: usize = 16_384;
fn finite(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() <= 1e9).then_some(v)
}

fn tokens(s: &str) -> Option<Vec<String>> {
    if s.len() > 128 * 1024 {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if b[i].is_ascii_digit() || b[i] == b'.' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && matches!(b[i], b'e' | b'E') {
                i += 1;
                if i < b.len() && matches!(b[i], b'+' | b'-') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
        } else if b[i].is_ascii_alphabetic() || matches!(b[i], b'?' | b'$') {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else if b"+-*/(),".contains(&b[i]) {
            i += 1;
        } else {
            return None;
        }
        out.push(s[start..i].into());
        if out.len() > LIMIT {
            return None;
        }
    }
    Some(out)
}

struct Equations<'a> {
    formulas: BTreeMap<&'a str, &'a str>,
    values: BTreeMap<String, f64>,
    modifiers: Vec<f64>,
    bounds: [f64; 4],
    budget: usize,
}
impl Equations<'_> {
    fn atom(&mut self, token: &str, depth: usize) -> Option<f64> {
        if depth > 64 {
            return None;
        }
        if let Some(key) = token.strip_prefix('?') {
            if let Some(value) = self.values.get(key) {
                return Some(*value);
            }
            let expression = tokens(self.formulas.get(key)?)?;
            let mut at = 0;
            let value = self.expression(&expression, &mut at, 0, depth + 1)?;
            if at != expression.len() {
                return None;
            }
            self.values.insert(key.to_string(), value);
            return Some(value);
        }
        if let Some(index) = token.strip_prefix('$') {
            return self.modifiers.get(index.parse::<usize>().ok()?).copied();
        }
        finite(match token {
            "pi" => std::f64::consts::PI,
            "left" => self.bounds[0],
            "top" => self.bounds[1],
            "right" => self.bounds[0] + self.bounds[2],
            "bottom" => self.bounds[1] + self.bounds[3],
            "width" => self.bounds[2],
            "height" => self.bounds[3],
            _ => token.parse().ok()?,
        })
    }
    fn expression(
        &mut self,
        t: &[String],
        at: &mut usize,
        precedence: u8,
        depth: usize,
    ) -> Option<f64> {
        if depth > 64 || self.budget == 0 {
            return None;
        }
        self.budget -= 1;
        let token = t.get(*at)?.as_str();
        *at += 1;
        let mut value = match token {
            "+" => self.expression(t, at, 3, depth + 1)?,
            "-" => -self.expression(t, at, 3, depth + 1)?,
            "(" => {
                let v = self.expression(t, at, 0, depth + 1)?;
                if t.get(*at)?.as_str() != ")" {
                    return None;
                }
                *at += 1;
                v
            }
            function if t.get(*at).map(String::as_str) == Some("(") => {
                *at += 1;
                let mut args = Vec::new();
                loop {
                    args.push(self.expression(t, at, 0, depth + 1)?);
                    if args.len() > 3 {
                        return None;
                    }
                    let end = t.get(*at)?.as_str();
                    *at += 1;
                    if end == ")" {
                        break;
                    }
                    if end != "," {
                        return None;
                    }
                }
                match (function, args.as_slice()) {
                    ("sin", [x]) => x.sin(),
                    ("cos", [x]) => x.cos(),
                    ("tan", [x]) => x.tan(),
                    ("sqrt", [x]) => x.sqrt(),
                    ("abs", [x]) => x.abs(),
                    ("atan", [x]) => x.atan(),
                    ("atan2", [y, x]) => y.atan2(*x),
                    ("min", [x, y]) => x.min(*y),
                    ("max", [x, y]) => x.max(*y),
                    ("if", [c, a, b]) => {
                        if *c > 0.0 {
                            *a
                        } else {
                            *b
                        }
                    }
                    _ => return None,
                }
            }
            _ => self.atom(token, depth + 1)?,
        };
        loop {
            let Some(op) = t.get(*at).map(String::as_str) else {
                break;
            };
            let level = match op {
                "+" | "-" => 1,
                "*" | "/" => 2,
                _ => break,
            };
            if level <= precedence {
                break;
            }
            *at += 1;
            let rhs = self.expression(t, at, level, depth + 1)?;
            value = finite(match op {
                "+" => value + rhs,
                "-" => value - rhs,
                "*" => value * rhs,
                _ => value / rhs,
            })?;
        }
        finite(value)
    }
}

#[derive(Default)]
struct Part {
    path: String,
    no_fill: bool,
    no_stroke: bool,
}
fn enhanced_path(path: &str, equations: &mut Equations) -> Option<Vec<Part>> {
    let t = tokens(path)?;
    let mut i = 0;
    let mut cmd = String::new();
    let mut parts = Vec::new();
    let mut part = Part::default();
    let mut current = (0.0, 0.0);
    let mut start = current;
    let mut quadrant_x = false;
    while i < t.len() {
        if t[i].len() == 1 && t[i].as_bytes()[0].is_ascii_uppercase() {
            cmd = t[i].clone();
            i += 1;
            quadrant_x = cmd == "X";
            match cmd.as_str() {
                "N" => {
                    parts.push(part);
                    part = Part::default();
                    cmd.clear();
                    continue;
                }
                "F" => {
                    part.no_fill = true;
                    cmd.clear();
                    continue;
                }
                "S" => {
                    part.no_stroke = true;
                    cmd.clear();
                    continue;
                }
                "Z" => {
                    part.path.push_str("Z ");
                    current = start;
                    cmd.clear();
                    continue;
                }
                _ => {}
            }
        }
        let count = match cmd.as_str() {
            "M" | "L" | "X" | "Y" => 2,
            "Q" => 4,
            "C" | "U" | "T" => 6,
            "A" | "B" | "V" | "W" => 8,
            _ => return None,
        };
        let mut v = Vec::new();
        for _ in 0..count {
            if t.get(i).map(String::as_str) == Some(",") {
                i += 1;
            }
            let sign = if t.get(i).map(String::as_str) == Some("-") {
                i += 1;
                -1.0
            } else {
                1.0
            };
            v.push(sign * equations.atom(t.get(i)?, 0)?);
            i += 1;
        }
        match cmd.as_str() {
            "M" | "L" | "C" | "Q" => {
                write!(part.path, "{} ", cmd).ok()?;
                for n in &v {
                    write!(part.path, "{n} ").ok()?;
                }
                current = (v[count - 2], v[count - 1]);
                if cmd == "M" {
                    start = current;
                    cmd = "L".into();
                }
            }
            "X" | "Y" => {
                let dx = v[0] - current.0;
                let dy = v[1] - current.1;
                let sweep = if quadrant_x {
                    dx * dy > 0.0
                } else {
                    dx * dy < 0.0
                };
                write!(
                    part.path,
                    "A {} {} 0 0 {} {} {} ",
                    dx.abs(),
                    dy.abs(),
                    u8::from(sweep),
                    v[0],
                    v[1]
                )
                .ok()?;
                current = (v[0], v[1]);
                quadrant_x = !quadrant_x;
            }
            _ => {
                let (cx, cy, rx, ry, a, b, clockwise, move_to) =
                    if matches!(cmd.as_str(), "U" | "T") {
                        (
                            v[0],
                            v[1],
                            v[2].abs(),
                            v[3].abs(),
                            v[4].to_radians(),
                            v[5].to_radians(),
                            true,
                            cmd == "U",
                        )
                    } else {
                        let (cx, cy, rx, ry) = (
                            (v[0] + v[2]) / 2.0,
                            (v[1] + v[3]) / 2.0,
                            (v[2] - v[0]).abs() / 2.0,
                            (v[3] - v[1]).abs() / 2.0,
                        );
                        if rx == 0.0 || ry == 0.0 {
                            return None;
                        }
                        (
                            cx,
                            cy,
                            rx,
                            ry,
                            ((v[5] - cy) / ry).atan2((v[4] - cx) / rx),
                            ((v[7] - cy) / ry).atan2((v[6] - cx) / rx),
                            matches!(cmd.as_str(), "V" | "W"),
                            matches!(cmd.as_str(), "B" | "V"),
                        )
                    };
                let tau = std::f64::consts::TAU;
                let mut delta = if clockwise {
                    (b - a).rem_euclid(tau)
                } else {
                    -((a - b).rem_euclid(tau))
                };
                if delta.abs() < 1e-8 {
                    delta = if clockwise { tau } else { -tau };
                }
                let first = (cx + rx * a.cos(), cy + ry * a.sin());
                let movement = if move_to || part.path.is_empty() {
                    "M"
                } else {
                    "L"
                };
                write!(part.path, "{movement} {} {} ", first.0, first.1).ok()?;
                if movement == "M" {
                    start = first;
                }
                // Two arcs also represent a full ellipse, which SVG cannot
                // express with coincident start/end points in a single arc.
                for fraction in [0.5, 1.0] {
                    let angle = a + delta * fraction;
                    current = (cx + rx * angle.cos(), cy + ry * angle.sin());
                    write!(
                        part.path,
                        "A {rx} {ry} 0 0 {} {} {} ",
                        u8::from(clockwise),
                        current.0,
                        current.1
                    )
                    .ok()?;
                }
            }
        }
        if part.path.len() > 512 * 1024 || parts.len() > 1024 {
            return None;
        }
    }
    if !part.path.is_empty() {
        parts.push(part);
    }
    Some(parts)
}

fn numbers(s: &str) -> Option<Vec<f64>> {
    s.split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(|s| finite(s.parse().ok()?))
        .collect()
}
fn color(s: &str) -> Option<&str> {
    (s == "none"
        || (s.starts_with('#')
            && matches!(s.len(), 4 | 7)
            && s[1..].bytes().all(|c| c.is_ascii_hexdigit())))
    .then_some(s)
}

pub(crate) fn artwork(node: &XmlNode, styles: &OdfStyles) -> Option<ImageElement> {
    // Unsupported transforms keep the existing conservative fallback.
    if node.attr("transform").is_some() {
        return None;
    }
    let geometry = node.child("enhanced-geometry");
    if geometry.is_some_and(|g| g.attr("extrusion") == Some("true")) {
        return None; // 3D projection and lighting require a different renderer.
    }
    let view = geometry
        .and_then(|g| g.attr("viewBox"))
        .or_else(|| node.attr("viewBox"))
        .unwrap_or("0 0 21600 21600");
    let bounds: [f64; 4] = numbers(view)?.try_into().ok()?;
    if bounds[2] <= 0.0 || bounds[3] <= 0.0 {
        return None;
    }
    let w = odf_length(node.attr("width")?)?;
    let h = odf_length(node.attr("height")?)?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let x = node.attr("x").and_then(odf_length).unwrap_or(0.0);
    let y = node.attr("y").and_then(odf_length).unwrap_or(0.0);
    let parts = if let Some(g) = geometry {
        let mut eq = Equations {
            formulas: g
                .children_named("equation")
                .map(|n| Some((n.attr("name")?, n.attr("formula")?)))
                .collect::<Option<_>>()?,
            values: BTreeMap::new(),
            modifiers: numbers(g.attr("modifiers").unwrap_or(""))?,
            bounds,
            budget: LIMIT,
        };
        if eq.formulas.len() > 4096 || eq.modifiers.len() > 1024 {
            return None;
        }
        enhanced_path(g.attr("enhanced-path")?, &mut eq)?
    } else if node.local_name() == "path" {
        let path = node.attr("d")?;
        if path.len() > 128 * 1024
            || !path
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+-. ,\t\r\n".contains(&c))
        {
            return None;
        }
        vec![Part {
            path: path.into(),
            ..Part::default()
        }]
    } else {
        let points = numbers(node.attr("points")?)?;
        if points.len() < 4 || points.len() % 2 != 0 || points.len() > LIMIT {
            return None;
        }
        let mut path = String::new();
        for (i, p) in points.chunks_exact(2).enumerate() {
            write!(
                path,
                "{} {} {} ",
                if i == 0 { "M" } else { "L" },
                p[0],
                p[1]
            )
            .ok()?;
        }
        if node.local_name() == "polygon" {
            path.push('Z');
        }
        vec![Part {
            path,
            no_fill: node.local_name() == "polyline",
            ..Part::default()
        }]
    };
    let style = node.attr("style-name").unwrap_or("");
    let prop = |key| styles.property(style, "graphic-properties", key);
    let fill = if prop("fill") == Some("none") {
        "none"
    } else {
        prop("fill-color").and_then(color).unwrap_or("#729fcf")
    };
    let stroke = if prop("stroke") == Some("none") {
        "none"
    } else {
        prop("stroke-color").and_then(color).unwrap_or("#000000")
    };
    let stroke_pt = prop("stroke-width")
        .and_then(odf_length)
        .unwrap_or(0.75)
        // ODF zero-width strokes are visible hairlines, not SVG's no-stroke.
        .max(0.5);
    let stroke_width = stroke_pt as f64 * bounds[2] / w as f64;
    let pad = stroke_pt.max(1.0);
    let width = w + pad * 2.0;
    let height = h + pad * 2.0;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><svg x="{pad}" y="{pad}" width="{w}" height="{h}" viewBox="{} {} {} {}" preserveAspectRatio="none" overflow="visible">"#,
        bounds[0], bounds[1], bounds[2], bounds[3]
    );
    let mx = geometry.is_some_and(|g| g.attr("mirror-horizontal") == Some("true"));
    let my = geometry.is_some_and(|g| g.attr("mirror-vertical") == Some("true"));
    write!(
        svg,
        r#"<g transform="translate({} {}) scale({} {})">"#,
        if mx { 2.0 * bounds[0] + bounds[2] } else { 0.0 },
        if my { 2.0 * bounds[1] + bounds[3] } else { 0.0 },
        if mx { -1 } else { 1 },
        if my { -1 } else { 1 }
    )
    .ok()?;
    for part in parts {
        write!(svg,r#"<path d="{}" fill="{}" fill-rule="evenodd" stroke="{}" stroke-width="{stroke_width}" stroke-linejoin="round"/>"#,part.path,if part.no_fill{"none"}else{fill},if part.no_stroke{"none"}else{stroke}).ok()?;
    }
    svg.push_str("</g></svg></svg>");
    Some(ImageElement {
                crop: [0.0; 4], rotation: 0.0, flip_horizontal: false, flip_vertical: false,
        name: "ODF geometry".into(),
        mime_type: "image/svg+xml".into(),
        data: svg.into_bytes(),
        frame: Rect::new(
            Length::pt(x - pad),
            Length::pt(y - pad),
            Length::pt(width),
            Length::pt(height),
        ),
        alt: String::new(),
        opacity: 255,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equations_preserve_precedence_modifiers_and_reject_cycles() {
        let mut eq = Equations {
            formulas: BTreeMap::from([("f0", "$0 * sin(pi/2) + 4/2"), ("cycle", "?cycle")]),
            values: BTreeMap::new(),
            modifiers: vec![10.0],
            bounds: [0.0, 0.0, 21600.0, 21600.0],
            budget: LIMIT,
        };
        assert_eq!(eq.atom("?f0", 0), Some(12.0));
        assert!(eq.atom("?cycle", 0).is_none());
        assert!(eq.atom("$1", 0).is_none());
        assert!(finite(f64::INFINITY).is_none());
    }
    #[test]
    fn full_ellipse_and_modifier_triangle_are_real_paths() {
        let node=lo_core::parse_xml_document(r#"<custom-shape width="2cm" height="2cm"><enhanced-geometry modifiers="10800" enhanced-path="M ?f0 0 L 21600 21600 0 21600 Z N U 10800 10800 3000 3000 0 360 Z N"><equation name="f0" formula="$0"/></enhanced-geometry></custom-shape>"#).unwrap();
        let image = artwork(&node, &OdfStyles::default()).unwrap();
        let svg = String::from_utf8(image.data).unwrap();
        assert!(svg.contains("M 10800 0 L 21600 21600 L 0 21600 Z"));
        assert_eq!(svg.matches("A 3000 3000").count(), 2);
        assert!(resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).is_ok());
    }
    #[test]
    fn rasterized_ring_keeps_transparent_hole_instead_of_a_rectangle() {
        let node=lo_core::parse_xml_document(r#"<custom-shape width="1in" height="1in"><enhanced-geometry enhanced-path="U 10800 10800 10800 10800 0 360 Z U 10800 10800 5400 5400 0 360 Z N"/></custom-shape>"#).unwrap();
        let image = artwork(&node, &OdfStyles::default()).unwrap();
        let tree =
            resvg::usvg::Tree::from_data(&image.data, &resvg::usvg::Options::default()).unwrap();
        let mut pixels = resvg::tiny_skia::Pixmap::new(74, 74).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixels.as_mut(),
        );
        assert_eq!(pixels.pixel(37, 37).unwrap().alpha(), 0);
        assert_eq!(pixels.pixel(4, 4).unwrap().alpha(), 0);
        assert!(pixels.pixel(37, 10).unwrap().alpha() > 200);
    }
    #[test]
    fn odf_zero_width_stroke_remains_visible_and_extrusion_uses_fallback() {
        let mut styles = OdfStyles::default();
        styles.collect(&lo_core::parse_xml_document(r#"<styles><style name="Hairline" family="graphic"><graphic-properties stroke="solid" stroke-width="0cm"/></style></styles>"#).unwrap());
        let node=lo_core::parse_xml_document(r#"<custom-shape style-name="Hairline" width="1in" height="1in"><enhanced-geometry enhanced-path="M 0 0 L 21600 21600 N"/></custom-shape>"#).unwrap();
        let image = artwork(&node, &styles).unwrap();
        assert!(
            String::from_utf8(image.data)
                .unwrap()
                .contains("stroke-width=\"150\"")
        );
        let node=lo_core::parse_xml_document(r#"<custom-shape width="1in" height="1in"><enhanced-geometry extrusion="true" enhanced-path="M 0 0 L 21600 21600 N"/></custom-shape>"#).unwrap();
        assert!(artwork(&node, &styles).is_none());
    }
    #[test]
    fn malformed_and_excessive_geometry_is_rejected() {
        for path in ["M 0", "M ?missing 0", "M 0 0 R 2 3", "M NaN 0"] {
            let xml = format!(
                r#"<custom-shape width="1cm" height="1cm"><enhanced-geometry enhanced-path="{path}"/></custom-shape>"#
            );
            assert!(
                artwork(
                    &lo_core::parse_xml_document(&xml).unwrap(),
                    &OdfStyles::default()
                )
                .is_none()
            );
        }
        assert!(tokens(&"0 ".repeat(LIMIT + 1)).is_none());
    }
}
