//! Base Helvetica widths shared by ODF grid, slide and chart layout.
//! ASCII widths follow the existing Writer metrics; other scripts use fallback widths.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 222, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 222, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 278, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 278, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

pub fn measure_helvetica(text: &str, size: f32, bold: bool) -> f32 {
    let widths = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    text.chars().map(|c| {
        let width=if (' '..='~').contains(&c) {widths[c as usize-32] as f32}
            else if matches!(c as u32,0x2e80..=0x9fff|0xac00..=0xd7a3|0xf900..=0xfaff|0xff01..=0xff60) {1000.0}else{600.0};
        width*size/1000.0
    }).sum()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proportional_fonts_do_not_use_character_count_for_alignment() {
        assert!(measure_helvetica("WWW", 12.0, false) > measure_helvetica("iii", 12.0, false));
        assert!((measure_helvetica("12.50", 10.0, false) - 25.02).abs() < 0.01);
    }
}

const TIMES_ROMAN: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 333, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
];

const TIMES_BOLD: [u16; 95] = [
    250, 333, 555, 500, 500, 1000, 833, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722, 722, 667,
    611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722, 722, 1000, 722,
    722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556,
    278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, 394, 220, 394, 520,
];

const TIMES_ITALIC: [u16; 95] = [
    250, 333, 420, 500, 500, 833, 778, 333, 333, 333, 500, 675, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500, 920, 611, 611, 667, 722, 611,
    611, 722, 722, 333, 444, 667, 556, 833, 667, 722, 611, 722, 611, 500, 556, 722, 611, 833, 611,
    556, 556, 389, 278, 389, 422, 500, 333, 500, 500, 444, 500, 444, 278, 500, 500, 278, 278, 444,
    278, 722, 500, 500, 500, 500, 389, 389, 278, 500, 444, 667, 444, 444, 389, 400, 275, 400, 541,
];

pub fn measure_base_font(text: &str, size: f32, font: crate::PdfFont) -> f32 {
    use crate::PdfFont;
    let widths = match font {
        PdfFont::Source(id) => return crate::source_font::measure(id, text, size),
        PdfFont::Helvetica | PdfFont::HelveticaOblique => &HELVETICA,
        PdfFont::HelveticaBold | PdfFont::HelveticaBoldOblique => &HELVETICA_BOLD,
        PdfFont::TimesRoman => &TIMES_ROMAN,
        PdfFont::TimesBold => &TIMES_BOLD,
        PdfFont::TimesItalic => &TIMES_ITALIC,
        PdfFont::Courier => return text.chars().count() as f32 * size * 0.6,
    };
    text.chars()
        .map(|c| {
            if (' '..='~').contains(&c) {
                widths[c as usize - 32] as f32 * size / 1000.0
            } else {
                measure_helvetica(&c.to_string(), size, false)
            }
        })
        .sum()
}
