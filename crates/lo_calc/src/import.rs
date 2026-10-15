//! Binary importers for `Workbook`.
//!
//! Supports `csv`, `xlsx`, `ods` format hints.

use std::collections::BTreeMap;

use lo_core::{
    parse_xml_document, CellAddr, CellValue, LoError, Result, Sheet, Workbook, XmlItem, XmlNode,
};
use lo_zip::{rels_path_for, resolve_part_target, ZipArchive};

use crate::workbook_from_csv;

pub fn load_bytes(title: impl Into<String>, bytes: &[u8], format: &str) -> Result<Workbook> {
    let title = title.into();
    match format.to_ascii_lowercase().as_str() {
        "csv" => workbook_from_csv(title.clone(), &title, &bytes_to_utf8(bytes)?),
        "xlsx" => from_xlsx_bytes(title, bytes),
        "ods" => from_ods_bytes(title, bytes),
        other => Err(LoError::Unsupported(format!("calc import format {other}"))),
    }
}

fn bytes_to_utf8(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|err| LoError::Parse(format!("invalid utf-8 input: {err}")))
}

pub fn from_xlsx_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<Workbook> {
    let _ = title.into();
    let zip = ZipArchive::new(bytes)?;
    // Pull real title from docProps/core.xml; skip the placeholder caller
    // so synthesized "workbook" titles never bleed into Markdown / PDFs.
    let title: String = if zip.contains("docProps/core.xml") {
        parse_xml_document(&zip.read_string("docProps/core.xml")?)
            .ok()
            .and_then(|root| root.child("title").map(|n| n.text_content()))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let workbook_root = parse_xml_document(&zip.read_string("xl/workbook.xml")?)?;
    let rels = parse_relationships(&zip, "xl/workbook.xml")?;
    let shared_strings = if zip.contains("xl/sharedStrings.xml") {
        parse_shared_strings(&parse_xml_document(
            &zip.read_string("xl/sharedStrings.xml")?,
        )?)
    } else {
        Vec::new()
    };

    let styles_xml = if zip.contains("xl/styles.xml") {
        Some(parse_xml_document(&zip.read_string("xl/styles.xml")?)?)
    } else {
        None
    };
    let theme_xml = if zip.contains("xl/theme/theme1.xml") {
        Some(parse_xml_document(
            &zip.read_string("xl/theme/theme1.xml")?,
        )?)
    } else {
        None
    };
    let styles = crate::xlsx_styles::parse(styles_xml.as_ref(), theme_xml.as_ref());
    let date_1904 = workbook_root
        .child("workbookPr")
        .is_some_and(|n| crate::xlsx_styles::truth(n.attr("date1904")));
    let mut workbook = Workbook::new(title);
    workbook.sheets.clear();

    if let Some(sheets_node) = workbook_root.child("sheets") {
        for (index, sheet_node) in sheets_node.children_named("sheet").enumerate() {
            let name = sheet_node.attr("name").unwrap_or("Sheet").to_string();
            let target = sheet_node
                .attr("id")
                .or_else(|| sheet_node.attr("r:id"))
                .and_then(|id| rels.get(id))
                .cloned()
                .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", index + 1));
            if !zip.contains(&target) {
                continue;
            }
            let sheet_xml = parse_xml_document(&zip.read_string(&target)?)?;
            let mut sheet =
                parse_xlsx_sheet(&name, &sheet_xml, &shared_strings, &styles, date_1904)?;
            sheet.layout.as_mut().unwrap().hidden =
                matches!(sheet_node.attr("state"), Some("hidden" | "veryHidden"));
            crate::xlsx_drawings::import(
                &zip,
                &target,
                &sheet_xml,
                sheet.layout.as_mut().unwrap(),
            )?;
            if let Some(names) = workbook_root.child("definedNames") {
                for name in names.children_named("definedName") {
                    if name.attr("name") != Some("_xlnm.Print_Titles")
                        || name
                            .attr("localSheetId")
                            .and_then(|v| v.parse::<usize>().ok())
                            != Some(index)
                    {
                        continue;
                    }
                    for range in name.text_content().split(',') {
                        let Some((_, range)) = range.rsplit_once('!') else {
                            continue;
                        };
                        let range = range.replace('$', "");
                        let Some((a, b)) = range.split_once(':') else {
                            continue;
                        };
                        let layout = sheet.layout.as_mut().unwrap();
                        if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                            if a > 0 && b >= a && b <= 1048576 && b - a < 1024 {
                                layout.repeat_rows = Some((a - 1, b - 1));
                            }
                        } else if a.len() <= 3 && b.len() <= 3 {
                            if let (Some((_, a)), Some((_, b))) = (
                                parse_cell_ref(&format!("{a}1")),
                                parse_cell_ref(&format!("{b}1")),
                            ) {
                                if b >= a {
                                    layout.repeat_columns = Some((a - 1, b - 1));
                                }
                            }
                        }
                    }
                }
            }
            crate::xlsx_conditional::apply(
                &mut sheet,
                &sheet_xml,
                styles_xml.as_ref(),
                theme_xml.as_ref(),
            );
            workbook.sheets.push(sheet);
        }
    }
    if workbook.sheets.is_empty() {
        workbook.sheets.push(Sheet::new("Sheet1"));
    }
    Ok(workbook)
}

