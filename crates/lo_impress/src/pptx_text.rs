//! Rich slide text uses the same embedded source fonts for measurement and PDF output.
use lo_core::{calc::CellTextRun, PdfFont, TextBoxStyle};
pub(crate) fn number_marker(number: u32, kind: &str) -> String {
    let mut text = if kind.starts_with("alpha") {
        let mut n = number.max(1);
        let mut chars = vec![];
        while n > 0 {
            n -= 1;
            chars.push((b'a' + (n % 26) as u8) as char);
            n /= 26;
        }
        chars.into_iter().rev().collect::<String>()
    } else if kind.starts_with("roman") && number <= 3999 {
        let mut n = number;
        let mut s = String::new();
        for (value, symbol) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while n >= value {
                s.push_str(symbol);
                n -= value;
            }
        }
        s
    } else {
        number.to_string()
    };
    if kind.contains("Uc") {
        text.make_ascii_uppercase();
    }
    if kind.ends_with("ParenBoth") {
        format!("({text})")
    } else if kind.ends_with("ParenR") {
        format!("{text})")
    } else if kind.ends_with("Plain") {
        text
    } else {
        format!("{text}.")
    }
}
#[derive(Clone)]
struct Fragment {
    source_style: lo_core::style::CellStyle,
    text: String,
    font: PdfFont,
    size: f32,
    color: (f32, f32, f32),
    underline: bool,
    background: Option<(f32, f32, f32)>,
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
pub(crate) fn natural_line_height(runs: &[CellTextRun]) -> f32 {
    runs.iter()
        .map(|r| {
            let ratio = match font(r) {
                PdfFont::Source(id) => lo_core::source_font::line_height_ratio(id).unwrap_or(1.2),
                _ => 1.2,
            };
            r.style.font_size_mpt.unwrap_or(18000) as f32 / 1000.0 * ratio
        })
        .fold(0.0, f32::max)
}
pub(crate) fn descent(runs: &[CellTextRun]) -> f32 {
    runs.iter()
        .map(|r| {
            let ratio = match font(r) {
                PdfFont::Source(id) => lo_core::source_font::descent_ratio(id).unwrap_or(0.2),
                _ => 0.2,
            };
            r.style.font_size_mpt.unwrap_or(18000) as f32 / 1000.0 * ratio
        })
        .fold(0.0, f32::max)
}
#[cfg(test)]
fn layout(runs: &[CellTextRun], width: f32) -> Vec<Line> {
    layout_indented(runs, width, 0.0)
}
fn layout_indented(runs: &[CellTextRun], width: f32, indent: f32) -> Vec<Line> {
    let mut lines = vec![];
    let mut line = Line::default();
    let mut word: Vec<Fragment> = vec![];
    let flush = |word: &mut Vec<Fragment>, line: &mut Line, lines: &mut Vec<Line>| {
        let w: f32 = word.iter().map(|p| p.width).sum();
        let available =
            |lines: &Vec<Line>| (width - if lines.is_empty() { indent } else { 0.0 }).max(1.0);
        if line.width + w > available(lines) && !line.parts.is_empty() {
            lines.push(std::mem::take(line));
        }
        if w > available(lines) {
            for p in word.drain(..) {
                for ch in p.text.chars() {
                    let text = ch.to_string();
                    let cw = lo_core::font_metrics::measure_base_font(&text, p.size, p.font);
                    if line.width + cw > available(lines) && !line.parts.is_empty() {
                        lines.push(std::mem::take(line));
                    }
                    line.width += cw;
                    line.size = line.size.max(p.size);
                    if let Some(last) = line.parts.last_mut().filter(|last| {
                        last.font == p.font
                            && last.size == p.size
                            && last.color == p.color
                            && last.underline == p.underline
                            && last.background == p.background
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
            source_style: r.style.clone(),
            width: lo_core::font_metrics::measure_base_font(&s, size, font),
            text: s,
            font,
            size,
            color,
            underline: r.style.underline,
            background: crate::pdf::parse_color(&r.style.background),
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
                } else {
                    line.size = line.size.max(size);
                    if !line.parts.is_empty() {
                        let p = fragment(" ".into());
                        line.width += p.width;
                        line.parts.push(p);
                    }
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
#[cfg(test)]
pub(crate) fn height(runs: &[CellTextRun], width: f32, spacing: f32) -> f32 {
    text_height(&layout(runs, width), spacing)
}
pub(crate) fn first_line_indent(style: &TextBoxStyle) -> f32 {
    if style.paragraph_markers.first().is_some_and(|m| m.is_some()) {
        0.0
    } else {
        style.paragraph_indents.first().copied().unwrap_or(0.0)
    }
}
pub(crate) fn height_indented(runs: &[CellTextRun], width: f32, spacing: f32, indent: f32) -> f32 {
    text_height(&layout_indented(runs, width, indent), spacing)
}
/// Split at measured visual lines while retaining rich-run properties. Used to
/// flow a paragraph across columns without losing its font/color boundaries.
pub(crate) fn wrapped_runs(
    runs: &[CellTextRun],
    width: f32,
    indent: f32,
) -> Vec<(Vec<CellTextRun>, f32)> {
    layout_indented(runs, width, indent)
        .into_iter()
        .map(|line| {
            let size = line.size.max(1.0);
            let mut parts: Vec<_> = line
                .parts
                .into_iter()
                .map(|p| CellTextRun {
                    text: p.text,
                    style: p.source_style,
                })
                .collect();
            if parts.is_empty() {
                parts.push(CellTextRun {
                    text: " ".into(),
                    style: lo_core::style::CellStyle {
                        font_size_mpt: Some((size * 1000.0) as u32),
                        ..Default::default()
                    },
                });
            }
            (parts, size)
        })
        .collect()
}
pub(crate) fn render(
    page: &mut lo_core::PdfPage,
    top: f32,
    x: f32,
    width: f32,
    height: f32,
    style: &TextBoxStyle,
) {
    let first_indent = first_line_indent(style);
    let lines = layout_indented(
        &style.runs,
        if style.wrap_text { width } else { 1e8 },
        first_indent,
    );
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
    for (index, line) in lines.into_iter().enumerate() {
        y -= line.size;
        if y < top - height - 0.5 {
            break;
        }
        if index == 0 {
            if let Some(Some(marker)) = style.paragraph_markers.first() {
                if let Some(run) = style.runs.first() {
                    let color =
                        crate::pdf::parse_color(&run.style.foreground).unwrap_or((0.0, 0.0, 0.0));
                    page.text_rgb(
                        x + style.paragraph_indents.first().copied().unwrap_or(0.0),
                        y,
                        style.font_size_pt.max(1.0),
                        font(run),
                        marker,
                        color.0,
                        color.1,
                        color.2,
                    );
                }
            }
        }
        let indent = if index == 0 { first_indent } else { 0.0 };
        let mut cursor = x
            + indent
            + match style.alignment {
                lo_core::style::Alignment::Center => (width - indent - line.width).max(0.0) / 2.0,
                lo_core::style::Alignment::End => (width - indent - line.width).max(0.0),
                _ => 0.0,
            };
        for p in line.parts {
            if let Some((r, g, b)) = p.background {
                page.rect_fill_rgb(cursor, y - p.size * 0.2, p.width, p.size * 1.2, r, g, b);
            }
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
    fn blank_paragraph_retains_font_height_for_autofit_measurement() {
        let run = CellTextRun {
            text: " ".into(),
            style: lo_core::style::CellStyle {
                font_size_mpt: Some(30000),
                ..Default::default()
            },
        };
        assert_eq!(height(&[run.clone()], 100.0, 1.2), 30.0);
        assert_eq!(wrapped_runs(&[run], 100.0, 0.0)[0].1, 30.0);
    }
    #[test]
    fn first_line_indent_changes_only_first_line_width() {
        let run = CellTextRun {
            text: "one two three four five six seven eight".into(),
            style: lo_core::style::CellStyle {
                font_size_mpt: Some(12000),
                ..Default::default()
            },
        };
        let lines = layout_indented(&[run], 65.0, -30.0);
        assert!(lines[0].width > 65.0 && lines[0].width <= 95.01);
        assert!(lines.iter().skip(1).all(|l| l.width <= 65.01));
    }
    #[test]
    fn numbering_styles_cover_letter_rollover_and_roman_subtraction() {
        assert_eq!(number_marker(27, "alphaUcPeriod"), "AA.");
        assert_eq!(number_marker(14, "romanLcParenBoth"), "(xiv)");
        assert_eq!(number_marker(1, "arabicParenR"), "1)");
        assert_eq!(number_marker(u32::MAX, "romanUcPeriod"), "4294967295.");
    }
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
    #[test]
    fn wrapping_keeps_different_highlight_runs_separate() {
        let run = |text: &str, color: &str| CellTextRun {
            text: text.into(),
            style: lo_core::style::CellStyle {
                background: color.into(),
                font_size_mpt: Some(12000),
                ..Default::default()
            },
        };
        let lines = layout(&[run("AA", "#00FF00"), run("BB", "#FF0000")], 25.0);
        let parts: Vec<_> = lines.iter().flat_map(|l| &l.parts).collect();
        assert!(parts.iter().any(|p| p.background == Some((0.0, 1.0, 0.0))));
        assert!(parts.iter().any(|p| p.background == Some((1.0, 0.0, 0.0))));
        assert!(!parts.iter().any(|p| p.text.contains("AB")));
    }
}
