//! Binary importers for `Presentation`.
//!
//! Supports `pptx`, `odp`, and a `txt` outline form.


use lo_core::{
    parse_xml_document, Length, LoError, Presentation, Rect, Result, Shape, ShapeKind, ShapeStyle,
    Slide, SlideElement, TextBox, TextBoxStyle, XmlItem, XmlNode,
};
use lo_zip::ZipArchive;

use lo_core::style::{OdfStyles, odf_length};


pub fn load_bytes(title: impl Into<String>, bytes: &[u8], format: &str) -> Result<Presentation> {
    let title = title.into();
    match format.to_ascii_lowercase().as_str() {
        "pptx" => from_pptx_bytes(title, bytes),
        "odp" => from_odp_bytes(title, bytes),
        "txt" | "text" => Ok(from_text_outline(title, &String::from_utf8_lossy(bytes))),
        other => Err(LoError::Unsupported(format!(
            "impress import format {other}"
        ))),
    }
}

pub fn from_text_outline(title: impl Into<String>, text: &str) -> Presentation {
    let mut deck = Presentation::new(title);
    let mut current = Slide {
        name: "Slide 1".to_string(),
        elements: Vec::new(),
        notes: Vec::new(),
        chart_tokens: Vec::new(),
        odf_charts: Vec::new(),
        background: None,
    };
    let mut bullets: Vec<String> = Vec::new();
    let mut bullet_offset = 0u32;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            if !bullets.is_empty() {
                push_bullets(&mut current, &mut bullets);
            }
            if !current.elements.is_empty() || current.name != "Slide 1" {
                deck.slides.push(current);
                bullet_offset = 0;
            }
            current = Slide {
                name: rest.trim().to_string(),
                elements: Vec::new(),
                notes: Vec::new(),
                chart_tokens: Vec::new(),
                odf_charts: Vec::new(),
                background: None,
            };
        } else if let Some(rest) = trimmed.strip_prefix('-') {
            bullets.push(rest.trim().to_string());
        } else if !trimmed.is_empty() {
            let _ = bullet_offset;
            current.elements.push(SlideElement::TextBox(TextBox {
                frame: Rect::new(
                    Length::mm(20.0),
                    Length::mm(20.0 + current.elements.len() as f32 * 15.0),
                    Length::mm(220.0),
                    Length::mm(12.0),
                ),
                text: trimmed.to_string(),
                style: TextBoxStyle::default(),
            }));
        }
    }
    if !bullets.is_empty() {
        push_bullets(&mut current, &mut bullets);
    }
    deck.slides.push(current);
    deck
}

fn push_bullets(slide: &mut Slide, bullets: &mut Vec<String>) {
    let body = bullets
        .drain(..)
        .map(|line| format!("• {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    slide.elements.push(SlideElement::TextBox(TextBox {
        frame: Rect::new(
            Length::mm(20.0),
            Length::mm(40.0),
            Length::mm(220.0),
            Length::mm(80.0),
        ),
        text: body,
        style: TextBoxStyle::default(),
    }));
}

pub fn from_pptx_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<Presentation> {
    crate::pptx_import::load(title.into(), bytes)
}