pub fn from_ods_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<Workbook> {
    let title = title.into();
    let zip = ZipArchive::new(bytes)?;
    let content = parse_xml_document(&zip.read_string("content.xml")?)?;
    let body = content
        .child("body")
        .ok_or_else(|| LoError::Parse("content.xml missing office:body".to_string()))?;
    let spreadsheet = body
        .child("spreadsheet")
        .ok_or_else(|| LoError::Parse("content.xml missing office:spreadsheet".to_string()))?;

    let mut styles = lo_core::style::OdfStyles::default();
    let page_styles = if zip.contains("styles.xml") {
        Some(parse_xml_document(&zip.read_string("styles.xml")?)?)
    } else {
        None
    };
    if let Some(root) = &page_styles {
        styles.collect(root);
    }
    styles.collect(&content);
    let mut workbook = Workbook::new(title);
    workbook.sheets.clear();
    for table in spreadsheet.children_named("table") {
        let mut sheet = parse_ods_sheet(table)?;
        apply_ods_layout(table, &styles, page_styles.as_ref(), &mut sheet);
        sheet.layout.as_mut().unwrap().charts = lo_zip::odf_charts(table, &zip);
        workbook.sheets.push(sheet);
    }
    if workbook.sheets.is_empty() {
        workbook.sheets.push(Sheet::new("Sheet1"));
    }
    Ok(workbook)
}

// ---------------------------------------------------------------------------

pub(crate) fn parse_relationships(
    zip: &ZipArchive,
    part: &str,
) -> Result<BTreeMap<String, String>> {
    let rels_path = rels_path_for(part);
    if !zip.contains(&rels_path) {
        return Ok(BTreeMap::new());
    }
    let root = parse_xml_document(&zip.read_string(&rels_path)?)?;
    let mut map = BTreeMap::new();
    for rel in root.children_named("Relationship") {
        if rel.attr("TargetMode") == Some("External") {
            continue;
        }
        if let (Some(id), Some(target)) = (rel.attr("Id"), rel.attr("Target")) {
            map.insert(id.to_string(), resolve_part_target(part, target));
        }
    }
    Ok(map)
}

fn parse_shared_strings(root: &XmlNode) -> Vec<String> {
    let mut strings = Vec::new();
    for si in root.children_named("si") {
        strings.push(xlsx_text(si));
    }
    strings
}

fn xlsx_text(node: &XmlNode) -> String {
    let mut out = String::new();
    for item in &node.items {
        match item {
            XmlItem::Text(text) => out.push_str(text),
            XmlItem::Node(child) => match child.local_name() {
                "t" => out.push_str(&child.text_content()),
                "rPh" | "phoneticPr" => {}
                _ => out.push_str(&xlsx_text(child)),
            },
        }
    }
    out
}

