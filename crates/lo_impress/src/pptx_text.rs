//! Rich slide text uses the same embedded source fonts for measurement and PDF output.
use lo_core::{calc::CellTextRun, PdfFont, TextBoxStyle};
#[derive(Clone)]
struct Fragment {
    text: String,
    font: PdfFont,
    size: f32,
    color: (f32, f32, f32),
    underline: bool,
    width: f32,
}
#[derive(Default)]
struct Line {
    parts: Vec<Fragment>,
    width: f32,
    size: f32,
}
fn font(r: &CellTextRun) -> PdfFont {
    lo_core::source_font::resolve(&r.style.font_family, r.style.bold, r.style.italic).unwrap_or(
        match (r.style.bold, r.style.italic) {
            (true, true) => PdfFont::HelveticaBoldOblique,
            (true, false) => PdfFont::HelveticaBold,
            (false, true) => PdfFont::HelveticaOblique,
            _ => PdfFont::Helvetica,
        },
    )
}
fn layout(runs: &[CellTextRun], width: f32) -> Vec<Line> {
    let mut lines = vec![];
    let mut line = Line::default();
    let mut word: Vec<Fragment> = vec![];
    let flush = |word: &mut Vec<Fragment>, line: &mut Line, lines: &mut Vec<Line>| {
        let w: f32 = word.iter().map(|p| p.width).sum();
        if line.width + w > width && !line.parts.is_empty() {
            lines.push(std::mem::take(line));
        }
        if w > width {
            for p in word.drain(..) {
                for ch in p.text.chars() {
                    let text = ch.to_string();
                    let cw = lo_core::font_metrics::measure_base_font(&text, p.size, p.font);
                    if line.width + cw > width && !line.parts.is_empty() {
                        lines.push(std::mem::take(line));
                    }
                    line.width += cw;
                    line.size = line.size.max(p.size);
                    if let Some(last) = line.parts.last_mut().filter(|last| {
                        last.font == p.font
                            && last.size == p.size
                            && last.color == p.color
                            && last.underline == p.underline
                    }) {
                        last.text.push(ch);
                        last.width += cw;
                    } else {
                        line.parts.push(Fragment {
                            text,
                            width: cw,
                            ..p.clone()
                        });
                    }
                }
            }
            return;
        }
        for p in word.drain(..) {
            line.width += p.width;
            line.size = line.size.max(p.size);
            line.parts.push(p);
        }
    };
    for r in runs {
        let size = r.style.font_size_mpt.unwrap_or(18000) as f32 / 1000.0;
        let font = font(r);
        let color = crate::pdf::parse_color(&r.style.foreground).unwrap_or((0.0, 0.0, 0.0));
        let fragment = |s: String| Fragment {
            width: lo_core::font_metrics::measure_base_font(&s, size, font),
            text: s,
            font,
            size,
            color,
            underline: r.style.underline,
        };
        let mut text = String::new();
        for ch in r.text.chars() {
            if ch.is_whitespace() {
                if !text.is_empty() {
                    word.push(fragment(std::mem::take(&mut text)));
                }
                flush(&mut word, &mut line, &mut lines);
                if ch == '\n' {
                    line.size = line.size.max(size);
                    lines.push(std::mem::take(&mut line));
                } else if !line.parts.is_empty() {
                    let p = fragment(" ".into());
                    line.width += p.width;
                    line.parts.push(p);
                }
            } else {
                text.push(ch);
            }
        }
        if !text.is_empty() {
            word.push(fragment(text));
        }
    }
    flush(&mut word, &mut line, &mut lines);
    if !line.parts.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}
fn text_height(lines: &[Line], spacing: f32) -> f32 {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| l.size.max(1.0) * if i + 1 == lines.len() { 1.0 } else { spacing })
        .sum()
}
pub(crate) fn height(runs: &[CellTextRun], width: f32, spacing: f32) -> f32 {
    text_height(&layout(runs, width), spacing)
}
pub(crate) fn render(
    page: &mut lo_core::PdfPage,
    top: f32,
    x: f32,
    width: f32,
    height: f32,
    style: &TextBoxStyle,
) {
    let lines = layout(&style.runs, if style.wrap_text { width } else { 1e8 });
    let spacing = if style.line_spacing > 0.0 {
        style.line_spacing
    } else {
        1.2
    };
    let full = text_height(&lines, spacing);
    let mut y = top
        - match style.vertical_alignment {
            lo_core::style::Alignment::Center => (height - full).max(0.0) / 2.0,
            lo_core::style::Alignment::End => (height - full).max(0.0),
            _ => 0.0,
        };
    for line in lines {
        y -= line.size;
        if y < top - height - 0.5 {
            break;
        }
        let mut cursor = x + match style.alignment {
            lo_core::style::Alignment::Center => (width - line.width).max(0.0) / 2.0,
            lo_core::style::Alignment::End => (width - line.width).max(0.0),
            _ => 0.0,
        };
        for p in line.parts {
            page.text_rgb(
                cursor, y, p.size, p.font, &p.text, p.color.0, p.color.1, p.color.2,
            );
            if p.underline {
                page.line_rgb(
                    cursor,
                    y - 1.0,
                    cursor + p.width,
                    y - 1.0,
                    p.color.0,
                    p.color.1,
                    p.color.2,
                );
            }
            cursor += p.width;
        }
        y -= line.size * (spacing - 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tight_line_spacing_keeps_last_baseline_inside_frame() {
        let run = CellTextRun {
            text: "One\nTwo".into(),
            style: lo_core::style::CellStyle {
                font_size_mpt: Some(32000),
                ..Default::default()
            },
        };
        assert!((height(&[run], 500.0, 0.84) - (32.0 * 0.84 + 32.0)).abs() < 0.01);
    }
    #[test]
    fn rich_run_boundaries_do_not_force_line_breaks() {
        let run = |text: &str, bold| CellTextRun {
            text: text.into(),
            style: lo_core::style::CellStyle {
                bold,
                font_size_mpt: Some(12000),
                ..Default::default()
            },
        };
        let lines = layout(&[run("Hel", false), run("lo", true)], 200.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].parts.len(), 2);
    }
    #[test]
    fn long_words_wrap_without_splitting_pdf_runs_per_character() {
        let run = CellTextRun {
            text: "abcdefghijklmnopqrstuvwxyz".into(),
            style: lo_core::style::CellStyle {
                font_size_mpt: Some(12000),
                ..Default::default()
            },
        };
        let lines = layout(&[run], 45.0);
        assert!(lines.len() > 1);
        assert!(lines
            .iter()
            .all(|line| line.parts.len() == 1 && line.width <= 45.01));
        assert_eq!(
            lines
                .iter()
                .flat_map(|line| line.parts.iter())
                .map(|p| p.text.as_str())
                .collect::<String>(),
            "abcdefghijklmnopqrstuvwxyz"
        );
    }
}
