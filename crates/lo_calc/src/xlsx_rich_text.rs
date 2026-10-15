//! Rich cell text: resolve saved run styles and lay out words across run boundaries.
use lo_core::{
    calc::CellTextRun,
    style::{Alignment, CellStyle},
    PdfFont, PdfPage, XmlNode,
};

pub(crate) fn parse(node: &XmlNode, base: &CellStyle, theme: &[String]) -> Vec<CellTextRun> {
    if node.children_named("r").count() > 4096 {
        eprintln!("XLSX preview: excessive rich-text runs; preserving text with cell style");
        return Vec::new();
    }
    node.children_named("r")
        .map(|run| {
            let mut style = base.clone();
            if let Some(p) = run.child("rPr") {
                for (key, target) in [
                    ("b", &mut style.bold),
                    ("i", &mut style.italic),
                    ("u", &mut style.underline),
                ] {
                    if let Some(n) = p.child(key) {
                        *target = !matches!(n.attr("val"), Some("0" | "false" | "none"));
                    }
                }
                if let Some(name) = p.child("rFont").and_then(|n| n.attr("val")) {
                    style.font_family = name.into();
                }
                if let Some(size) = p
                    .child("sz")
                    .and_then(|n| n.attr("val"))
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    style.font_size_mpt = Some((size.clamp(1.0, 409.0) * 1000.0).round() as u32);
                }
                if p.child("color").is_some() {
                    style.foreground = crate::xlsx_styles::color(p.child("color"), theme, &[]);
                }
            }
            CellTextRun {
                text: run.child("t").map(|t| t.text_content()).unwrap_or_default(),
                style,
            }
        })
        .collect()
}
#[derive(Clone)]
struct Fragment {
    text: String,
    font: PdfFont,
    size: f32,
    width: f32,
    color: (f32, f32, f32),
    underline: bool,
}
struct Line {
    fragments: Vec<Fragment>,
    width: f32,
    height: f32,
    size: f32,
}
impl Line {
    fn new() -> Self {
        Self {
            fragments: Vec::new(),
            width: 0.0,
            height: 0.0,
            size: 0.0,
        }
    }
    fn push(&mut self, f: Fragment) {
        self.width += f.width;
        self.size = self.size.max(f.size);
        self.height = self.height.max(f.size * 1.2);
        self.fragments.push(f);
    }
}
fn layout(runs: &[CellTextRun], width: f32, wrap: bool) -> Vec<Line> {
    // A word can span multiple style runs. Keep it together unless it is wider
    // than a whole line; preserve spaces, including nonbreaking spaces.
    let mut lines = Vec::new();
    let mut line = Line::new();
    let mut word: Vec<Fragment> = Vec::new();
    let flush = |word: &mut Vec<Fragment>, line: &mut Line, lines: &mut Vec<Line>| {
        let total: f32 = word.iter().map(|f| f.width).sum();
        if wrap && line.width > 0.0 && line.width + total > width {
            lines.push(std::mem::replace(line, Line::new()));
        }
        for fragment in word.drain(..) {
            if wrap && total > width {
                for ch in fragment.text.chars() {
                    let mut f = fragment.clone();
                    f.text = ch.to_string();
                    f.width = lo_core::font_metrics::measure_base_font(&f.text, f.size, f.font);
                    if line.width > 0.0 && line.width + f.width > width {
                        lines.push(std::mem::replace(line, Line::new()));
                    }
                    line.push(f);
                }
            } else {
                line.push(fragment);
            }
        }
    };
    for run in runs {
        let font = crate::pdf::cell_font(Some(&run.style));
        let size = run
            .style
            .font_size_mpt
            .map(|v| v as f32 / 1000.0)
            .unwrap_or(f32::from(run.style.font_size_pt).max(10.0));
        let color = crate::pdf::grid_color(&run.style.foreground).unwrap_or((0.0, 0.0, 0.0));
        let mut text = String::new();
        let fragment = |text: String| Fragment {
            width: lo_core::font_metrics::measure_base_font(&text, size, font),
            text,
            font,
            size,
            color,
            underline: run.style.underline,
        };
        for ch in run.text.chars() {
            if matches!(ch, ' ' | '\t' | '\n') {
                if !text.is_empty() {
                    word.push(fragment(std::mem::take(&mut text)));
                }
                flush(&mut word, &mut line, &mut lines);
                if ch == '\n' {
                    if line.height == 0.0 {
                        line.height = size * 1.2;
                        line.size = size;
                    }
                    lines.push(std::mem::replace(&mut line, Line::new()));
                } else {
                    line.push(fragment(if ch == '\t' {
                        "    ".into()
                    } else {
                        " ".into()
                    }));
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
    if !line.fragments.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}
pub(crate) fn height(runs: &[CellTextRun], width: f32, wrap: bool) -> f32 {
    layout(runs, width.max(1.0), wrap)
        .iter()
        .map(|line| line.height)
        .sum::<f32>()
        + 3.0
}
pub(crate) fn render(
    page: &mut PdfPage,
    runs: &[CellTextRun],
    style: &CellStyle,
    x: f32,
    top: f32,
    width: f32,
    height: f32,
    visible_width: f32,
    indent: f32,
) {
    let lines = layout(runs, (width - 4.0 - indent).max(1.0), style.wrap);
    let total: f32 = lines.iter().map(|l| l.height).sum();
    let mut y = match style.vertical_alignment {
        Some(Alignment::End) => top - height + total + 1.0,
        Some(Alignment::Center) => top - (height - total) / 2.0,
        _ => top - 2.0,
    };
    page.raw(&format!(
        "q {x} {} {visible_width} {height} re W n",
        top - height
    ));
    for line in lines {
        let offset = match style.alignment {
            Alignment::End => (width - 2.0 - indent - line.width).max(2.0),
            Alignment::Center => ((width - line.width) / 2.0).max(2.0),
            _ => 2.0 + indent,
        };
        let baseline = y - line.size * 0.918;
        if baseline < top - height {
            break;
        }
        let mut cursor = x + offset;
        for f in line.fragments {
            page.text_rgb(
                cursor, baseline, f.size, f.font, &f.text, f.color.0, f.color.1, f.color.2,
            );
            if f.underline {
                page.line_rgb(
                    cursor,
                    baseline - 1.0,
                    cursor + f.width,
                    baseline - 1.0,
                    f.color.0,
                    f.color.1,
                    f.color.2,
                );
            }
            cursor += f.width;
        }
        y -= line.height;
    }
    page.raw("Q");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_runs_preserve_styles_and_share_one_word() {
        let node = lo_core::parse_xml_document(r#"<is><r><rPr><b/><color rgb="FFFF0000"/></rPr><t>Hel</t></r><r><rPr><i/><sz val="18"/></rPr><t>lo world</t></r></is>"#).unwrap();
        let base = CellStyle {
            font_size_mpt: Some(12000),
            wrap: true,
            ..Default::default()
        };
        let runs = parse(&node, &base, &[]);
        assert!(runs[0].style.bold);
        assert!(runs[1].style.italic);
        assert_eq!(runs[0].style.foreground, "#FF0000");
        let lines = layout(&runs, 60.0, true);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0]
                .fragments
                .iter()
                .map(|f| f.text.as_str())
                .collect::<String>()
                .trim_end(),
            "Hello"
        );
        assert_eq!(lines[0].size, 18.0);
        assert!(height(&runs, 60.0, true) > 40.0);
    }
}
