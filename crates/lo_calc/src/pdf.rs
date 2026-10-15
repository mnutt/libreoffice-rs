//! Paginated text preview of a workbook. Sheet boundaries start new pages.

use lo_core::{CellValue, Sheet, Workbook};
use lo_core::pdf_canvas::{PdfDocument, PdfFont};

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
            (&layout.headers, layout.page_height - layout.margin - 10.0),
            (&layout.footers, layout.margin + 4.0),
        ] {
            for (alignment, fields) in fields {
                use lo_core::OdfPageField as F;
                let text: String = fields
                    .iter()
                    .map(|f| match f {
                        F::Text(s) => s.clone(),
                        F::PageNumber => (index + 1).to_string(),
                        F::PageCount => count.to_string(),
                        F::Tab => "   ".into(),
                        F::LineBreak => "\n".into(),
                    })
                    .collect();
                let width = lo_core::font_metrics::measure_helvetica(&text, 10.0, false);
                let x = match alignment {
                    0 => layout.margin,
                    2 => layout.page_width - layout.margin - width,
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
    use lo_core::{CellAddr, style::Alignment};
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
    let available_w = layout.page_width - layout.margin * 2.0;
    let available_h =
        layout.page_height - layout.margin * 2.0 - layout.header_height - layout.footer_height;
    let widths: Vec<_> = (0..=max_col.min(16_383))
        .map(|c| {
            layout
                .columns
                .get(&c)
                .copied()
                .unwrap_or(64.0)
                .clamp(0.0, available_w)
        })
        .collect();
    let heights: Vec<_> = (0..=max_row.min(1_048_575))
        .map(|r| {
            layout
                .rows
                .get(&r)
                .copied()
                .unwrap_or(15.0)
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
            let mut top = layout.page_height - layout.margin - layout.header_height;
            for r in rows.clone() {
                let mut x = layout.margin;
                let h = heights[r];
                if h == 0.0 {
                    continue;
                }
                for c in columns.clone() {
                    let w = widths[c];
                    if w == 0.0 {
                        continue;
                    }
                    let addr = CellAddr::new(r as u32, c as u32);
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
                        if let Some(border) = style
                            .map(|s| s.border.as_str())
                            .filter(|b| !b.is_empty() && *b != "none")
                        {
                            let color = border
                                .split_whitespace()
                                .last()
                                .and_then(grid_color)
                                .unwrap_or((0.0, 0.0, 0.0));
                            let thickness = border
                                .split_whitespace()
                                .next()
                                .and_then(lo_core::style::odf_length)
                                .unwrap_or(0.75)
                                .max(0.1);
                            page.line_width(thickness);
                            page.rect_stroke_rgb(x, top - h, w, h, color.0, color.1, color.2);
                            page.line_width(1.0);
                        }
                        let color = style
                            .and_then(|s| grid_color(&s.foreground))
                            .unwrap_or((0.0, 0.0, 0.0));
                        let size = style
                            .map(|s| s.font_size_pt as f32)
                            .filter(|v| *v > 0.0)
                            .unwrap_or(10.0)
                            .min((h - 1.0).max(1.0));
                        let font = if style.is_some_and(|s| s.bold) {
                            PdfFont::HelveticaBold
                        } else if style.is_some_and(|s| s.italic) {
                            PdfFont::HelveticaOblique
                        } else {
                            PdfFont::Helvetica
                        };
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
                                visible_w = layout.page_width - layout.margin - x;
                            }
                        }
                        page.raw(&format!("q {} {} {} {} re W n", x, top - h, visible_w, h));
                        let estimated = lo_core::font_metrics::measure_helvetica(
                            &text,
                            size,
                            font == PdfFont::HelveticaBold,
                        );
                        let offset = match style.map(|s| &s.alignment) {
                            Some(Alignment::End) => (w - 2.0 - estimated).max(2.0),
                            Some(Alignment::Center) => ((w - estimated) / 2.0).max(2.0),
                            _ => 2.0,
                        };
                        let lines = if style.is_some_and(|s| s.wrap) {
                            grid_wrap(&text, (w - 4.0).max(1.0), size)
                        } else {
                            text.split('\n').map(str::to_string).collect()
                        };
                        for (line_index, line) in lines.iter().enumerate() {
                            // Helvetica ascent is 0.718 em; a baseline is not a full em below the top.
                            let y = top - size * 0.718 - 2.0 - line_index as f32 * size * 1.2;
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
                    layout.margin, layout.margin, available_w, available_h
                ));
                for chart in &layout.charts {
                    chart.render(page, layout.page_height, layout.margin, layout.margin);
                }
                page.raw("Q");
            }
        }
    }
}

fn grid_wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        for word in raw.split_whitespace() {
            if !line.is_empty()
                && lo_core::font_metrics::measure_helvetica(&format!("{line} {word}"), size, false)
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
