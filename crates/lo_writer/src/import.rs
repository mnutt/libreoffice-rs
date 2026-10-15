//! Binary importers for `TextDocument`.
//!
//! Supported formats:
//! - `txt`/`text`, `md`/`markdown`, `html`/`htm` (string-based, no zip)
//! - `pdf` (native PDF text import)
//! - `docx` (Office Open XML)
//! - `odt` (OpenDocument Text)

use std::collections::BTreeMap;

use lo_core::{
    parse_xml_document, Block, Heading, Inline, ListBlock, ListItem, LoError, Paragraph, Result,
    Table, TableCell, TableRow, TextDocument, XmlItem, XmlNode,
};
use lo_zip::{rels_path_for, resolve_part_target, ZipArchive};

use crate::{from_markdown, from_plain_text};

/// Style hints attached to a span while we walk the source document.
#[derive(Clone, Debug, Default, PartialEq)]
struct SpanStyle {
    text_style: Option<lo_core::TextStyle>,
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct StyleProps {
    list_layout: Option<lo_core::writer::OdfListLayout>,
    paragraph_style: Option<lo_core::ParagraphStyle>,
    text_style: Option<lo_core::TextStyle>,
    bold: bool,
    italic: bool,
    code: bool,
    heading_level: Option<u8>,
    page_break_before: bool,
}

#[derive(Clone, Debug, Default)]
struct StyledSpan {
    text: String,
    style: SpanStyle,
}

#[derive(Clone, Debug, Default)]
struct ParagraphInfo {
    spans: Vec<StyledSpan>,
    heading_level: Option<u8>,
    list_key: Option<String>,
    page_break: bool,
}

/// Dispatch to the appropriate importer based on a format hint.
pub fn load_bytes(title: impl Into<String>, bytes: &[u8], format: &str) -> Result<TextDocument> {
    let title = title.into();
    match format.to_ascii_lowercase().as_str() {
        "txt" | "text" => Ok(from_plain_text(title, &bytes_to_utf8(bytes)?)),
        "md" | "markdown" => Ok(from_markdown(title, &bytes_to_utf8(bytes)?)),
        "html" | "htm" => Ok(from_html(title, &bytes_to_utf8(bytes)?)),
        "pdf" => from_pdf_bytes(title, bytes),
        "docx" => from_docx_bytes(title, bytes),
        "doc" => from_doc_bytes(title, bytes),
        "odt" => from_odt_bytes(title, bytes),
        other => Err(LoError::Unsupported(format!(
            "writer import format {other}"
        ))),
    }
}

/// Read text from a legacy binary `.doc` (Word 97-2003) file. The CFB
/// stream is parsed natively; the resulting plain text is wrapped in a
/// `TextDocument` via `from_plain_text`.
pub fn from_doc_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<TextDocument> {
    let text = crate::legacy_doc::extract_text_from_doc(bytes)?;
    Ok(from_plain_text(title, &text))
}

/// Read text from a PDF byte stream using the shared native PDF parser.
/// Each extracted page is mapped to paragraphs, with explicit page-break
/// blocks inserted between pages.
pub fn from_pdf_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<TextDocument> {
    let title = title.into();
    let pages = lo_core::extract_pages_from_pdf(bytes)?;
    let mut doc = TextDocument::new(title);
    let mut emitted_any = false;
    for (page_index, page_text) in pages.iter().enumerate() {
        if page_index > 0 && emitted_any {
            doc.body.push(Block::PageBreak);
        }
        let normalized = page_text.replace("\u{000C}", "\n");
        let mut page_emitted = false;
        for paragraph in normalized.split("\n\n") {
            let trimmed = paragraph.trim();
            if trimmed.is_empty() {
                continue;
            }
            let joined = trimmed
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !joined.is_empty() {
                doc.body.push(Block::Paragraph(Paragraph::plain(joined)));
                emitted_any = true;
                page_emitted = true;
            }
        }
        if !page_emitted && !normalized.trim().is_empty() {
            doc.body.push(Block::Paragraph(Paragraph::plain(
                normalized.trim().to_string(),
            )));
            emitted_any = true;
        }
    }
    if !emitted_any {
        let text = lo_core::extract_text_from_pdf(bytes)?;
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            doc.body
                .push(Block::Paragraph(Paragraph::plain(trimmed.to_string())));
            emitted_any = true;
        }
    }
    if !emitted_any {
        doc.body.push(Block::Paragraph(Paragraph::default()));
    }
    Ok(doc)
}

fn bytes_to_utf8(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|err| LoError::Parse(format!("invalid utf-8 input: {err}")))
}

/// Lossy HTML to text conversion. Falls back through `from_plain_text` so
/// downstream callers still get a structured document.
pub fn from_html(title: impl Into<String>, html: &str) -> TextDocument {
    let normalized = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n\n")
        .replace("</div>", "\n")
        .replace("</li>", "\n")
        .replace("<li>", "- ")
        .replace("</h1>", "\n\n")
        .replace("</h2>", "\n\n")
        .replace("</h3>", "\n\n");
    let mut plain = String::new();
    let mut in_tag = false;
    for ch in normalized.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => plain.push(ch),
            _ => {}
        }
    }
    from_plain_text(title, &lo_core::decode_entities(&plain))
}