/// Pull text out of `<a:tbl>` table cells embedded in a `<p:graphicFrame>`.
/// Returns one row per inner Vec.
pub fn from_odp_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<Presentation> {
    let title = title.into();
    let zip = ZipArchive::new(bytes)?;
    let content = parse_xml_document(&zip.read_string("content.xml")?)?;
    let body = content
        .child("body")
        .ok_or_else(|| LoError::Parse("content.xml missing office:body".to_string()))?;
    let presentation = body
        .child("presentation")
        .ok_or_else(|| LoError::Parse("content.xml missing office:presentation".to_string()))?;

    let mut deck = Presentation::new(title);
    let mut named_styles = OdfStyles::default();
    let mut master_styles = None;
    if zip.contains("styles.xml") {
        let styles = parse_xml_document(&zip.read_string("styles.xml")?)?;
        named_styles.collect(&styles);
        if let Some(size) = odp_page_size(presentation, &styles) {
            deck.page_size = size;
        }
        master_styles = Some(styles);
    }
    named_styles.collect(&content);
    let page_count = presentation.children_named("page").count();
    for (page_index, page) in presentation.children_named("page").enumerate() {
        let mut resolved_page = page.clone();
        resolve_odp_fields(
            &mut resolved_page,
            page,
            presentation,
            page_index + 1,
            page_count,
        );
        let mut slide = parse_odp_styled_slide(&resolved_page, &named_styles, Some(&zip));
        if let Some(master) = master_styles
            .as_ref()
            .and_then(|root| root.child("master-styles"))
            .and_then(|root| {
                root.children_named("master-page")
                    .find(|m| m.attr("name") == page.attr("master-page-name"))
            })
        {
            let page_style = page.attr("style-name").unwrap_or("");
            if named_styles.property(
                page_style,
                "drawing-page-properties",
                "background-objects-visible",
            ) != Some("false")
            {
                let mut artwork = master.clone();
                artwork.items.retain(|item| match item {
                    XmlItem::Node(n) => match n.attr("class") {
                        Some(class @ ("footer" | "header" | "date-time" | "page-number")) => {
                            let display = format!("display-{class}");
                            named_styles.property(page_style, "drawing-page-properties", &display)
                                == Some("true")
                        }
                        Some(_) => false,
                        None => n.local_name() != "notes" && n.attr("placeholder") != Some("true"),
                    },
                    _ => false,
                });
                resolve_odp_fields(&mut artwork, page, presentation, page_index + 1, page_count);
                let mut inherited = parse_odp_styled_slide(&artwork, &named_styles, Some(&zip));
                inherited.elements.append(&mut slide.elements);
                slide.elements = inherited.elements;
            }
            if let Some(name) = master.attr("style-name") {
                if named_styles.property(name, "drawing-page-properties", "fill") == Some("solid") {
                    slide.background = named_styles
                        .property(name, "drawing-page-properties", "fill-color")
                        .map(str::to_string);
                }
            }
        }
        slide.odf_charts = lo_zip::odf_charts(page, &zip);
        if let Some(name) = page.attr("style-name") {
            if named_styles.property(name, "drawing-page-properties", "fill") != Some("none") {
                if let Some(color) =
                    named_styles.property(name, "drawing-page-properties", "fill-color")
                {
                    slide.background = Some(color.into());
                }
            }
        }
        deck.slides.push(slide);
    }
    if deck.slides.is_empty() {
        deck.slides.push(Slide::default());
    }
    Ok(deck)
}

// Resolve presentation fields against their slide, preserving saved date/time
// text rather than substituting the server clock or guessing the user's locale.
fn resolve_odp_fields(
    node: &mut XmlNode,
    page: &XmlNode,
    presentation: &XmlNode,
    number: usize,
    count: usize,
) {
    let value = match node.local_name() {
        "page-number" => Some(number.to_string()),
        "page-count" => Some(count.to_string()),
        "page-name" => Some(page.attr("name").unwrap_or("").to_string()),
        field @ ("footer" | "header" | "date-time") => {
            let reference = format!("use-{field}-name");
            let declaration = format!("{field}-decl");
            page.attr(&reference)
                .and_then(|name| {
                    presentation
                        .children_named(&declaration)
                        .find(|n| n.attr("name") == Some(name))
                })
                .and_then(|decl| {
                    // A current-date declaration has no saved value; retain any
                    // cached field text instead of inventing a timestamp.
                    (decl.attr("source") != Some("current-date"))
                        .then(|| lo_core::style::odf_text(decl))
                })
        }
        _ => None,
    };
    if let Some(value) = value {
        node.text = value.clone();
        node.children.clear();
        node.items = vec![XmlItem::Text(value)];
        return;
    }
    for child in &mut node.children {
        resolve_odp_fields(std::sync::Arc::make_mut(child), page, presentation, number, count);
    }
    // Resolve each subtree once, then restore sharing in the ordered view.
    let mut children = node.children.iter();
    for item in &mut node.items {
        if let XmlItem::Node(child) = item {
            *child = children.next().expect("XML child/item correspondence").clone();
        }
    }
}

// ---------------------------------------------------------------------------

fn odp_page_size(presentation: &XmlNode, styles: &XmlNode) -> Option<lo_core::Size> {
    let master_name = presentation
        .children_named("page")
        .next()?
        .attr("master-page-name")?;
    let master = styles
        .child("master-styles")?
        .children_named("master-page")
        .find(|node| node.attr("name") == Some(master_name))?;
    let layout_name = master.attr("page-layout-name")?;
    let layout = styles
        .child("automatic-styles")?
        .children_named("page-layout")
        .find(|node| node.attr("name") == Some(layout_name))?;
    let props = layout.child("page-layout-properties")?;
    let w = parse_odf_len_mm(props.attr("page-width"))?;
    let h = parse_odf_len_mm(props.attr("page-height"))?;
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 || w > 10_000.0 || h > 10_000.0 {
        return None;
    }
    Some(lo_core::Size::new(Length::mm(w), Length::mm(h)))
}

