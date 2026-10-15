//! Bundled source-font selection shared by spreadsheet measurement and PDF embedding.
use crate::{
    unicode_font::{build_from_face, EmbeddedUnicodeFont},
    LoError, PdfFont, Result,
};
use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use std::{collections::BTreeSet, sync::OnceLock};
use ttf_parser::Face;

struct Catalog {
    database: Database,
    ids: Vec<fontdb::ID>,
}
static CATALOG: OnceLock<Catalog> = OnceLock::new();
fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        let mut database = Database::new();
        // Only an explicitly supplied font directory participates. This keeps output
        // reproducible and avoids searching the host's fonts in a preview sandbox.
        if let Some(directory) = std::env::var_os("DAVROS_PREVIEW_FONTS") {
            if let Ok(entries) = std::fs::read_dir(directory) {
                let mut paths = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .collect::<Vec<_>>();
                paths.sort();
                for path in paths {
                    if path
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| matches!(s, "ttf" | "otf"))
                    {
                        if let Ok(bytes) = std::fs::read(&path) {
                            database.load_font_data(bytes);
                        }
                    }
                }
            }
        }
        let ids = database.faces().map(|f| f.id).collect();
        Catalog { database, ids }
    })
}

/// Resolve source family/style against the bundled fonts, then metric-compatible
/// substitutions. The directory is read once per process; missing fonts use Base14.
pub fn resolve(family: &str, bold: bool, italic: bool) -> Option<PdfFont> {
    let family_lower = family.to_ascii_lowercase();
    let substitute = if family_lower.contains("mono")
        || family_lower.contains("courier")
        || family_lower.contains("typewriter")
        || family_lower == "consolas"
    {
        "Liberation Mono"
    } else if family_lower.contains("times")
        || family_lower == "cambria"
        || (family_lower.contains("serif") && !family_lower.contains("sans"))
    {
        "Liberation Serif"
    } else {
        "Liberation Sans"
    };
    let c = catalog();
    let id = c.database.query(&Query {
        families: &[Family::Name(family), Family::Name(substitute)],
        weight: if bold { Weight::BOLD } else { Weight::NORMAL },
        stretch: Stretch::Normal,
        style: if italic { Style::Italic } else { Style::Normal },
    })?;
    c.ids
        .iter()
        .position(|candidate| *candidate == id)
        .map(|i| PdfFont::Source(i as u32))
}
fn with_face<T>(id: u32, f: impl FnOnce(&[u8], u32) -> T) -> Option<T> {
    let c = catalog();
    c.database.with_face_data(*c.ids.get(id as usize)?, f)
}
// Cache measured glyph advances; parsing a font once per cell character is
// prohibitively expensive on large worksheets. Round exactly as PDF /W does.
static ADVANCES: OnceLock<std::sync::Mutex<std::collections::BTreeMap<(u32, char), Option<u16>>>> =
    OnceLock::new();
fn advance(id: u32, ch: char) -> Option<u16> {
    let mut cache = ADVANCES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(value) = cache.get(&(id, ch)) {
        return *value;
    }
    let value = with_face(id, |data, index| {
        let face = Face::parse(data, index).ok()?;
        let a = face.glyph_hor_advance(face.glyph_index(ch)?)?;
        let upem = u32::from(face.units_per_em()).max(1);
        Some(((u32::from(a) * 1000 + upem / 2) / upem).min(u16::MAX as u32) as u16)
    })
    .flatten();
    // Bound cache growth for adversarial Unicode input.
    if cache.len() < 65536 {
        cache.insert((id, ch), value);
    }
    value
}
pub(crate) fn covers(id: u32, ch: char) -> bool {
    advance(id, ch).is_some()
}
pub fn measure(id: u32, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|ch| {
            advance(id, ch)
                .map(|a| f32::from(a) * size / 1000.0)
                .unwrap_or_else(|| {
                    crate::font_metrics::measure_helvetica(&ch.to_string(), size, false)
                })
        })
        .sum()
}
pub(crate) fn embed(id: u32, chars: &BTreeSet<char>) -> Result<EmbeddedUnicodeFont> {
    let c = catalog();
    let face = c
        .ids
        .get(id as usize)
        .and_then(|id| c.database.face(*id))
        .ok_or_else(|| LoError::InvalidInput("unknown source font".into()))?;
    with_face(id, |data, index| {
        build_from_face(data, index, &face.post_script_name, chars)
    })
    .ok_or_else(|| LoError::InvalidInput("unreadable source font".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_fonts_preserve_variants_metrics_and_aliased_characters() {
        if std::env::var_os("DAVROS_PREVIEW_FONTS").is_none() {
            eprintln!("source font fixture test requires DAVROS_PREVIEW_FONTS");
            return;
        }
        let regular = resolve("Liberation Sans", false, false).expect("bundled regular font");
        let bold = resolve("Liberation Sans", true, false).expect("bundled bold font");
        let italic = resolve("Liberation Sans", false, true).expect("bundled italic font");
        assert_ne!(regular, bold);
        assert_ne!(regular, italic);
        let PdfFont::Source(id) = bold else {
            panic!("expected source face")
        };
        let text = "Wide WWW i é € \u{a0}";
        let font = embed(id, &text.chars().collect()).unwrap();
        assert_ne!(font.char_to_cid[&' '], font.char_to_cid[&'\u{a0}']);
        let measured = measure(id, text, 12.0);
        let pdf_width: f32 = text
            .chars()
            .map(|ch| f32::from(font.widths[font.char_to_cid[&ch] as usize]) * 12.0 / 1000.0)
            .sum();
        assert!((measured - pdf_width).abs() < 0.001);
        assert!(
            font.bytes.len() < 30000,
            "only used glyphs should be embedded"
        );
        let mut doc = crate::PdfDocument::new();
        let page = doc.add_page(200.0, 200.0);
        doc.page_mut(page)
            .unwrap()
            .text(10.0, 100.0, 12.0, bold, text);
        let pdf = doc.try_finish().unwrap();
        let pdf = String::from_utf8_lossy(&pdf);
        assert!(pdf.contains("LiberationSans-Bold"));
        assert!(pdf.contains("/ToUnicode"));
        assert!(pdf.contains("<00A0>"));
        assert!(!pdf.contains("__LO_UNICODE_TEXT_"));
    }
}
