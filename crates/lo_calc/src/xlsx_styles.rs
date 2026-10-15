//! SpreadsheetML style resolution for the preview grid.
use crate::xlsx_format;
use lo_core::{
    style::{Alignment, CellStyle},
    XmlNode,
};

pub(crate) struct Styles {
    pub cells: Vec<CellStyle>,
    pub general_alignment: Vec<bool>,
}

pub(crate) fn positive(node: &XmlNode, attr: &str, fallback: f32) -> f32 {
    node.attr(attr)
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(fallback)
}
pub(crate) fn truth(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true"))
}
fn index(node: &XmlNode, key: &str) -> usize {
    node.attr(key).and_then(|s| s.parse().ok()).unwrap_or(0)
}
fn children<'a>(root: &'a XmlNode, key: &str) -> Vec<&'a XmlNode> {
    root.child(key)
        .map(|n| n.children.iter().map(|n| n.as_ref()).collect())
        .unwrap_or_default()
}
fn enabled(node: Option<&XmlNode>) -> bool {
    node.is_some_and(|n| !matches!(n.attr("val"), Some("0" | "false")))
}

fn theme_colors(theme: Option<&XmlNode>) -> Vec<String> {
    let default = [
        "FFFFFF", "000000", "EEECE1", "1F497D", "4F81BD", "C0504D", "9BBB59", "8064A2", "4BACC6",
        "F79646", "0000FF", "800080",
    ];
    let names = [
        "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5",
        "accent6", "hlink", "folHlink",
    ];
    let scheme = theme
        .and_then(|n| n.child("themeElements"))
        .and_then(|n| n.child("clrScheme"));
    names
        .iter()
        .zip(default)
        .map(|(name, fallback)| {
            scheme
                .and_then(|s| s.child(name))
                .and_then(|s| s.children.first())
                .and_then(|n| n.attr("lastClr").or_else(|| n.attr("val")))
                .unwrap_or(fallback)
                .to_string()
        })
        .collect()
}

fn color(node: Option<&XmlNode>, theme: &[String], indexed: &[String]) -> String {
    let Some(node) = node else {
        return String::new();
    };
    let defaults = [
        "000000", "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "000000",
        "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "800000", "008000",
        "000080", "808000", "800080", "008080", "C0C0C0", "808080",
    ];
    let value = if let Some(rgb) = node.attr("rgb") {
        Some(rgb)
    } else if let Some(t) = node.attr("theme").and_then(|s| s.parse::<usize>().ok()) {
        theme.get(t).map(String::as_str)
    } else if let Some(i) = node.attr("indexed").and_then(|s| s.parse::<usize>().ok()) {
        indexed
            .get(i)
            .map(String::as_str)
            .or_else(|| defaults.get(i).copied())
    } else {
        None
    };
    let Some(raw) = value else {
        return String::new();
    };
    if !raw.is_ascii() {
        return String::new();
    }
    let raw = if raw.len() == 8 { &raw[2..] } else { raw };
    if raw.len() != 6 || !raw.is_ascii() {
        return String::new();
    }
    let Ok(rgb) = u32::from_str_radix(raw, 16) else {
        return String::new();
    };
    let tint = node
        .attr("tint")
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|t| t.is_finite())
        .unwrap_or(0.0)
        .clamp(-1.0, 1.0);
    let channel = |shift| {
        let v = ((rgb >> shift) & 255u32) as f32;
        (if tint < 0.0 {
            v * (1.0 + tint)
        } else {
            v + (255.0 - v) * tint
        })
        .round() as u8
    };
    format!("#{:02X}{:02X}{:02X}", channel(16), channel(8), channel(0))
}