#[cfg(test)]
fn parse_odp_slide(page: &XmlNode) -> Slide {
    parse_odp_styled_slide(page, &OdfStyles::default(), None)
}

fn parse_odp_styled_slide(page: &XmlNode, styles: &OdfStyles, zip: Option<&ZipArchive>) -> Slide {
    let mut slide = Slide {
        name: page.attr("name").unwrap_or("Slide").to_string(),
        elements: Vec::new(),
        notes: Vec::new(),
        chart_tokens: Vec::new(),
        odf_charts: Vec::new(),
        background: None,
    };
    for item in &page.items {
        let XmlItem::Node(node) = item else {
            continue;
        };
        let first_element = slide.elements.len();
        #[cfg(feature = "svg")]
        if matches!(
            node.local_name(),
            "custom-shape" | "path" | "polygon" | "polyline"
        ) {
            if let Some(image) = crate::odf_shapes::artwork(node, styles) {
                slide.elements.push(SlideElement::Image(image));
                parse_odp_frame(node, &mut slide, styles, zip);
                continue;
            }
        }
        match node.local_name() {
            "g" if node.attr("transform").is_none() => {
                let group = parse_odp_styled_slide(node, styles, zip);
                slide.elements.extend(group.elements);
            }
            "frame" => parse_odp_frame(node, &mut slide, styles, zip),
            "ellipse" => slide.elements.push(SlideElement::Shape(Shape {
                kind: ShapeKind::Ellipse,
                frame: Rect::new(
                    Length::mm(parse_odf_len_mm(node.attr("x")).unwrap_or(0.0)),
                    Length::mm(parse_odf_len_mm(node.attr("y")).unwrap_or(0.0)),
                    Length::mm(parse_odf_len_mm(node.attr("width")).unwrap_or(40.0)),
                    Length::mm(parse_odf_len_mm(node.attr("height")).unwrap_or(25.0)),
                ),
                style: ShapeStyle::default(),
            })),
            "line" => {
                let x1 = parse_odf_len_mm(node.attr("x1")).unwrap_or(0.0);
                let y1 = parse_odf_len_mm(node.attr("y1")).unwrap_or(0.0);
                let x2 = parse_odf_len_mm(node.attr("x2")).unwrap_or(0.0);
                let y2 = parse_odf_len_mm(node.attr("y2")).unwrap_or(0.0);
                slide.elements.push(SlideElement::Shape(Shape {
                    kind: ShapeKind::Line,
                    frame: Rect::new(
                        Length::mm(x1),
                        Length::mm(y1),
                        Length::mm(x2 - x1),
                        Length::mm(y2 - y1),
                    ),
                    style: ShapeStyle::default(),
                }));
            }
            "custom-shape" | "rect" => slide.elements.push(SlideElement::Shape(Shape {
                kind: ShapeKind::Rectangle,
                frame: Rect::new(
                    Length::mm(parse_odf_len_mm(node.attr("x")).unwrap_or(0.0)),
                    Length::mm(parse_odf_len_mm(node.attr("y")).unwrap_or(0.0)),
                    Length::mm(parse_odf_len_mm(node.attr("width")).unwrap_or(40.0)),
                    Length::mm(parse_odf_len_mm(node.attr("height")).unwrap_or(25.0)),
                ),
                style: ShapeStyle::default(),
            })),
            _ => {}
        }
        if matches!(node.local_name(), "custom-shape" | "rect" | "ellipse") {
            let name = node.attr("style-name").unwrap_or("");
            if styles.property(name, "graphic-properties", "fill") == Some("none")
                && styles.property(name, "graphic-properties", "stroke") == Some("none")
            {
                slide.elements.truncate(first_element);
            } else if let Some(SlideElement::Shape(shape)) = slide.elements.get_mut(first_element) {
                if let Some((dx, dy, angle)) = node.attr("transform").and_then(odp_text_transform) {
                    shape.frame.origin.x = Length::pt(shape.frame.origin.x.as_pt() + dx);
                    shape.frame.origin.y = Length::pt(shape.frame.origin.y.as_pt() + dy);
                    shape.style.rotation_millidegrees =
                        (angle.to_degrees() * 1000.0).round() as i32;
                }
                if let Some(fill) = styles.property(name, "graphic-properties", "fill-color") {
                    shape.style.fill = fill.to_string();
                }
                if let Some(stroke) = styles.property(name, "graphic-properties", "stroke-color") {
                    shape.style.stroke = stroke.to_string();
                }
                if styles.property(name, "graphic-properties", "stroke") == Some("none") {
                    shape.style.stroke = "none".into();
                }
                if styles.property(name, "graphic-properties", "fill") == Some("none") {
                    shape.style.fill = "none".into();
                }
            }
            parse_odp_frame(node, &mut slide, styles, zip);
        }
    }
    slide
}