fn parse_xlsx_sheet(
    name: &str,
    root: &XmlNode,
    shared_strings: &[String],
    styles: &crate::xlsx_styles::Styles,
    date_1904: bool,
) -> Result<Sheet> {
    use crate::xlsx_styles::{positive, truth};
    use lo_core::{calc::SheetLayout, style::Alignment};
    let mut sheet = Sheet::new(name.to_string());
    let mut layout = SheetLayout {
        page_width: 612.0,
        page_height: 792.0,
        margins: Some([50.4, 50.4, 54.0, 54.0]),
        default_column_width: 48.0,
        default_row_height: 15.0,
        ..SheetLayout::default()
    };
    let normal = &styles.cells[0];
    let normal_size = normal.font_size_mpt.unwrap_or(11000) as f32 / 1000.0;
    let digit = lo_core::font_metrics::measure_base_font(
        "0",
        normal_size,
        crate::pdf::cell_font(Some(normal)),
    )
    .round()
    .max(1.0);
    layout.default_column_width = 10.0 * digit + 5.0;
    layout.default_row_height = (normal_size * 1.2 + 3.0).ceil().max(15.0);
    if let Some(format) = root.child("sheetFormatPr") {
        layout.default_column_width = if format.attr("defaultColWidth").is_some() {
            excel_column_width(positive(format, "defaultColWidth", 8.43), digit)
        } else {
            positive(format, "baseColWidth", 8.0) * digit + 5.0
        };
        layout.default_row_height = positive(format, "defaultRowHeight", 15.0).clamp(0.0, 409.0);
    }
    if let Some(setup) = root.child("pageSetup") {
        let (w, h) = match setup.attr("paperSize") {
            Some("9") => (595.28, 841.89),
            Some("8") => (841.89, 1190.55),
            Some("5") => (612.0, 1008.0),
            Some("3" | "4") => (792.0, 1224.0),
            _ => (612.0, 792.0),
        };
        (layout.page_width, layout.page_height) = if setup.attr("orientation") == Some("landscape")
        {
            (h, w)
        } else {
            (w, h)
        };
    }
    if let Some(margins) = root.child("pageMargins") {
        layout.margins = Some([
            positive(margins, "left", 0.7) * 72.0,
            positive(margins, "right", 0.7) * 72.0,
            positive(margins, "top", 0.75) * 72.0,
            positive(margins, "bottom", 0.75) * 72.0,
        ]);
    }
    let margins = layout.margins.unwrap();
    if margins[0] + margins[1] >= layout.page_width || margins[2] + margins[3] >= layout.page_height
    {
        return Err(LoError::InvalidInput(
            "XLSX margins leave no printable area".into(),
        ));
    }
    let mut column_styles = BTreeMap::new();
    if let Some(cols) = root.child("cols") {
        let mut expanded = 0usize;
        for col in cols.children_named("col") {
            let min = col
                .attr("min")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1);
            let max = col
                .attr("max")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(min);
            if min == 0 || max < min || max > 16384 {
                return Err(LoError::InvalidInput("Invalid XLSX column range".into()));
            }
            expanded += (max - min + 1) as usize;
            if expanded > 100_000 {
                return Err(LoError::InvalidInput("Excessive XLSX column ranges".into()));
            }
            let width = if truth(col.attr("hidden")) {
                0.0
            } else if col.attr("width").is_some() {
                excel_column_width(positive(col, "width", 8.43), digit)
            } else {
                layout.default_column_width
            };
            for c in min - 1..max {
                layout.columns.insert(c, width);
                if let Some(s) = col.attr("style").and_then(|s| s.parse::<usize>().ok()) {
                    column_styles.insert(c, s);
                }
            }
        }
    }
    let mut missing_cache = 0;
    let mut explicit_rows = std::collections::BTreeSet::new();
    if let Some(sheet_data) = root.child("sheetData") {
        let mut next_row = 1;
        let mut cell_count = 0;
        for row in sheet_data.children_named("row") {
            let r = row
                .attr("r")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(next_row);
            if !(1..=1_048_576).contains(&r) {
                return Err(LoError::InvalidInput("Invalid XLSX row".into()));
            }
            next_row = r + 1;
            if row.attr("ht").is_some() || truth(row.attr("hidden")) {
                explicit_rows.insert(r - 1);
                layout.rows.insert(
                    r - 1,
                    if truth(row.attr("hidden")) {
                        0.0
                    } else {
                        positive(row, "ht", layout.default_row_height).clamp(0.0, 409.0)
                    },
                );
            }
            let row_style = row.attr("s").and_then(|s| s.parse::<usize>().ok());
            let mut next_col = 1;
            for cell in row.children_named("c") {
                cell_count += 1;
                if cell_count > 100_000 {
                    return Err(LoError::InvalidInput(
                        "XLSX preview exceeds 100,000 cells per sheet".into(),
                    ));
                }
                let (cr, cc) = match cell.attr("r") {
                    Some(reference) => parse_cell_ref(reference)
                        .ok_or_else(|| LoError::InvalidInput("Invalid XLSX cell address".into()))?,
                    None => (r, next_col),
                };
                next_col = cc + 1;
                let addr = CellAddr::new(cr - 1, cc - 1);
                let index = cell
                    .attr("s")
                    .and_then(|s| s.parse::<usize>().ok())
                    .or(row_style)
                    .or_else(|| column_styles.get(&(cc - 1)).copied())
                    .unwrap_or(0);
                let mut style = styles.cells.get(index).unwrap_or(&styles.cells[0]).clone();
                let cached = parse_xlsx_value(cell, shared_strings);
                if styles.general_alignment.get(index).copied().unwrap_or(true) {
                    style.alignment = match cached {
                        CellValue::Number(_) => Alignment::End,
                        CellValue::Bool(_) | CellValue::Error(_) => Alignment::Center,
                        _ => Alignment::Start,
                    };
                }
                let display = match &cached {
                    CellValue::Number(n) => {
                        crate::xlsx_format::number(*n, &style.format_code, date_1904)
                    }
                    CellValue::Text(s) | CellValue::Error(s) => s.clone(),
                    CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
                    _ => String::new(),
                };
                let value = if let Some(formula) = cell.child("f") {
                    if matches!(cached, CellValue::Empty) {
                        missing_cache += 1;
                    }
                    CellValue::Formula(formula.text_content())
                } else {
                    cached
                };
                if value == CellValue::Empty
                    && style.background.is_empty()
                    && style.borders.iter().all(String::is_empty)
                {
                    continue;
                }
                sheet.set(addr, value);
                sheet.display_values.insert(addr, display);
                sheet.cells.get_mut(&addr).unwrap().style = Some(style);
            }
        }
    }
    if missing_cache > 0 {
        eprintln!("XLSX preview: {missing_cache} formula cells have no saved result; left blank");
    }
    if let Some(merges) = root.child("mergeCells") {
        let mut total = 0u64;
        for merge in merges.children_named("mergeCell") {
            let Some((a, b)) = merge.attr("ref").and_then(|s| s.split_once(':')) else {
                continue;
            };
            let (ar, ac) = parse_cell_ref(a)
                .ok_or_else(|| LoError::InvalidInput("Invalid XLSX merge".into()))?;
            let (br, bc) = parse_cell_ref(b)
                .ok_or_else(|| LoError::InvalidInput("Invalid XLSX merge".into()))?;
            if br < ar || bc < ac {
                return Err(LoError::InvalidInput("Reversed XLSX merge".into()));
            }
            total += u64::from(br - ar + 1) * u64::from(bc - ac + 1);
            if total > 1_000_000 {
                return Err(LoError::InvalidInput(
                    "XLSX merged area exceeds preview limit".into(),
                ));
            }
            let anchor = CellAddr::new(ar - 1, ac - 1);
            // Blank anchors can carry borders/backgrounds and still occupy the merged region.
            sheet.cells.entry(anchor).or_default();
            layout.merges.insert(anchor, (bc - ac + 1, br - ar + 1));
        }
    }
    let (mut max_r, mut max_c) = sheet.max_extent();
    for (a, (cols, rows)) in &layout.merges {
        max_r = max_r.max(a.row + rows - 1);
        max_c = max_c.max(a.col + cols - 1);
    }
    if u64::from(max_r + 1) * u64::from(max_c + 1) > 2_000_000 {
        return Err(LoError::InvalidInput(
            "XLSX preview grid exceeds 2,000,000 positions".into(),
        ));
    }
    // Grow automatic wrapped rows; explicit heights and merged multi-row boxes are authoritative.
    for (addr, cell) in &sheet.cells {
        if explicit_rows.contains(&addr.row) {
            continue;
        }
        let Some(style) = cell.style.as_ref().filter(|s| s.wrap) else {
            continue;
        };
        let (span, rows) = layout.merges.get(addr).copied().unwrap_or((1, 1));
        if rows > 1 {
            continue;
        }
        let width = (addr.col..addr.col + span)
            .map(|c| {
                layout
                    .columns
                    .get(&c)
                    .copied()
                    .unwrap_or(layout.default_column_width)
            })
            .sum::<f32>();
        let size = style.font_size_mpt.unwrap_or(11000) as f32 / 1000.0;
        let text = sheet
            .display_values
            .get(addr)
            .map(String::as_str)
            .unwrap_or("");
        let height =
            crate::pdf::grid_wrap(text, (width - 4.0).max(1.0), size).len() as f32 * size * 1.2
                + 3.0;
        let current = layout
            .rows
            .entry(addr.row)
            .or_insert(layout.default_row_height);
        *current = current.max(height.min(409.0));
    }
    if let Some(options) = root.child("printOptions") {
        layout.print_headings = truth(options.attr("headings"));
        layout.print_gridlines = truth(options.attr("gridLines"));
    }
    if let Some(hf) = root.child("headerFooter") {
        layout.headers = xlsx_header_footer(hf.child("oddHeader"), name);
        layout.footers = xlsx_header_footer(hf.child("oddFooter"), name);
    }
    sheet.layout = Some(layout);
    Ok(sheet)
}

