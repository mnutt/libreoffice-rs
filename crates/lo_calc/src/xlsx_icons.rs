//! Small vector icons, independent of platform fonts and bitmap assets.
use lo_core::{calc::ConditionalIcon, PdfPage};

pub(crate) fn render(
    page: &mut PdfPage,
    icon: &ConditionalIcon,
    x: f32,
    top: f32,
    width: f32,
    height: f32,
) -> f32 {
    let size = 12.0_f32
        .min((height - 4.0).max(0.0))
        .min((width - 4.0).max(0.0));
    if size <= 0.0 {
        return 0.0;
    }
    let left = x + 2.0;
    let bottom = top - (height + size) / 2.0;
    let paint = if icon.set == "3ArrowsGray" {
        (0.55, 0.55, 0.55)
    } else {
        match icon.rank {
            0 => (0.95, 0.25, 0.25),
            1 => (1.0, 0.7, 0.05),
            _ => (0.25, 0.7, 0.35),
        }
    };
    let border = (paint.0 * 0.65, paint.1 * 0.65, paint.2 * 0.65);
    page.raw(&format!(
        "q {x} {} {width} {height} re W n 0.5 w",
        top - height
    ));
    match icon.set.as_str() {
        "3TrafficLights1" => page.ellipse_fill_stroke_rgb(
            left + size / 2.0,
            bottom + size / 2.0,
            size * 0.4,
            size * 0.4,
            paint,
            border,
        ),
        "3Flags" => {
            page.line_rgb(
                left + size * 0.2,
                bottom,
                left + size * 0.2,
                bottom + size,
                0.3,
                0.3,
                0.3,
            );
            let points = [(0.2, 1.0), (0.95, 0.9), (0.95, 0.4), (0.2, 0.5)]
                .map(|(a, b)| (left + a * size, bottom + b * size));
            page.polygon_fill_stroke_rgb(&points, paint, border);
        }
        _ => {
            let points = [
                (0.3, 0.0),
                (0.7, 0.0),
                (0.7, 0.55),
                (1.0, 0.55),
                (0.5, 1.0),
                (0.0, 0.55),
                (0.3, 0.55),
            ]
            .map(|(a, b)| {
                let (a, b) = match icon.rank {
                    0 => (1.0 - a, 1.0 - b),
                    1 => (b, 1.0 - a),
                    _ => (a, b),
                };
                (left + a * size, bottom + b * size)
            });
            page.polygon_fill_stroke_rgb(&points, paint, border);
        }
    }
    page.raw("Q");
    size + 4.0
}