fn parse_odp_frame(
    frame: &XmlNode,
    slide: &mut Slide,
    styles: &OdfStyles,
    zip: Option<&ZipArchive>,
) {
    if let Some(mut image) = zip.and_then(|z| lo_zip::odf_image(frame, z)) {
        image.opacity = frame
            .attr("style-name")
            .and_then(|name| styles.property(name, "graphic-properties", "image-opacity"))
            .and_then(|v| v.strip_suffix('%'))
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map(|v| (v.clamp(0.0, 100.0) * 2.55).round() as u8)
            .unwrap_or(255);
        slide.elements.push(SlideElement::Image(image));
        return;
    }
    let mut x = parse_odf_len_mm(frame.attr("x")).unwrap_or(0.0);
    let mut y = parse_odf_len_mm(frame.attr("y")).unwrap_or(0.0);
    let width = parse_odf_len_mm(frame.attr("width")).unwrap_or(60.0);
    let height = parse_odf_len_mm(frame.attr("height")).unwrap_or(20.0);
    if let Some(table) = frame.child("table") {
        parse_odp_table(
            table,
            Rect::new(
                Length::mm(x),
                Length::mm(y),
                Length::mm(width),
                Length::mm(height),
            ),
            styles,
            slide,
        );
        return;
    }
    let mut style = odp_text_style(frame, styles);
    if let Some((dx, dy, angle)) = frame.attr("transform").and_then(odp_text_transform) {
        x += dx * 25.4 / 72.0;
        y += dy * 25.4 / 72.0;
        style.rotation = angle;
    }
    let text_box = frame.child("text-box").unwrap_or(frame);
    let mut paragraphs = Vec::new();
    collect_odp_list_lines(
        text_box,
        styles,
        0,
        None,
        &mut paragraphs,
        &mut style.paragraph_indents,
        &mut style.paragraph_markers,
    );
    if paragraphs.is_empty() {
        return;
    }
    if slide.elements.is_empty() {
        slide.name = paragraphs[0].clone();
    }
    slide.elements.push(SlideElement::TextBox(TextBox {
        frame: Rect::new(
            Length::mm(x),
            Length::mm(y),
            Length::mm(width),
            Length::mm(height),
        ),
        text: paragraphs.join("\n"),
        style,
    }));
}

// ODF emits rotation followed by translation for ordinary rotated text boxes.
// Reject other transform forms instead of partially applying a matrix.
fn odp_text_transform(value: &str) -> Option<(f32, f32, f32)> {
    let mut rest = value.trim();
    let mut angle = 0.0;
    let mut x = 0.0;
    let mut y = 0.0;
    let mut translated = false;
    while !rest.is_empty() {
        let (name, args) = rest.split_once('(')?;
        let (args, tail) = args.split_once(')')?;
        let values: Vec<_> = args
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .collect();
        match (name.trim(), values.as_slice()) {
            ("rotate", [value]) if !translated => {
                angle += value.parse::<f32>().ok()?;
            }
            ("translate", [dx, dy]) => {
                x += odf_length(dx)?;
                y += odf_length(dy)?;
                translated = true;
            }
            _ => return None,
        }
        rest = tail.trim();
    }
    (angle.is_finite() && x.is_finite() && y.is_finite()).then_some((x, y, angle))
}