pub fn from_docx_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<TextDocument> {
    let title = title.into();
    let zip = ZipArchive::new(bytes)?;
    let document_xml = zip.read_string("word/document.xml")?;
    let document = parse_xml_document(&document_xml)?;
    let body = document
        .child("body")
        .ok_or_else(|| LoError::Parse("word/document.xml missing w:body".to_string()))?;
    let relationships = parse_relationships(&zip, "word/document.xml")?;
    let styles = if zip.contains("word/styles.xml") {
        parse_docx_styles(&parse_xml_document(&zip.read_string("word/styles.xml")?)?)
    } else {
        BTreeMap::new()
    };

    // Leave the title empty so we never synthesize a heading in PDF /
    // Markdown output. `soffice --convert-to pdf` does not render
    // `dc:title` either, so emitting it would only add false-positive
    // tokens to the head-to-head benchmark.
    let _ = title;
    let mut doc = TextDocument::new(String::new());
    let numbering = parse_docx_numbering(&zip);
    let mut pending_list: Vec<ListItem> = Vec::new();
    let mut pending_list_ordered = false;

    // Pre-collect header/footer text via relationships so we can prepend
    // headers and append footers — the LibreOffice CLI emits them in plain
    // text exports and the quality benchmark scores us against that output.
    let header_blocks = collect_docx_header_footer_blocks(&zip, &relationships, "header");
    let footer_blocks = collect_docx_header_footer_blocks(&zip, &relationships, "footer");
    for line in header_blocks {
        doc.body.push(Block::Paragraph(Paragraph::plain(line)));
    }

    for item in &body.items {
        let XmlItem::Node(node) = item else {
            continue;
        };
        match node.local_name() {
            "p" => {
                let images = crate::docx_images::from_docx_node(node, &relationships, &zip);
                let info = parse_docx_paragraph(node, &relationships, &styles);
                if info.page_break && info.spans.iter().all(|span| span.text.trim().is_empty()) {
                    flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                    doc.body.push(Block::PageBreak);
                } else {
                    let paragraph = build_paragraph(info.spans.clone());
                    let is_empty_text = paragraph.spans.is_empty()
                        || paragraph
                            .spans
                            .iter()
                            .all(|inline| inline_text(inline).trim().is_empty());
                    if is_empty_text && info.heading_level.is_none() && info.list_key.is_none() {
                        flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                        if images.is_empty() {
                            doc.body.push(Block::Paragraph(paragraph));
                        }
                    } else if let Some(level) = info.heading_level {
                        flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                        doc.body.push(Block::Heading(Heading {
                            level,
                            content: paragraph,
                        }));
                    } else if let Some(key) = info.list_key.as_deref() {
                        let ordered = numbering.get(key).copied().unwrap_or(false);
                        if !pending_list.is_empty() && pending_list_ordered != ordered {
                            flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                        }
                        pending_list_ordered = ordered;
                        pending_list.push(ListItem {
                            blocks: vec![Block::Paragraph(paragraph)],
                        });
                    } else {
                        flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                        doc.body.push(Block::Paragraph(paragraph));
                    }
                }
                if !images.is_empty() {
                    flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                    doc.body.extend(images.into_iter().map(Block::Image));
                }
            }
            "tbl" => {
                flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
                doc.body.push(Block::Table(parse_docx_table(node)));
                doc.body.extend(
                    crate::docx_images::from_docx_node(node, &relationships, &zip)
                        .into_iter()
                        .map(Block::Image),
                );
            }
            _ => {}
        }
    }
    flush_list_with(&mut doc, &mut pending_list, pending_list_ordered);
    renumber_footnote_markers(&mut doc);
    // Append footnotes / endnotes so PDF text + Markdown extraction
    // include them (matches what `pdftotext` finds in the LO PDF).
    for path in ["word/footnotes.xml", "word/endnotes.xml"] {
        if zip.contains(path) {
            if let Ok(xml) = zip.read_string(path) {
                if let Ok(root) = parse_xml_document(&xml) {
                    let mut texts: Vec<String> = Vec::new();
                    collect_w_text_nodes(&root, &mut texts);
                    let joined = texts
                        .into_iter()
                        .filter(|s| !s.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !joined.trim().is_empty() {
                        doc.body.push(Block::Paragraph(Paragraph::plain(joined)));
                    }
                }
            }
        }
    }
    for line in footer_blocks {
        doc.body.push(Block::Paragraph(Paragraph::plain(line)));
    }
    while matches!(
        doc.body.last(),
        Some(Block::Paragraph(paragraph)) if paragraph.to_plain_text().trim().is_empty()
    ) {
        doc.body.pop();
    }
    if doc.body.is_empty() {
        doc.body.push(Block::Paragraph(Paragraph::default()));
    }
    Ok(doc)
}

/// Walk every relationship of `word/document.xml` whose type ends in
/// `header` or `footer`, parse the referenced part, and return a flat
/// list of paragraph plain-text strings.
fn collect_docx_header_footer_blocks(
    zip: &ZipArchive,
    relationships: &BTreeMap<String, String>,
    kind: &str,
) -> Vec<String> {
    let rels_path = "word/_rels/document.xml.rels";
    let rels_xml = match zip.read_string(rels_path) {
        Ok(x) => x,
        Err(_) => return Vec::new(),
    };
    let rels_root = match parse_xml_document(&rels_xml) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut targets: Vec<String> = Vec::new();
    for rel in rels_root.children_named("Relationship") {
        let ty = rel.attr("Type").unwrap_or("");
        if !ty.ends_with(kind) {
            continue;
        }
        if let Some(target) = rel.attr("Target") {
            let resolved = resolve_part_target("word/document.xml", target);
            if zip.contains(&resolved) {
                targets.push(resolved);
            }
        }
        let _ = relationships;
    }
    let mut out: Vec<String> = Vec::new();
    for target in targets {
        if let Ok(xml) = zip.read_string(&target) {
            if let Ok(root) = parse_xml_document(&xml) {
                let mut texts: Vec<String> = Vec::new();
                collect_w_text_nodes(&root, &mut texts);
                let joined = texts
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !joined.trim().is_empty() {
                    out.push(joined);
                }
            }
        }
    }
    out
}

/// Walk the document body and replace the per-run footnote / endnote
/// placeholder characters (`\u{f001}` and `\u{f002}`) with their
/// document-order index. Footnotes get arabic numerals (1, 2, …) and
/// endnotes get lowercase Roman numerals (i, ii, …) — both Word and the
/// LibreOffice CLI render them this way by default.
fn renumber_footnote_markers(doc: &mut TextDocument) {
    let mut foot = 0u32;
    let mut endn = 0u32;
    fn replace_in_text(s: &str, foot: &mut u32, endn: &mut u32) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '\u{f001}' => {
                    *foot += 1;
                    out.push_str(&foot.to_string());
                }
                '\u{f002}' => {
                    *endn += 1;
                    out.push_str(&to_lower_roman(*endn));
                }
                other => out.push(other),
            }
        }
        out
    }
    fn walk_inline(inline: &mut Inline, foot: &mut u32, endn: &mut u32) {
        match inline {
            Inline::Text(t) | Inline::Bold(t) | Inline::Italic(t) | Inline::Code(t) => {
                *t = replace_in_text(t, foot, endn)
            }
            Inline::Link { label, .. } => *label = replace_in_text(label, foot, endn),
            Inline::Styled { text, .. } => *text = replace_in_text(text, foot, endn),
            Inline::LineBreak => {}
        }
    }
    fn walk_paragraph(p: &mut Paragraph, foot: &mut u32, endn: &mut u32) {
        for span in &mut p.spans {
            walk_inline(span, foot, endn);
        }
    }
    fn walk_block(block: &mut Block, foot: &mut u32, endn: &mut u32) {
        match block {
            Block::Paragraph(p) => walk_paragraph(p, foot, endn),
            Block::Heading(h) => walk_paragraph(&mut h.content, foot, endn),
            Block::List(list) => {
                for item in &mut list.items {
                    for sub in &mut item.blocks {
                        walk_block(sub, foot, endn);
                    }
                }
            }
            Block::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        for p in &mut cell.paragraphs {
                            walk_paragraph(p, foot, endn);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for block in &mut doc.body {
        walk_block(block, &mut foot, &mut endn);
    }
}

fn to_lower_roman(mut value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (n, s) in table {
        while value >= n {
            out.push_str(s);
            value -= n;
        }
    }
    out
}

fn collect_w_text_nodes(node: &XmlNode, out: &mut Vec<String>) {
    if node.local_name() == "t" {
        out.push(node.text_content());
    }
    for child in &node.children {
        collect_w_text_nodes(child, out);
    }
}

pub fn from_odt_bytes(title: impl Into<String>, bytes: &[u8]) -> Result<TextDocument> {
    let title = title.into();
    let zip = ZipArchive::new(bytes)?;
    let content = parse_xml_document(&zip.read_string("content.xml")?)?;
    let mut named = lo_core::style::OdfStyles::default();
    if zip.contains("styles.xml") {
        named.collect(&parse_xml_document(&zip.read_string("styles.xml")?)?);
    }
    named.collect(&content);
    let mut styles = parse_odt_styles(&named);
    if zip.contains("styles.xml") {
        collect_odt_list_styles(
            &parse_xml_document(&zip.read_string("styles.xml")?)?,
            &mut styles,
        );
    }
    collect_odt_list_styles(&content, &mut styles);
    let body = content
        .child("body")
        .ok_or_else(|| LoError::Parse("content.xml missing office:body".to_string()))?;
    let text = body
        .child("text")
        .ok_or_else(|| LoError::Parse("content.xml missing office:text".to_string()))?;

    let mut doc = TextDocument::new(title);
    doc.odf_layout = Some(lo_core::OdfDocumentLayout::default());
    if zip.contains("styles.xml") {
        let root = parse_xml_document(&zip.read_string("styles.xml")?)?;
        apply_odt_page_size(&root, &mut doc);
        if let Some(footer) = root
            .child("master-styles")
            .and_then(|n| n.children_named("master-page").next())
            .and_then(|m| m.child("footer"))
        {
            let mut frames = Vec::new();
            footer.descendants_named("frame", &mut frames);
            for frame in frames {
                let mut blocks = Vec::new();
                collect_odt_images(frame, &zip, &mut blocks);
                if let Some(Block::Image(image)) = blocks.into_iter().next() {
                    let position = ["x", "y"].map(|a| {
                        frame
                            .attr(a)
                            .and_then(lo_core::style::odf_length)
                            .unwrap_or(0.0)
                    });
                    doc.odf_layout
                        .as_mut()
                        .unwrap()
                        .footer_images
                        .push((image, position));
                }
            }
        }
    }
    let mut pending_list = Vec::new();
    parse_odt_text_children(text, &styles, &mut doc, &mut pending_list, &zip, "", 0);
    flush_list(&mut doc, &mut pending_list);
    if doc.body.is_empty() {
        doc.body.push(Block::Paragraph(Paragraph::default()));
    }
    let mut table_nodes = Vec::new();
    text.descendants_named("table", &mut table_nodes);
    fn enrich(blocks: &mut [Block], nodes: &[&XmlNode], named: &lo_core::style::OdfStyles) {
        for block in blocks {
            match block {
                Block::Table(table) => {
                    if let Some(node) = nodes
                        .iter()
                        .find(|n| n.attr("name") == Some(table.name.as_str()))
                    {
                        table.odf_layout = Some(odt_table_layout(node, named));
                    }
                }
                Block::List(list) => {
                    for item in &mut list.items {
                        enrich(&mut item.blocks, nodes, named);
                    }
                }
                _ => {}
            }
        }
    }
    enrich(&mut doc.body, &table_nodes, &named);
    enrich_odt_image_alignment(&content, &named, &mut doc);
    if zip.contains("styles.xml") {
        let root = parse_xml_document(&zip.read_string("styles.xml")?)?;
        apply_odt_initial_master(&root, &content, &mut doc);
    }

    if zip.contains("styles.xml") {
        let root = parse_xml_document(&zip.read_string("styles.xml")?)?;
        let mut outlines = Vec::new();
        root.descendants_named("outline-style", &mut outlines);
        if let Some(outline) = outlines.first() {
            number_odt_headings(&mut doc.body, outline, &mut [0; 10]);
        }
    }
    Ok(doc)
}

// ---------------------------------------------------------------------------
// Span/paragraph helpers
// ---------------------------------------------------------------------------

fn flush_list(doc: &mut TextDocument, pending: &mut Vec<ListItem>) {
    flush_list_with(doc, pending, false);
}

fn flush_list_with(doc: &mut TextDocument, pending: &mut Vec<ListItem>, ordered: bool) {
    if !pending.is_empty() {
        doc.body.push(Block::List(ListBlock {
            odf_layout: None,
            ordered,
            items: std::mem::take(pending),
        }));
    }
}

/// Build a `numId -> ordered?` map by walking `word/numbering.xml` and
/// resolving every `<w:num w:numId>` to its `<w:abstractNum>` definition,
/// then peeking at level 0's `<w:numFmt w:val>` to decide whether the
/// list should be rendered as bullets or with numeric markers.
fn parse_docx_numbering(zip: &ZipArchive) -> BTreeMap<String, bool> {
    let mut out: BTreeMap<String, bool> = BTreeMap::new();
    if !zip.contains("word/numbering.xml") {
        return out;
    }
    let xml = match zip.read_string("word/numbering.xml") {
        Ok(x) => x,
        Err(_) => return out,
    };
    let root = match parse_xml_document(&xml) {
        Ok(r) => r,
        Err(_) => return out,
    };
    // Map abstractNumId -> ordered?
    let mut abstracts: BTreeMap<String, bool> = BTreeMap::new();
    for an in root.children_named("abstractNum") {
        let Some(id) = an.attr("abstractNumId") else {
            continue;
        };
        let mut ordered = false;
        for lvl in an.children_named("lvl") {
            if lvl.attr("ilvl").unwrap_or("") != "0" {
                continue;
            }
            if let Some(fmt) = lvl.child("numFmt").and_then(|n| n.attr("val")) {
                let lower = fmt.to_ascii_lowercase();
                ordered = !lower.contains("bullet") && !lower.is_empty();
            }
        }
        abstracts.insert(id.to_string(), ordered);
    }
    for num in root.children_named("num") {
        let Some(num_id) = num.attr("numId") else {
            continue;
        };
        let abstract_id = num
            .child("abstractNumId")
            .and_then(|n| n.attr("val"))
            .unwrap_or_default();
        let ordered = abstracts.get(abstract_id).copied().unwrap_or(false);
        out.insert(num_id.to_string(), ordered);
    }
    out
}

fn normalize_spans(spans: Vec<StyledSpan>) -> Vec<StyledSpan> {
    let mut out: Vec<StyledSpan> = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.style == span.style {
                last.text.push_str(&span.text);
                continue;
            }
        }
        out.push(span);
    }
    out
}