fn excel_column_width(width: f32, digit: f32) -> f32 {
    // Declared widths include padding and use the Normal font's digit advance.
    // Substitute-font metrics are approximate when the original font is absent.
    (width.min(255.0) * digit).round()
}

fn xlsx_header_footer(
    node: Option<&XmlNode>,
    sheet: &str,
) -> Vec<(u8, Vec<lo_core::OdfPageField>)> {
    use lo_core::OdfPageField as F;
    let Some(node) = node else { return Vec::new() };
    let text = node.text_content();
    let mut chars = text.chars().peekable();
    let mut regions: BTreeMap<u8, Vec<F>> = BTreeMap::new();
    let mut region = 1;
    let mut literal = String::new();
    while let Some(c) = chars.next() {
        if c != '&' {
            literal.push(c);
            continue;
        }
        let Some(code) = chars.next() else { break };
        if !literal.is_empty() {
            regions
                .entry(region)
                .or_default()
                .push(F::Text(std::mem::take(&mut literal)));
        }
        match code {
            'L' => region = 0,
            'C' => region = 1,
            'R' => region = 2,
            'P' => regions.entry(region).or_default().push(F::PageNumber),
            'N' => regions.entry(region).or_default().push(F::PageCount),
            'A' => literal.push_str(sheet),
            '&' => literal.push('&'),
            '"' => {
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                }
            }
            '0'..='9' => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            'K' => {
                for _ in 0..6 {
                    chars.next();
                }
            }
            // Formatting toggles and unsupported dynamic fields have no literal text.
            _ => {}
        }
    }
    if !literal.is_empty() {
        regions.entry(region).or_default().push(F::Text(literal));
    }
    regions.into_iter().collect()
}