fn odp_text_style(node: &XmlNode, styles: &OdfStyles) -> TextBoxStyle {
    let mut result = TextBoxStyle {
        border: false,
        font_size_pt: 18.0,
        ..TextBoxStyle::default()
    };
    fn visit(node: &XmlNode, styles: &OdfStyles, out: &mut TextBoxStyle) {
        for attribute in ["style-name", "text-style-name"] {
            if let Some(name) = node.attr(attribute) {
                let prop = |kind, attr| styles.explicit_property(name, kind, attr);
                if let Some(size) = prop("text-properties", "font-size").and_then(odf_length) {
                    if size > 0.0 {
                        out.font_size_pt = size;
                    }
                }
                if let Some(family) = prop("text-properties", "font-family")
                    .or_else(|| prop("text-properties", "font-name"))
                {
                    out.font_family = family.replace("_20_", " ");
                }
                if let Some(color) = prop("text-properties", "color") {
                    out.foreground = color.to_string();
                }
                if let Some(weight) = prop("text-properties", "font-weight") {
                    out.bold = weight == "bold";
                }
                if let Some(style) = prop("text-properties", "font-style") {
                    out.italic = style == "italic";
                }
                if let Some(align) = prop("paragraph-properties", "text-align") {
                    out.alignment = match align {
                        "center" => lo_core::style::Alignment::Center,
                        "end" | "right" => lo_core::style::Alignment::End,
                        _ => lo_core::style::Alignment::Start,
                    };
                }
            }
        }
        // Current text boxes have one text style; use the first text run.
        if let Some(child) = node.children.iter().find(|n| {
            matches!(
                n.local_name(),
                "text-box" | "p" | "h" | "span" | "list" | "list-item"
            )
        }) {
            visit(child, styles, out);
        }
    }
    if let Some(color) = styles.property_in("", "paragraph", "text-properties", "color") {
        result.foreground = color.to_string();
    }
    visit(node, styles, &mut result);
    let name = node.attr("style-name").unwrap_or("");
    result.vertical_alignment =
        match styles.property(name, "graphic-properties", "textarea-vertical-align") {
            Some("middle") => lo_core::style::Alignment::Center,
            Some("bottom") => lo_core::style::Alignment::End,
            _ => lo_core::style::Alignment::Start,
        };
    if let Some(padding) = styles
        .property(name, "graphic-properties", "padding")
        .or_else(|| styles.property(name, "graphic-properties", "padding-left"))
        .and_then(odf_length)
    {
        result.padding = Length::pt(padding.max(0.0));
    }
    result
}

fn parse_odp_table(table: &XmlNode, frame: Rect, styles: &OdfStyles, slide: &mut Slide) {
    let rows: Vec<_> = table.children_named("table-row").collect();
    if rows.is_empty() {
        return;
    }
    let mut widths = Vec::new();
    for column in table.children_named("table-column") {
        let count = column
            .attr("number-columns-repeated")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1)
            .min(1024);
        let w = column
            .attr("style-name")
            .and_then(|s| styles.property(s, "table-column-properties", "column-width"))
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .unwrap_or(72.0);
        widths.extend(std::iter::repeat_n(w, count));
    }
    if widths.is_empty() {
        widths.resize(rows[0].children.len().max(1), 72.0);
    }
    let scale = frame.size.width.as_pt() / widths.iter().sum::<f32>();
    for w in &mut widths {
        *w *= scale;
    }
    let mut heights: Vec<_> = rows
        .iter()
        .map(|row| {
            row.attr("style-name")
                .and_then(|s| styles.property(s, "table-row-properties", "row-height"))
                .and_then(odf_length)
                .filter(|v| *v > 0.0)
                .unwrap_or(frame.size.height.as_pt() / rows.len() as f32)
        })
        .collect();
    let scale = frame.size.height.as_pt() / heights.iter().sum::<f32>();
    for h in &mut heights {
        *h *= scale;
    }
    let mut y = frame.origin.y.as_pt();
    for (r, row) in rows.iter().enumerate() {
        let mut col = 0;
        let mut x = frame.origin.x.as_pt();
        for cell in row
            .children
            .iter()
            .filter(|n| matches!(n.local_name(), "table-cell" | "covered-table-cell"))
        {
            if col >= widths.len() {
                break;
            }
            let repeat = cell
                .attr("number-columns-repeated")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .min(widths.len() - col);
            for _ in 0..repeat {
                if cell.local_name() != "covered-table-cell" {
                    let span = cell
                        .attr("number-columns-spanned")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(1)
                        .max(1)
                        .min(widths.len() - col);
                    let row_span = cell
                        .attr("number-rows-spanned")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(1)
                        .max(1)
                        .min(rows.len() - r);
                    let rect = Rect::new(
                        Length::pt(x),
                        Length::pt(y),
                        Length::pt(widths[col..col + span].iter().sum()),
                        Length::pt(heights[r..r + row_span].iter().sum()),
                    );
                    let name = cell
                        .attr("style-name")
                        .or_else(|| row.attr("default-cell-style-name"))
                        .unwrap_or("");
                    let fill = styles
                        .property(name, "graphic-properties", "fill-color")
                        .or_else(|| {
                            styles.property(name, "table-cell-properties", "background-color")
                        })
                        .unwrap_or("#ffffff");
                    slide.elements.push(SlideElement::Shape(Shape {
                        frame: rect,
                        kind: ShapeKind::Rectangle,
                        style: ShapeStyle {
                            rotation_millidegrees: 0,
                            fill: fill.to_string(),
                            stroke: "#ffffff".to_string(),
                            stroke_width_mm: 0,
                        },
                    }));
                    let mut paragraphs = Vec::new();
                    collect_odp_paragraphs(cell, &mut paragraphs);
                    if !paragraphs.is_empty() {
                        let styled = cell.clone();
                        // Resolve row default cell styles without altering source XML.
                        let mut style = odp_text_style(&styled, styles);
                        if let Some(size) = styles
                            .property(name, "text-properties", "font-size")
                            .and_then(odf_length)
                        {
                            style.font_size_pt = size;
                        }
                        if let Some(color) = styles.property(name, "text-properties", "color") {
                            style.foreground = color.to_string();
                        }
                        style.bold =
                            styles.property(name, "text-properties", "font-weight") == Some("bold");
                        slide.elements.push(SlideElement::TextBox(TextBox {
                            frame: rect,
                            text: paragraphs.join("\n"),
                            style,
                        }));
                    }
                }
                x += widths[col];
                col += 1;
            }
        }
        y += heights[r];
    }
}