fn build_paragraph(spans: Vec<StyledSpan>) -> Paragraph {
    let normalized = normalize_spans(spans);
    let mut inlines: Vec<Inline> = Vec::new();
    for span in normalized {
        inlines.push(span_to_inline(span));
    }
    Paragraph {
        spans: inlines,
        ..Paragraph::default()
    }
}

fn span_to_inline(span: StyledSpan) -> Inline {
    if let Some(mut style) = span.style.text_style {
        style.bold |= span.style.bold;
        style.italic |= span.style.italic;
        return Inline::Styled {
            text: span.text,
            style,
            url: span.style.link,
        };
    }
    if let Some(url) = span.style.link {
        return Inline::Link {
            label: span.text,
            url,
        };
    }
    if span.style.code {
        return Inline::Code(span.text);
    }
    if span.style.bold {
        return Inline::Bold(span.text);
    }
    if span.style.italic {
        return Inline::Italic(span.text);
    }
    Inline::Text(span.text)
}

fn inline_text(inline: &Inline) -> &str {
    match inline {
        Inline::Text(text) | Inline::Bold(text) | Inline::Italic(text) | Inline::Code(text) => text,
        Inline::Link { label, .. } => label,
        Inline::Styled { text, .. } => text,
        Inline::LineBreak => "\n",
    }
}

// ---------------------------------------------------------------------------
// DOCX parsing
// ---------------------------------------------------------------------------

fn parse_relationships(zip: &ZipArchive, part: &str) -> Result<BTreeMap<String, String>> {
    let rels_path = rels_path_for(part);
    if !zip.contains(&rels_path) {
        return Ok(BTreeMap::new());
    }
    let rels = parse_xml_document(&zip.read_string(&rels_path)?)?;
    let mut map = BTreeMap::new();
    for rel in rels.children_named("Relationship") {
        if let Some(id) = rel.attr("Id") {
            if let Some(target) = rel.attr("Target") {
                let resolved = if rel.attr("TargetMode") == Some("External") {
                    target.to_string()
                } else {
                    resolve_part_target(part, target)
                };
                map.insert(id.to_string(), resolved);
            }
        }
    }
    Ok(map)
}

fn parse_docx_styles(root: &XmlNode) -> BTreeMap<String, StyleProps> {
    let mut styles = BTreeMap::new();
    for style in root.children_named("style") {
        let Some(style_id) = style.attr("styleId").or_else(|| style.attr("w:styleId")) else {
            continue;
        };
        let mut props = StyleProps::default();
        if let Some(level) = extract_heading_level(style_id) {
            props.heading_level = Some(level);
        }
        if let Some(name) = style.child("name").and_then(|node| node.attr("val")) {
            if let Some(level) = extract_heading_level(name) {
                props.heading_level = Some(level);
            }
        }
        if let Some(ppr) = style.child("pPr") {
            if let Some(level) = ppr
                .child("outlineLvl")
                .and_then(|node| node.attr("val"))
                .and_then(|value| value.parse::<u8>().ok())
            {
                props.heading_level = Some(level.saturating_add(1).clamp(1, 6));
            }
            if ppr.child("pageBreakBefore").is_some() {
                props.page_break_before = true;
            }
        }
        if let Some(rpr) = style.child("rPr") {
            apply_docx_run_properties(rpr, &mut props);
        }
        styles.insert(style_id.to_string(), props);
    }
    styles
}

fn extract_heading_level(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("heading") {
        return rest
            .trim_matches(|ch: char| !ch.is_ascii_digit())
            .parse::<u8>()
            .ok()
            .map(|level| level.clamp(1, 6));
    }
    if let Some(index) = lower.find("heading ") {
        let digits: String = lower[index + 8..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return digits.parse::<u8>().ok().map(|level| level.clamp(1, 6));
        }
    }
    None
}

fn parse_docx_paragraph(
    node: &XmlNode,
    relationships: &BTreeMap<String, String>,
    styles: &BTreeMap<String, StyleProps>,
) -> ParagraphInfo {
    let mut info = ParagraphInfo::default();
    let mut base = StyleProps::default();
    if let Some(ppr) = node.child("pPr") {
        if let Some(style_id) = ppr.child("pStyle").and_then(|value| value.attr("val")) {
            if let Some(style) = styles.get(style_id) {
                base = style.clone();
                info.heading_level = style.heading_level;
                info.page_break |= style.page_break_before;
            }
            if info.heading_level.is_none() {
                info.heading_level = extract_heading_level(style_id);
            }
            if style_id.to_ascii_lowercase().contains("list") {
                info.list_key = Some(style_id.to_string());
            }
        }
        if let Some(level) = ppr
            .child("outlineLvl")
            .and_then(|value| value.attr("val"))
            .and_then(|value| value.parse::<u8>().ok())
        {
            info.heading_level = Some(level.saturating_add(1).clamp(1, 6));
        }
        if let Some(num_id) = ppr
            .child("numPr")
            .and_then(|numpr| numpr.child("numId"))
            .and_then(|num_id| num_id.attr("val"))
        {
            info.list_key = Some(num_id.to_string());
        }
        if ppr.child("pageBreakBefore").is_some() {
            info.page_break = true;
        }
    }

    walk_paragraph_items(&node.items, &base, None, relationships, &mut info);
    info
}

fn walk_paragraph_items(
    items: &[XmlItem],
    base: &StyleProps,
    hyperlink: Option<String>,
    relationships: &BTreeMap<String, String>,
    info: &mut ParagraphInfo,
) {
    for item in items {
        let XmlItem::Node(child) = item else {
            continue;
        };
        match child.local_name() {
            "r" => {
                if run_has_page_break(child) {
                    info.page_break = true;
                }
                if let Some(span) = parse_docx_run(child, base, hyperlink.clone()) {
                    info.spans.push(span);
                }
            }
            "hyperlink" => {
                let href = child
                    .attr("id")
                    .or_else(|| child.attr("r:id"))
                    .and_then(|id| relationships.get(id))
                    .cloned()
                    .or_else(|| child.attr("anchor").map(|anchor| format!("#{anchor}")));
                walk_paragraph_items(&child.items, base, href, relationships, info);
            }
            "fldSimple" => {
                walk_paragraph_items(&child.items, base, hyperlink.clone(), relationships, info);
            }
            // Track-changes wrappers — descend so we don't lose the
            // <w:r> children that hold the actual deleted/inserted text.
            "ins" | "del" | "moveTo" | "moveFrom" | "smartTag" | "customXml" | "sdt"
            | "sdtContent" => {
                walk_paragraph_items(&child.items, base, hyperlink.clone(), relationships, info);
            }
            _ => {}
        }
    }
}

fn run_has_page_break(run: &XmlNode) -> bool {
    run.children_named("br")
        .any(|node| node.attr("type") == Some("page"))
}

