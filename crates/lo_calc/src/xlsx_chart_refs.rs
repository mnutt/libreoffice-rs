//! Fill absent chart caches from saved local worksheet cells, without evaluating formulas.
use lo_core::{CellAddr, CellValue, Result, XmlNode};
use lo_zip::ZipArchive;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) struct ChartReferences {
    targets: BTreeMap<String, String>,
    sheets: BTreeMap<String, BTreeMap<CellAddr, CellValue>>,
    budget: usize,
}
impl ChartReferences {
    pub(crate) fn new(workbook: &XmlNode, rels: &BTreeMap<String, String>) -> Self {
        let mut targets = BTreeMap::new();
        if let Some(sheets) = workbook.child("sheets") {
            for sheet in sheets.children_named("sheet") {
                if let (Some(name), Some(path)) = (
                    sheet.attr("name"),
                    sheet.attr("id").and_then(|id| rels.get(id)),
                ) {
                    targets.insert(name.to_lowercase(), path.clone());
                }
            }
        }
        Self {
            targets,
            sheets: BTreeMap::new(),
            budget: 100_000,
        }
    }
    pub(crate) fn fill(
        &mut self,
        node: &mut XmlNode,
        zip: &ZipArchive,
        shared: &[String],
    ) -> Result<()> {
        let cache = match node.local_name() {
            "numRef" => Some("numCache"),
            "strRef" => Some("strCache"),
            _ => None,
        };
        if let Some(cache) = cache {
            // Existing caches are authoritative, even when empty or sparse.
            if node.child(cache).is_none() {
                if let Some(reference) = node.child("f").map(|f| f.text_content()) {
                    if let Some((name, first, last)) = range(&reference) {
                        if !self.sheets.contains_key(&name) {
                            if let Some(path) = self.targets.get(&name) {
                                let root = lo_core::parse_xml_document(&zip.read_string(path)?)?;
                                let mut cells = BTreeMap::new();
                                if let Some(data) = root.child("sheetData") {
                                    for row in data.children_named("row") {
                                        for cell in row.children_named("c") {
                                            if self.budget == 0 {
                                                break;
                                            }
                                            self.budget -= 1;
                                            if let Some(a) = cell.attr("r").and_then(address) {
                                                cells.insert(
                                                    a,
                                                    crate::import::parse_xlsx_value(cell, shared),
                                                );
                                            }
                                        }
                                        if self.budget == 0 {
                                            break;
                                        }
                                    }
                                }
                                self.sheets.insert(name.clone(), cells);
                            }
                        }
                        if let Some(cells) = self.sheets.get(&name) {
                            let count = (last.row - first.row + 1).max(last.col - first.col + 1);
                            if count as usize <= self.budget {
                                self.budget -= count as usize;
                                let mut result = XmlNode {
                                    name: cache.into(),
                                    ..Default::default()
                                };
                                for i in 0..count {
                                    let a = CellAddr::new(
                                        first.row + if first.row == last.row { 0 } else { i },
                                        first.col + if first.col == last.col { 0 } else { i },
                                    );
                                    let value = match cells.get(&a) {
                                        Some(CellValue::Number(n)) if n.is_finite() => {
                                            Some(n.to_string())
                                        }
                                        Some(CellValue::Text(s)) if cache == "strCache" => {
                                            Some(s.clone())
                                        }
                                        Some(CellValue::Bool(b)) if cache == "strCache" => {
                                            Some(if *b { "TRUE" } else { "FALSE" }.into())
                                        }
                                        _ => None,
                                    };
                                    if let Some(value) = value {
                                        let v = XmlNode {
                                            name: "v".into(),
                                            text: value,
                                            ..Default::default()
                                        };
                                        let mut pt = XmlNode {
                                            name: "pt".into(),
                                            children: vec![Arc::new(v)],
                                            ..Default::default()
                                        };
                                        pt.attributes.insert("idx".into(), i.to_string());
                                        result.children.push(Arc::new(pt));
                                    }
                                }
                                node.children.push(Arc::new(result));
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        for child in &mut node.children {
            self.fill(Arc::make_mut(child), zip, shared)?;
        }
        Ok(())
    }
}
fn address(s: &str) -> Option<CellAddr> {
    if s.len() > 16 {
        return None;
    }
    let (row, col) = crate::import::parse_cell_ref(&s.replace('$', ""))?;
    Some(CellAddr::new(row - 1, col - 1))
}
fn range(s: &str) -> Option<(String, CellAddr, CellAddr)> {
    let (name, cells) = s.trim().rsplit_once('!')?;
    let name = if name.starts_with('\'') && name.ends_with('\'') && name.len() >= 2 {
        name[1..name.len() - 1].replace("''", "'")
    } else {
        name.to_string()
    };
    // External books, 3-D references, unions and expressions are not local ranges.
    if name.contains(['[', ']', ':']) {
        return None;
    }
    let (first, last) = cells.split_once(':').unwrap_or((cells, cells));
    let first = address(first)?;
    let last = address(last)?;
    if last.row < first.row
        || last.col < first.col
        || (first.row != last.row && first.col != last.col)
        || (last.row - first.row).max(last.col - first.col) >= 10_000
    {
        return None;
    }
    Some((name.to_lowercase(), first, last))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fills_missing_caches_from_saved_values_and_preserves_gaps_and_existing_caches() {
        let entries = vec![lo_zip::ZipEntry::new("xl/worksheets/data.xml", br#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1"><f>UNSUPPORTED()</f><v>42</v></c><c r="B2"><f>1+1</f></c><c r="B3"><v>7</v></c></row></sheetData></worksheet>"#.to_vec())];
        let bytes = lo_zip::write_zip_to_vec(&entries).unwrap();
        let zip = ZipArchive::new(&bytes).unwrap();
        let book = lo_core::parse_xml_document(
            r#"<workbook><sheets><sheet name="Data" id="r1" state="hidden"/></sheets></workbook>"#,
        )
        .unwrap();
        let mut refs = ChartReferences::new(
            &book,
            &BTreeMap::from([("r1".into(), "xl/worksheets/data.xml".into())]),
        );
        let mut root = lo_core::parse_xml_document(r#"<root><strRef><f>Data!A1</f></strRef><numRef><f>Data!B1:B3</f></numRef><numRef><f>Data!B1</f><numCache><pt idx="0"><v>99</v></pt></numCache></numRef><numRef><f>[other.xlsx]Data!B1</f></numRef></root>"#).unwrap();
        refs.fill(&mut root, &zip, &["Series label".into()])
            .unwrap();
        assert_eq!(
            root.children[0].child("strCache").unwrap().children[0]
                .child("v")
                .unwrap()
                .text_content(),
            "Series label"
        );
        let points = &root.children[1].child("numCache").unwrap().children;
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].child("v").unwrap().text_content(), "42");
        assert_eq!(points[1].attr("idx"), Some("2"));
        assert_eq!(
            root.children[2].child("numCache").unwrap().children[0]
                .child("v")
                .unwrap()
                .text_content(),
            "99"
        );
        assert!(root.children[3].child("numCache").is_none());
    }
    #[test]
    fn local_ranges_are_bounded_and_quoted_names_are_unescaped() {
        assert_eq!(
            range("'Bob''s data'!$B$2:$B$4").unwrap(),
            (
                "bob's data".into(),
                CellAddr::new(1, 1),
                CellAddr::new(3, 1)
            )
        );
        for s in [
            "[book.xlsx]Sheet1!A1",
            "Sheet1:Sheet2!A1",
            "Sheet1!A1:B2",
            "Sheet1!A1:A10001",
            "Sheet1!A0",
            "Sheet1!XFE1",
            "Sheet1!AAAAAAAAAAAAAAA1",
            "Sheet1!SUM(A1:A2)",
        ] {
            assert!(range(s).is_none(), "{s}");
        }
    }
}