fn collect_odp_list_lines(
    node: &XmlNode,
    styles: &OdfStyles,
    depth: usize,
    mut marker: Option<String>,
    texts: &mut Vec<String>,
    indents: &mut Vec<f32>,
    markers: &mut Vec<Option<String>>,
) {
    for child in &node.children {
        match child.local_name() {
            "p" | "h" => {
                let text = lo_core::style::odf_text(child);
                if !text.trim().is_empty() {
                    for line in text.split('\n') {
                        texts.push(line.to_string());
                        indents.push(depth.min(32) as f32 * 18.0);
                        markers.push(marker.take());
                    }
                }
            }
            "list" => {
                let level = child
                    .attr("style-name")
                    .and_then(|name| styles.0.get(name))
                    .and_then(|style| {
                        style.children.iter().find(|n| {
                            n.attr("level").and_then(|v| v.parse::<usize>().ok()) == Some(depth + 1)
                        })
                    });
                for (i, item) in child.children_named("list-item").enumerate() {
                    let marker =
                        if level.is_some_and(|n| n.local_name() == "list-level-style-number") {
                            format!("{}.", i + 1)
                        } else {
                            level
                                .and_then(|n| n.attr("bullet-char"))
                                .unwrap_or("•")
                                .to_string()
                        };
                    collect_odp_list_lines(
                        item,
                        styles,
                        depth + 1,
                        Some(marker),
                        texts,
                        indents,
                        markers,
                    );
                }
            }
            "text-box" | "list-header" => {
                collect_odp_list_lines(child, styles, depth, marker.take(), texts, indents, markers)
            }
            _ => {}
        }
    }
}

// ODF list items contain paragraphs, often beneath several nested lists.
// Stop at paragraphs so inline spans are not emitted a second time.
fn collect_odp_paragraphs(node: &XmlNode, paragraphs: &mut Vec<String>) {
    for child in &node.children {
        if matches!(child.local_name(), "p" | "h") {
            let text = lo_core::style::odf_text(child);
            if !text.trim().is_empty() {
                paragraphs.push(text);
            }
        } else if matches!(child.local_name(), "list" | "list-item" | "list-header") {
            collect_odp_paragraphs(child, paragraphs);
        }
    }
}