fn parse_xlsx_value(cell: &XmlNode, shared_strings: &[String]) -> CellValue {
    let cell_type = cell.attr("t").unwrap_or("");
    let value_text = cell
        .child("v")
        .map(|node| node.text_content())
        .unwrap_or_default();
    match cell_type {
        "s" => {
            let index = value_text.parse::<usize>().unwrap_or(0);
            CellValue::Text(shared_strings.get(index).cloned().unwrap_or_default())
        }
        "inlineStr" => CellValue::Text(cell.child("is").map(xlsx_text).unwrap_or_default()),
        "str" => CellValue::Text(value_text),
        "b" => CellValue::Bool(matches!(value_text.as_str(), "1" | "true" | "TRUE")),
        "e" => CellValue::Error(value_text),
        _ => {
            if value_text.is_empty() {
                CellValue::Empty
            } else if let Ok(number) = value_text.parse::<f64>() {
                CellValue::Number(number)
            } else {
                CellValue::Text(value_text)
            }
        }
    }
}

/// Parse a cell reference like "A1" -> (row, col) (1-based).
fn parse_cell_ref(reference: &str) -> Option<(u32, u32)> {
    let reference = reference.strip_prefix('$').unwrap_or(reference);
    let mut col = 0u32;
    let mut index = 0;
    for ch in reference.bytes().take_while(u8::is_ascii_alphabetic) {
        col = col
            .checked_mul(26)?
            .checked_add((ch.to_ascii_uppercase() - b'A' + 1) as u32)?;
        index += 1;
    }
    let digits = reference
        .get(index..)?
        .strip_prefix('$')
        .unwrap_or(&reference[index..]);
    let row = digits.parse::<u32>().ok()?;
    ((1..=1_048_576).contains(&row) && (1..=16_384).contains(&col)).then_some((row, col))
}

// ---------------------------------------------------------------------------
// ODS

fn ods_rows<'a>(node: &'a XmlNode, rows: &mut Vec<&'a XmlNode>) {
    for child in &node.children {
        match child.local_name() {
            "table-row" => rows.push(child),
            "table-header-rows" | "table-rows" | "table-row-group" => ods_rows(child, rows),
            _ => {}
        }
    }
}

