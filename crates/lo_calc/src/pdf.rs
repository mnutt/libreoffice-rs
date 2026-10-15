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

pub(crate) fn grid_color(value: &str) -> Option<(f32, f32, f32)> {
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
    let [l, r, t, b] = layout.margins.unwrap_or([layout.margin; 4]);
    let mut scale = layout.print_scale.clamp(0.1, 4.0);
    if let Some((across, down)) = if layout.chart_sheet {
        Some((1, 1))
    } else {
        layout.fit_to_pages
    } {
        let (mut row, mut col) = sheet.max_extent();
        for (a, (cols, rows)) in &layout.merges {
            row = row.max(a.row.saturating_add(*rows).saturating_sub(1));
            col = col.max(a.col.saturating_add(*cols).saturating_sub(1));
        }
        let (first_row, first_col) = layout
            .print_area
            .map(|(a, b)| {
                row = row.min(b.row);
                col = col.min(b.col);
                (a.row, a.col)
            })
            .unwrap_or((0, 0));
        let mut w = (first_col..=col.min(16383))
            .map(|c| {
                layout
                    .columns
                    .get(&c)
                    .copied()
                    .unwrap_or(layout.default_column_width)
            })
            .sum::<f32>();
        let mut h = (first_row..=row.min(1048575))
            .map(|r| {
                layout
                    .rows
                    .get(&r)
                    .copied()
                    .unwrap_or(layout.default_row_height)
            })
            .sum::<f32>();
        for f in layout
            .images
            .iter()
            .map(|i| i.frame)
            .chain(layout.charts.iter().map(|c| c.frame))
            .chain(layout.text_boxes.iter().map(|t| t.frame))
        {
            if layout.print_area.is_none() {
                w = w.max(f.origin.x.as_pt() + f.size.width.as_pt());
                h = h.max(f.origin.y.as_pt() + f.size.height.as_pt());
            }
        }
        scale = 1.0;
        if across > 0 {
            scale = scale.min((layout.page_width - l - r) * across as f32 / w.max(1.0));
        }
        if down > 0 {
            scale = scale.min((layout.page_height - t - b) * down as f32 / h.max(1.0));
        }
        scale = scale.clamp(0.1, 1.0);
    }
    let mut logical = layout.clone();
    logical.page_width /= scale;
    logical.page_height /= scale;
    logical.margins = Some([l / scale, r / scale, t / scale, b / scale]);
    render_scaled_grid(
        sheet,
        &logical,
        pdf,
        (layout.page_width, layout.page_height),
        scale,
    );
}

