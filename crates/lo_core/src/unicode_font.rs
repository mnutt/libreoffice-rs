//! Unicode font discovery, subsetting, and PDF CID font metadata.

use std::collections::{BTreeMap, BTreeSet};

use fontdb::Database;
use subsetter::{subset, GlyphRemapper};
use ttf_parser::{Face, GlyphId};

use crate::{LoError, Result};

const FONT_OVERRIDE_ENV: &str = "LIBREOFFICE_PURE_UNICODE_FONT";

pub(crate) struct EmbeddedUnicodeFont {
    pub(crate) base_name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) is_true_type: bool,
    pub(crate) widths: Vec<u16>,
    pub(crate) cid_to_gid: Option<Vec<u16>>,
    pub(crate) ascent: i16,
    pub(crate) descent: i16,
    pub(crate) bbox: (i16, i16, i16, i16),
    pub(crate) italic_angle: f32,
    pub(crate) char_to_cid: BTreeMap<char, u16>,
}

impl EmbeddedUnicodeFont {
    pub(crate) fn encode_hex(&self, text: &str) -> Result<String> {
        let mut out = String::with_capacity(text.chars().count() * 4 + 2);
        out.push('<');
        for ch in text.chars() {
            let cid = self.char_to_cid.get(&ch).ok_or_else(|| {
                LoError::Unsupported(format!(
                    "Unicode PDF font does not contain U+{:04X}",
                    ch as u32
                ))
            })?;
            out.push_str(&format!("{cid:04X}"));
        }
        out.push('>');
        Ok(out)
    }

    pub(crate) fn widths_array(&self) -> String {
        self.widths
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(crate) fn to_unicode_cmap(&self) -> Vec<u8> {
        let mut mappings = self
            .char_to_cid
            .iter()
            .map(|(ch, cid)| (*cid, *ch))
            .collect::<Vec<_>>();
        mappings.sort_by_key(|(cid, _)| *cid);

        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n\
             12 dict begin\n\
             begincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /LOUnicode def\n\
             /CMapType 2 def\n\
             1 begincodespacerange\n\
             <0000> <FFFF>\n\
             endcodespacerange\n",
        );
        for chunk in mappings.chunks(100) {
            cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (cid, ch) in chunk {
                cmap.push_str(&format!("<{cid:04X}> <{}>\n", utf16be_hex(*ch)));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        cmap.into_bytes()
    }
}

pub(crate) fn build_unicode_fonts(chars: &BTreeSet<char>) -> Result<Vec<EmbeddedUnicodeFont>> {
    let mut database = Database::new();
    if let Some(directory) = std::env::var_os("DAVROS_PREVIEW_FONTS") {
        let mut paths = std::fs::read_dir(directory)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            if path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| matches!(s, "ttf" | "otf"))
            {
                database.load_font_file(&path).map_err(|error| {
                    LoError::Io(format!(
                        "loading bundled PDF font {}: {error}",
                        path.display()
                    ))
                })?;
            }
        }
    } else if let Some(paths) = std::env::var_os(FONT_OVERRIDE_ENV) {
        for path in std::env::split_paths(&paths) {
            database.load_font_file(&path).map_err(|error| {
                LoError::Io(format!(
                    "loading Unicode PDF font {}: {error}",
                    path.display()
                ))
            })?;
        }
    } else {
        database.load_system_fonts();
    }

