//! Paginated text preview of a workbook. Sheet boundaries start new pages.

use lo_core::pdf_canvas::{PdfDocument, PdfFont};
use lo_core::{CellValue, Sheet, Workbook};

use crate::{evaluate_formula, Value};

fn cell_text(sheet: &Sheet, value: &CellValue) -> String {
    match value {
        CellValue::Empty => String::new(),
        CellValue::Number(n) => format!("{n}"),
        CellValue::Text(t) => t.clone(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Formula(formula) => match evaluate_formula(formula, sheet) {
            Ok(Value::Number(n)) => format!("{n}"),
            Ok(Value::Text(t)) => t,
            Ok(Value::Bool(b)) => b.to_string(),
            Ok(Value::Blank) => String::new(),
            _ => "#ERR".to_string(),
        },
        CellValue::Error(_) => "#ERR".to_string(),
    }
}

// Use occupied rows rather than scanning a potentially enormous sparse range.
// This remains a text preview; it does not reproduce spreadsheet print layouts.
fn sheet_lines(sheet: &Sheet) -> Vec<String> {
    let mut lines = vec![format!("[{}]", sheet.name)];
    let mut row = None;
    let mut text = String::new();
    for (addr, cell) in &sheet.cells {
        if row != Some(addr.row) {
            if row.is_some() {
                lines.push(std::mem::take(&mut text));
            }
            row = Some(addr.row);
        }
        if !text.is_empty() {
            text.push_str("    ");
        }
        text.push_str(
            &sheet
                .display_values
                .get(addr)
                .cloned()
                .unwrap_or_else(|| cell_text(sheet, &cell.value)),
        );
    }
    if row.is_some() {
        lines.push(text);
    }
    lines
}

fn render(workbook: &Workbook) -> PdfDocument {
    let mut pdf = PdfDocument::new();
    let mut page_layouts = Vec::new();
    for sheet in &workbook.sheets {
        if let Some(layout) = &sheet.layout {
            if !layout.hidden {
                let first = pdf.pages().len();
                render_grid(sheet, layout, &mut pdf);
                for index in first..pdf.pages().len() {
                    page_layouts.push((index, layout));
                }
            }
            continue;
        }
        let mut page = pdf.add_page(842.0, 595.0);
        let mut y = 545.0;
        for line in sheet_lines(sheet) {
            // Courier's fixed advance is 0.6 em. Wrap before the right margin;
            // split on Unicode scalar boundaries, never on UTF-8 byte offsets.
            for logical in line.split('\n') {
                let chars: Vec<char> = logical.chars().collect();
                let chunks: Vec<String> = if chars.is_empty() {
                    vec![String::new()]
                } else {
                    chars.chunks(103).map(|c| c.iter().collect()).collect()
                };
                for chunk in chunks {
                    if y < 50.0 {
                        page = pdf.add_page(842.0, 595.0);
                        y = 545.0;
                    }
                    pdf.page_mut(page).expect("new page").text(
                        50.0,
                        y,
                        12.0,
                        PdfFont::Courier,
                        &chunk,
                    );
                    y -= 14.0;
                }
            }
        }
    }
    if pdf.pages().is_empty() {
        pdf.add_page(842.0, 595.0);
    }
    let count = pdf.pages().len();
    for (index, layout) in page_layouts {
        let page = pdf.page_mut(index).expect("sheet page");
        for (fields, y) in [
            (
                &layout.headers,
                layout
                    .margins
                    .map(|m| layout.page_height - m[2] / 2.0)
                    .unwrap_or(layout.page_height - layout.margin - 10.0),
            ),
            (
                &layout.footers,
                layout
                    .margins
                    .map(|m| m[3] / 2.0)
                    .unwrap_or(layout.margin + 4.0),
            ),
        ] {
            for (alignment, fields) in fields {
                use lo_core::OdfPageField as F;
                fn field_text(f: &F, index: usize, count: usize) -> String {
                    match f {
                        F::Text(s) => s.clone(),
                        F::PageNumber => (index + 1).to_string(),
                        F::PageCount => count.to_string(),
                        F::Tab => "   ".into(),
                        F::LineBreak => "\n".into(),
                        F::Styled(fields, _) => {
                            fields.iter().map(|f| field_text(f, index, count)).collect()
                        }
                    }
                }
                let text: String = fields.iter().map(|f| field_text(f, index, count)).collect();
                let width = lo_core::font_metrics::measure_helvetica(&text, 10.0, false);
                let x = match alignment {
                    0 => layout.margins.map(|m| m[0]).unwrap_or(layout.margin),
                    2 => {
                        layout.page_width
                            - layout.margins.map(|m| m[1]).unwrap_or(layout.margin)
                            - width
                    }
                    _ => (layout.page_width - width) / 2.0,
                };
                page.text(x, y, 10.0, PdfFont::Helvetica, &text);
            }
        }
    }
    pdf
}

fn grid_color(value: &str) -> Option<(f32, f32, f32)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0,
        u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0,
        u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0,
    ))
}