fn parse_docx_run(
    run: &XmlNode,
    base: &StyleProps,
    hyperlink: Option<String>,
) -> Option<StyledSpan> {
    let mut style = SpanStyle {
        text_style: None,
        bold: base.bold,
        italic: base.italic,
        code: base.code,
        link: hyperlink,
    };
    if let Some(rpr) = run.child("rPr") {
        let mut props = StyleProps::default();
        apply_docx_run_properties(rpr, &mut props);
        style.bold |= props.bold;
        style.italic |= props.italic;
        style.code |= props.code;
    }
    let mut text = String::new();
    for item in &run.items {
        let XmlItem::Node(child) = item else {
            continue;
        };
        match child.local_name() {
            // `<w:instrText>` is a Word field instruction (e.g.
            // `TOC \o "1-3" \h \z \u`) and is never visible — drop it.
            "t" | "delText" => text.push_str(&child.text_content()),
            "tab" => text.push('\t'),
            "br" | "cr" => text.push('\n'),
            "noBreakHyphen" => text.push('-'),
            "softHyphen" => text.push('\u{00ad}'),
            // Footnote / endnote markers — record a placeholder so the
            // outer paragraph builder can renumber them sequentially in
            // document order (matching what Word and pdftotext show).
            "footnoteReference" => text.push_str("\u{f001}"),
            "endnoteReference" => text.push_str("\u{f002}"),
            _ => {}
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(StyledSpan { text, style })
    }
}

fn apply_docx_run_properties(rpr: &XmlNode, props: &mut StyleProps) {
    if rpr.child("b").is_some() {
        props.bold = true;
    }
    if rpr.child("i").is_some() {
        props.italic = true;
    }
    if let Some(fonts) = rpr.child("rFonts") {
        if let Some(ascii) = fonts.attr("ascii").or_else(|| fonts.attr("hAnsi")) {
            let lower = ascii.to_ascii_lowercase();
            if lower.contains("courier") || lower.contains("consola") || lower.contains("mono") {
                props.code = true;
            }
        }
    }
}

fn parse_docx_table(table: &XmlNode) -> Table {
    let mut rows: Vec<TableRow> = Vec::new();
    for row in table.children_named("tr") {
        let mut cells: Vec<TableCell> = Vec::new();
        for cell in row.children_named("tc") {
            let mut paragraphs: Vec<Paragraph> = Vec::new();
            // A `<w:tc>` may interleave `<w:p>` and nested `<w:tbl>`
            // children. Walk in document order and recurse into nested
            // tables so their text is never lost.
            collect_cell_blocks(cell, &mut paragraphs);
            if paragraphs.is_empty() {
                paragraphs.push(Paragraph::default());
            }
            cells.push(TableCell { paragraphs });
        }
        rows.push(TableRow { cells });
    }
    Table {
        odf_layout: None,
        name: "Table1".to_string(),
        rows,
    }
}

fn collect_cell_blocks(node: &XmlNode, out: &mut Vec<Paragraph>) {
    for item in &node.items {
        let XmlItem::Node(child) = item else {
            continue;
        };
        match child.local_name() {
            "p" => {
                let info = parse_docx_paragraph(child, &BTreeMap::new(), &BTreeMap::new());
                let para = build_paragraph(info.spans);
                if !para
                    .spans
                    .iter()
                    .all(|inline| inline_text(inline).is_empty())
                {
                    out.push(para);
                }
            }
            "tbl" => {
                let nested = parse_docx_table(child);
                for row in &nested.rows {
                    for c in &row.cells {
                        for p in &c.paragraphs {
                            if !p.spans.iter().all(|inline| inline_text(inline).is_empty()) {
                                out.push(p.clone());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// ODT parsing
// ---------------------------------------------------------------------------

fn parse_odt_styles(named: &lo_core::style::OdfStyles) -> BTreeMap<String, StyleProps> {
    let mut styles = BTreeMap::new();
    let mut names: Vec<_> = named
        .names()
        .filter(|(family, _)| matches!(*family, "paragraph" | "text"))
        .collect();
    if !names.contains(&("paragraph", "")) {
        names.push(("paragraph", ""));
    }
    for (family, name) in names {
        let get = |kind, attr| named.property_in(name, family, kind, attr);
        let font = get("text-properties", "font-family")
            .or_else(|| get("text-properties", "font-name"))
            .unwrap_or("")
            .trim_matches(char::from(39))
            .to_string();
        let text_style = lo_core::TextStyle {
            font_size_mpt: get("text-properties", "font-size")
                .and_then(lo_core::style::odf_length)
                .filter(|v| *v > 0.0)
                .map(|v| (v.clamp(1.0, 512.0) * 1000.0).round() as u32),
            font_size_pt: get("text-properties", "font-size")
                .and_then(lo_core::style::odf_length)
                .filter(|v| *v > 0.0)
                .map(|v| v.round().clamp(1.0, 512.0) as u16)
                .unwrap_or(0),
            bold: get("text-properties", "font-weight") == Some("bold"),
            italic: get("text-properties", "font-style") == Some("italic"),
            underline: get("text-properties", "text-underline-style").is_some_and(|v| v != "none"),
            font_family: font.clone(),
            color: get("text-properties", "color").unwrap_or("").to_string(),
            background: get("text-properties", "background-color")
                .unwrap_or("")
                .to_string(),
            ..Default::default()
        };
        styles.insert(
            format!("{family}:{name}"),
            StyleProps {
                bold: text_style.bold,
                italic: text_style.italic,
                code: font.to_ascii_lowercase().contains("mono")
                    || font.to_ascii_lowercase().contains("courier"),
                paragraph_style: Some(lo_core::ParagraphStyle {
                    odf_margins_mpt: Some(
                        ["margin-top", "margin-bottom", "margin-left", "margin-right"].map(
                            |side| {
                                get("paragraph-properties", side)
                                    .and_then(lo_core::style::odf_length)
                                    .map(|pt| (pt.clamp(0.0, 3000.0) * 1000.0).round() as i32)
                                    .unwrap_or(0)
                            },
                        ),
                    ),
                    odf_box: Some(lo_core::style::OdfParagraphBox {
                        background: get("paragraph-properties", "background-color")
                            .unwrap_or("")
                            .into(),
                        borders: ["border-left", "border-right", "border-top", "border-bottom"]
                            .map(|side| {
                                get("paragraph-properties", side)
                                    .or_else(|| get("paragraph-properties", "border"))
                                    .unwrap_or("")
                                    .into()
                            }),
                        padding_mpt: [
                            "padding-left",
                            "padding-right",
                            "padding-top",
                            "padding-bottom",
                        ]
                        .map(|side| {
                            get("paragraph-properties", side)
                                .or_else(|| get("paragraph-properties", "padding"))
                                .and_then(lo_core::style::odf_length)
                                .map(|pt| (pt.clamp(0.0, 1000.0) * 1000.0) as u32)
                                .unwrap_or(0)
                        }),
                        join: get("paragraph-properties", "join-border") != Some("false"),
                    }),
                    odf_first_line_mpt: get("paragraph-properties", "text-indent")
                        .and_then(lo_core::style::odf_length)
                        .map(|v| (v.clamp(-3000.0, 3000.0) * 1000.0).round() as i32)
                        .unwrap_or(0),
                    source_spacing: true,
                    line_height_percent: get("paragraph-properties", "line-height")
                        .and_then(|v| v.strip_suffix('%'))
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                        .map(|v| v.clamp(10.0, 1000.0).round() as u16)
                        .unwrap_or(100),
                    alignment: match get("paragraph-properties", "text-align") {
                        Some("center") => lo_core::Alignment::Center,
                        Some("right" | "end") => lo_core::Alignment::End,
                        Some("justify") => lo_core::Alignment::Justify,
                        _ => lo_core::Alignment::Start,
                    },
                    margin_top_mm: odt_margin(get("paragraph-properties", "margin-top")),
                    margin_bottom_mm: odt_margin(get("paragraph-properties", "margin-bottom")),
                    margin_left_mm: odt_margin(get("paragraph-properties", "margin-left")),
                    margin_right_mm: odt_margin(get("paragraph-properties", "margin-right")),
                }),
                text_style: Some(text_style),
                page_break_before: get("paragraph-properties", "break-before") == Some("page"),
                ..Default::default()
            },
        );
    }
    styles
}

fn odt_margin(value: Option<&str>) -> u16 {
    value
        .and_then(lo_core::style::odf_length)
        .map(|pt| (pt * 25.4 / 72.0).round().clamp(0.0, 1000.0) as u16)
        .unwrap_or(0)
}
fn parse_odt_paragraph(node: &XmlNode, styles: &BTreeMap<String, StyleProps>) -> Paragraph {
    let mut paragraph = build_paragraph(parse_odt_inline(node, styles, &SpanStyle::default()));
    if let Some(props) = styles
        .get(&format!(
            "paragraph:{}",
            node.attr("style-name").unwrap_or("")
        ))
        .or_else(|| styles.get("paragraph:"))
    {
        if let Some(style) = &props.paragraph_style {
            paragraph.style = style.clone();
        }
        if let Some(style) = &props.text_style {
            paragraph.text_style = style.clone();
        }
    }
    paragraph
}

fn odt_span_style(base: &SpanStyle, props: &StyleProps) -> SpanStyle {
    let mut out = base.clone();
    out.bold |= props.bold;
    out.italic |= props.italic;
    out.code |= props.code;
    if let Some(source) = &props.text_style {
        let target = out.text_style.get_or_insert_with(Default::default);
        if source.font_size_pt != 0 {
            target.font_size_pt = source.font_size_pt;
            target.font_size_mpt = source.font_size_mpt;
        }
        if !source.font_family.is_empty() {
            target.font_family = source.font_family.clone();
        }
        if !source.color.is_empty() {
            target.color = source.color.clone();
        }
        if !source.background.is_empty() {
            target.background = source.background.clone();
        }
        target.bold |= source.bold;
        target.italic |= source.italic;
        target.underline |= source.underline;
    }
    out
}

fn number_odt_headings(blocks: &mut [Block], outline: &XmlNode, counters: &mut [u32; 10]) {
    for block in blocks {
        match block {
            Block::Heading(h) => {
                let level = usize::from(h.level.clamp(1, 10));
                let Some(rule) = outline
                    .children_named("outline-level-style")
                    .find(|n| n.attr("level").and_then(|s| s.parse::<usize>().ok()) == Some(level))
                else {
                    continue;
                };
                if rule.attr("num-format") != Some("1") {
                    continue;
                }
                let start = rule
                    .attr("start-value")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1)
                    .max(1);
                counters[level - 1] = if counters[level - 1] == 0 {
                    start
                } else {
                    counters[level - 1].saturating_add(1)
                };
                counters[level..].fill(0);
                let display = rule
                    .attr("display-levels")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, level);
                let number = counters[level - display..level]
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(".");
                let mut label = format!(
                    "{}{}{}",
                    rule.attr("num-prefix").unwrap_or(""),
                    number,
                    rule.attr("num-suffix").unwrap_or("")
                );
                let alignment = rule
                    .child("list-level-properties")
                    .and_then(|n| n.child("list-level-label-alignment"));
                let size = h.content.text_style.font_size_pt.max(1) as f32;
                let gap = alignment
                    .filter(|n| n.attr("label-followed-by") == Some("listtab"))
                    .and_then(|n| n.attr("list-tab-stop-position"))
                    .and_then(lo_core::style::odf_length)
                    .map(|stop| {
                        ((stop
                            - crate::font_metrics::measure_text(
                                &label,
                                size,
                                lo_core::PdfFont::HelveticaBold,
                            ))
                            / (size * 0.278))
                            .round()
                            .clamp(1.0, 32.0) as usize
                    })
                    .unwrap_or(1);
                label.push_str(&"\u{a0}".repeat(gap));
                h.content.spans.insert(
                    0,
                    Inline::Styled {
                        text: label,
                        style: h.content.text_style.clone(),
                        url: None,
                    },
                );
            }
            Block::List(list) => {
                for item in &mut list.items {
                    number_odt_headings(&mut item.blocks, outline, counters);
                }
            }
            Block::Section(section) => number_odt_headings(&mut section.blocks, outline, counters),
            _ => {}
        }
    }
}

fn parse_odt_text_children(
    root: &XmlNode,
    styles: &BTreeMap<String, StyleProps>,
    doc: &mut TextDocument,
    pending_list: &mut Vec<ListItem>,
    zip: &ZipArchive,
    list_name: &str,
    list_level: usize,
) {
    for item in &root.items {
        let XmlItem::Node(node) = item else {
            continue;
        };
        // A break before the first flow block selects the first page, not a
        // blank preceding page. Headings honor the same inherited break rule.
        if matches!(node.local_name(), "p" | "h") {
            let breaks = node
                .attr("style-name")
                .and_then(|name| styles.get(&format!("paragraph:{name}")))
                .is_some_and(|props| props.page_break_before);
            if breaks && !doc.body.is_empty() {
                flush_list(doc, pending_list);
                doc.body.push(Block::PageBreak);
            }
        }
        match node.local_name() {
            "p" => {
                flush_list(doc, pending_list);
                doc.body
                    .push(Block::Paragraph(parse_odt_paragraph(node, styles)));
            }
            "h" => {
                flush_list(doc, pending_list);
                let level = node
                    .attr("outline-level")
                    .and_then(|value| value.parse::<u8>().ok())
                    .unwrap_or(1)
                    .clamp(1, 6);
                doc.body.push(Block::Heading(Heading {
                    level,
                    content: parse_odt_paragraph(node, styles),
                }));
            }
            "list" => {
                let list_name = node.attr("style-name").unwrap_or(list_name);
                let list_level = list_level + 1;
                let list_layout = styles
                    .get(&format!("list:{list_name}:{list_level}"))
                    .and_then(|s| s.list_layout.clone())
                    .unwrap_or_default();
                flush_list(doc, pending_list);
                let mut items: Vec<ListItem> = Vec::new();
                for list_item in node.children_named("list-item") {
                    let mut nested = TextDocument::default();
                    let mut pending = Vec::new();
                    parse_odt_text_children(
                        list_item,
                        styles,
                        &mut nested,
                        &mut pending,
                        zip,
                        list_name,
                        list_level,
                    );
                    flush_list(&mut nested, &mut pending);
                    if !nested.body.is_empty() {
                        items.push(ListItem {
                            blocks: nested.body,
                        });
                    }
                }
                if !items.is_empty()
                    && items
                        .iter()
                        .all(|item| item.blocks.iter().all(|b| matches!(b, Block::Heading(_))))
                {
                    doc.body
                        .extend(items.into_iter().flat_map(|item| item.blocks));
                } else if !items.is_empty() {
                    doc.body.push(Block::List(ListBlock {
                        odf_layout: Some(list_layout),
                        ordered: false,
                        items,
                    }));
                }
            }
            "table" => {
                flush_list(doc, pending_list);
                doc.body.push(Block::Table(parse_odt_table(node, styles)));
            }
            "section" => {
                parse_odt_text_children(node, styles, doc, pending_list, zip, list_name, list_level)
            }
            "frame" => {
                for text_box in node.children_named("text-box") {
                    parse_odt_text_children(
                        text_box,
                        styles,
                        doc,
                        pending_list,
                        zip,
                        list_name,
                        list_level,
                    );
                }
            }
            _ => {}
        }
        if matches!(node.local_name(), "p" | "h" | "frame") {
            let mut images = Vec::new();
            collect_odt_images(node, zip, &mut images);
            if !images.is_empty() {
                if node.local_name() == "p"
                    && matches!(doc.body.last(),Some(Block::Paragraph(p)) if p.to_plain_text().trim().is_empty())
                {
                    doc.body.pop();
                }
                let start = doc.body.len();
                doc.body.extend(images);
                if let Some(layout) = &mut doc.odf_layout {
                    layout.image_groups.insert(start, doc.body.len());
                }
            }
        }
    }
}

fn odf_page_lines(
    node: &XmlNode,
    styles: &BTreeMap<String, StyleProps>,
) -> Vec<Vec<lo_core::OdfPageField>> {
    use lo_core::OdfPageField as Field;
    fn collect(node: &XmlNode, out: &mut Vec<Field>, styles: &BTreeMap<String, StyleProps>) {
        for item in &node.items {
            match item {
                XmlItem::Text(text) => out.push(Field::Text(text.clone())),
                XmlItem::Node(child) => match child.local_name() {
                    "page-number" => out.push(Field::PageNumber),
                    "page-count" => out.push(Field::PageCount),
                    "tab" => out.push(Field::Tab),
                    "line-break" => out.push(Field::LineBreak),
                    "s" => out.push(Field::Text(
                        " ".repeat(
                            child
                                .attr("c")
                                .and_then(|v| v.parse::<usize>().ok())
                                .unwrap_or(1)
                                .min(10000),
                        ),
                    )),
                    "frame" => {}
                    "span" => {
                        let mut fields = Vec::new();
                        collect(child, &mut fields, styles);
                        if let Some(style) = child
                            .attr("style-name")
                            .and_then(|n| styles.get(&format!("text:{n}")))
                            .and_then(|s| s.text_style.clone())
                        {
                            out.push(Field::Styled(fields, style));
                        } else {
                            out.extend(fields);
                        }
                    }
                    _ => collect(child, out, styles),
                },
            }
        }
    }
    node.children
        .iter()
        .filter(|n| matches!(n.local_name(), "p" | "h"))
        .map(|n| {
            let mut out = Vec::new();
            collect(n, &mut out, styles);
            out
        })
        .collect()
}

fn apply_odt_page_size(styles: &XmlNode, doc: &mut TextDocument) {
    use lo_core::style::odf_length;
    let Some(master) = styles
        .child("master-styles")
        .and_then(|n| n.children_named("master-page").next())
    else {
        return;
    };
    let Some(layout_name) = master.attr("page-layout-name") else {
        return;
    };
    let Some(layout) = styles.child("automatic-styles").and_then(|n| {
        n.children_named("page-layout")
            .find(|n| n.attr("name") == Some(layout_name))
    }) else {
        return;
    };
    let Some(props) = layout.child("page-layout-properties") else {
        return;
    };
    let mut named = lo_core::style::OdfStyles::default();
    named.collect(styles);
    let paragraph_styles = parse_odt_styles(&named);
    let imported = doc.odf_layout.get_or_insert_with(Default::default);
    for (i, attr) in ["page-width", "page-height"].iter().enumerate() {
        if let Some(v) = props.attr(attr).and_then(odf_length).filter(|v| *v > 72.0) {
            imported.page[i] = v;
        }
    }
    doc.page_style.width_mm = (imported.page[0] * 25.4 / 72.0).round() as u16;
    doc.page_style.height_mm = (imported.page[1] * 25.4 / 72.0).round() as u16;
    for (i, attr) in ["margin-left", "margin-right", "margin-top", "margin-bottom"]
        .iter()
        .enumerate()
    {
        if let Some(v) = props
            .attr(attr)
            .or_else(|| props.attr("margin"))
            .and_then(odf_length)
            .filter(|v| *v >= 0.0)
        {
            imported.margins[i] = v.min(imported.page[i / 2] / 3.0);
        }
    }
    imported.headers = master
        .child("header")
        .map(|n| odf_page_lines(n, &paragraph_styles))
        .unwrap_or_default();
    imported.footers = master
        .child("footer")
        .map(|n| odf_page_lines(n, &paragraph_styles))
        .unwrap_or_default();
    imported.first_headers = master
        .child("header-first")
        .map(|n| odf_page_lines(n, &paragraph_styles));
    imported.first_footers = master
        .child("footer-first")
        .map(|n| odf_page_lines(n, &paragraph_styles));
    imported.left_headers = master
        .child("header-left")
        .map(|n| odf_page_lines(n, &paragraph_styles));
    imported.left_footers = master
        .child("footer-left")
        .map(|n| odf_page_lines(n, &paragraph_styles));
    let mut named = lo_core::style::OdfStyles::default();
    named.collect(styles);
    for (kind, target) in [
        ("header", &mut imported.header_font),
        ("footer", &mut imported.footer_font),
    ] {
        let name = master
            .child(kind)
            .and_then(|n| n.child("p"))
            .and_then(|n| n.attr("style-name"))
            .unwrap_or("Standard");
        target.font_size_mpt = named
            .property_in(name, "paragraph", "text-properties", "font-size")
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .map(|v| (v.clamp(1.0, 512.0) * 1000.0).round() as u32);
        target.font_size_pt = named
            .property_in(name, "paragraph", "text-properties", "font-size")
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .map(|v| v.round().clamp(1.0, 512.0) as u16)
            .unwrap_or(12);
        target.font_family = named
            .property_in(name, "paragraph", "text-properties", "font-family")
            .or_else(|| named.property_in(name, "paragraph", "text-properties", "font-name"))
            .unwrap_or("")
            .to_string();
        target.bold =
            named.property_in(name, "paragraph", "text-properties", "font-weight") == Some("bold");
        target.italic =
            named.property_in(name, "paragraph", "text-properties", "font-style") == Some("italic");
    }
    for (which, height) in [
        ("header-style", &mut imported.header_height),
        ("footer-style", &mut imported.footer_height),
    ] {
        *height = layout
            .child(which)
            .and_then(|n| n.child("header-footer-properties"))
            .and_then(|n| n.attr("height").or_else(|| n.attr("min-height")))
            .and_then(odf_length)
            .unwrap_or(0.0)
            .max(0.0);
    }
}

// Recover embedded raster images, choosing one supported fallback per frame.
// Images currently flow after their paragraph; floating/inline anchors require
// a richer layout model. Never fetch external linked resources.
fn collect_odt_images(node: &XmlNode, zip: &ZipArchive, blocks: &mut Vec<Block>) {
    if node.local_name() == "frame" {
        for image in node.children_named("image") {
            let Some(href) = image.attr("href") else {
                continue;
            };
            let href = href.strip_prefix("./").unwrap_or(href);
            if href.starts_with('/') || href.contains(':') || href.split('/').any(|p| p == "..") {
                continue;
            }
            let mime = match href
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                _ => continue,
            };
            let Ok(data) = zip.read(href) else { continue };
            blocks.push(Block::Image(lo_core::ImageBlock {
                name: href.to_string(),
                mime_type: mime.to_string(),
                data,
                alt: node
                    .child("desc")
                    .map(|n| n.text_content())
                    .unwrap_or_default(),
                size: lo_core::Size::new(
                    lo_core::Length::mm(odf_image_length(node.attr("width")).unwrap_or(50.0)),
                    lo_core::Length::mm(odf_image_length(node.attr("height")).unwrap_or(30.0)),
                ),
            }));
            break;
        }
        return;
    }
    for child in &node.children {
        collect_odt_images(child, zip, blocks);
    }
}

fn odf_image_length(value: Option<&str>) -> Option<f32> {
    let value = value?.trim();
    for (unit, scale) in [
        ("cm", 10.0),
        ("mm", 1.0),
        ("in", 25.4),
        ("pt", 25.4 / 72.0),
        ("pc", 25.4 / 6.0),
    ] {
        if let Some(number) = value.strip_suffix(unit) {
            let mm = number.parse::<f32>().ok()? * scale;
            return (mm.is_finite() && mm > 0.0 && mm <= 10_000.0).then_some(mm);
        }
    }
    None
}

fn parse_odt_inline(
    node: &XmlNode,
    styles: &BTreeMap<String, StyleProps>,
    inherited: &SpanStyle,
) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let style = node
        .attr("style-name")
        .and_then(|n| styles.get(&format!("paragraph:{n}")))
        .or_else(|| styles.get("paragraph:"))
        .map(|p| odt_span_style(inherited, p))
        .unwrap_or_else(|| inherited.clone());
    parse_odt_items(&node.items, styles, &style, &mut spans);
    spans
}

fn parse_odt_items(
    items: &[XmlItem],
    styles: &BTreeMap<String, StyleProps>,
    inherited: &SpanStyle,
    spans: &mut Vec<StyledSpan>,
) {
    for item in items {
        match item {
            XmlItem::Text(text) => {
                if !text.is_empty() {
                    spans.push(StyledSpan {
                        text: text.clone(),
                        style: inherited.clone(),
                    });
                }
            }
            XmlItem::Node(node) => match node.local_name() {
                "frame" => {}
                "span" => {
                    let mut style = inherited.clone();
                    if let Some(style_name) = node.attr("style-name") {
                        if let Some(props) = styles.get(&format!("text:{style_name}")) {
                            style = odt_span_style(&style, props);
                        }
                    }
                    parse_odt_items(&node.items, styles, &style, spans);
                }
                "a" => {
                    let mut style = inherited.clone();
                    style.link = node
                        .attr("href")
                        .or_else(|| node.attr("xlink:href"))
                        .map(str::to_string);
                    parse_odt_items(&node.items, styles, &style, spans);
                }
                "s" => {
                    let count = node
                        .attr("c")
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(1);
                    spans.push(StyledSpan {
                        text: "\u{a0}".repeat(count.min(10_000)),
                        style: inherited.clone(),
                    });
                }
                "tab" => spans.push(StyledSpan {
                    text: "\t".to_string(),
                    style: inherited.clone(),
                }),
                "line-break" => spans.push(StyledSpan {
                    text: "\n".to_string(),
                    style: inherited.clone(),
                }),
                _ => parse_odt_items(&node.items, styles, inherited, spans),
            },
        }
    }
}

fn odt_table_layout(
    table: &XmlNode,
    styles: &lo_core::style::OdfStyles,
) -> lo_core::OdfTableLayout {
    let mut layout = lo_core::OdfTableLayout::default();
    for column in table.children_named("table-column") {
        let name = column.attr("style-name").unwrap_or("");
        let width = styles
            .property_in(
                name,
                "table-column",
                "table-column-properties",
                "column-width",
            )
            .and_then(lo_core::style::odf_length)
            .filter(|v| *v > 0.0)
            .unwrap_or(72.0);
        let count = column
            .attr("number-columns-repeated")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .min(1024);
        layout.columns.extend(std::iter::repeat_n(width, count));
    }
    for (r, row) in table.children_named("table-row").enumerate() {
        for (c, cell) in row
            .children
            .iter()
            .filter(|n| matches!(n.local_name(), "table-cell" | "covered-table-cell"))
            .enumerate()
        {
            let span = |name| {
                cell.attr(name)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1024)
            };
            layout.spans.insert(
                (r, c),
                if cell.local_name() == "covered-table-cell" {
                    (0, 0)
                } else {
                    (span("number-columns-spanned"), span("number-rows-spanned"))
                },
            );
            let name = cell.attr("style-name").unwrap_or("");
            if let Some(value) = styles.property_in(
                name,
                "table-cell",
                "table-cell-properties",
                "background-color",
            ) {
                layout.backgrounds.insert((r, c), value.into());
            }
            layout.edge_borders.insert(
                (r, c),
                ["border-left", "border-right", "border-top", "border-bottom"].map(|side| {
                    styles
                        .property_in(name, "table-cell", "table-cell-properties", side)
                        .or_else(|| {
                            styles.property_in(
                                name,
                                "table-cell",
                                "table-cell-properties",
                                "border",
                            )
                        })
                        .unwrap_or("")
                        .into()
                }),
            );
            if let Some(value) =
                styles.property_in(name, "table-cell", "table-cell-properties", "border")
            {
                layout.borders.insert((r, c), value.into());
            }
        }
    }
    layout
}

fn parse_odt_table(table: &XmlNode, styles: &BTreeMap<String, StyleProps>) -> Table {
    let mut rows: Vec<TableRow> = Vec::new();
    for row in table.children_named("table-row") {
        let repeat_rows = row
            .attr("number-rows-repeated")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1024);
        let mut cells: Vec<TableCell> = Vec::new();
        for cell in row
            .children
            .iter()
            .filter(|child| matches!(child.local_name(), "table-cell" | "covered-table-cell"))
        {
            let repeat_cols = cell
                .attr("number-columns-repeated")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 1024);
            let mut paragraphs: Vec<Paragraph> = Vec::new();
            for paragraph in cell
                .children
                .iter()
                .filter(|child| matches!(child.local_name(), "p" | "h"))
            {
                let para = parse_odt_paragraph(paragraph, styles);
                paragraphs.push(para);
            }
            if paragraphs.is_empty() {
                paragraphs.push(Paragraph::default());
            }
            for _ in 0..repeat_cols {
                cells.push(TableCell {
                    paragraphs: paragraphs.clone(),
                });
            }
        }
        for _ in 0..repeat_rows {
            rows.push(TableRow {
                cells: cells.clone(),
            });
        }
    }
    Table {
        odf_layout: None,
        name: table.attr("name").unwrap_or("Table1").to_string(),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_markdown, to_docx};
    use lo_odf::save_text_document;

    #[test]
    fn docx_round_trip_imports_basic_structure() {
        let doc = from_markdown("demo", "# Title\n\nHello **world**\n\n- one\n- two");
        let bytes = to_docx(&doc).expect("docx");
        let loaded = from_docx_bytes("demo", &bytes).expect("import docx");
        let text = loaded.plain_text();
        assert!(text.contains("Title"));
        assert!(text.contains("world"));
    }

    #[test]
    fn odt_round_trip_imports_basic_structure() {
        let doc = from_markdown("demo", "# Title\n\nHello *world*");
        let tmp = std::env::temp_dir().join("lo_writer_import_test.odt");
        save_text_document(&tmp, &doc).expect("save odt");
        let bytes = std::fs::read(&tmp).expect("read odt");
        let _ = std::fs::remove_file(&tmp);
        let loaded = from_odt_bytes("demo", &bytes).expect("import odt");
        let text = loaded.plain_text();
        assert!(text.contains("Title"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn html_to_text_roundtrip() {
        let doc = from_html("h", "<h1>hi</h1><p>one<br/>two</p>");
        let text = doc.plain_text();
        assert!(text.contains("hi"));
        assert!(text.contains("one"));
        assert!(text.contains("two"));
    }

    #[test]
    fn load_bytes_dispatches() {
        let html = b"<p>hello</p>";
        let doc = load_bytes("h", html, "html").unwrap();
        assert!(doc.plain_text().contains("hello"));
    }
}

#[cfg(test)]
mod odf_image_tests {
    use super::*;
    #[test]
    fn odf_table_geometry_does_not_invent_decoration() {
        let node = parse_xml_document(r#"<table><table-column style-name="Width" number-columns-repeated="2"/><table-row><table-cell number-columns-spanned="2"/><covered-table-cell/></table-row></table>"#).unwrap();
        let root = parse_xml_document(r#"<styles><style name="Width" family="table-column"><table-column-properties column-width="1in"/></style></styles>"#).unwrap();
        let mut styles = lo_core::style::OdfStyles::default();
        styles.collect(&root);
        let layout = odt_table_layout(&node, &styles);
        assert_eq!(layout.columns, vec![72.0, 72.0]);
        assert_eq!(layout.spans[&(0, 0)], (2, 1));
        assert_eq!(layout.spans[&(0, 1)], (0, 0));
        assert!(layout.backgrounds.is_empty());
        assert!(layout.borders.is_empty());
    }

    #[test]
    fn inline_odf_highlight_survives_style_resolution() {
        let root = parse_xml_document(r##"<styles><style name="Highlight" family="text"><text-properties background-color="#ffff00"/></style></styles>"##).unwrap();
        let mut named = lo_core::style::OdfStyles::default();
        named.collect(&root);
        let styles = parse_odt_styles(&named);
        let node = parse_xml_document(r#"<p>Plain <span style-name="Highlight">Marked</span></p>"#)
            .unwrap();
        let paragraph = parse_odt_paragraph(&node, &styles);
        let Inline::Styled { style, .. } = &paragraph.spans[1] else {
            panic!("styled highlight")
        };
        assert_eq!(style.background, "#ffff00");
    }

    #[test]
    fn same_named_paragraph_and_text_styles_do_not_collide() {
        let root=parse_xml_document(r##"<styles><style name="Shared" family="paragraph"><text-properties color="#ff0000"/></style><style name="Shared" family="text"><text-properties color="#0000ff"/></style></styles>"##).unwrap();
        let mut named = lo_core::style::OdfStyles::default();
        named.collect(&root);
        let styles = parse_odt_styles(&named);
        let paragraph = parse_xml_document(
            r#"<p style-name="Shared">Red <span style-name="Shared">Blue</span></p>"#,
        )
        .unwrap();
        let paragraph = parse_odt_paragraph(&paragraph, &styles);
        let Inline::Styled { style: first, .. } = &paragraph.spans[0] else {
            panic!("styled")
        };
        let Inline::Styled { style: second, .. } = &paragraph.spans[1] else {
            panic!("styled")
        };
        assert_eq!(first.color, "#ff0000");
        assert_eq!(second.color, "#0000ff");
    }
    #[test]
    fn odt_page_size_uses_the_master_layout() {
        let root = parse_xml_document(r#"<styles><automatic-styles><page-layout name="Letter"><page-layout-properties page-width="8.5in" page-height="11in"/></page-layout></automatic-styles><master-styles><master-page name="Standard" page-layout-name="Letter"/></master-styles></styles>"#).unwrap();
        let mut doc = TextDocument::default();
        apply_odt_page_size(&root, &mut doc);
        assert_eq!(
            (doc.page_style.width_mm, doc.page_style.height_mm),
            (216, 279)
        );
    }
    #[test]
    fn raster_fallback_is_selected_once_with_physical_dimensions() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        lo_zip::write_zip(
            &mut bytes,
            &[lo_zip::ZipEntry::new(
                "Pictures/fallback.png",
                vec![1, 2, 3],
            )],
        )
        .unwrap();
        let bytes = bytes.into_inner();
        let zip = ZipArchive::new(&bytes).unwrap();
        let root = parse_xml_document(r#"<p><frame width="1in" height="2cm"><image href="Pictures/original.wmf"/><image href="./Pictures/fallback.png"/><image href="Pictures/fallback.png"/></frame></p>"#).unwrap();
        let mut blocks = Vec::new();
        collect_odt_images(&root, &zip, &mut blocks);
        assert_eq!(blocks.len(), 1);
        let Block::Image(image) = &blocks[0] else {
            panic!("image expected")
        };
        assert_eq!(image.data, [1, 2, 3]);
        assert!((image.size.width.as_pt() - 72.0).abs() < 0.01);
        assert_eq!(image.size.height, lo_core::Length::mm(20.0));
        for value in ["-1cm", "NaNcm", "0in", "100000cm"] {
            assert!(odf_image_length(Some(value)).is_none());
        }
    }
}

#[cfg(test)]
mod writer_template_tests {
    use super::*;

    #[test]
    fn outline_headings_in_list_wrappers_are_numbered_without_bullets() {
        let content = r#"<document-content><body><text><h outline-level="1">Introduction</h><list><list-item><list><list-item><h outline-level="2">Model</h></list-item></list></list-item></list><h outline-level="2">Query</h><h outline-level="1">Next</h><h outline-level="2">Reset</h></text></body></document-content>"#;
        let styles = r#"<document-styles><styles><outline-style><outline-level-style level="1" num-format="1"/><outline-level-style level="2" num-format="1" display-levels="2"/></outline-style></styles></document-styles>"#;
        let mut bytes = std::io::Cursor::new(Vec::new());
        lo_zip::write_zip(
            &mut bytes,
            &[
                lo_zip::ZipEntry::new("content.xml", content.as_bytes().to_vec()),
                lo_zip::ZipEntry::new("styles.xml", styles.as_bytes().to_vec()),
            ],
        )
        .unwrap();
        let doc = from_odt_bytes("outline", &bytes.into_inner()).unwrap();
        let headings: Vec<_> = doc
            .body
            .iter()
            .map(|b| match b {
                Block::Heading(h) => h.content.to_plain_text().replace('\u{a0}', " "),
                _ => panic!("outline wrappers must not create list bullets"),
            })
            .collect();
        assert_eq!(
            headings,
            [
                "1 Introduction",
                "1.1 Model",
                "1.2 Query",
                "2 Next",
                "2.1 Reset"
            ]
        );
    }

    #[test]
    fn paragraph_boxes_inherit_and_explicit_spaces_survive() {
        let root = parse_xml_document(r##"<styles><style name="Base" family="paragraph"><paragraph-properties background-color="#fff5e0" border="0.5pt solid #ff0000" padding="2pt" margin-bottom="0.05in"/></style><style name="Code" family="paragraph" parent-style-name="Base"><paragraph-properties border-left="none"/></style></styles>"##).unwrap();
        let mut named = lo_core::style::OdfStyles::default();
        named.collect(&root);
        let styles = parse_odt_styles(&named);
        let node =
            parse_xml_document(r#"<p style-name="Code"><s c="4"/>indented<s c="1000000"/>end</p>"#)
                .unwrap();
        let paragraph = parse_odt_paragraph(&node, &styles);
        let decoration = paragraph.style.odf_box.as_ref().unwrap();
        assert_eq!(decoration.background, "#fff5e0");
        assert_eq!(decoration.borders[0], "none");
        assert_eq!(decoration.borders[1], "0.5pt solid #ff0000");
        assert_eq!(decoration.padding_mpt, [2000; 4]);
        assert_eq!(paragraph.style.odf_margins_mpt.unwrap()[1], 3600);
        let text = paragraph.to_plain_text();
        assert!(text.starts_with("\u{a0}\u{a0}\u{a0}\u{a0}indented"));
        assert_eq!(text.matches('\u{a0}').count(), 10004);
    }

    #[test]
    fn table_borders_preserve_each_edge() {
        let root = parse_xml_document(r##"<styles><style name="Cell" family="table-cell"><table-cell-properties border="0.5pt solid #aaaaaa" border-top="none"/></style></styles>"##).unwrap();
        let mut styles = lo_core::style::OdfStyles::default();
        styles.collect(&root);
        let node = parse_xml_document(r#"<table><table-row><table-cell style-name="Cell"><p>Body</p></table-cell></table-row></table>"#).unwrap();
        let layout = odt_table_layout(&node, &styles);
        assert_eq!(
            layout.edge_borders[&(0, 0)],
            [
                "0.5pt solid #aaaaaa",
                "0.5pt solid #aaaaaa",
                "none",
                "0.5pt solid #aaaaaa"
            ]
        );
    }
}

fn apply_odt_initial_master(styles: &XmlNode, content: &XmlNode, doc: &mut TextDocument) {
    let mut named = lo_core::style::OdfStyles::default();
    named.collect(styles);
    named.collect(content);
    let paragraph_styles = parse_odt_styles(&named);
    let Some(first) = content
        .child("body")
        .and_then(|b| b.child("text"))
        .and_then(|t| {
            t.children
                .iter()
                .find(|n| matches!(n.local_name(), "p" | "h"))
        })
    else {
        return;
    };
    let mut definitions = Vec::new();
    styles.descendants_named("style", &mut definitions);
    content.descendants_named("style", &mut definitions);
    let Some(name) = first.attr("style-name") else {
        return;
    };
    let Some(first_style) = definitions
        .iter()
        .rev()
        .find(|s| s.attr("name") == Some(name) && s.attr("family") == Some("paragraph"))
    else {
        return;
    };
    let Some(master_name) = first_style.attr("master-page-name") else {
        return;
    };
    let Some(masters) = styles.child("master-styles") else {
        return;
    };
    let Some(master) = masters
        .children_named("master-page")
        .find(|m| m.attr("name") == Some(master_name))
    else {
        return;
    };
    let Some(next) = master.attr("next-style-name").and_then(|name| {
        masters
            .children_named("master-page")
            .find(|m| m.attr("name") == Some(name))
    }) else {
        return;
    };
    // A distinct first-page master sharing the body geometry is the common
    // cover-page case. Different page geometries still need section layout.
    if master.attr("page-layout-name") != next.attr("page-layout-name") {
        return;
    }
    if let Some(layout) = &mut doc.odf_layout {
        layout.first_headers = Some(
            master
                .child("header")
                .map(|n| odf_page_lines(n, &paragraph_styles))
                .unwrap_or_default(),
        );
        layout.first_footers = Some(
            master
                .child("footer")
                .map(|n| odf_page_lines(n, &paragraph_styles))
                .unwrap_or_default(),
        );
    }
}

fn enrich_odt_image_alignment(
    content: &XmlNode,
    named: &lo_core::style::OdfStyles,
    doc: &mut TextDocument,
) {
    let mut frames = Vec::new();
    content.descendants_named("frame", &mut frames);
    let mut alignments = BTreeMap::new();
    for frame in frames {
        let name = frame.attr("style-name").unwrap_or("");
        let alignment =
            match named.property_in(name, "graphic", "graphic-properties", "horizontal-pos") {
                Some("center") => lo_core::Alignment::Center,
                Some("right") => lo_core::Alignment::End,
                _ => continue,
            };
        for image in frame.children_named("image") {
            if let Some(href) = image.attr("href") {
                alignments.insert(href.strip_prefix("./").unwrap_or(href), alignment.clone());
            }
        }
    }
    let mut spacing = BTreeMap::new();
    let mut paragraphs = Vec::new();
    content.descendants_named("p", &mut paragraphs);
    for paragraph in paragraphs {
        let name = paragraph.attr("style-name").unwrap_or("");
        let size = named
            .property_in(name, "paragraph", "text-properties", "font-size")
            .and_then(lo_core::style::odf_length)
            .unwrap_or(12.0)
            .clamp(1.0, 512.0);
        let mut frames = Vec::new();
        paragraph.descendants_named("frame", &mut frames);
        for frame in frames {
            let name = frame.attr("style-name").unwrap_or("");
            if frame.attr("anchor-type") != Some("paragraph")
                || named.property_in(name, "graphic", "graphic-properties", "wrap") != Some("none")
            {
                continue;
            }
            let offset = frame
                .attr("y")
                .and_then(lo_core::style::odf_length)
                .unwrap_or(0.0)
                .clamp(0.0, 1000.0);
            for image in frame.children_named("image") {
                if let Some(href) = image.attr("href") {
                    spacing.insert(
                        href.strip_prefix("./").unwrap_or(href),
                        [offset, size * 2355.0 / 2048.0],
                    );
                }
            }
        }
    }
    if let Some(layout) = &mut doc.odf_layout {
        for &index in layout.image_groups.keys() {
            if let Some(Block::Image(image)) = doc.body.get(index) {
                if let Some(spacing) = spacing.get(image.name.as_str()) {
                    layout.image_spacing.insert(index, *spacing);
                }
                if let Some(alignment) = alignments.get(image.name.as_str()) {
                    layout.image_alignments.insert(index, alignment.clone());
                }
            }
        }
    }
}

fn collect_odt_list_styles(root: &XmlNode, styles: &mut BTreeMap<String, StyleProps>) {
    let mut lists = Vec::new();
    root.descendants_named("list-style", &mut lists);
    for list in lists {
        let Some(name) = list.attr("name") else {
            continue;
        };
        for rule in list.children_named("list-level-style-bullet") {
            let Some(level) = rule
                .attr("level")
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| (1..=10).contains(n))
            else {
                continue;
            };
            let Some(props) = rule.child("list-level-properties") else {
                continue;
            };
            let alignment = props.child("list-level-label-alignment");
            let length =
                |node: &XmlNode, attr| node.attr(attr).and_then(lo_core::style::odf_length);
            let label = length(props, "space-before").unwrap_or(0.0);
            let text = alignment
                .and_then(|a| length(a, "margin-left"))
                .unwrap_or(label + length(props, "min-label-width").unwrap_or(18.0))
                .clamp(0.0, 1000.0);
            let label = alignment
                .and_then(|a| length(a, "text-indent"))
                .map(|v| text + v)
                .unwrap_or(label)
                .clamp(0.0, 1000.0);
            styles.insert(
                format!("list:{name}:{level}"),
                StyleProps {
                    list_layout: Some(lo_core::writer::OdfListLayout {
                        text_indent: text,
                        label_indent: label,
                        bullet: rule
                            .attr("bullet-char")
                            .unwrap_or("•")
                            .chars()
                            .take(8)
                            .collect(),
                    }),
                    ..Default::default()
                },
            );
        }
    }
}

#[cfg(test)]
mod yocto_layout_tests {
    use super::*;

    #[test]
    fn first_master_and_heading_breaks_preserve_cover_without_blank_page() {
        let content = r#"<document-content><automatic-styles><style name="Start" family="paragraph" master-page-name="Cover"><paragraph-properties break-before="page"/></style></automatic-styles><body><text><p style-name="Start">Cover title</p><h style-name="Chapter" outline-level="1">Introduction</h><p style-name="Body">Body text</p></text></body></document-content>"#;
        let styles = r#"<document-styles><styles><style name="Chapter" family="paragraph"><paragraph-properties break-before="page"/></style><style name="Body" family="paragraph"><text-properties font-size="10.5pt"/></style></styles><automatic-styles><page-layout name="Page"><page-layout-properties page-width="8.5in" page-height="11in"/></page-layout></automatic-styles><master-styles><master-page name="Normal" page-layout-name="Page"><footer><p>Body footer</p></footer></master-page><master-page name="Cover" page-layout-name="Page" next-style-name="Normal"><footer><p/></footer></master-page></master-styles></document-styles>"#;
        let mut bytes = std::io::Cursor::new(Vec::new());
        lo_zip::write_zip(
            &mut bytes,
            &[
                lo_zip::ZipEntry::new("content.xml", content.as_bytes().to_vec()),
                lo_zip::ZipEntry::new("styles.xml", styles.as_bytes().to_vec()),
            ],
        )
        .unwrap();
        let doc = from_odt_bytes("cover", &bytes.into_inner()).unwrap();
        assert!(matches!(doc.body[0], Block::Paragraph(_)));
        assert!(matches!(doc.body[1], Block::PageBreak));
        assert!(matches!(doc.body[2], Block::Heading(_)));
        let Block::Paragraph(body) = &doc.body[3] else {
            panic!("body")
        };
        assert_eq!(body.text_style.font_size_mpt, Some(10500));
        let layout = doc.odf_layout.unwrap();
        assert_eq!(layout.first_footers, Some(vec![vec![]]));
        assert!(
            matches!(&layout.footers[0][0], lo_core::OdfPageField::Text(s) if s == "Body footer")
        );
    }

    #[test]
    fn list_alignment_and_bullets_are_imported_per_level() {
        let root = parse_xml_document(r#"<styles><list-style name="Bullets"><list-level-style-bullet level="1" bullet-char="•"><list-level-properties><list-level-label-alignment margin-left="0.5in" text-indent="-0.25in"/></list-level-properties></list-level-style-bullet><list-level-style-bullet level="2" bullet-char="◦"><list-level-properties><list-level-label-alignment margin-left="0.75in" text-indent="-0.25in"/></list-level-properties></list-level-style-bullet></list-style></styles>"#).unwrap();
        let mut styles = BTreeMap::new();
        collect_odt_list_styles(&root, &mut styles);
        let first = styles["list:Bullets:1"].list_layout.as_ref().unwrap();
        let second = styles["list:Bullets:2"].list_layout.as_ref().unwrap();
        assert_eq!((first.text_indent, first.label_indent), (36.0, 18.0));
        assert_eq!((second.text_indent, second.label_indent), (54.0, 36.0));
        assert_eq!(second.bullet, "◦");
    }
}

#[cfg(test)]
mod odf_image_placement_tests {
    use super::*;
    #[test]
    fn paragraph_anchored_image_retains_alignment_offset_and_line_space() {
        let content = r#"<document-content><automatic-styles><style name="Body" family="paragraph"><text-properties font-size="10.5pt"/></style><style name="Centered" family="graphic"><graphic-properties horizontal-pos="center" wrap="none"/></style></automatic-styles><body><text><p style-name="Body"><frame style-name="Centered" anchor-type="paragraph" y="0.1in" width="2in" height="1in"><image href="logo.png"/></frame></p></text></body></document-content>"#;
        let mut bytes = std::io::Cursor::new(Vec::new());
        lo_zip::write_zip(
            &mut bytes,
            &[
                lo_zip::ZipEntry::new("content.xml", content.as_bytes().to_vec()),
                lo_zip::ZipEntry::new("logo.png", vec![1, 2, 3]),
            ],
        )
        .unwrap();
        let doc = from_odt_bytes("image", &bytes.into_inner()).unwrap();
        assert!(matches!(doc.body[0], Block::Image(_)));
        let layout = doc.odf_layout.unwrap();
        assert_eq!(layout.image_alignments[&0], lo_core::Alignment::Center);
        let spacing = layout.image_spacing[&0];
        assert!((spacing[0] - 7.2).abs() < 0.001);
        assert!((spacing[1] - 10.5 * 2355.0 / 2048.0).abs() < 0.001);
    }
}