// Keep dynamic page fields typed until pagination is complete.
fn ods_page_fields(node: Option<&XmlNode>, sheet: &str) -> Vec<(u8, Vec<lo_core::OdfPageField>)> {
    use lo_core::OdfPageField as F;
    fn collect(node: &XmlNode, sheet: &str, out: &mut Vec<F>) {
        for item in &node.items {
            match item {
                XmlItem::Text(text) => out.push(F::Text(text.clone())),
                XmlItem::Node(n) => match n.local_name() {
                    "page-number" => out.push(F::PageNumber),
                    "page-count" => out.push(F::PageCount),
                    "sheet-name" => out.push(F::Text(sheet.to_string())),
                    "line-break" => out.push(F::LineBreak),
                    "tab" => out.push(F::Tab),
                    _ => collect(n, sheet, out),
                },
            }
        }
    }
    let Some(node) = node.filter(|n| n.attr("display") != Some("false")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for child in &node.children {
        let alignment = match child.local_name() {
            "region-left" => 0,
            "region-right" => 2,
            _ => 1,
        };
        let mut fields = Vec::new();
        collect(child, sheet, &mut fields);
        if !fields.is_empty() {
            out.push((alignment, fields));
        }
    }
    out
}

fn apply_ods_layout(
    table: &XmlNode,
    styles: &lo_core::style::OdfStyles,
    page_styles: Option<&XmlNode>,
    sheet: &mut Sheet,
) {
    use lo_core::style::{odf_length, Alignment, CellStyle};
    let mut layout = lo_core::calc::SheetLayout::default();
    let name = table.attr("style-name").unwrap_or("");
    layout.hidden = styles.property(name, "table-properties", "display") == Some("false");
    if let Some(master_name) = styles.0.get(name).and_then(|n| n.attr("master-page-name")) {
        if let Some(root) = page_styles {
            let master = root.child("master-styles").and_then(|n| {
                n.children_named("master-page")
                    .find(|n| n.attr("name") == Some(master_name))
            });
            if let Some(master) = master {
                layout.headers = ods_page_fields(master.child("header"), &sheet.name);
                layout.footers = ods_page_fields(master.child("footer"), &sheet.name);
                if !layout.headers.is_empty() {
                    layout.header_height = 24.0;
                }
                if !layout.footers.is_empty() {
                    layout.footer_height = 24.0;
                }
            }
            let props = master
                .and_then(|n| n.attr("page-layout-name"))
                .and_then(|name| {
                    root.child("automatic-styles").and_then(|n| {
                        n.children_named("page-layout")
                            .find(|n| n.attr("name") == Some(name))
                    })
                })
                .and_then(|n| n.child("page-layout-properties"));
            if let Some(props) = props {
                if let Some(w) = props
                    .attr("page-width")
                    .and_then(odf_length)
                    .filter(|v| *v > 72.0)
                {
                    layout.page_width = w;
                }
                if let Some(h) = props
                    .attr("page-height")
                    .and_then(odf_length)
                    .filter(|v| *v > 72.0)
                {
                    layout.page_height = h;
                }
                if let Some(m) = props
                    .attr("margin")
                    .or_else(|| props.attr("margin-left"))
                    .and_then(odf_length)
                    .filter(|v| *v >= 0.0)
                {
                    layout.margin = m.min(layout.page_width / 4.0).min(layout.page_height / 4.0);
                }
            }
        }
    }
    let (max_row, mut max_col) = sheet.max_extent();
    let mut cells = Vec::new();
    table.descendants_named("table-cell", &mut cells);
    let extra = cells
        .iter()
        .filter_map(|c| {
            c.attr("number-columns-spanned")
                .and_then(|s| s.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    max_col = max_col.saturating_add(extra).min(16_383);
    let mut defaults = BTreeMap::new();
    let mut col = 0u32;
    for column in table.children_named("table-column") {
        let count = column
            .attr("number-columns-repeated")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        let width = column
            .attr("style-name")
            .and_then(|n| styles.property(n, "table-column-properties", "column-width"))
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .unwrap_or(64.0);
        for offset in 0..count
            .min(max_col.saturating_add(1).saturating_sub(col))
            .min(16_384)
        {
            layout.columns.insert(
                col + offset,
                if matches!(column.attr("visibility"), Some("collapse" | "filter")) {
                    0.0
                } else {
                    width
                },
            );
            if let Some(name) = column.attr("default-cell-style-name") {
                defaults.insert(col + offset, name);
            }
        }
        col = col.saturating_add(count);
    }
    let mut rows = Vec::new();
    ods_rows(table, &mut rows);
    let mut row_index = 0u32;
    for row in rows {
        let repeats = row
            .attr("number-rows-repeated")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        let height = row
            .attr("style-name")
            .and_then(|n| styles.property(n, "table-row-properties", "row-height"))
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .unwrap_or(15.0);
        for offset in 0..repeats
            .min(max_row.saturating_add(1).saturating_sub(row_index))
            .min(1_048_576)
        {
            let r = row_index + offset;
            layout.rows.insert(
                r,
                if matches!(row.attr("visibility"), Some("collapse" | "filter")) {
                    0.0
                } else {
                    height
                },
            );
            let mut c = 0u32;
            for cell in row
                .children
                .iter()
                .filter(|n| matches!(n.local_name(), "table-cell" | "covered-table-cell"))
            {
                let count = cell
                    .attr("number-columns-repeated")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1);
                for offset in 0..count
                    .min(max_col.saturating_add(1).saturating_sub(c))
                    .min(16_384)
                {
                    let addr = CellAddr::new(r, c + offset);
                    let Some(target) = sheet.cells.get_mut(&addr) else {
                        continue;
                    };
                    let name = cell
                        .attr("style-name")
                        .or_else(|| row.attr("default-cell-style-name"))
                        .or_else(|| defaults.get(&(c + offset)).copied())
                        .unwrap_or("Default");
                    let columns = cell
                        .attr("number-columns-spanned")
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(1)
                        .clamp(1, 16_384);
                    let rows = cell
                        .attr("number-rows-spanned")
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(1)
                        .clamp(1, 1_048_576);
                    if columns > 1 || rows > 1 {
                        layout.merges.insert(addr, (columns, rows));
                    }
                    let mut style = CellStyle::default();
                    style.border = styles
                        .property_in(name, "table-cell", "table-cell-properties", "border")
                        .unwrap_or("")
                        .to_string();
                    style.wrap = styles.property_in(
                        name,
                        "table-cell",
                        "table-cell-properties",
                        "wrap-option",
                    ) == Some("wrap");
                    style.background = styles
                        .property_in(
                            name,
                            "table-cell",
                            "table-cell-properties",
                            "background-color",
                        )
                        .unwrap_or("")
                        .to_string();
                    style.foreground = styles
                        .property_in(name, "table-cell", "text-properties", "color")
                        .unwrap_or("")
                        .to_string();
                    style.font_size_pt = styles
                        .property_in(name, "table-cell", "text-properties", "font-size")
                        .and_then(odf_length)
                        .filter(|v| *v > 0.0)
                        .map(|v| v.round().clamp(1.0, 512.0) as u16)
                        .unwrap_or(10);
                    style.bold =
                        styles.property_in(name, "table-cell", "text-properties", "font-weight")
                            == Some("bold");
                    style.italic =
                        styles.property_in(name, "table-cell", "text-properties", "font-style")
                            == Some("italic");
                    style.alignment = match styles.property_in(
                        name,
                        "table-cell",
                        "paragraph-properties",
                        "text-align",
                    ) {
                        Some("center") => Alignment::Center,
                        Some("end" | "right") => Alignment::End,
                        _ if matches!(target.value, CellValue::Number(_))
                            || matches!(
                                cell.attr("value-type"),
                                Some("float" | "percentage" | "currency")
                            ) =>
                        {
                            Alignment::End
                        }
                        _ => Alignment::Start,
                    };
                    target.style = Some(style);
                }
                c = c.saturating_add(count);
            }
        }
        row_index = row_index.saturating_add(repeats);
    }
    sheet.layout = Some(layout);
}

fn parse_ods_sheet(node: &XmlNode) -> Result<Sheet> {
    let mut expanded_cells = 0usize;
    let name = node.attr("name").unwrap_or("Sheet1").to_string();
    let mut sheet = Sheet::new(name);
    let mut row_index: u32 = 1;
    let mut rows = Vec::new();
    ods_rows(node, &mut rows);
    for row_node in rows {
        let repeat_rows = row_node
            .attr("number-rows-repeated")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        let row_values = parse_ods_row(row_node);
        if row_values.is_empty() {
            row_index = row_index.saturating_add(repeat_rows);
            continue;
        }
        let count = repeat_rows.min(1_048_576u32.saturating_sub(row_index - 1)) as usize;
        expanded_cells = expanded_cells.saturating_add(count.saturating_mul(row_values.len()));
        if expanded_cells > 100_000 {
            return Err(LoError::Parse(
                "ODS expanded cells exceed preview limit".into(),
            ));
        }
        for _ in 0..count {
            for (col, value, display) in &row_values {
                if *value != CellValue::Empty {
                    sheet.set(
                        CellAddr::new(row_index.saturating_sub(1), col.saturating_sub(1)),
                        value.clone(),
                    );
                    if let Some(text) = display {
                        sheet.display_values.insert(
                            CellAddr::new(row_index.saturating_sub(1), col.saturating_sub(1)),
                            text.clone(),
                        );
                    }
                }
            }
            row_index += 1;
        }
    }
    Ok(sheet)
}

fn parse_ods_row(row_node: &XmlNode) -> Vec<(u32, CellValue, Option<String>)> {
    let mut values = Vec::new();
    let mut col_index: u32 = 1;
    for cell in row_node
        .children
        .iter()
        .filter(|child| matches!(child.local_name(), "table-cell" | "covered-table-cell"))
    {
        let repeat_cols = cell
            .attr("number-columns-repeated")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        let value = if cell.local_name() == "covered-table-cell" {
            CellValue::Empty
        } else {
            parse_ods_value(cell)
        };
        // ODF stores the displayed result in text:p alongside typed values.
        // Preserve it for preview without discarding formulas or numeric types.
        let paragraphs: Vec<String> = cell
            .children
            .iter()
            .filter(|node| matches!(node.local_name(), "p" | "h"))
            .map(|node| lo_core::style::odf_text(node))
            .collect();
        let display = (!paragraphs.is_empty()).then(|| paragraphs.join("\n"));
        if value != CellValue::Empty {
            for offset in 0..repeat_cols.min(16_384u32.saturating_sub(col_index - 1)) {
                values.push((col_index + offset, value.clone(), display.clone()));
            }
        }
        col_index = col_index.saturating_add(repeat_cols);
    }
    values
}

fn parse_ods_value(cell: &XmlNode) -> CellValue {
    if let Some(formula) = cell.attr("formula") {
        return CellValue::Formula(
            formula
                .trim_start_matches("of:=")
                .trim_start_matches('=')
                .to_string(),
        );
    }
    let value_type = cell.attr("value-type").unwrap_or("string");
    let text = cell
        .children
        .iter()
        .filter(|child| matches!(child.local_name(), "p" | "h"))
        .map(|paragraph| lo_core::style::odf_text(paragraph))
        .collect::<Vec<_>>()
        .join("\n");
    match value_type {
        "float" | "percentage" | "currency" => cell
            .attr("value")
            .and_then(|value| value.parse::<f64>().ok())
            .map(CellValue::Number)
            .unwrap_or_else(|| CellValue::Text(text)),
        "boolean" => CellValue::Bool(matches!(cell.attr("boolean-value"), Some("true" | "1"))),
        "date" | "time" => CellValue::Text(
            cell.attr("date-value")
                .or_else(|| cell.attr("time-value"))
                .unwrap_or(&text)
                .to_string(),
        ),
        "string" => {
            if text.is_empty() {
                CellValue::Empty
            } else {
                CellValue::Text(text)
            }
        }
        _ => {
            if text.is_empty() {
                CellValue::Empty
            } else {
                CellValue::Text(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::to_xlsx;
    use lo_core::{CellAddr, CellValue, Workbook};

    #[test]
    fn hidden_grid_dimensions_and_sheet_fields_are_preserved() {
        let node = parse_xml_document(r#"<table><table-column visibility="collapse"/><table-column/><table-row visibility="filter"><table-cell value-type="string"><p>hidden</p></table-cell></table-row><table-row><table-cell/><table-cell value-type="string"><p>shown</p></table-cell></table-row></table>"#).unwrap();
        let mut sheet = parse_ods_sheet(&node).unwrap();
        apply_ods_layout(
            &node,
            &lo_core::style::OdfStyles::default(),
            None,
            &mut sheet,
        );
        let layout = sheet.layout.unwrap();
        assert_eq!(layout.rows[&0], 0.0);
        assert_eq!(layout.columns[&0], 0.0);
        assert!(layout.rows[&1] > 0.0);
        let header = parse_xml_document(r#"<header><region-right><p><sheet-name>stale</sheet-name> <page-number>99</page-number>/<page-count>99</page-count></p></region-right></header>"#).unwrap();
        let fields = ods_page_fields(Some(&header), "Actual sheet");
        assert_eq!(fields[0].0, 2);
        assert_eq!(
            fields[0].1[0],
            lo_core::OdfPageField::Text("Actual sheet".into())
        );
        assert!(fields[0].1.contains(&lo_core::OdfPageField::PageNumber));
        assert!(fields[0].1.contains(&lo_core::OdfPageField::PageCount));
    }

    #[test]
    fn grid_import_retains_merges_borders_and_header_groups() {
        let root=parse_xml_document(r##"<table><table-header-rows><table-row><table-cell style-name="Cell" number-columns-spanned="2" value-type="string"><p>Merged</p></table-cell><covered-table-cell/></table-row></table-header-rows></table>"##).unwrap();
        let style_root=parse_xml_document(r##"<styles><style name="Cell" family="table-cell"><table-cell-properties border="1pt solid #123456" wrap-option="wrap"/></style></styles>"##).unwrap();
        let mut styles = lo_core::style::OdfStyles::default();
        styles.collect(&style_root);
        let mut sheet = parse_ods_sheet(&root).unwrap();
        apply_ods_layout(&root, &styles, None, &mut sheet);
        assert_eq!(
            sheet.layout.as_ref().unwrap().merges[&CellAddr::new(0, 0)],
            (2, 1)
        );
        let style = sheet
            .get(CellAddr::new(0, 0))
            .unwrap()
            .style
            .as_ref()
            .unwrap();
        assert_eq!(style.border, "1pt solid #123456");
        assert!(style.wrap);
    }

    #[test]
    fn preview_preserves_odf_display_values() {
        let root = parse_xml_document(r#"<table><table-row number-rows-repeated="2"><table-cell value-type="percentage" value="0.125"><p>12.5%</p></table-cell><table-cell value-type="currency" value="12.5"><p>$12.50</p></table-cell></table-row></table>"#).unwrap();
        let sheet = parse_ods_sheet(&root).unwrap();
        assert_eq!(sheet.display_values[&CellAddr::new(1, 0)], "12.5%");
        assert_eq!(sheet.display_values[&CellAddr::new(1, 1)], "$12.50");
        assert_eq!(
            sheet.get(CellAddr::new(1, 0)).unwrap().value,
            CellValue::Number(0.125)
        );
    }

    #[test]
    fn parse_cell_ref_handles_basic() {
        assert_eq!(parse_cell_ref("A1"), Some((1, 1)));
        assert_eq!(parse_cell_ref("B2"), Some((2, 2)));
        assert_eq!(parse_cell_ref("AA10"), Some((10, 27)));
    }

    #[test]
    fn xlsx_round_trip_imports_cells() {
        let mut wb = Workbook::new("book");
        let sheet = wb.sheet_mut(0).unwrap();
        sheet.set(CellAddr::new(0, 0), CellValue::Text("a".to_string()));
        sheet.set(CellAddr::new(0, 1), CellValue::Number(42.0));
        sheet.set(CellAddr::new(1, 0), CellValue::Formula("A1".to_string()));
        let bytes = to_xlsx(&wb).expect("xlsx");
        let loaded = from_xlsx_bytes("book", &bytes).expect("import xlsx");
        let sheet = loaded.sheet(0).expect("sheet");
        assert_eq!(
            sheet.get(CellAddr::new(0, 0)).map(|c| c.value.clone()),
            Some(CellValue::Text("a".to_string()))
        );
        assert_eq!(
            sheet.get(CellAddr::new(0, 1)).map(|c| c.value.clone()),
            Some(CellValue::Number(42.0))
        );
    }

    #[test]
    fn ods_round_trip_imports_cells() {
        let mut wb = Workbook::new("book");
        let sheet = wb.sheet_mut(0).unwrap();
        sheet.set(CellAddr::new(0, 0), CellValue::Text("a".to_string()));
        let tmp = std::env::temp_dir().join("lo_calc_import_test.ods");
        lo_odf::save_spreadsheet_document(&tmp, &wb).expect("save ods");
        let bytes = std::fs::read(&tmp).expect("read");
        let _ = std::fs::remove_file(&tmp);
        let loaded = from_ods_bytes("book", &bytes).expect("import ods");
        let sheet = loaded.sheet(0).expect("sheet");
        assert_eq!(
            sheet.get(CellAddr::new(0, 0)).map(|c| c.value.clone()),
            Some(CellValue::Text("a".to_string()))
        );
    }
}

#[cfg(test)]
mod expansion_tests {
    use super::*;
    #[test]
    fn rejects_multiplicative_row_column_expansion() {
        let xml = r#"<table><table-row number-rows-repeated="1000000"><table-cell number-columns-repeated="16384" value-type="float" value="1"/></table-row></table>"#;
        let node = parse_xml_document(xml).unwrap();
        assert!(parse_ods_sheet(&node)
            .unwrap_err()
            .to_string()
            .contains("expanded cells"));
    }
}