fn render_grid(sheet: &Sheet, layout: &lo_core::calc::SheetLayout, pdf: &mut PdfDocument) {
    use lo_core::{style::Alignment, CellAddr};
    if sheet.cells.is_empty()
        && layout.charts.is_empty()
        && layout.headers.is_empty()
        && layout.footers.is_empty()
    {
        return;
    }
    let (mut max_row, mut max_col) = sheet.max_extent();
    for (addr, (columns, rows)) in &layout.merges {
        max_col = max_col.max(addr.col.saturating_add(*columns - 1));
        max_row = max_row.max(addr.row.saturating_add(*rows - 1));
    }
    let [left, right, top_margin, bottom] = layout.margins.unwrap_or([layout.margin; 4]);
    let heading_w = if layout.print_headings { 24.0 } else { 0.0 };
    let heading_h = if layout.print_headings { 15.0 } else { 0.0 };
    let available_w = layout.page_width - left - right - heading_w;
    let available_h = layout.page_height
        - top_margin
        - bottom
        - layout.header_height
        - layout.footer_height
        - heading_h;
    let mut covered = std::collections::BTreeSet::new();
    let mut merge_budget = 1_000_000_u64;
    for (anchor, (cols, rows)) in &layout.merges {
        // Bounds are enforced by importers; do not let public model input expand without limit.
        let area = u64::from(*cols) * u64::from(*rows);
        if area > merge_budget {
            continue;
        }
        merge_budget -= area;
        for r in anchor.row..anchor.row.saturating_add(*rows) {
            for c in anchor.col..anchor.col.saturating_add(*cols) {
                let addr = CellAddr::new(r, c);
                if addr != *anchor {
                    covered.insert(addr);
                }
            }
        }
    }
    let widths: Vec<_> = (0..=max_col.min(16_383))
        .map(|c| {
            layout
                .columns
                .get(&c)
                .copied()
                .unwrap_or(layout.default_column_width)
                .clamp(0.0, available_w)
        })
        .collect();
    let heights: Vec<_> = (0..=max_row.min(1_048_575))
        .map(|r| {
            layout
                .rows
                .get(&r)
                .copied()
                .unwrap_or(layout.default_row_height)
                .clamp(0.0, available_h)
        })
        .collect();
    let column_pages = grid_bands(&widths, available_w);
    let row_pages = grid_bands(&heights, available_h);
    for columns in column_pages {
        for rows in &row_pages {
            // Empty page tiles do not need a printed page.
            if !sheet.cells.keys().any(|a| {
                columns.contains(&(a.col as usize))
                    && rows.contains(&(a.row as usize))
                    && widths[a.col as usize] > 0.0
                    && heights[a.row as usize] > 0.0
            }) && !(columns.start == 0
                && rows.start == 0
                && (!layout.charts.is_empty()
                    || !layout.headers.is_empty()
                    || !layout.footers.is_empty()))
            {
                continue;
            }
            let index = pdf.add_page(layout.page_width, layout.page_height);
            let page = pdf.page_mut(index).expect("grid page");
            let mut top = layout.page_height - top_margin - layout.header_height - heading_h;
            if layout.print_headings {
                let mut hx = left + heading_w;
                for c in columns.clone() {
                    if widths[c] > 0.0 {
                        let label = CellAddr::new(0, c as u32)
                            .to_a1()
                            .trim_end_matches('1')
                            .to_string();
                        let text_w = lo_core::font_metrics::measure_helvetica(&label, 10.0, false);
                        page.text(
                            hx + (widths[c] - text_w) / 2.0,
                            top + 4.0,
                            10.0,
                            PdfFont::Helvetica,
                            &label,
                        );
                    }
                    hx += widths[c];
                }
            }
            for r in rows.clone() {
                let mut x = left + heading_w;
                let h = heights[r];
                if h == 0.0 {
                    continue;
                }
                if layout.print_headings {
                    page.text(
                        left + 2.0,
                        top - h + 3.0,
                        10.0,
                        PdfFont::Helvetica,
                        &(r + 1).to_string(),
                    );
                }
                for c in columns.clone() {
                    let w = widths[c];
                    if w == 0.0 {
                        continue;
                    }
                    let addr = CellAddr::new(r as u32, c as u32);
                    if layout.print_gridlines && !covered.contains(&addr) {
                        page.raw("q");
                        page.line_width(0.25);
                        page.rect_stroke_rgb(x, top - h, w, h, 0.75, 0.75, 0.75);
                        page.raw("Q");
                    }
                    if covered.contains(&addr) {
                        x += w;
                        continue;
                    }
                    if let Some(cell) = sheet.get(addr) {
                        let (col_span, row_span) =
                            layout.merges.get(&addr).copied().unwrap_or((1, 1));
                        let w = widths[c..(c + col_span as usize).min(columns.end)]
                            .iter()
                            .sum::<f32>();
                        let h = heights[r..(r + row_span as usize).min(rows.end)]
                            .iter()
                            .sum::<f32>();
                        let text = sheet
                            .display_values
                            .get(&addr)
                            .cloned()
                            .unwrap_or_else(|| cell_text(sheet, &cell.value));
                        let style = cell.style.as_ref();
                        if let Some((red, green, blue)) =
                            style.and_then(|s| grid_color(&s.background))
                        {
                            page.rect_fill_rgb(x, top - h, w, h, red, green, blue);
                        }
                        if let Some(style) = style {
                            let edges = [
                                (x, top - h, x, top),
                                (x + w, top - h, x + w, top),
                                (x, top, x + w, top),
                                (x, top - h, x + w, top - h),
                            ];
                            for (i, (x1, y1, x2, y2)) in edges.into_iter().enumerate() {
                                let border = if style.borders[i].is_empty() {
                                    &style.border
                                } else {
                                    &style.borders[i]
                                };
                                draw_border(page, border, x1, y1, x2, y2);
                            }
                        }
                        let color = style
                            .and_then(|s| grid_color(&s.foreground))
                            .unwrap_or((0.0, 0.0, 0.0));
                        let size = style
                            .map(|s| {
                                s.font_size_mpt
                                    .map(|v| v as f32 / 1000.0)
                                    .unwrap_or(s.font_size_pt as f32)
                            })
                            .filter(|v| *v > 0.0)
                            .unwrap_or(10.0);
                        let size = if style.is_some_and(|s| s.font_size_mpt.is_some()) {
                            size
                        } else {
                            size.min((h - 1.0).max(1.0))
                        };
                        let font = cell_font(style);
                        // Text can overflow into adjacent empty cells, as in Calc;
                        // occupied cells and page edges stop it. Numeric values stay
                        // within their own column. Clip to the source row height.
                        let mut visible_w = w;
                        if matches!(cell.value, CellValue::Text(_))
                            && !style.is_some_and(|s| s.wrap)
                            && col_span == 1
                        {
                            let mut blocked = false;
                            for next in c + 1..columns.end {
                                if sheet.get(CellAddr::new(r as u32, next as u32)).is_some() {
                                    blocked = true;
                                    break;
                                }
                                visible_w += widths[next];
                            }
                            if !blocked {
                                visible_w = layout.page_width - right - x;
                            }
                        }
                        page.raw(&format!("q {} {} {} {} re W n", x, top - h, visible_w, h));
                        let lines = if style.is_some_and(|s| s.wrap) {
                            grid_wrap_font(&text, (w - 4.0).max(1.0), size, font)
                        } else {
                            text.split('\n').map(str::to_string).collect()
                        };
                        let text_height = size * 1.2 * lines.len() as f32;
                        let baseline = match style.and_then(|s| s.vertical_alignment.as_ref()) {
                            Some(Alignment::End) => top - h + text_height - size * 0.918 + 1.0,
                            Some(Alignment::Center) => top - (h - text_height) / 2.0 - size * 0.918,
                            _ => top - size * 0.718 - 2.0,
                        };
                        for (line_index, line) in lines.iter().enumerate() {
                            let estimated =
                                lo_core::font_metrics::measure_base_font(line, size, font);
                            let offset = match style.map(|s| &s.alignment) {
                                Some(Alignment::End) => (w - 2.0 - estimated).max(2.0),
                                Some(Alignment::Center) => ((w - estimated) / 2.0).max(2.0),
                                _ => 2.0,
                            };
                            let y = baseline - line_index as f32 * size * 1.2;
                            if y < top - h {
                                break;
                            }
                            page.text_rgb(
                                x + offset,
                                y,
                                size,
                                font,
                                line,
                                color.0,
                                color.1,
                                color.2,
                            );
                        }
                        page.raw("Q");
                    }
                    x += w;
                }
                top -= h;
            }
            if columns.start == 0 && rows.start == 0 {
                page.raw(&format!(
                    "q {} {} {} {} re W n",
                    left, bottom, available_w, available_h
                ));
                for chart in &layout.charts {
                    chart.render(page, layout.page_height, left, top_margin);
                }
                page.raw("Q");
            }
        }
    }
}