#[cfg(test)]
mod fidelity_tests {
    use super::*;
    #[test]
    fn text_styles_cascade_without_resetting_parent_color_and_keep_font_and_alignment() {
        let xml = parse_xml_document(r##"<styles><default-style family="text"><text-properties color="#000000"/></default-style><style name="Master" family="presentation"><text-properties color="#ffffff" font-family="Liberation Serif"/><graphic-properties textarea-vertical-align="middle" padding="0.1in"/></style><style name="Size" family="text"><text-properties font-size="14pt" font-weight="bold" font-style="italic"/></style></styles>"##).unwrap();
        let mut styles = OdfStyles::default();
        styles.collect(&xml);
        let frame = parse_xml_document(r#"<frame style-name="Master"><text-box><p><span style-name="Size">Styled text</span></p></text-box></frame>"#).unwrap();
        let text = odp_text_style(&frame, &styles);
        assert_eq!(text.foreground, "#ffffff");
        assert_eq!(text.font_family, "Liberation Serif");
        assert_eq!(text.font_size_pt, 14.0);
        assert!(text.bold && text.italic);
        assert_eq!(text.vertical_alignment, lo_core::style::Alignment::Center);
        assert!((text.padding.as_pt() - 7.2).abs() < 0.001);
    }

    #[test]
    fn enabled_master_fields_use_slide_context_and_hidden_fields_stay_hidden() {
        use lo_zip::ZipEntry;
        let content = r#"<document><automatic-styles><style name="Visible" family="drawing-page"><drawing-page-properties display-footer="true" display-page-number="true"/></style></automatic-styles><body><presentation><footer-decl name="Footer">Shared footer</footer-decl><page name="First" master-page-name="Master" style-name="Visible" use-footer-name="Footer"/><page name="Second" master-page-name="Master"/><page name="Third" master-page-name="Master" style-name="Visible" use-footer-name="Footer"/></presentation></body></document>"#;
        let styles = r#"<document><master-styles><master-page name="Master"><frame class="footer" placeholder="true"><text-box><p><footer/></p></text-box></frame><frame class="page-number"><text-box><p><page-name/>: <page-number>99</page-number> of <page-count>99</page-count></p></text-box></frame><frame class="title"><text-box><p>Placeholder title</p></text-box></frame></master-page></master-styles></document>"#;
        let bytes = lo_zip::odf_package(
            "application/vnd.oasis.opendocument.presentation",
            vec![
                ZipEntry::new("content.xml", content),
                ZipEntry::new("styles.xml", styles),
            ],
        )
        .unwrap();
        let deck = from_odp_bytes("test", &bytes).unwrap();
        let text = |index: usize| {
            deck.slides[index]
                .elements
                .iter()
                .filter_map(|e| match e {
                    SlideElement::TextBox(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(text(0), "Shared footer\nFirst: 1 of 3");
        assert_eq!(text(1), "");
        assert_eq!(text(2), "Shared footer\nThird: 3 of 3");
    }

    #[test]
    fn master_artwork_is_inherited_with_opacity_but_not_placeholders() {
        use lo_zip::ZipEntry;
        let content = r#"<document><body><presentation><page name="Page" master-page-name="Master"/></presentation></body></document>"#;
        let styles = r#"<document><automatic-styles><style name="Image" family="graphic"><graphic-properties image-opacity="49%"/></style></automatic-styles><master-styles><master-page name="Master"><frame style-name="Image" width="10cm" height="5cm"><image href="Pictures/image.png"/></frame><frame placeholder="true" class="title"><text-box><p>Do not render placeholder</p></text-box></frame></master-page></master-styles></document>"#;
        let bytes = lo_zip::odf_package(
            "application/vnd.oasis.opendocument.presentation",
            vec![
                ZipEntry::new("content.xml", content),
                ZipEntry::new("styles.xml", styles),
                ZipEntry::new("Pictures/image.png", vec![0]),
            ],
        )
        .unwrap();
        let deck = from_odp_bytes("test", &bytes).unwrap();
        assert_eq!(deck.slides[0].elements.len(), 1);
        let SlideElement::Image(image) = &deck.slides[0].elements[0] else {
            panic!("master image")
        };
        assert_eq!(image.opacity, 125);
        assert!((image.frame.size.width.as_mm() - 100.0).abs() < 0.01);
    }

    #[test]
    fn table_cells_keep_geometry_fills_and_spans() {
        let root = parse_xml_document(r##"<styles><style name="blue" family="table-cell"><graphic-properties fill-color="#123456"/></style></styles>"##).unwrap();
        let mut styles = OdfStyles::default();
        styles.collect(&root);
        let table = parse_xml_document(r#"<table><table-column number-columns-repeated="2"/><table-row><table-cell style-name="blue" number-columns-spanned="2"><p>Merged</p></table-cell><covered-table-cell/></table-row></table>"#).unwrap();
        let mut slide = Slide::default();
        parse_odp_table(
            &table,
            Rect::new(
                Length::pt(10.0),
                Length::pt(20.0),
                Length::pt(200.0),
                Length::pt(50.0),
            ),
            &styles,
            &mut slide,
        );
        assert_eq!(slide.elements.len(), 2);
        let SlideElement::Shape(shape) = &slide.elements[0] else {
            panic!("shape")
        };
        assert_eq!(shape.style.fill, "#123456");
        assert!((shape.frame.size.width.as_pt() - 200.0).abs() < 0.01);
    }
    #[test]
    fn rotated_text_transform_is_bounded_and_complete() {
        let (x, y, a) = odp_text_transform("rotate (-0.2) translate (1in 2cm)").unwrap();
        assert!((x - 72.0).abs() < 0.01);
        assert!((y - 56.6929).abs() < 0.01);
        assert_eq!(a, -0.2);
        assert!(odp_text_transform("scale (2 2) translate (1in 2cm)").is_none());
        assert!(odp_text_transform("rotate (NaN)").is_none());
    }
    #[test]
    fn odp_dimensions_follow_slide_master_not_notes_layout() {
        let presentation =
            parse_xml_document(r#"<presentation><page master-page-name="Wide"/></presentation>"#)
                .unwrap();
        let styles = parse_xml_document(r#"<styles><automatic-styles><page-layout name="Notes"><page-layout-properties page-width="21cm" page-height="29.7cm"/></page-layout><page-layout name="Slide"><page-layout-properties page-width="10in" page-height="7.5in"/></page-layout></automatic-styles><master-styles><master-page name="Wide" page-layout-name="Slide"/></master-styles></styles>"#).unwrap();
        let size = odp_page_size(&presentation, &styles).unwrap();
        assert!((size.width.as_pt() - 720.0).abs() < 0.01);
        assert!((size.height.as_pt() - 540.0).abs() < 0.01);
    }
    #[test]
    fn custom_shapes_keep_their_text() {
        let root = parse_xml_document(r#"<page><custom-shape width="10cm" height="2cm"><p>Shape text</p></custom-shape></page>"#).unwrap();
        let slide = parse_odp_slide(&root);
        assert!(slide.elements.iter().any(|element| matches!(element,
            SlideElement::TextBox(t) if t.text == "Shape text")));
    }
    #[test]
    fn nested_odp_list_text_is_preserved_once_in_order() {
        let root = parse_xml_document(r#"<text-box><p>Title</p><list><list-item><p>First <span>item</span></p><list><list-item><p>Nested</p></list-item></list></list-item><list-item><p>Last</p></list-item></list></text-box>"#).unwrap();
        let mut paragraphs = Vec::new();
        collect_odp_paragraphs(&root, &mut paragraphs);
        assert_eq!(paragraphs, ["Title", "First item", "Nested", "Last"]);
    }
}

fn parse_odf_len_mm(value: Option<&str>) -> Option<f32> {
    let value = value?;
    let trimmed = value.trim();
    if let Some(number) = trimmed.strip_suffix("cm") {
        return number.parse::<f32>().ok().map(|cm| cm * 10.0);
    }
    if let Some(number) = trimmed.strip_suffix("mm") {
        return number.parse::<f32>().ok();
    }
    if let Some(number) = trimmed.strip_suffix("in") {
        return number.parse::<f32>().ok().map(|inch| inch * 25.4);
    }
    if let Some(number) = trimmed.strip_suffix("pt") {
        return number.parse::<f32>().ok().map(|pt| pt * 25.4 / 72.0);
    }
    trimmed.parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{demo_presentation, to_pptx};

    #[test]
    fn grouped_shapes_apply_rotation_and_translation() {
        let node = parse_xml_document(r#"<page><g><custom-shape width="1cm" height="2cm" transform="rotate (0.34906585) translate (11cm 7cm)"/></g></page>"#).unwrap();
        let slide = parse_odp_slide(&node);
        let SlideElement::Shape(shape) = &slide.elements[0] else {
            panic!("shape")
        };
        assert!((shape.frame.origin.x.as_mm() - 110.0).abs() < 0.01);
        assert!((shape.frame.origin.y.as_mm() - 70.0).abs() < 0.01);
        assert_eq!(shape.style.rotation_millidegrees, 20_000);
    }

    #[test]
    fn pptx_round_trip_imports_slide_text() {
        let deck = demo_presentation("Demo");
        let bytes = to_pptx(&deck).expect("pptx");
        let loaded = from_pptx_bytes("Demo", &bytes).expect("import pptx");
        assert!(!loaded.slides.is_empty());
    }

    #[test]
    fn odp_round_trip_imports_slide_text() {
        let deck = demo_presentation("Demo");
        let tmp = std::env::temp_dir().join("lo_impress_import_test.odp");
        lo_odf::save_presentation_document(&tmp, &deck).expect("save odp");
        let bytes = std::fs::read(&tmp).expect("read odp");
        let _ = std::fs::remove_file(&tmp);
        let loaded = from_odp_bytes("Demo", &bytes).expect("import odp");
        assert!(!loaded.slides.is_empty());
    }

    #[test]
    fn text_outline_creates_slides() {
        let deck = from_text_outline("Outline", "# First\n- one\n- two\n# Second\n- three\n");
        assert_eq!(deck.slides.len(), 2);
    }
}