    let mut candidates = database.faces().collect::<Vec<_>>();
    candidates.sort_by_key(|face| std::cmp::Reverse(font_preference(face)));
    let mut remaining = chars.clone();
    let mut fonts = Vec::new();
    while !remaining.is_empty() {
        let mut selected = None;
        let mut selected_chars = BTreeSet::new();
        for face in &candidates {
            let covered = database
                .with_face_data(face.id, |data, index| {
                    Face::parse(data, index)
                        .ok()
                        .map(|parsed| {
                            remaining
                                .iter()
                                .filter(|ch| parsed.glyph_index(**ch).is_some())
                                .copied()
                                .collect::<BTreeSet<_>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            if covered.len() > selected_chars.len() {
                selected = Some(face);
                selected_chars = covered;
            }
        }
        let Some(selected) = selected.filter(|_| !selected_chars.is_empty()) else {
            let sample = remaining
                .iter()
                .take(8)
                .map(|ch| format!("U+{:04X}", *ch as u32))
                .collect::<Vec<_>>()
                .join(", ");
            // Use an actual outlined square, not .notdef (which can be blank).
            // Distinct CIDs preserve each missing character in ToUnicode even
            // though every CID draws the same glyph.
            let fallback = candidates
                .iter()
                .find(|candidate| {
                    database
                        .with_face_data(candidate.id, |data, index| {
                            has_sfnt_table(data, b"glyf")
                                && Face::parse(data, index)
                                    .ok()
                                    .is_some_and(|face| face.glyph_index('□').is_some())
                        })
                        .unwrap_or(false)
                })
                .ok_or_else(|| {
                    LoError::Unsupported(
                        "no TrueType font with a missing-character square is available".into(),
                    )
                })?;
            let missing = remaining.iter().copied().collect::<Vec<_>>();
            for chunk in missing.chunks(u16::MAX as usize) {
                let font = database
                    .with_face_data(fallback.id, |data, index| {
                        build_tofu_font(data, index, &fallback.post_script_name, chunk)
                    })
                    .ok_or_else(|| LoError::Io("reading tofu PDF font".into()))??;
                fonts.push(font);
            }
            eprintln!("Preview font warning: rendering {} unsupported Unicode characters as boxes; missing {sample}", remaining.len());
            break;
        };
        let font = database
            .with_face_data(selected.id, |data, index| {
                build_from_face(data, index, &selected.post_script_name, &selected_chars)
            })
            .ok_or_else(|| LoError::Io("reading selected Unicode PDF font".to_string()))??;
        remaining.retain(|ch| !selected_chars.contains(ch));
        fonts.push(font);
    }
    Ok(fonts)
}

pub(crate) fn build_from_face(
    data: &[u8],
    face_index: u32,
    post_script_name: &str,
    chars: &BTreeSet<char>,
) -> Result<EmbeddedUnicodeFont> {
    let face = Face::parse(data, face_index)
        .map_err(|_| LoError::Parse("selected Unicode PDF font is malformed".to_string()))?;
    let mut remapper = GlyphRemapper::new();
    for ch in chars {
        let glyph = face.glyph_index(*ch).ok_or_else(|| {
            LoError::Unsupported(format!(
                "selected Unicode PDF font is missing U+{:04X}",
                *ch as u32
            ))
        })?;
        remapper.remap(glyph.0);
    }

    let units_per_em = face.units_per_em().max(1) as u32;
    let mut widths = remapper
        .remapped_gids()
        .map(|old_gid| {
            let advance = face
                .glyph_hor_advance(GlyphId(old_gid))
                .unwrap_or(units_per_em as u16);
            ((advance as u32 * 1000 + units_per_em / 2) / units_per_em).min(u16::MAX as u32) as u16
        })
        .collect::<Vec<_>>();
    let mut char_to_cid = BTreeMap::new();
    for ch in chars {
        let old_gid = face.glyph_index(*ch).expect("coverage checked").0;
        char_to_cid.insert(*ch, remapper.get(old_gid).expect("glyph remapped"));
    }
    let bbox = face.global_bounding_box();
    let scale_metric = |value: i16| -> i16 {
        ((value as i32 * 1000) / units_per_em as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16
    };
    let subset = subset(data, face_index, &remapper)
        .map_err(|error| LoError::Unsupported(format!("subsetting Unicode PDF font: {error}")))?;
    let is_true_type = has_sfnt_table(&subset, b"glyf");
    if !is_true_type && !has_sfnt_table(&subset, b"CFF ") {
        return Err(LoError::Unsupported(
            "Unicode PDF font subset has neither TrueType nor CFF outlines".to_string(),
        ));
    }

    // TrueType CIDs are character identities, independent of glyph IDs. In
    // particular space/NBSP may share an outline but must extract distinctly.
    let cid_to_gid = if is_true_type {
        let glyph_widths = widths;
        widths = vec![glyph_widths[0]];
        let mut mapping = vec![0];
        for (i, ch) in chars.iter().enumerate() {
            let gid = char_to_cid[ch];
            let cid = u16::try_from(i + 1)
                .map_err(|_| LoError::Unsupported("too many font characters".into()))?;
            widths.push(glyph_widths[gid as usize]);
            mapping.push(gid);
            char_to_cid.insert(*ch, cid);
        }
        Some(mapping)
    } else {
        let unique = char_to_cid.values().copied().collect::<BTreeSet<_>>();
        if unique.len() != char_to_cid.len() {
            return Err(LoError::Unsupported(
                "CFF font aliases multiple characters to one glyph".into(),
            ));
        }
        None
    };

    Ok(EmbeddedUnicodeFont {
        base_name: format!("LOUNIC+{}", pdf_name(post_script_name)),
        bytes: subset,
        is_true_type,
        widths,
        cid_to_gid,
        ascent: scale_metric(face.ascender()),
        descent: scale_metric(face.descender()),
        bbox: (
            scale_metric(bbox.x_min),
            scale_metric(bbox.y_min),
            scale_metric(bbox.x_max),
            scale_metric(bbox.y_max),
        ),
        italic_angle: face.italic_angle(),
        char_to_cid,
    })
}

fn build_tofu_font(
    data: &[u8],
    face_index: u32,
    post_script_name: &str,
    chars: &[char],
) -> Result<EmbeddedUnicodeFont> {
    let mut font = build_from_face(data, face_index, post_script_name, &BTreeSet::from(['□']))?;
    let square_cid = font.char_to_cid[&'□'];
    let width = font.widths[square_cid as usize];
    let square_gid = font
        .cid_to_gid
        .as_ref()
        .map_or(square_cid, |m| m[square_cid as usize]);
    font.base_name = "LOTOFU+MissingGlyph".into();
    font.char_to_cid.clear();
    let mut mapping = vec![0];
    font.widths = vec![0];
    for (index, ch) in chars.iter().enumerate() {
        let cid = u16::try_from(index + 1)
            .map_err(|_| LoError::Unsupported("too many tofu characters in one font".into()))?;
        font.char_to_cid.insert(*ch, cid);
        font.widths.push(width);
        mapping.push(square_gid);
    }
    font.cid_to_gid = Some(mapping);
    Ok(font)
}

fn font_preference(face: &fontdb::FaceInfo) -> u8 {
    let names = face
        .families
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .chain(std::iter::once(face.post_script_name.to_ascii_lowercase()))
        .collect::<Vec<_>>()
        .join(" ");
    for (needle, score) in [
        ("noto sans cjk sc", 100),
        ("noto sans sc", 95),
        ("source han sans sc", 90),
        ("arial unicode", 85),
        ("pingfang sc", 80),
        ("hiragino sans gb", 75),
        ("droid sans fallback", 70),
        ("wenquanyi", 65),
    ] {
        if names.contains(needle) {
            return score;
        }
    }
    0
}

fn pdf_name(value: &str) -> String {
    let cleaned = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect::<String>();
    if cleaned.is_empty() {
        "UnicodeFont".to_string()
    } else {
        cleaned
    }
}

fn has_sfnt_table(bytes: &[u8], wanted: &[u8; 4]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    (0..count).any(|index| {
        let start = 12 + index * 16;
        bytes.get(start..start + 4) == Some(wanted.as_slice())
    })
}

fn utf16be_hex(ch: char) -> String {
    let mut units = [0_u16; 2];
    ch.encode_utf16(&mut units)
        .iter()
        .map(|unit| format!("{unit:04X}"))
        .collect::<String>()
}
