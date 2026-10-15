use crate::units::Length;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alignment {
    Start,
    Center,
    End,
    Justify,
}

impl Default for Alignment {
    fn default() -> Self {
        Self::Start
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextStyle {
    pub font_family: String,
    /// Exact imported size, in thousandths of a point, when available.
    pub font_size_mpt: Option<u32>,
    pub font_size_pt: u16,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: String,
    pub background: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OdfParagraphBox {
    pub background: String,
    /// Left, right, top, bottom.
    pub borders: [String; 4],
    pub padding_mpt: [u32; 4],
    pub join: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParagraphStyle {
    /// ODF paragraph decoration; lengths are thousandths of a point.
    pub odf_box: Option<OdfParagraphBox>,
    /// Top, bottom, left, right margins in thousandths of a point.
    pub odf_margins_mpt: Option<[i32; 4]>,
    pub odf_first_line_mpt: i32,
    pub source_spacing: bool,
    pub line_height_percent: u16,
    pub alignment: Alignment,
    pub margin_top_mm: u16,
    pub margin_bottom_mm: u16,
    pub margin_left_mm: u16,
    pub margin_right_mm: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageStyle {
    pub width_mm: u16,
    pub height_mm: u16,
    pub margin_mm: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub underline: bool,
    pub rotation_degrees: i16,
    pub indent: u16,
    pub shrink_to_fit: bool,
    /// Per-edge borders (left, right, top, bottom), overriding the common border.
    pub borders: [String; 4],
    pub font_family: String,
    pub font_size_mpt: Option<u32>,
    /// None retains the legacy top-aligned ODF preview behavior.
    pub vertical_alignment: Option<Alignment>,
    pub border: String,
    pub wrap: bool,
    pub font_size_pt: u16,
    pub bold: bool,
    pub italic: bool,
    pub format_code: String,
    pub background: String,
    pub foreground: String,
    pub alignment: Alignment,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeStyle {
    /// Optional affine transform in top-left slide coordinates.
    pub transform: Option<[f32;6]>,
    /// Optional exact line width, in thousandths of a point.
    pub stroke_width_mpt: Option<u32>,
    /// Dash lengths in thousandths of a point; empty means a solid stroke.
    pub stroke_dash_mpt: Vec<u32>,
    pub rotation_millidegrees: i32,
    pub fill: String,
    pub stroke: String,
    pub stroke_width_mm: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextBoxStyle {
    pub transform: Option<[f32;6]>,
    pub wrap_text: bool,
    /// Rotation center in slide coordinates (points); None retains the legacy frame origin.
    pub rotation_origin: Option<(f32,f32)>,
    pub runs: Vec<crate::calc::CellTextRun>,
    pub line_spacing: f32,
    pub font_family: String,
    pub vertical_alignment: Alignment,
    pub paragraph_indents: Vec<f32>,
    pub paragraph_markers: Vec<Option<String>>,
    pub font_size_pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub border: bool,
    pub rotation: f32,
    pub alignment: Alignment,
    pub padding: Length,
    pub background: String,
    pub foreground: String,
}

impl Default for TextBoxStyle {
    fn default() -> Self {
        Self {
            transform: None,
            rotation_origin: None,
            wrap_text: true,
            runs: Vec::new(),
            line_spacing: 0.0,
            font_family: String::new(),
            vertical_alignment: Alignment::Start,
            paragraph_indents: Vec::new(),
            paragraph_markers: Vec::new(),
            font_size_pt: 0.0,
            bold: false,
            italic: false,
            border: true,
            rotation: 0.0,
            alignment: Alignment::Start,
            padding: Length::mm(1.5),
            background: String::new(),
            foreground: String::new(),
        }
    }
}

/// ODF automatic/named styles, with bounded parent inheritance.
#[derive(Default)]
pub struct OdfStyles(
    pub std::collections::BTreeMap<String, crate::XmlNode>,
    std::collections::BTreeMap<(String, String), crate::XmlNode>,
);
impl OdfStyles {
    pub fn names(&self) -> impl Iterator<Item = (&str, &str)> {
        self.1
            .keys()
            .map(|(family, name)| (family.as_str(), name.as_str()))
    }
    pub fn collect(&mut self, root: &crate::XmlNode) {
        if root.local_name() == "list-style" {
            if let Some(name) = root.attr("name") {
                self.0.insert(name.to_string(), root.clone());
            }
        }
        if matches!(root.local_name(), "style" | "default-style") {
            if let Some(family) = root.attr("family") {
                let name = root.attr("name").unwrap_or("");
                self.1
                    .insert((family.to_string(), name.to_string()), root.clone());
                if !name.is_empty() {
                    self.0.insert(name.to_string(), root.clone());
                }
            }
        }
        for child in &root.children {
            self.collect(child);
        }
    }
    pub fn property<'a>(&'a self, name: &str, kind: &str, attribute: &str) -> Option<&'a str> {
        let family = self.0.get(name)?.attr("family")?;
        self.property_in(name, family, kind, attribute)
    }
    /// Named-style inheritance only. A nested span's family default must not
    /// replace properties inherited from its enclosing paragraph or frame.
    pub fn explicit_property<'a>(
        &'a self,
        name: &str,
        kind: &str,
        attribute: &str,
    ) -> Option<&'a str> {
        let family = self.0.get(name)?.attr("family")?;
        let mut style = self.1.get(&(family.to_string(), name.to_string()));
        for _ in 0..32 {
            let node = style?;
            if let Some(value) = node.child(kind).and_then(|n| n.attr(attribute)) {
                return Some(value);
            }
            style = node
                .attr("parent-style-name")
                .and_then(|name| self.1.get(&(family.to_string(), name.to_string())));
        }
        None
    }
    pub fn property_in<'a>(
        &'a self,
        name: &str,
        family: &str,
        kind: &str,
        attribute: &str,
    ) -> Option<&'a str> {
        let default = self.1.get(&(family.to_string(), String::new()));
        let mut style = self
            .1
            .get(&(family.to_string(), name.to_string()))
            .or(default);
        for _ in 0..32 {
            let node = style?;
            if let Some(value) = node.child(kind).and_then(|n| n.attr(attribute)) {
                return Some(value);
            }
            style = if node.local_name() == "default-style" {
                None
            } else {
                node.attr("parent-style-name")
                    .and_then(|name| self.1.get(&(family.to_string(), name.to_string())))
                    .or(default)
            };
        }
        None
    }
}

/// Parse physical ODF lengths into points; reject non-finite/extreme values.
pub fn odf_length(value: &str) -> Option<f32> {
    let value = value.trim();
    for (unit, scale) in [
        ("cm", 72.0 / 2.54),
        ("mm", 72.0 / 25.4),
        ("in", 72.0),
        ("pt", 1.0),
        ("pc", 12.0),
        ("px", 0.75),
    ] {
        if let Some(n) = value.strip_suffix(unit) {
            let n = n.parse::<f32>().ok()? * scale;
            return (n.is_finite() && n.abs() <= 30_000.0).then_some(n);
        }
    }
    None
}

#[cfg(test)]
mod odf_style_tests {
    use super::*;
    #[test]
    fn families_defaults_and_parent_cycles_are_resolved_safely() {
        let root=crate::parse_xml_document(r##"<styles><default-style family="table-cell"><text-properties font-size="10pt"/></default-style><style name="Default" family="table-cell"><text-properties color="#123456"/></style><style name="Default" family="graphic"><text-properties color="#ffffff"/></style><style name="Child" family="table-cell" parent-style-name="Default"/><style name="Loop" family="table-cell" parent-style-name="Loop"/></styles>"##).unwrap();
        let mut styles = OdfStyles::default();
        styles.collect(&root);
        assert_eq!(
            styles.property("Child", "text-properties", "color"),
            Some("#123456")
        );
        assert_eq!(
            styles.property("Child", "text-properties", "font-size"),
            Some("10pt")
        );
        assert_eq!(
            styles.property_in("Default", "table-cell", "text-properties", "color"),
            Some("#123456")
        );
        assert_eq!(
            styles.property("Loop", "text-properties", "font-size"),
            None
        );
    }
}

/// ODF inline text in document order, including explicit spaces and breaks.
pub fn odf_text(node: &crate::XmlNode) -> String {
    let mut out = String::new();
    if node.items.is_empty() {
        return node.text.clone();
    }
    for item in &node.items {
        match item {
            crate::XmlItem::Text(text) => out.push_str(text),
            crate::XmlItem::Node(child) => match child.local_name() {
                "s" => out.push_str(
                    &" ".repeat(
                        child
                            .attr("c")
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(1)
                            .min(10_000),
                    ),
                ),
                "tab" => out.push('\t'),
                "line-break" => out.push('\n'),
                "frame" => {}
                _ => out.push_str(&odf_text(child)),
            },
        }
    }
    out
}
#[cfg(test)]
mod odf_inline_tests {
    #[test]
    fn mixed_runs_and_explicit_whitespace_stay_in_order() {
        let node = crate::parse_xml_document(
            r#"<p>Before <span>middle</span> after<s c="2"/>end<line-break/>next</p>"#,
        )
        .unwrap();
        assert_eq!(super::odf_text(&node), "Before middle after  end\nnext");
    }
}
