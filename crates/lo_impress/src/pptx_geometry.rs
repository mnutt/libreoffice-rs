//! Bounded DrawingML geometry evaluation and common preset outlines.
use lo_core::{PathCommand as C, ShapeKind, VectorPath, XmlNode};
use std::collections::BTreeMap;
const U: i32 = 100000;
fn polygon(points: &[(i32, i32)]) -> ShapeKind {
    ShapeKind::Custom(vec![VectorPath::polygon(points)])
}
fn ellipse() -> VectorPath {
    VectorPath {
        fill: true,
        stroke: true,
        commands: vec![
            C::Move(U, 50000),
            C::Cubic([(U, 77614), (77614, U), (50000, U)]),
            C::Cubic([(22386, U), (0, 77614), (0, 50000)]),
            C::Cubic([(0, 22386), (22386, 0), (50000, 0)]),
            C::Cubic([(77614, 0), (U, 22386), (U, 50000)]),
            C::Close,
        ],
    }
}
fn rounded(rx: i32, ry: i32) -> ShapeKind {
    let (kx, ky) = (
        (rx as f32 * 0.5522848) as i32,
        (ry as f32 * 0.5522848) as i32,
    );
    ShapeKind::Custom(vec![VectorPath {
        fill: true,
        stroke: true,
        commands: vec![
            C::Move(rx, 0),
            C::Line(U - rx, 0),
            C::Cubic([(U - rx + kx, 0), (U, ry - ky), (U, ry)]),
            C::Line(U, U - ry),
            C::Cubic([(U, U - ry + ky), (U - rx + kx, U), (U - rx, U)]),
            C::Line(rx, U),
            C::Cubic([(rx - kx, U), (0, U - ry + ky), (0, U - ry)]),
            C::Line(0, ry),
            C::Cubic([(0, ry - ky), (rx - kx, 0), (rx, 0)]),
            C::Close,
        ],
    }])
}
fn value(token: &str, vars: &BTreeMap<String, f64>) -> f64 {
    token
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .or_else(|| vars.get(token).copied())
        .unwrap_or(0.0)
}
fn formula(text: &str, vars: &BTreeMap<String, f64>) -> f64 {
    let mut parts = text.split_whitespace();
    let op = parts.next().unwrap_or("");
    let mut next = || value(parts.next().unwrap_or("0"), vars);
    let (a, b, c) = (next(), next(), next());
    let angle = |v: f64| v / 60000.0 * std::f64::consts::PI / 180.0;
    let answer = match op {
        "val" => a,
        "+-" => a + b - c,
        "*/" => {
            if c != 0.0 {
                a * b / c
            } else {
                0.0
            }
        }
        "+/" => {
            if c != 0.0 {
                (a + b) / c
            } else {
                0.0
            }
        }
        "?:" => {
            if a > 0.0 {
                b
            } else {
                c
            }
        }
        "abs" => a.abs(),
        "min" => a.min(b),
        "max" => a.max(b),
        "pin" => b.clamp(a.min(c), a.max(c)),
        "sqrt" => a.max(0.0).sqrt(),
        "mod" => a.hypot(b).hypot(c),
        "sin" => a * angle(b).sin(),
        "cos" => a * angle(b).cos(),
        "tan" => a * angle(b).tan(),
        "at2" => b.atan2(a).to_degrees() * 60000.0,
        "cat2" => a * c.atan2(b).cos(),
        "sat2" => a * c.atan2(b).sin(),
        _ => 0.0,
    };
    if answer.is_finite() {
        answer.clamp(-1e12, 1e12)
    } else {
        0.0
    }
}
fn variables(n: &XmlNode, w: f64, h: f64) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (key, v) in [
        ("w", w),
        ("h", h),
        ("l", 0.0),
        ("t", 0.0),
        ("r", w),
        ("b", h),
        ("hc", w / 2.0),
        ("vc", h / 2.0),
        ("ss", w.min(h)),
        ("ls", w.max(h)),
        ("cd2", 10800000.0),
        ("cd4", 5400000.0),
        ("cd8", 2700000.0),
        ("3cd4", 16200000.0),
        ("3cd8", 8100000.0),
        ("5cd8", 13500000.0),
        ("7cd8", 18900000.0),
    ] {
        out.insert(key.into(), v);
    }
    for d in [2, 3, 4, 5, 6, 8, 10, 12, 16, 32] {
        out.insert(format!("wd{d}"), w / d as f64);
        out.insert(format!("hd{d}"), h / d as f64);
        out.insert(format!("ssd{d}"), w.min(h) / d as f64);
    }
    for list in ["avLst", "gdLst"] {
        if let Some(list) = n.child(list) {
            for gd in list.children_named("gd").take(2048) {
                if let (Some(name), Some(f)) = (gd.attr("name"), gd.attr("fmla")) {
                    let v = formula(f, &out);
                    out.insert(name.into(), v);
                }
            }
        }
    }
    out
}
pub(crate) fn geometry(n: &XmlNode, width: f32, height: f32) -> Option<ShapeKind> {
    let w = (width as f64 * 12700.0).max(1.0);
    let h = (height as f64 * 12700.0).max(1.0);
    let vars = variables(n, w, h);
    if n.local_name() == "custGeom" {
        return custom(n, &vars, w, h);
    }
    let adjustment = |key: &str, default: i32, maximum: i32| {
        vars.get(key)
            .copied()
            .unwrap_or(default as f64)
            .clamp(0.0, maximum as f64) as i32
    };
    let kind = n.attr("prst")?;
    Some(match kind {
        "rect" => ShapeKind::Rectangle,
        "ellipse" => ShapeKind::Custom(vec![ellipse()]),
        "line" | "straightConnector1" => ShapeKind::Custom(vec![VectorPath {
            commands: vec![C::Move(0, 0), C::Line(U, U)],
            fill: false,
            stroke: true,
        }]),
        "roundRect" => {
            let r = adjustment("adj", 16667, 50000) as f64 * w.min(h);
            rounded((r / w) as i32, (r / h) as i32)
        }
        "triangle" => polygon(&[(adjustment("adj", 50000, U), 0), (U, U), (0, U)]),
        "rtTriangle" => polygon(&[(0, 0), (U, U), (0, U)]),
        "diamond" => polygon(&[(50000, 0), (U, 50000), (50000, U), (0, 50000)]),
        "parallelogram" => {
            let a = adjustment("adj", 25000, 50000);
            polygon(&[(a, 0), (U, 0), (U - a, U), (0, U)])
        }
        "trapezoid" => {
            let a = adjustment("adj", 25000, 50000);
            polygon(&[(a, 0), (U - a, 0), (U, U), (0, U)])
        }
        "hexagon" => {
            let a = adjustment("adj", 25000, 50000);
            polygon(&[
                (a, 0),
                (U - a, 0),
                (U, 50000),
                (U - a, U),
                (a, U),
                (0, 50000),
            ])
        }
        "octagon" => {
            let a = adjustment("adj", 29289, 50000);
            polygon(&[
                (a, 0),
                (U - a, 0),
                (U, a),
                (U, U - a),
                (U - a, U),
                (a, U),
                (0, U - a),
                (0, a),
            ])
        }
        "pentagon" | "star5" => {
            let count = if kind == "pentagon" { 5 } else { 10 };
            let points: Vec<_> = (0..count)
                .map(|i| {
                    let angle = i as f64 * std::f64::consts::TAU / count as f64
                        - std::f64::consts::FRAC_PI_2;
                    let radius = if kind == "star5" && i % 2 == 1 {
                        19098.0
                    } else {
                        50000.0
                    };
                    (
                        (50000.0 + radius * angle.cos()) as i32,
                        (50000.0 + radius * angle.sin()) as i32,
                    )
                })
                .collect();
            polygon(&points)
        }
        "plus" => {
            let a = adjustment("adj", 25000, 50000);
            polygon(&[
                (a, 0),
                (U - a, 0),
                (U - a, a),
                (U, a),
                (U, U - a),
                (U - a, U - a),
                (U - a, U),
                (a, U),
                (a, U - a),
                (0, U - a),
                (0, a),
                (a, a),
            ])
        }
        "chevron" => {
            let a = adjustment("adj", 25000, 50000);
            polygon(&[
                (0, 0),
                (U - a, 0),
                (U, 50000),
                (U - a, U),
                (0, U),
                (a, 50000),
            ])
        }
        "homePlate" => {
            let a = adjustment("adj", 50000, U);
            polygon(&[(0, 0), (U - a, 0), (U, 50000), (U - a, U), (0, U)])
        }
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" => {
            let half = adjustment("adj1", 50000, U) / 2;
            let head = adjustment("adj2", 50000, U);
            let mut points = vec![
                (0, 50000 - half),
                (U - head, 50000 - half),
                (U - head, 0),
                (U, 50000),
                (U - head, U),
                (U - head, 50000 + half),
                (0, 50000 + half),
            ];
            for p in &mut points {
                *p = match kind {
                    "leftArrow" => (U - p.0, p.1),
                    "upArrow" => (p.1, U - p.0),
                    "downArrow" => (p.1, p.0),
                    _ => *p,
                };
            }
            polygon(&points)
        }
        "leftRightArrow" => polygon(&[
            (0, 50000),
            (25000, 0),
            (25000, 25000),
            (75000, 25000),
            (75000, 0),
            (U, 50000),
            (75000, U),
            (75000, 75000),
            (25000, 75000),
            (25000, U),
        ]),
        "bentConnector2" => ShapeKind::Custom(vec![VectorPath {
            commands: vec![C::Move(0, 0), C::Line(U, 0), C::Line(U, U)],
            fill: false,
            stroke: true,
        }]),
        "bentConnector3" => {
            let a = adjustment("adj1", 50000, U);
            ShapeKind::Custom(vec![VectorPath {
                commands: vec![C::Move(0, 0), C::Line(a, 0), C::Line(a, U), C::Line(U, U)],
                fill: false,
                stroke: true,
            }])
        }
        _ => return None,
    })
}
fn custom(n: &XmlNode, vars: &BTreeMap<String, f64>, w: f64, h: f64) -> Option<ShapeKind> {
    let mut paths = vec![];
    let mut budget = 10000;
    for path in n.child("pathLst")?.children_named("path").take(256) {
        let pw = path
            .attr("w")
            .map(|v| value(v, vars))
            .filter(|v| *v > 0.0)
            .unwrap_or(w);
        let ph = path
            .attr("h")
            .map(|v| value(v, vars))
            .filter(|v| *v > 0.0)
            .unwrap_or(h);
        let norm = |x: f64, y: f64| {
            (
                (x / pw * 100000.0).clamp(-1e9, 1e9) as i32,
                (y / ph * 100000.0).clamp(-1e9, 1e9) as i32,
            )
        };
        let point = |n: &XmlNode| {
            (
                value(n.attr("x").unwrap_or("0"), vars),
                value(n.attr("y").unwrap_or("0"), vars),
            )
        };
        let mut commands = vec![];
        let mut current = (0.0, 0.0);
        let mut first = current;
        for command in &path.children {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let pts: Vec<_> = command.children_named("pt").take(3).map(point).collect();
            match command.local_name() {
                "moveTo" | "lnTo" => {
                    if let Some(&(x, y)) = pts.first() {
                        let p = norm(x, y);
                        current = (x, y);
                        commands.push(if command.local_name() == "moveTo" {
                            first = current;
                            C::Move(p.0, p.1)
                        } else {
                            C::Line(p.0, p.1)
                        });
                    }
                }
                "cubicBezTo" if pts.len() == 3 => {
                    commands.push(C::Cubic([
                        norm(pts[0].0, pts[0].1),
                        norm(pts[1].0, pts[1].1),
                        norm(pts[2].0, pts[2].1),
                    ]));
                    current = pts[2];
                }
                "quadBezTo" if pts.len() == 2 => {
                    let a = (
                        current.0 + 2.0 / 3.0 * (pts[0].0 - current.0),
                        current.1 + 2.0 / 3.0 * (pts[0].1 - current.1),
                    );
                    let b = (
                        pts[1].0 + 2.0 / 3.0 * (pts[0].0 - pts[1].0),
                        pts[1].1 + 2.0 / 3.0 * (pts[0].1 - pts[1].1),
                    );
                    commands.push(C::Cubic([
                        norm(a.0, a.1),
                        norm(b.0, b.1),
                        norm(pts[1].0, pts[1].1),
                    ]));
                    current = pts[1];
                }
                "arcTo" => {
                    let attr = |key| value(command.attr(key).unwrap_or("0"), vars);
                    let (rx, ry) = (attr("wR").abs(), attr("hR").abs());
                    let start = (attr("stAng") / 60000.0).to_radians();
                    let sweep = (attr("swAng") / 60000.0).clamp(-360.0, 360.0).to_radians();
                    let steps =
                        (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
                    let center = (current.0 - rx * start.cos(), current.1 - ry * start.sin());
                    for i in 0..steps {
                        let a = start + sweep * i as f64 / steps as f64;
                        let b = start + sweep * (i + 1) as f64 / steps as f64;
                        let k = 4.0 / 3.0 * ((b - a) / 4.0).tan();
                        let p1 = (
                            center.0 + rx * (a.cos() - k * a.sin()),
                            center.1 + ry * (a.sin() + k * a.cos()),
                        );
                        let p2 = (
                            center.0 + rx * (b.cos() + k * b.sin()),
                            center.1 + ry * (b.sin() - k * b.cos()),
                        );
                        current = (center.0 + rx * b.cos(), center.1 + ry * b.sin());
                        commands.push(C::Cubic([
                            norm(p1.0, p1.1),
                            norm(p2.0, p2.1),
                            norm(current.0, current.1),
                        ]));
                    }
                }
                "close" => {
                    commands.push(C::Close);
                    current = first;
                }
                _ => {}
            }
        }
        if !commands.is_empty() {
            paths.push(VectorPath {
                commands,
                fill: path.attr("fill") != Some("none"),
                stroke: !path
                    .attr("stroke")
                    .is_some_and(|v| matches!(v, "0" | "false")),
            });
        }
    }
    (!paths.is_empty()).then_some(ShapeKind::Custom(paths))
}
pub(crate) fn flip(kind: &mut ShapeKind, horizontal: bool, vertical: bool) {
    if let ShapeKind::Custom(paths) = kind {
        for path in paths {
            for c in &mut path.commands {
                let point = |x: &mut i32, y: &mut i32| {
                    if horizontal {
                        *x = U.saturating_sub(*x);
                    }
                    if vertical {
                        *y = U.saturating_sub(*y);
                    }
                };
                match c {
                    C::Move(x, y) | C::Line(x, y) => point(x, y),
                    C::Cubic(points) => {
                        for (x, y) in points {
                            point(x, y)
                        }
                    }
                    C::Close => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_guides_and_quadratic_curves_preserve_endpoints() {
        let n=lo_core::parse_xml_document(r#"<custGeom><avLst><gd name="adj" fmla="val 25000"/></avLst><gdLst><gd name="center" fmla="*/ w 1 2"/></gdLst><pathLst><path w="100000" h="100000" fill="none"><moveTo><pt x="0" y="0"/></moveTo><quadBezTo><pt x="50000" y="100000"/><pt x="100000" y="0"/></quadBezTo></path></pathLst></custGeom>"#).unwrap();
        let ShapeKind::Custom(paths) = geometry(&n, 100.0, 100.0).unwrap() else {
            panic!()
        };
        assert!(!paths[0].fill);
        assert_eq!(paths[0].commands[0], C::Move(0, 0));
        let C::Cubic(points) = paths[0].commands[1] else {
            panic!()
        };
        assert_eq!(points[2], (100000, 0));
        assert!((points[0].0 - 33333).abs() <= 1);
    }
    #[test]
    fn arrows_are_polygons_and_flips_mirror_the_tip() {
        let n = lo_core::parse_xml_document("<prstGeom prst=\"rightArrow\"/>").unwrap();
        let mut shape = geometry(&n, 200.0, 100.0).unwrap();
        let ShapeKind::Custom(paths) = &shape else {
            panic!()
        };
        assert!(paths[0].commands.contains(&C::Line(100000, 50000)));
        flip(&mut shape, true, false);
        let ShapeKind::Custom(paths) = shape else {
            panic!()
        };
        assert!(paths[0].commands.contains(&C::Line(0, 50000)));
    }
    #[test]
    fn formulas_never_produce_non_finite_coordinates() {
        let vars = BTreeMap::new();
        for f in [
            "*/ 1 1 0",
            "sqrt -9",
            "tan 1 5400000",
            "val NaN",
            "*/ 1e308 1e308 1",
        ] {
            assert!(formula(f, &vars).is_finite());
        }
    }
}