pub(crate) fn parse(root: Option<&XmlNode>, theme: Option<&XmlNode>) -> Styles {
    let default = CellStyle {
        font_size_pt: 11,
        font_size_mpt: Some(11000),
        font_family: "Calibri".into(),
        vertical_alignment: Some(Alignment::End),
        ..CellStyle::default()
    };
    let Some(root) = root else {
        return Styles {
            cells: vec![default],
            general_alignment: vec![true],
        };
    };
    let colors = theme_colors(theme);
    let indexed = root
        .child("colors")
        .and_then(|n| n.child("indexedColors"))
        .map(|n| {
            n.children
                .iter()
                .filter_map(|n| n.attr("rgb").map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let fonts = children(root, "fonts");
    let fills = children(root, "fills");
    let borders = children(root, "borders");
    let bases = children(root, "cellStyleXfs");
    let custom: std::collections::BTreeMap<_, _> = children(root, "numFmts")
        .iter()
        .filter_map(|n| {
            Some((
                n.attr("numFmtId")?.parse::<u32>().ok()?,
                n.attr("formatCode")?,
            ))
        })
        .collect();
    let mut cells = Vec::new();
    let mut general_alignment = Vec::new();
    for xf in children(root, "cellXfs") {
        let base = bases.get(index(xf, "xfId")).copied();
        let mut style = default.clone();
        let id = |name, apply| {
            if matches!(xf.attr(apply), Some("0" | "false")) {
                base.map(|b| index(b, name)).unwrap_or(0)
            } else {
                xf.attr(name)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| base.map(|b| index(b, name)).unwrap_or(0))
            }
        };
        if let Some(font) = fonts.get(id("fontId", "applyFont")) {
            style.font_family = font
                .child("name")
                .and_then(|n| n.attr("val"))
                .unwrap_or("Calibri")
                .into();
            let size = font
                .child("sz")
                .map(|n| positive(n, "val", 11.0))
                .unwrap_or(11.0)
                .clamp(1.0, 409.0);
            style.font_size_mpt = Some((size * 1000.0).round() as u32);
            style.font_size_pt = size.round() as u16;
            style.bold = enabled(font.child("b"));
            style.italic = enabled(font.child("i"));
            style.foreground = color(font.child("color"), &colors, &indexed);
        }
        if let Some(fill) = fills
            .get(id("fillId", "applyFill"))
            .and_then(|n| n.child("patternFill"))
        {
            if fill.attr("patternType") == Some("solid") {
                style.background = color(fill.child("fgColor"), &colors, &indexed);
            }
        }
        if let Some(border) = borders.get(id("borderId", "applyBorder")) {
            for (i, edge) in ["left", "right", "top", "bottom"].iter().enumerate() {
                if let Some(n) = border.child(edge) {
                    if let Some(kind) = n.attr("style").filter(|s| *s != "none") {
                        let width = match kind {
                            "hair" => 0.25,
                            "medium" | "mediumDashed" | "mediumDashDot" => 1.5,
                            "thick" => 2.25,
                            _ => 0.75,
                        };
                        let rgb = color(n.child("color"), &colors, &indexed);
                        style.borders[i] = format!(
                            "{width}pt {kind} {}",
                            if rgb.is_empty() { "#000000" } else { &rgb }
                        );
                    }
                }
            }
        }
        let fmt = id("numFmtId", "applyNumberFormat") as u32;
        style.format_code = custom
            .get(&fmt)
            .copied()
            .unwrap_or_else(|| xlsx_format::builtin(fmt))
            .into();
        let align = xf
            .child("alignment")
            .or_else(|| base.and_then(|n| n.child("alignment")));
        let horizontal = align.and_then(|n| n.attr("horizontal"));
        general_alignment.push(horizontal.is_none() || horizontal == Some("general"));
        style.alignment = match horizontal {
            Some("right") => Alignment::End,
            Some("center" | "centerContinuous") => Alignment::Center,
            Some("justify" | "distributed") => Alignment::Justify,
            _ => Alignment::Start,
        };
        if let Some(align) = align {
            style.wrap = truth(align.attr("wrapText"));
            style.vertical_alignment = Some(match align.attr("vertical") {
                Some("top") => Alignment::Start,
                Some("center" | "justify" | "distributed") => Alignment::Center,
                _ => Alignment::End,
            });
        }
        cells.push(style);
    }
    if cells.is_empty() {
        cells.push(default);
        general_alignment.push(true);
    }
    Styles {
        cells,
        general_alignment,
    }
}
