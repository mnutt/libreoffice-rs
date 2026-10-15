//! Normalized presentation geometry: 100000 units span the shape frame.
use crate::{PdfPage, RasterImage, Rgba};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathCommand {
    Move(i32, i32),
    Line(i32, i32),
    Cubic([(i32, i32); 3]),
    Close,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorPath {
    pub commands: Vec<PathCommand>,
    pub fill: bool,
    pub stroke: bool,
}
impl VectorPath {
    pub fn polygon(points: &[(i32, i32)]) -> Self {
        let mut commands = Vec::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            commands.push(if i == 0 {
                PathCommand::Move(x, y)
            } else {
                PathCommand::Line(x, y)
            });
        }
        commands.push(PathCommand::Close);
        Self {
            commands,
            fill: true,
            stroke: true,
        }
    }
    pub fn svg_data(&self, x: f32, y: f32, w: f32, h: f32) -> String {
        let point = |a: i32, b: i32| {
            format!(
                "{} {}",
                x + w * a as f32 / 100000.0,
                y + h * b as f32 / 100000.0
            )
        };
        let mut out = String::new();
        for command in &self.commands {
            match command {
                PathCommand::Move(a, b) => out.push_str(&format!("M {} ", point(*a, *b))),
                PathCommand::Line(a, b) => out.push_str(&format!("L {} ", point(*a, *b))),
                PathCommand::Cubic(p) => out.push_str(&format!(
                    "C {} {} {} ",
                    point(p[0].0, p[0].1),
                    point(p[1].0, p[1].1),
                    point(p[2].0, p[2].1)
                )),
                PathCommand::Close => out.push_str("Z "),
            }
        }
        out
    }
    pub fn render_pdf(
        &self,
        page: &mut PdfPage,
        frame: [f32; 4],
        fill: Option<(f32, f32, f32)>,
        stroke: Option<(f32, f32, f32)>,
    ) {
        let [x, top, w, h] = frame;
        let point = |a: i32, b: i32| {
            format!(
                "{} {}",
                x + w * a as f32 / 100000.0,
                top - h * b as f32 / 100000.0
            )
        };
        let fill = fill.filter(|_| self.fill);
        let stroke = stroke.filter(|_| self.stroke);
        page.raw("q");
        if let Some((r, g, b)) = fill {
            page.raw(&format!("{r} {g} {b} rg"));
        }
        if let Some((r, g, b)) = stroke {
            page.raw(&format!("{r} {g} {b} RG"));
        }
        for command in &self.commands {
            match command {
                PathCommand::Move(a, b) => page.raw(&format!("{} m", point(*a, *b))),
                PathCommand::Line(a, b) => page.raw(&format!("{} l", point(*a, *b))),
                PathCommand::Cubic(p) => page.raw(&format!(
                    "{} {} {} c",
                    point(p[0].0, p[0].1),
                    point(p[1].0, p[1].1),
                    point(p[2].0, p[2].1)
                )),
                PathCommand::Close => page.raw("h"),
            }
        }
        page.raw(match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B",
            (true, false) => "f",
            (false, true) => "S",
            _ => "n",
        });
        page.raw("Q");
    }
    pub fn render_raster(
        &self,
        image: &mut RasterImage,
        frame: [f32; 4],
        fill: Option<Rgba>,
        stroke: Option<Rgba>,
        line_width: i32,
    ) {
        let [x, y, w, h] = frame;
        let point = |a: i32, b: i32| (x + w * a as f32 / 100000.0, y + h * b as f32 / 100000.0);
        let mut paths: Vec<Vec<(f32, f32)>> = vec![];
        let mut path = vec![];
        let mut current = (0.0, 0.0);
        for c in &self.commands {
            match c {
                PathCommand::Move(a, b) => {
                    if !path.is_empty() {
                        paths.push(std::mem::take(&mut path));
                    }
                    current = point(*a, *b);
                    path.push(current);
                }
                PathCommand::Line(a, b) => {
                    current = point(*a, *b);
                    path.push(current);
                }
                PathCommand::Cubic(p) => {
                    let (a, b, c) = (
                        point(p[0].0, p[0].1),
                        point(p[1].0, p[1].1),
                        point(p[2].0, p[2].1),
                    );
                    for i in 1..=24 {
                        let t = i as f32 / 24.0;
                        let u = 1.0 - t;
                        path.push((
                            u * u * u * current.0
                                + 3.0 * u * u * t * a.0
                                + 3.0 * u * t * t * b.0
                                + t * t * t * c.0,
                            u * u * u * current.1
                                + 3.0 * u * u * t * a.1
                                + 3.0 * u * t * t * b.1
                                + t * t * t * c.1,
                        ));
                    }
                    current = c;
                }
                PathCommand::Close => {
                    if let Some(first) = path.first().copied() {
                        path.push(first);
                        current = first;
                    }
                }
            }
        }
        if !path.is_empty() {
            paths.push(path);
        }
        if let Some(fill) = fill.filter(|_| self.fill) {
            // Clip scanlines to the target canvas; malformed coordinates cannot cause unbounded loops.
            for row in 0..image.height as i32 {
                let scan = row as f32 + 0.5;
                let mut intersections = vec![];
                for path in &paths {
                    for i in 0..path.len() {
                        let a = path[i];
                        let b = path[(i + 1) % path.len()];
                        if (a.1 <= scan && b.1 > scan) || (b.1 <= scan && a.1 > scan) {
                            intersections.push(a.0 + (scan - a.1) * (b.0 - a.0) / (b.1 - a.1));
                        }
                    }
                }
                intersections.sort_by(f32::total_cmp);
                for pair in intersections.chunks_exact(2) {
                    let left = pair[0].ceil().clamp(0.0, image.width as f32) as i32;
                    let right = pair[1].ceil().clamp(0.0, image.width as f32) as i32;
                    image.fill_rect(left, row, right - left, 1, fill);
                }
            }
        }
        if let Some(stroke) = stroke.filter(|_| self.stroke) {
            for path in paths {
                for pair in path.windows(2) {
                    if let Some((a, b)) =
                        clip_segment(pair[0], pair[1], image.width as f32, image.height as f32)
                    {
                        image.draw_line(
                            a.0 as i32, a.1 as i32, b.0 as i32, b.1 as i32, line_width, stroke,
                        );
                    }
                }
            }
        }
    }
}

