//! Spreadsheet drawing positions are measured from the sheet's top left, in points.
use lo_core::{calc::SheetLayout, ImageElement, Length, Rect, Result, XmlNode};
use lo_zip::ZipArchive;

fn number(node: &XmlNode, key: &str) -> Option<f32> {
    node.attr(key)?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}
fn marker(node: &XmlNode, layout: &SheetLayout) -> Option<(f32, f32)> {
    let index = |key: &str, limit: u32| -> Option<u32> {
        node.child(key)?
            .text_content()
            .parse::<u32>()
            .ok()
            .filter(|v| *v <= limit)
    };
    let col = index("col", 16383)?;
    let row = index("row", 1048575)?;
    let offset = |key: &str| {
        node.child(key)
            .and_then(|n| n.text_content().parse::<f32>().ok())
            .unwrap_or(0.0)
            / 12700.0
    };
    // Sparse overrides avoid work proportional to a possibly enormous row index.
    let x = col as f32 * layout.default_column_width
        + layout
            .columns
            .range(..col)
            .map(|(_, v)| v - layout.default_column_width)
            .sum::<f32>()
        + offset("colOff");
    let y = row as f32 * layout.default_row_height
        + layout
            .rows
            .range(..row)
            .map(|(_, v)| v - layout.default_row_height)
            .sum::<f32>()
        + offset("rowOff");
    Some((x, y))
}
fn frame(anchor: &XmlNode, layout: &SheetLayout) -> Option<Rect> {
    let (x, y) = if let Some(n) = anchor.child("from") {
        marker(n, layout)?
    } else {
        let n = anchor.child("pos")?;
        (number(n, "x")? / 12700.0, number(n, "y")? / 12700.0)
    };
    let (w, h) = if let Some(n) = anchor.child("to") {
        let (right, bottom) = marker(n, layout)?;
        (right - x, bottom - y)
    } else {
        let n = anchor.child("ext")?;
        (number(n, "cx")? / 12700.0, number(n, "cy")? / 12700.0)
    };
    if ![x, y, w, h]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 100_000.0)
        || w == 0.0
        || h == 0.0
    {
        return None;
    }
    Some(Rect::new(
        Length::pt(x),
        Length::pt(y),
        Length::pt(w),
        Length::pt(h),
    ))
}
pub(crate) fn import(
    zip: &ZipArchive,
    part: &str,
    root: &XmlNode,
    layout: &mut SheetLayout,
) -> Result<()> {
    let rels = crate::import::parse_relationships(zip, part)?;
    for drawing in root.children_named("drawing").take(64) {
        let Some(path) = drawing.attr("id").and_then(|id| rels.get(id)) else {
            continue;
        };
        let root = lo_core::parse_xml_document(&zip.read_string(path)?)?;
        let rels = crate::import::parse_relationships(zip, path)?;
        for anchor in root.children.iter().take(1024) {
            let Some(frame) = frame(anchor, layout) else {
                continue;
            };
            let Some(pic) = anchor.child("pic") else {
                continue;
            };
            let Some(path) = pic
                .child("blipFill")
                .and_then(|n| n.child("blip"))
                .and_then(|n| n.attr("embed"))
                .and_then(|id| rels.get(id))
            else {
                continue;
            };
            let mime = match path
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                _ => continue,
            };
            let data = zip.read(path)?;
            if data.len() > 32 * 1024 * 1024 {
                continue;
            }
            layout.images.push(ImageElement {
                frame,
                data,
                mime_type: mime.into(),
                name: path.clone(),
                alt: String::new(),
                opacity: 255,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anchors_use_hidden_dimensions_and_emu_offsets() {
        let mut l = SheetLayout::default();
        l.columns.insert(0, 0.0);
        l.columns.insert(1, 30.0);
        l.rows.insert(0, 20.0);
        let xml=lo_core::parse_xml_document(r#"<twoCellAnchor><from><col>2</col><colOff>12700</colOff><row>1</row><rowOff>25400</rowOff></from><to><col>3</col><row>3</row></to></twoCellAnchor>"#).unwrap();
        let f = frame(&xml, &l).unwrap();
        assert!((f.origin.x.as_pt() - 31.0).abs() < 0.001);
        assert!((f.origin.y.as_pt() - 22.0).abs() < 0.001);
        assert!((f.size.width.as_pt() - 63.0).abs() < 0.001);
        assert!((f.size.height.as_pt() - 28.0).abs() < 0.001);
        let bad = lo_core::parse_xml_document(
            r#"<absoluteAnchor><pos x="NaN" y="0"/><ext cx="12700" cy="12700"/></absoluteAnchor>"#,
        )
        .unwrap();
        assert!(frame(&bad, &l).is_none());
    }
    #[test]
    fn drawing_only_sheet_imports_and_paginates_without_external_fetches() {
        use lo_zip::{write_zip, ZipEntry};
        let entries=vec![
            ZipEntry::new("xl/workbook.xml",br#"<workbook><sheets><sheet name="Image" id="s"/></sheets></workbook>"#.to_vec()),
            ZipEntry::new("xl/_rels/workbook.xml.rels",br#"<Relationships><Relationship Id="s" Target="worksheets/sheet1.xml"/></Relationships>"#.to_vec()),
            ZipEntry::new("xl/worksheets/sheet1.xml",br#"<worksheet><drawing id="d"/></worksheet>"#.to_vec()),
            ZipEntry::new("xl/worksheets/_rels/sheet1.xml.rels",br#"<Relationships><Relationship Id="d" Target="../drawings/d.xml"/></Relationships>"#.to_vec()),
            ZipEntry::new("xl/drawings/d.xml",br#"<wsDr><absoluteAnchor><pos x="0" y="0"/><ext cx="12700000" cy="12700000"/><pic><blipFill><blip embed="i"/></blipFill></pic></absoluteAnchor><absoluteAnchor><pos x="0" y="0"/><ext cx="12700" cy="12700"/><pic><blipFill><blip embed="external"/></blipFill></pic></absoluteAnchor></wsDr>"#.to_vec()),
            ZipEntry::new("xl/drawings/_rels/d.xml.rels",br#"<Relationships><Relationship Id="i" Target="../media/a.png"/><Relationship Id="external" Target="https://invalid/a.png" TargetMode="External"/></Relationships>"#.to_vec()),
            ZipEntry::new("xl/media/a.png",lo_core::RasterImage::new(2,2,lo_core::Rgba::BLACK).encode_png()),
        ];
        let mut out = std::io::Cursor::new(Vec::new());
        write_zip(&mut out, &entries).unwrap();
        let book = crate::from_xlsx_bytes("", &out.into_inner()).unwrap();
        assert_eq!(book.sheets[0].layout.as_ref().unwrap().images.len(), 1);
        let pdf = String::from_utf8_lossy(&crate::to_pdf(&book)).into_owned();
        assert!(pdf.contains("/Subtype /Image"));
        assert!(
            pdf.matches("/Type /Page ").count() >= 4,
            "drawing crosses horizontal and vertical pages"
        );
    }
}