pub(crate) fn grid_wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    grid_wrap_font(text, width, size, PdfFont::Helvetica)
}

fn grid_wrap_font(text: &str, width: f32, size: f32, font: PdfFont) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        for word in raw.split_whitespace() {
            if !line.is_empty()
                && lo_core::font_metrics::measure_base_font(&format!("{line} {word}"), size, font)
                    > width
            {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn grid_bands(sizes: &[f32], available: f32) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut occupied = 0.0;
    for (index, size) in sizes.iter().enumerate() {
        if index > start && occupied + size > available + 0.01 {
            ranges.push(start..index);
            start = index;
            occupied = 0.0;
        }
        occupied += size;
    }
    if start < sizes.len() {
        ranges.push(start..sizes.len());
    }
    ranges
}

pub fn to_pdf(workbook: &Workbook) -> Vec<u8> {
    render(workbook).finish()
}

#[cfg(test)]
mod pagination_tests {
    use super::*;
    use lo_core::CellAddr;
    #[test]
    fn grid_paginates_both_axes_and_skips_hidden_sheets() {
        let mut book = Workbook::default();
        let sheet = &mut book.sheets[0];
        sheet.set(CellAddr::new(0, 0), CellValue::Text("first".into()));
        sheet.set(CellAddr::new(100, 10), CellValue::Text("last".into()));
        sheet.layout = Some(lo_core::calc::SheetLayout::default());
        assert_eq!(render(&book).pages().len(), 2);
        book.sheets[0].layout.as_mut().unwrap().hidden = true;
        assert_eq!(render(&book).pages().len(), 1); // empty document fallback
        assert_eq!(grid_bands(&[40.0, 40.0, 40.0], 80.0), [0..2, 2..3]);
    }
    #[test]
    fn long_sheets_and_sheet_boundaries() {
        let mut book = Workbook::default();
        book.sheets.clear();
        let mut sheet = Sheet::new("Long");
        for row in 0..205 {
            sheet.set(CellAddr::new(row, 0), CellValue::Number(row as f64));
        }
        book.sheets.push(sheet);
        book.sheets.push(Sheet::new("Empty"));
        assert_eq!(render(&book).pages().len(), 7);
    }
    #[test]
    fn wide_multiline_and_sparse_cells_are_bounded() {
        let mut book = Workbook::default();
        book.sheets.clear();
        let mut sheet = Sheet::new("Sparse");
        sheet.set(
            CellAddr::new(u32::MAX, u32::MAX),
            CellValue::Text("é".repeat(103 * 40)),
        );
        book.sheets.push(sheet);
        assert_eq!(render(&book).pages().len(), 2);
    }
}

/// Davros-style preview: preserve natural column widths on one wide page band.
/// Source page height and vertical pagination remain in effect; print scaling is ignored.
pub fn to_pdf_preview(workbook: &Workbook) -> Vec<u8> {
    let mut workbook = workbook.clone();
    for sheet in &mut workbook.sheets {
        let (_, mut max_col) = sheet.max_extent();
        let Some(layout) = sheet.layout.as_mut() else {
            continue;
        };
        for (a, (cols, _)) in &layout.merges {
            max_col = max_col.max(a.col.saturating_add(*cols).saturating_sub(1));
        }
        let width = (0..=max_col.min(16383))
            .map(|c| {
                layout
                    .columns
                    .get(&c)
                    .copied()
                    .unwrap_or(layout.default_column_width)
            })
            .sum::<f32>();
        let margins = layout.margins.unwrap_or([layout.margin; 4]);
        layout.page_width = layout.page_width.max(
            (width + margins[0] + margins[1] + if layout.print_headings { 24.0 } else { 0.0 })
                .ceil()
                + 1.0,
        );
    }
    render(&workbook).finish()
}

pub(crate) fn cell_font(style: Option<&lo_core::style::CellStyle>) -> PdfFont {
    let Some(style) = style else {
        return PdfFont::Helvetica;
    };
    let family = style.font_family.to_ascii_lowercase();
    if family.contains("courier") || family.contains("mono") || family == "consolas" {
        return PdfFont::Courier;
    }
    if family.contains("times") || family.contains("serif") || family == "cambria" {
        return if style.bold {
            PdfFont::TimesBold
        } else if style.italic {
            PdfFont::TimesItalic
        } else {
            PdfFont::TimesRoman
        };
    }
    match (style.bold, style.italic) {
        (true, true) => PdfFont::HelveticaBoldOblique,
        (true, false) => PdfFont::HelveticaBold,
        (false, true) => PdfFont::HelveticaOblique,
        _ => PdfFont::Helvetica,
    }
}

fn draw_border(
    page: &mut lo_core::pdf_canvas::PdfPage,
    border: &str,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) {
    if border.is_empty() || border == "none" {
        return;
    }
    let mut parts = border.split_whitespace();
    let thickness = parts
        .next()
        .and_then(lo_core::style::odf_length)
        .unwrap_or(0.75)
        .max(0.1);
    let kind = parts.next().unwrap_or("solid");
    let (r, g, b) = parts.next().and_then(grid_color).unwrap_or((0.0, 0.0, 0.0));
    page.raw("q");
    page.line_width(thickness);
    match kind {
        "dashed" | "mediumDashed" => page.raw("[3 2] 0 d"),
        "dotted" => page.raw("[1 1] 0 d"),
        "dashDot" | "mediumDashDot" => page.raw("[3 2 1 2] 0 d"),
        _ => {}
    }
    page.line_rgb(x1, y1, x2, y2, r, g, b);
    if kind == "double" {
        let d = thickness * 2.0;
        if x1 == x2 {
            page.line_rgb(x1 + d, y1, x2 + d, y2, r, g, b);
        } else {
            page.line_rgb(x1, y1 + d, x2, y2 + d, r, g, b);
        }
    }
    page.raw("Q");
}
