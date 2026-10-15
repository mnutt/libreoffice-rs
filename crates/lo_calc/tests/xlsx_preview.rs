use lo_core::{style::Alignment, CellAddr, CellValue};
use lo_zip::{write_zip, ZipEntry};
use std::io::Cursor;

fn workbook(sheet: &str, styles: &str, props: &str) -> Vec<u8> {
    let book = format!(
        r#"<workbook><workbookPr {props}/><sheets><sheet name="Visible" id="rId1"/><sheet name="Hidden" id="rId2" state="veryHidden"/></sheets></workbook>"#
    );
    let entries=vec![
        ZipEntry::new("xl/workbook.xml",book.into_bytes()),
        ZipEntry::new("xl/_rels/workbook.xml.rels",br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="worksheets/sheet2.xml"/></Relationships>"#.to_vec()),
        ZipEntry::new("xl/worksheets/sheet1.xml",sheet.as_bytes().to_vec()),
        ZipEntry::new("xl/worksheets/sheet2.xml",br#"<worksheet><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Hidden content</t></is></c></row></sheetData></worksheet>"#.to_vec()),
        ZipEntry::new("xl/styles.xml",styles.as_bytes().to_vec()),
    ];
    let mut out = Cursor::new(Vec::new());
    write_zip(&mut out, &entries).unwrap();
    out.into_inner()
}

const STYLES: &str = r##"<styleSheet><numFmts><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/><numFmt numFmtId="165" formatCode="&quot;$&quot;#,##0.00"/></numFmts><fonts><font><name val="Arial"/><sz val="10.5"/><b/><i/><color rgb="FF123456"/></font></fonts><fills><fill><patternFill patternType="solid"><fgColor rgb="FFEEDDCC"/></patternFill></fill></fills><borders><border><left style="thin"><color rgb="FF112233"/></left><bottom style="double"><color rgb="FF445566"/></bottom></border></borders><cellXfs><xf numFmtId="0"/><xf numFmtId="164"/><xf numFmtId="165"><alignment horizontal="right" vertical="center" wrapText="1"/></xf><xf numFmtId="10"/></cellXfs></styleSheet>"##;

#[test]
fn imports_cached_values_styles_dimensions_and_merges() {
    let bytes = workbook(
        r#"<worksheet><sheetFormatPr defaultRowHeight="18" defaultColWidth="10"/><cols><col min="1" max="1" width="20"/><col min="3" max="3" hidden="1"/></cols><sheetData><row r="1" ht="40"><c r="A1" t="inlineStr"><is><r><t>Rich </t></r><r><t>text</t></r><rPh><t>phonetic</t></rPh></is></c><c r="B1" s="1"><v>0</v></c></row><row r="2"><c r="A2" s="2"><f>UNSUPPORTED(1)</f><v>12.5</v></c><c r="B2" s="3"><v>0.125</v></c></row><row r="3" hidden="1"><c r="A3"><v>99</v></c></row><row r="4"><c r="A4" t="inlineStr"><is><t>Merged</t></is></c></row><row r="5"><c r="A5" t="b"><v>1</v></c><c r="B5" t="e"><v>#DIV/0!</v></c></row></sheetData><mergeCells><mergeCell ref="A4:B4"/></mergeCells><pageSetup paperSize="9" orientation="landscape"/><pageMargins left="0.25" right="0.5" top="0.75" bottom="1"/></worksheet>"#,
        STYLES,
        "date1904=\"1\"",
    );
    let book = lo_calc::from_xlsx_bytes("", &bytes).unwrap();
    let sheet = &book.sheets[0];
    let layout = sheet.layout.as_ref().unwrap();
    assert_eq!(sheet.display_values[&CellAddr::new(0, 0)], "Rich text");
    assert_eq!(sheet.display_values[&CellAddr::new(0, 1)], "1904-01-01");
    assert_eq!(sheet.display_values[&CellAddr::new(1, 0)], "$12.50");
    assert_eq!(sheet.display_values[&CellAddr::new(1, 1)], "12.50%");
    assert_eq!(sheet.display_values[&CellAddr::new(4, 0)], "TRUE");
    assert_eq!(sheet.display_values[&CellAddr::new(4, 1)], "#DIV/0!");
    assert!(matches!(
        sheet.get(CellAddr::new(1, 0)).unwrap().value,
        CellValue::Formula(_)
    ));
    let style = sheet
        .get(CellAddr::new(1, 0))
        .unwrap()
        .style
        .as_ref()
        .unwrap();
    assert_eq!(style.font_size_mpt, Some(10500));
    assert!(style.bold && style.italic && style.wrap);
    assert_eq!(style.foreground, "#123456");
    assert_eq!(style.background, "#EEDDCC");
    assert_eq!(style.borders[0], "0.75pt thin #112233");
    assert_eq!(style.vertical_alignment, Some(Alignment::Center));
    assert_eq!(layout.columns[&0], 120.0);
    assert_eq!(layout.columns[&2], 0.0);
    assert_eq!(layout.rows[&0], 40.0);
    assert_eq!(layout.rows[&2], 0.0);
    assert_eq!(layout.merges[&CellAddr::new(3, 0)], (2, 1));
    assert_eq!(layout.margins, Some([18.0, 36.0, 54.0, 72.0]));
    assert!(layout.page_width > layout.page_height);
    assert!(book.sheets[1].layout.as_ref().unwrap().hidden);
    assert!(lo_calc::pdf::to_pdf_preview(&book).starts_with(b"%PDF-"));
}

#[test]
fn missing_and_shared_formula_caches_never_trigger_preview_recalculation() {
    let bytes = workbook(
        r#"<worksheet><sheetData><row r="1"><c r="A1"><f>1+1</f></c><c r="B1"><f t="shared" si="0"/><v>7</v></c><c r="C1" t="str"><f>TEXT(1)</f><v>cached string</v></c></row></sheetData></worksheet>"#,
        "<styleSheet/>",
        "",
    );
    let book = lo_calc::from_xlsx_bytes("", &bytes).unwrap();
    let values = &book.sheets[0].display_values;
    assert_eq!(values[&CellAddr::new(0, 0)], "");
    assert_eq!(values[&CellAddr::new(0, 1)], "7");
    assert_eq!(values[&CellAddr::new(0, 2)], "cached string");
}

#[test]
fn rejects_invalid_addresses_and_excessive_merge_expansion() {
    for cell in ["A0", "XFE1", "A1048577", "ZZZZZZZZZZZZZZZZ1", "A1B2"] {
        let xml = format!(
            r#"<worksheet><sheetData><row r="1"><c r="{cell}"><v>1</v></c></row></sheetData></worksheet>"#
        );
        assert!(
            lo_calc::from_xlsx_bytes("", &workbook(&xml, "<styleSheet/>", "")).is_err(),
            "{cell}"
        );
    }
    let xml = r#"<worksheet><mergeCells><mergeCell ref="A1:XFD1048576"/></mergeCells></worksheet>"#;
    assert!(lo_calc::from_xlsx_bytes("", &workbook(xml, "<styleSheet/>", "")).is_err());
}

#[test]
fn theme_colors_and_inherited_styles_are_resolved() {
    let styles = r#"<styleSheet><fonts><font><sz val="11"/><color theme="4" tint="0.5"/></font></fonts><cellStyleXfs><xf numFmtId="10"/></cellStyleXfs><cellXfs><xf xfId="0" applyNumberFormat="0" numFmtId="0"/></cellXfs></styleSheet>"#;
    let xml =
        r#"<worksheet><sheetData><row><c r="A1"><v>0.125</v></c></row></sheetData></worksheet>"#;
    let book = lo_calc::from_xlsx_bytes("", &workbook(xml, styles, "")).unwrap();
    assert_eq!(
        book.sheets[0].display_values[&CellAddr::new(0, 0)],
        "12.50%"
    );
    assert_eq!(
        book.sheets[0]
            .get(CellAddr::new(0, 0))
            .unwrap()
            .style
            .as_ref()
            .unwrap()
            .foreground,
        "#A7C0DE"
    );
}

#[test]
fn malformed_non_ascii_argb_is_ignored_without_panicking() {
    let styles = r#"<styleSheet><fonts><font><color rgb="€12345"/></font></fonts><cellXfs><xf/></cellXfs></styleSheet>"#;
    let xml = r#"<worksheet><sheetData><row><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#;
    let book = lo_calc::from_xlsx_bytes("", &workbook(xml, styles, "")).unwrap();
    assert!(book.sheets[0]
        .get(CellAddr::new(0, 0))
        .unwrap()
        .style
        .as_ref()
        .unwrap()
        .foreground
        .is_empty());
}

#[test]
fn style_only_column_preserves_the_sheet_default_width() {
    let xml = r#"<worksheet><sheetFormatPr defaultColWidth="20"/><cols><col min="1" max="1" style="0"/></cols><sheetData><row><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#;
    let book = lo_calc::from_xlsx_bytes("", &workbook(xml, "<styleSheet/>", "")).unwrap();
    let layout = book.sheets[0].layout.as_ref().unwrap();
    assert_eq!(layout.columns[&0], layout.default_column_width);
}