fn clip_segment(a: (f32, f32), b: (f32, f32), w: f32, h: f32) -> Option<((f32, f32), (f32, f32))> {
    if ![a.0, a.1, b.0, b.1, w, h].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for (p, q) in [(-dx, a.0), (dx, w - a.0), (-dy, a.1), (dy, h - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let ratio = q / p;
            if p < 0.0 {
                low = low.max(ratio);
            } else {
                high = high.min(ratio);
            }
            if low > high {
                return None;
            }
        }
    }
    Some((
        (a.0 + low * dx, a.1 + low * dy),
        (a.0 + high * dx, a.1 + high * dy),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raster_clips_extreme_segments_without_overflow() {
        let p = VectorPath {
            commands: vec![
                PathCommand::Move(-1_000_000_000, 50000),
                PathCommand::Line(1_000_000_000, 50000),
            ],
            fill: false,
            stroke: true,
        };
        let mut image = RasterImage::new(20, 20, Rgba::WHITE);
        p.render_raster(
            &mut image,
            [0.0, 0.0, 20.0, 20.0],
            None,
            Some(Rgba::rgba(0, 0, 0, 255)),
            1,
        );
        assert!(image.pixels.chunks_exact(4).any(|p| p[0] == 0));
        assert!(clip_segment((f32::NAN, 0.0), (1.0, 1.0), 20.0, 20.0).is_none());
    }
    #[test]
    fn triangle_does_not_fill_its_bounding_rectangle() {
        let p = VectorPath::polygon(&[(50000, 0), (100000, 100000), (0, 100000)]);
        let mut image = RasterImage::new(20, 20, Rgba::WHITE);
        p.render_raster(
            &mut image,
            [0.0, 0.0, 20.0, 20.0],
            Some(Rgba::rgba(255, 0, 0, 255)),
            None,
            1,
        );
        assert_eq!(&image.pixels[..4], &[255, 255, 255, 255]);
        let center = (10 * 20 + 10) * 4;
        assert_eq!(&image.pixels[center..center + 4], &[255, 0, 0, 255]);
    }
}