fn render_scaled_grid(
    sheet: &Sheet,
    layout: &lo_core::calc::SheetLayout,
    pdf: &mut PdfDocument,
    paper: (f32, f32),
    scale: f32,
) {
    use lo_core::{style::Alignment, CellAddr};
    if sheet.cells.is_empty()
        && layout.charts.is_empty()
        && layout.images.is_empty()
        && layout.text_boxes.is_empty()
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
    let mut widths = widths;
    let mut heights = heights;
    // Drawing-only areas must participate in pagination too.
    let drawing_width = layout
        .images
        .iter()
        .map(|i| i.frame)
        .chain(layout.charts.iter().map(|c| c.frame))
        .chain(layout.text_boxes.iter().map(|t| t.frame))
        .map(|f| f.origin.x.as_pt() + f.size.width.as_pt())
        .fold(0.0_f32, f32::max);
    let drawing_height = layout
        .images
        .iter()
        .map(|i| i.frame)
        .chain(layout.charts.iter().map(|c| c.frame))
        .chain(layout.text_boxes.iter().map(|t| t.frame))
        .map(|f| f.origin.y.as_pt() + f.size.height.as_pt())
        .fold(0.0_f32, f32::max);
    extend_drawing_grid(
        &mut widths,
        drawing_width,
        layout.default_column_width,
        available_w,
    );
    extend_drawing_grid(
        &mut heights,
        drawing_height,
        layout.default_row_height,
        available_h,
    );
    let images: Vec<_> = layout
        .images
        .iter()
        .enumerate()
        .filter_map(|(index, i)| {
            register_image(pdf, i).map(|name| {
                (
                    i.frame,
                    name,
                    layout.image_crops.get(&index).copied().unwrap_or([0.0; 4]),
                )
            })
        })
        .collect();
    let valid_repeat = |repeat: Option<(u32, u32)>, sizes: &[f32], available: f32| {
        repeat.filter(|(a, b)| {
            a <= b
                && (*b as usize) < sizes.len()
                && sizes[*a as usize..=*b as usize].iter().sum::<f32>() < available
        })
    };
    let repeat_columns = valid_repeat(layout.repeat_columns, &widths, available_w);
    let repeat_rows = valid_repeat(layout.repeat_rows, &heights, available_h);
    let area = layout.print_area;
    let column_pages = grid_bands_region(
        &widths,
        available_w,
        repeat_columns,
        &layout.column_breaks,
        area.map(|(a, b)| (a.col, b.col)),
    );
    let row_pages = grid_bands_region(
        &heights,
        available_h,
        repeat_rows,
        &layout.row_breaks,
        area.map(|(a, b)| (a.row, b.row)),
    );
    for columns in column_pages {
        for rows in &row_pages {
            let repeated_columns = repeated_indices(repeat_columns, columns.start, widths.len());
            let repeated_rows = repeated_indices(repeat_rows, rows.start, heights.len());
            let repeat_w: f32 = repeated_columns.iter().map(|i| widths[*i]).sum();
            let repeat_h: f32 = repeated_rows.iter().map(|i| heights[*i]).sum();
            let column_indices: Vec<_> = repeated_columns
                .into_iter()
                .chain(columns.clone())
                .collect();
            let row_indices: Vec<_> = repeated_rows.into_iter().chain(rows.clone()).collect();
            let ox: f32 = widths[..columns.start].iter().sum::<f32>() - repeat_w;
            let oy: f32 = heights[..rows.start].iter().sum::<f32>() - repeat_h;
            let tw: f32 = widths[columns.clone()].iter().sum();
            let th: f32 = heights[rows.clone()].iter().sum();
            let image_present = images
                .iter()
                .map(|(f, _, _)| f)
                .chain(layout.charts.iter().map(|c| &c.frame))
                .chain(layout.text_boxes.iter().map(|t| &t.frame))
                .any(|f| {
                    f.origin.x.as_pt() < ox + tw
                        && f.origin.x.as_pt() + f.size.width.as_pt() > ox
                        && f.origin.y.as_pt() < oy + th
                        && f.origin.y.as_pt() + f.size.height.as_pt() > oy
                });
            // Empty page tiles do not need a printed page.
            if !sheet.cells.keys().any(|a| {
                columns.contains(&(a.col as usize))
                    && rows.contains(&(a.row as usize))
                    && widths[a.col as usize] > 0.0
                    && heights[a.row as usize] > 0.0
            }) && !image_present
                && !(columns.start == 0
                    && rows.start == 0
                    && (!layout.charts.is_empty()
                        || !layout.headers.is_empty()
                        || !layout.footers.is_empty()))
            {
                continue;
            }
            let left = if layout.horizontal_centered {
                left + ((available_w - tw - repeat_w) / 2.0).max(0.0)
            } else {
                left
            };
            let index = pdf.add_page(paper.0, paper.1);
            let page = pdf.page_mut(index).expect("grid page");
            page.raw(&format!("q {scale} 0 0 {scale} 0 0 cm"));
            let mut top = layout.page_height - top_margin - layout.header_height - heading_h;
            if layout.print_headings {
                let mut hx = left + heading_w;
                for c in column_indices.iter().copied() {
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
            for r in row_indices.iter().copied() {
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
                for c in column_indices.iter().copied() {
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
                        let mut text = sheet
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
                        if let Some((fraction, paint, show)) = layout.data_bars.get(&addr) {
                            if let Some((red, green, blue)) = grid_color(paint) {
                                page.rect_fill_rgb(
                                    x + 2.0,
                                    top - h + 2.0,
                                    (w - 4.0).max(0.0) * fraction,
                                    (h - 4.0).max(0.0),
                                    red,
                                    green,
                                    blue,
                                );
                            }
                            if !show {
                                text.clear();
                            }
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
                        let icon_inset = if let Some(icon) = layout.conditional_icons.get(&addr) {
                            if !icon.show_value { text.clear(); }
                            crate::xlsx_icons::render(page, icon, x, top, w, h)
                        } else { 0.0 };
                        let text_x = x + icon_inset;
                        let text_w = (w - icon_inset).max(0.0);
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
                        let indent = style.map(|s| s.indent as f32).unwrap_or(0.0)
                            * lo_core::font_metrics::measure_base_font("   ", size, font);
                        let size = if style.is_some_and(|s| s.shrink_to_fit && !s.wrap) {
                            size * ((text_w - 4.0 - indent).max(1.0)
                                / lo_core::font_metrics::measure_base_font(&text, size, font)
                                    .max(1.0))
                            .min(1.0)
                        } else {
                            size
                        };
                        let rotation = style.map(|s| s.rotation_degrees).unwrap_or(0);
                        // Text can overflow into adjacent empty cells, as in Calc;
                        // occupied cells and page edges stop it. Numeric values stay
                        // within their own column. Clip to the source row height.
                        let mut visible_w = text_w;
                        if matches!(cell.value, CellValue::Text(_))
                            && !style.is_some_and(|s| s.wrap || s.rotation_degrees != 0)
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
                                visible_w = layout.page_width - right - text_x;
                            }
                        }
                        if rotation == 0 {
                            if let (Some(runs), Some(style)) = (
                                layout.rich_text.get(&CellAddr::new(r as u32, c as u32)),
                                style,
                            ) {
                                crate::xlsx_rich_text::render(
                                    page, runs, style, text_x, top, text_w, h, visible_w, indent,
                                );
                                x += w;
                                continue;
                            }
                        }
                        page.raw(&format!("q {} {} {} {} re W n", text_x, top - h, visible_w, h));
                        let lines = if style.is_some_and(|s| s.wrap) {
                            grid_wrap_font(&text, (text_w - 4.0 - indent).max(1.0), size, font)
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
                            if rotation != 0 {
                                let angle = (rotation as f32).to_radians();
                                let (sin, cos) = angle.sin_cos();
                                page.raw(&format!(
                                    "q {cos} {sin} {} {cos} {} {} cm",
                                    -sin,
                                    text_x + text_w / 2.0,
                                    top - h / 2.0
                                ));
                                page.text_rgb(
                                    -estimated / 2.0,
                                    -size * 0.3 - line_index as f32 * size * 1.2,
                                    size,
                                    font,
                                    line,
                                    color.0,
                                    color.1,
                                    color.2,
                                );
                                page.raw("Q");
                                continue;
                            }
                            let offset = match style.map(|s| &s.alignment) {
                                Some(Alignment::End) => (text_w - 2.0 - indent - estimated).max(2.0),
                                Some(Alignment::Center) => ((text_w - estimated) / 2.0).max(2.0),
                                _ => 2.0 + indent,
                            };
                            let y = baseline - line_index as f32 * size * 1.2;
                            if y < top - h {
                                break;
                            }
                            if style.is_some_and(|s| s.underline) {
                                page.line_rgb(
                                    text_x + offset,
                                    y - 1.0,
                                    text_x + offset
                                        + lo_core::font_metrics::measure_base_font(
                                            line, size, font,
                                        ),
                                    y - 1.0,
                                    color.0,
                                    color.1,
                                    color.2,
                                );
                            }
                            page.text_rgb(
                                text_x + offset,
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
            page.raw(&format!(
                "q {} {} {} {} re W n",
                left + heading_w + repeat_w,
                bottom + layout.footer_height,
                available_w - repeat_w,
                available_h - repeat_h
            ));
            for (f, name, crop) in &images {
                if !frame_intersects(*f, ox, oy, tw, th) {
                    continue;
                }
                let x = left + heading_w + f.origin.x.as_pt() - ox;
                let y = layout.page_height
                    - top_margin
                    - layout.header_height
                    - heading_h
                    - f.origin.y.as_pt()
                    + oy
                    - f.size.height.as_pt();
                let w = f.size.width.as_pt();
                let h = f.size.height.as_pt();
                if let Some((ix, iy, iw, ih)) = cropped_image_rect(x, y, w, h, *crop) {
                    page.raw(&format!("q {x} {y} {w} {h} re W n"));
                    page.image(name, ix, iy, iw, ih);
                    page.raw("Q");
                }
            }
            page.raw("Q");
            {
                page.raw(&format!(
                    "q {} {} {} {} re W n",
                    left + heading_w + repeat_w,
                    bottom + layout.footer_height,
                    available_w - repeat_w,
                    available_h - repeat_h
                ));
                for text in &layout.text_boxes {
                    if !frame_intersects(text.frame, ox, oy, tw, th) {
                        continue;
                    }
                    let f = text.frame;
                    let x = left + heading_w + f.origin.x.as_pt() - ox;
                    let y = layout.page_height
                        - top_margin
                        - layout.header_height
                        - heading_h
                        - f.origin.y.as_pt()
                        + oy;
                    let w = f.size.width.as_pt();
                    let h = f.size.height.as_pt();
                    let pad = text.style.padding.as_pt();
                    let size = text.style.font_size_pt.clamp(1.0, 409.0);
                    page.raw(&format!("q {x} {} {w} {h} re W n", y - h));
                    if let Some((r, g, b)) = grid_color(&text.style.background) {
                        page.rect_fill_rgb(x, y - h, w, h, r, g, b);
                    }
                    let (r, g, b) = grid_color(&text.style.foreground).unwrap_or((0.0, 0.0, 0.0));
                    for (i, line) in grid_wrap(&text.text, (w - 2.0 * pad).max(1.0), size)
                        .iter()
                        .enumerate()
                    {
                        let baseline = y - pad - size - i as f32 * size * 1.2;
                        if baseline < y - h {
                            break;
                        }
                        if baseline < bottom + layout.footer_height
                            || baseline + size
                                > layout.page_height
                                    - top_margin
                                    - layout.header_height
                                    - heading_h
                                    - repeat_h
                        {
                            continue;
                        }
                        page.text_rgb(x + pad, baseline, size, PdfFont::Helvetica, line, r, g, b);
                    }
                    page.raw("Q");
                }
                for chart in &layout.charts {
                    if !frame_intersects(chart.frame, ox, oy, tw, th) {
                        continue;
                    }
                    chart.render(
                        page,
                        layout.page_height,
                        left + heading_w - ox,
                        top_margin + layout.header_height + heading_h - oy,
                    );
                }
                page.raw("Q");
            }
            page.raw("Q");
        }
    }
}

pub(crate) fn grid_wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    grid_wrap_font(text, width, size, PdfFont::Helvetica)
}

pub(crate) fn grid_wrap_font(text: &str, width: f32, size: f32, font: PdfFont) -> Vec<String> {
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

fn repeated_indices(repeat: Option<(u32, u32)>, start: usize, len: usize) -> Vec<usize> {
    match repeat {
        Some((a, b)) if a <= b && (b as usize) < start && (b - a) < 16384 => {
            (a as usize..=b as usize).filter(|i| *i < len).collect()
        }
        _ => Vec::new(),
    }
}
fn cropped_image_rect(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    crop: [f32; 4],
) -> Option<(f32, f32, f32, f32)> {
    let [left, top, right, bottom] = crop;
    let visible_w = 1.0 - left - right;
    let visible_h = 1.0 - top - bottom;
    if !crop.iter().all(|v| v.is_finite() && v.abs() <= 10.0)
        || visible_w <= 0.001
        || visible_h <= 0.001
    {
        return None;
    }
    let iw = w / visible_w;
    let ih = h / visible_h;
    Some((x - left * iw, y - bottom * ih, iw, ih))
}

fn grid_bands_region(
    sizes: &[f32],
    available: f32,
    repeat: Option<(u32, u32)>,
    breaks: &std::collections::BTreeSet<u32>,
    area: Option<(u32, u32)>,
) -> Vec<std::ops::Range<usize>> {
    let (first, end) = area
        .map(|(a, b)| (a as usize, (b as usize).saturating_add(1)))
        .unwrap_or((0, sizes.len()));
    let end = end.min(sizes.len());
    let mut start = first.min(end);
    let mut occupied = 0.0;
    let mut ranges = Vec::new();
    for i in start..end {
        let size = sizes[i];
        if i > start && (breaks.contains(&(i as u32)) || occupied + size > available + 0.01) {
            ranges.push(start..i);
            start = i;
            occupied = repeated_indices(repeat, start, sizes.len())
                .iter()
                .map(|i| sizes[*i])
                .sum::<f32>()
                .min((available - size).max(0.0));
        }
        occupied += size;
    }
    if start < end {
        ranges.push(start..end);
    }
    ranges
}

#[cfg(test)]
fn grid_bands_repeated(
    sizes: &[f32],
    available: f32,
    repeat: Option<(u32, u32)>,
) -> Vec<std::ops::Range<usize>> {
    grid_bands_region(sizes, available, repeat, &Default::default(), None)
}

#[cfg(test)]
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
    fn cropped_images_scale_and_translate_before_frame_clipping() {
        assert_eq!(
            cropped_image_rect(10.0, 20.0, 100.0, 50.0, [0.5, 0.0, 0.0, 0.0]),
            Some((-90.0, 20.0, 200.0, 50.0))
        );
        assert_eq!(
            cropped_image_rect(10.0, 20.0, 100.0, 50.0, [0.0, 0.0, 0.0, 0.5]),
            Some((10.0, -30.0, 100.0, 100.0))
        );
        assert!(cropped_image_rect(0.0, 0.0, 1.0, 1.0, [0.5; 4]).is_none());
    }
    #[test]
    fn print_area_and_manual_breaks_bound_page_ranges() {
        let sizes = [20.0; 10];
        assert_eq!(
            grid_bands_region(
                &sizes,
                200.0,
                None,
                &std::collections::BTreeSet::from([5]),
                None
            ),
            vec![0..5, 5..10]
        );
        assert_eq!(
            grid_bands_region(
                &sizes,
                200.0,
                None,
                &std::collections::BTreeSet::from([5]),
                Some((1, 3))
            ),
            vec![1..4]
        );
        assert_eq!(
            grid_bands_region(
                &sizes,
                60.0,
                Some((0, 0)),
                &std::collections::BTreeSet::from([4]),
                None
            ),
            vec![0..3, 3..4, 4..6, 6..8, 8..10]
        );
    }
    #[test]
    fn print_fit_scales_content_without_changing_paper_size() {
        let mut book = Workbook::default();
        let sheet = &mut book.sheets[0];
        for r in 0..8 {
            for c in 0..4 {
                sheet.set(CellAddr::new(r, c), CellValue::Number(42.0));
            }
        }
        sheet.layout = Some(lo_core::calc::SheetLayout {
            page_width: 200.0,
            page_height: 200.0,
            margin: 20.0,
            default_column_width: 80.0,
            default_row_height: 40.0,
            fit_to_pages: Some((1, 1)),
            ..Default::default()
        });
        assert_eq!(render(&book).pages().len(), 1);
        let pdf = String::from_utf8_lossy(&to_pdf(&book)).into_owned();
        assert!(pdf.contains("q 0.5 0 0 0.5 0 0 cm"));
        assert!(pdf.contains("/MediaBox [0 0 200.00 200.00]"));
        // Natural-width preview still retains vertical pagination at source scale.
        let pdf = String::from_utf8_lossy(&to_pdf_preview(&book)).into_owned();
        assert_eq!(pdf.matches("/Type /Page ").count(), 2);
        let layout = book.sheets[0].layout.as_mut().unwrap();
        layout.chart_sheet = true;
        layout.fit_to_pages = None;
        let pdf = String::from_utf8_lossy(&to_pdf_preview(&book)).into_owned();
        assert_eq!(pdf.matches("/Type /Page ").count(), 1);
        assert!(pdf.contains("/MediaBox [0 0 200.00 200.00]"));
    }
    #[test]
    fn repeated_titles_reserve_space_on_later_pages() {
        assert_eq!(
            grid_bands_repeated(&[20.0; 8], 60.0, Some((0, 0))),
            [0..3, 3..5, 5..7, 7..8]
        );
        let mut book = Workbook::default();
        let sheet = &mut book.sheets[0];
        for r in 0..8 {
            sheet.set(
                CellAddr::new(r, 0),
                CellValue::Text(if r == 0 {
                    "Repeated title".into()
                } else {
                    format!("row {r}")
                }),
            );
        }
        sheet.layout = Some(lo_core::calc::SheetLayout {
            page_width: 200.0,
            page_height: 100.0,
            margin: 20.0,
            default_row_height: 20.0,
            repeat_rows: Some((0, 0)),
            ..Default::default()
        });
        let pdf = String::from_utf8_lossy(&to_pdf(&book)).into_owned();
        assert_eq!(pdf.matches("(Repeated title)").count(), 4);
        assert_eq!(render(&book).pages().len(), 4);
    }
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
        if layout.chart_sheet {
            continue;
        }
        layout.print_scale = 1.0;
        layout.fit_to_pages = None;
        for (a, (cols, _)) in &layout.merges {
            max_col = max_col.max(a.col.saturating_add(*cols).saturating_sub(1));
        }
        let (first_col, last_col) = layout
            .print_area
            .map(|(a, b)| (a.col, b.col))
            .unwrap_or((0, max_col));
        let width = (first_col..=last_col.min(16383))
            .map(|c| {
                layout
                    .columns
                    .get(&c)
                    .copied()
                    .unwrap_or(layout.default_column_width)
            })
            .sum::<f32>();
        let width = if layout.print_area.is_some() {
            width
        } else {
            layout
                .images
                .iter()
                .map(|i| i.frame)
                .chain(layout.charts.iter().map(|c| c.frame))
                .chain(layout.text_boxes.iter().map(|t| t.frame))
                .map(|f| f.origin.x.as_pt() + f.size.width.as_pt())
                .fold(width, f32::max)
        };
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
    if let Some(font) = lo_core::source_font::resolve(&style.font_family, style.bold, style.italic)
    {
        return font;
    }
    let family = style.font_family.to_ascii_lowercase();
    if family.contains("courier")
        || family.contains("mono")
        || family == "consolas"
        || family.contains("typewriter")
    {
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

fn extend_drawing_grid(values: &mut Vec<f32>, extent: f32, default: f32, available: f32) {
    let step = default.max(1.0).min(available.max(1.0));
    let mut total: f32 = values.iter().sum();
    while total < extent && values.len() < 100_000 {
        let n = step.min(extent - total);
        values.push(n);
        total += n;
    }
}
fn register_image(pdf: &mut PdfDocument, image: &lo_core::ImageElement) -> Option<String> {
    if image.mime_type == "image/jpeg" {
        let (w, h) = jpeg_dimensions(&image.data)?;
        return Some(pdf.add_jpeg_image(w, h, image.data.clone()));
    }
    // Bound PNG allocation before invoking the decoder.
    let b = &image.data;
    if b.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(b.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(b.get(20..24)?.try_into().ok()?);
    let limits = lo_core::preview_limits::PreviewLimits::from_env().ok()?;
    if u64::from(w) * u64::from(h) > limits.png_pixels as u64 {
        return None;
    }
    let i = lo_core::decode_png(b).ok()?;
    Some(pdf.add_rgba_image(i.width, i.height, i.pixels))
}
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2usize;
    while index + 4 <= bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;
        if matches!(marker, 0xd8 | 0xd9) || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let len = u16::from_be_bytes(bytes.get(index..index + 2)?.try_into().ok()?) as usize;
        if len < 2 || index + len > bytes.len() {
            return None;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            let height =
                u16::from_be_bytes(bytes.get(index + 3..index + 5)?.try_into().ok()?) as u32;
            let width =
                u16::from_be_bytes(bytes.get(index + 5..index + 7)?.try_into().ok()?) as u32;
            return Some((width, height));
        }
        index += len;
    }
    None
}

fn frame_intersects(f: lo_core::Rect, x: f32, y: f32, w: f32, h: f32) -> bool {
    f.origin.x.as_pt() < x + w
        && f.origin.x.as_pt() + f.size.width.as_pt() > x
        && f.origin.y.as_pt() < y + h
        && f.origin.y.as_pt() + f.size.height.as_pt() > y
}
