//! Block-aware PDF layout for `TextDocument`.
//!
//! This renderer stays intentionally simple, but it is now visual enough
//! for Clark's document QA path: it respects page size/margins,
//! paragraph alignment and spacing, heading sizing, basic run styling,
//! tables with borders, and image placeholders with their real document
//! footprint.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use lo_core::{
    decode_png, decode_webp, parse_xml_document, Alignment, Block, Heading, ImageBlock, Inline,
    ListBlock, ListItem, PageStyle, Paragraph, PdfDocument, PdfFont, Result, Table, TableCell,
    TextDocument, XmlNode,
};

use crate::font_metrics::measure_text;

#[derive(Clone, Debug)]
struct StyledRun {
    text: String,
    font: PdfFont,
    size: f32,
    color: (f32, f32, f32),
    background: Option<(f32, f32, f32)>,
    underline: bool,
    link: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct LineLayout {
    runs: Vec<StyledRun>,
    width: f32,
}

#[derive(Clone, Debug)]
struct LayoutContext {
    page_w: f32,
    page_h: f32,
    margin_l: f32,
    margin_r: f32,
    margin_t: f32,
    margin_b: f32,
    fonts: FontResolver,
}

const BODY_FONT_SIZE: f32 = 12.0;
const TABLE_FONT_SIZE: f32 = 9.0;
const TABLE_CELL_HORIZONTAL_PADDING: f32 = 12.0;
const TABLE_CELL_VERTICAL_PADDING: f32 = 12.0;
const TABLE_WORD_FIT_SLOP: f32 = 3.0;
const HEADING_TRAILING_GAP: f32 = 4.0;
const LIST_ITEM_GAP: f32 = 3.0;
const LIST_TRAILING_GAP: f32 = 7.0;
const TABLE_TRAILING_GAP: f32 = 14.0;

pub fn render_document_pdf(doc: &TextDocument) -> Vec<u8> {
    try_render_document_pdf(doc)
        .expect("PDF text contains Unicode that no installed font can preserve")
}

fn render_inline_image_group(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    index: &mut usize,
    mut y: f32,
    blocks: &[Block],
) -> f32 {
    let max_width = ctx.page_w - ctx.margin_l - ctx.margin_r;
    let max_height = ctx.page_h - ctx.margin_t - ctx.margin_b;
    let images: Vec<_> = blocks
        .iter()
        .filter_map(|b| {
            if let Block::Image(image) = b {
                Some(image)
            } else {
                None
            }
        })
        .collect();
    let mut start = 0;
    while start < images.len() {
        let mut row = Vec::new();
        let mut used = 0.0f32;
        let mut height = 0.0f32;
        for image in &images[start..] {
            let w = image.size.width.as_pt().max(1.0);
            let h = image.size.height.as_pt().max(1.0);
            let scale = (max_width / w).min(max_height / h).min(1.0);
            let (w, h) = (w * scale, h * scale);
            if !row.is_empty() && used + w > max_width + 0.01 {
                break;
            }
            row.push((*image, w, h));
            used += w;
            height = height.max(h);
        }
        y = ensure_room(ctx, pdf, index, y, height);
        let bottom = y - height;
        let mut x = ctx.margin_l;
        for (image, w, h) in &row {
            let resource = register_pdf_image(pdf, image);
            let page = pdf.page_mut(*index).expect("image row page");
            page.begin_tag("Figure", Some(&image.alt));
            if let Some(resource) = resource {
                page.image(&resource, x, bottom, *w, *h);
            } else {
                page.rect_stroke(x, bottom, *w, *h);
            }
            page.end_tag();
            x += w;
        }
        start += row.len();
        y = bottom;
    }
    y - 4.0
}

fn render_odf_page_fields(
    pdf: &mut PdfDocument,
    layout: &lo_core::OdfDocumentLayout,
    fonts: &FontResolver,
) {
    use lo_core::OdfPageField as Field;
    let pages = pdf.pages().len();
    for index in 0..pages {
        let select = |normal, first: &Option<Vec<Vec<Field>>>, left: &Option<Vec<Vec<Field>>>| {
            if index == 0 {
                first.clone().unwrap_or(normal)
            } else if index % 2 == 1 {
                left.clone().unwrap_or(normal)
            } else {
                normal
            }
        };
        let header = select(
            layout.headers.clone(),
            &layout.first_headers,
            &layout.left_headers,
        );
        let footer = select(
            layout.footers.clone(),
            &layout.first_footers,
            &layout.left_footers,
        );
        let page = pdf.page_mut(index).expect("header page");
        for (lines, mut y, style) in [
            (
                header,
                layout.page[1] - layout.margins[2],
                &layout.header_font,
            ),
            (
                footer,
                layout.margins[3] + layout.footer_height.max(14.0),
                &layout.footer_font,
            ),
        ] {
            let size = (style.font_size_pt as f32).max(1.0);
            let font = fonts.pick(Some(&style.font_family), style.bold, style.italic);
            y -= size;
            for line in lines {
                let mut text = String::new();
                for field in line {
                    match field {
                        Field::Text(s) => text.push_str(&s),
                        Field::PageNumber => text.push_str(&(index + 1).to_string()),
                        Field::PageCount => text.push_str(&pages.to_string()),
                        Field::Tab => text.push('\t'),
                        Field::LineBreak => text.push('\n'),
                    }
                }
                for line in text.split('\n') {
                    let columns: Vec<_> = line.split('\t').collect();
                    let available = layout.page[0] - layout.margins[0] - layout.margins[1];
                    for (column, text) in columns.iter().enumerate() {
                        let width = measure_text(text, size, font);
                        let x = if column == 0 {
                            layout.margins[0]
                        } else if column == columns.len() - 1 {
                            layout.page[0] - layout.margins[1] - width
                        } else {
                            layout.margins[0] + (available - width) / 2.0
                        };
                        page.text(x, y, size, font, text);
                    }
                    y -= size * 1.2;
                }
            }
        }
    }
}

pub fn try_render_document_pdf(doc: &TextDocument) -> Result<Vec<u8>> {
    let mut ctx = LayoutContext::from_page_style(&doc.page_style);
    if let Some(layout) = &doc.odf_layout {
        ctx.page_w = layout.page[0];
        ctx.page_h = layout.page[1];
        ctx.margin_l = layout.margins[0];
        ctx.margin_r = layout.margins[1];
        let header = if !layout.headers.is_empty() || layout.first_headers.is_some() {
            layout.header_height.max(14.0)
        } else {
            0.0
        };
        let footer = if !layout.footers.is_empty() || layout.first_footers.is_some() {
            layout.footer_height.max(14.0)
        } else {
            0.0
        };
        ctx.margin_t = (layout.margins[2] + header).min(ctx.page_h * 0.45);
        ctx.margin_b = (layout.margins[3] + footer).min(ctx.page_h * 0.45);
    }
    let mut pdf = PdfDocument::new();
    let mut page_index = pdf.add_page(ctx.page_w, ctx.page_h);
    let mut y = ctx.page_h - ctx.margin_t;

    if !doc.meta.title.trim().is_empty() {
        let title_run = StyledRun {
            text: doc.meta.title.clone(),
            font: PdfFont::HelveticaBold,
            size: 22.0,
            color: (0.0, 0.0, 0.0),
            background: None,
            underline: false,
            link: None,
        };
        y = ensure_room(&ctx, &mut pdf, &mut page_index, y, 30.0);
        render_line(
            pdf.page_mut(page_index).expect("page"),
            &LineLayout {
                runs: vec![title_run],
                width: measure_text(&doc.meta.title, 22.0, PdfFont::HelveticaBold),
            },
            ctx.margin_l,
            y,
            Alignment::Start,
            ctx.page_w - ctx.margin_l - ctx.margin_r,
            false,
            "H1",
        );
        y -= 30.0;
    }

    let mut skip_until = 0;
    for (block_index, block) in doc.body.iter().enumerate() {
        if block_index < skip_until {
            continue;
        }
        if let Some(end) = doc
            .odf_layout
            .as_ref()
            .and_then(|l| l.image_groups.get(&block_index))
            .copied()
        {
            y = render_inline_image_group(
                &ctx,
                &mut pdf,
                &mut page_index,
                y,
                &doc.body[block_index..end],
            );
            skip_until = end;
            continue;
        }
        if let Block::Heading(heading) = block {
            let heading_height = heading_height(&ctx, heading);
            let following_height = doc
                .body
                .get(block_index + 1)
                .map(|following| following_keep_height(&ctx, following))
                .unwrap_or(0.0);
            let content_height = ctx.page_h - ctx.margin_t - ctx.margin_b;
            let maximum_following_keep =
                (content_height - heading_height).max(BODY_FONT_SIZE * 2.5);
            let keep_height = heading_height + following_height.min(maximum_following_keep);
            y = ensure_room(&ctx, &mut pdf, &mut page_index, y, keep_height);
        }
        match block {
            Block::Heading(heading) => {
                y = render_heading(&ctx, &mut pdf, &mut page_index, y, heading);
            }
            Block::Paragraph(paragraph) => {
                let mut joined = paragraph.clone();
                let mut end = block_index + 1;
                if paragraph.style.odf_box.as_ref().is_some_and(|b| {
                    b.join
                        && parse_color(&b.background).is_some()
                        && b.borders.iter().any(|v| v.contains("solid"))
                }) {
                    while let Some(Block::Paragraph(next)) = doc.body.get(end) {
                        if next.style != paragraph.style || next.text_style != paragraph.text_style
                        {
                            break;
                        }
                        joined.spans.push(Inline::LineBreak);
                        joined.spans.extend(next.spans.clone());
                        end += 1;
                    }
                    if end > block_index + 1 {
                        let size = paragraph.text_style.font_size_pt.max(1) as f32;
                        joined.style.line_height_percent = (joined.style.line_height_percent as f32
                            + (paragraph_margins(paragraph)[0] + paragraph_margins(paragraph)[1])
                                / (size
                                    * odf_line_metric(ctx.fonts.pick(
                                        Some(&paragraph.text_style.font_family),
                                        paragraph.text_style.bold,
                                        paragraph.text_style.italic,
                                    )))
                                * 100.0)
                            .round()
                            .clamp(10.0, 1000.0)
                            as u16;
                        skip_until = end;
                    }
                }
                let paragraph = &joined;
                y = render_paragraph(
                    &ctx,
                    &mut pdf,
                    &mut page_index,
                    y,
                    paragraph,
                    BODY_FONT_SIZE,
                    0.0,
                    "P",
                );
            }
            Block::List(list) => {
                y = render_list(&ctx, &mut pdf, &mut page_index, y, list);
            }
            Block::Table(table) => {
                y = render_table(&ctx, &mut pdf, &mut page_index, y, table);
            }
            Block::Image(image) => {
                let max_width = ctx.page_w - ctx.margin_l - ctx.margin_r;
                let mut width = image.size.width.as_pt().max(80.0).min(max_width);
                let mut height = image.size.height.as_pt().max(48.0);
                let max_height = ctx.page_h - ctx.margin_t - ctx.margin_b - 28.0;
                if height > max_height {
                    width *= max_height / height;
                    height = max_height;
                }
                y = ensure_room(&ctx, &mut pdf, &mut page_index, y, height + 18.0);
                let bottom = y - height;
                let resource = register_pdf_image(&mut pdf, image);
                let page = pdf.page_mut(page_index).expect("page");
                page.begin_tag("Figure", Some(&image.alt));
                let rendered = match image.mime_type.as_str() {
                    "image/svg+xml" => {
                        render_svg_image(page, &image.data, ctx.margin_l, bottom, width, height)
                    }
                    "application/vnd.mermaid+text" => {
                        render_mermaid(page, &image.data, ctx.margin_l, bottom, width, height)
                    }
                    "application/x-latex" => {
                        render_formula(page, &image.data, ctx.margin_l, bottom, width, height)
                    }
                    _ => false,
                };
                if let Some(resource) = resource {
                    page.image(&resource, ctx.margin_l, bottom, width, height);
                } else if !rendered {
                    page.rect_fill_stroke_rgb(
                        ctx.margin_l,
                        bottom,
                        width,
                        height,
                        (0.98, 0.98, 0.98),
                        (0.60, 0.60, 0.60),
                    );
                    page.text_rgb(
                        ctx.margin_l + 6.0,
                        bottom + 10.0,
                        10.0,
                        PdfFont::HelveticaOblique,
                        &format!("[image: {}]", image.alt),
                        0.30,
                        0.30,
                        0.30,
                    );
                }
                page.end_tag();
                if !image.alt.trim().is_empty() {
                    page.begin_tag("Caption", None);
                    page.text_rgb(
                        ctx.margin_l,
                        bottom - 11.0,
                        9.0,
                        PdfFont::HelveticaOblique,
                        &image.alt,
                        0.35,
                        0.33,
                        0.30,
                    );
                    page.end_tag();
                }
                y = bottom
                    - if image.alt.trim().is_empty() {
                        12.0
                    } else {
                        22.0
                    };
            }
            Block::Section(section) => {
                y = ensure_room(&ctx, &mut pdf, &mut page_index, y, 20.0);
                pdf.page_mut(page_index).expect("page").text_rgb(
                    ctx.margin_l,
                    y,
                    13.0,
                    PdfFont::HelveticaBold,
                    &format!("[{}]", section.name),
                    0.15,
                    0.15,
                    0.15,
                );
                y -= 18.0;
                for nested in &section.blocks {
                    if let Block::Paragraph(paragraph) = nested {
                        y = render_paragraph(
                            &ctx,
                            &mut pdf,
                            &mut page_index,
                            y,
                            paragraph,
                            BODY_FONT_SIZE,
                            0.0,
                            "P",
                        );
                    }
                }
            }
            Block::HorizontalRule => {
                y = ensure_room(&ctx, &mut pdf, &mut page_index, y, 14.0);
                let page = pdf.page_mut(page_index).expect("page");
                page.begin_artifact();
                page.line_rgb(
                    ctx.margin_l,
                    y,
                    ctx.page_w - ctx.margin_r,
                    y,
                    0.55,
                    0.55,
                    0.55,
                );
                page.end_artifact();
                y -= 10.0;
            }
            Block::PageBreak => {
                page_index = pdf.add_page(ctx.page_w, ctx.page_h);
                y = ctx.page_h - ctx.margin_t;
            }
        }
    }

    if let Some(layout) = &doc.odf_layout {
        render_odf_page_fields(&mut pdf, layout, &ctx.fonts);
    }
    pdf.try_finish()
}

fn render_heading(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    mut y: f32,
    heading: &Heading,
) -> f32 {
    let size = heading_font_size(heading.level);
    y = ensure_room(ctx, pdf, page_index, y, heading_height(ctx, heading));
    let paragraph = styled_heading_paragraph(heading, size);
    let role = format!("H{}", heading.level.clamp(1, 6));
    y = render_paragraph(ctx, pdf, page_index, y, &paragraph, size, 0.0, &role);
    y - if heading.content.style.source_spacing {
        0.0
    } else {
        HEADING_TRAILING_GAP
    }
}

fn heading_font_size(level: u8) -> f32 {
    match level {
        1 => 20.0,
        2 => 18.0,
        3 => 16.0,
        4 => 14.0,
        _ => 13.0,
    }
}

fn styled_heading_paragraph(heading: &Heading, size: f32) -> Paragraph {
    let mut paragraph = heading.content.clone();
    if paragraph.style.source_spacing {
        return paragraph;
    }
    paragraph.text_style.font_size_pt = size.round() as u16;
    paragraph.text_style.bold = true;
    paragraph.style.margin_top_mm =
        paragraph
            .style
            .margin_top_mm
            .max(if heading.level == 1 { 2 } else { 3 });
    paragraph.style.margin_bottom_mm = paragraph.style.margin_bottom_mm.max(2);
    paragraph
}

fn heading_height(ctx: &LayoutContext, heading: &Heading) -> f32 {
    let size = heading_font_size(heading.level);
    let paragraph = styled_heading_paragraph(heading, size);
    paragraph_height(
        &paragraph,
        size,
        ctx.page_w - ctx.margin_l - ctx.margin_r,
        &ctx.fonts,
        2,
    ) + if heading.content.style.source_spacing {
        0.0
    } else {
        HEADING_TRAILING_GAP
    }
}

fn render_list(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    mut y: f32,
    list: &ListBlock,
) -> f32 {
    let list_height = list_height(ctx, list);
    let content_height = ctx.page_h - ctx.margin_t - ctx.margin_b;
    if list_height <= content_height
        && y - list_height < ctx.margin_b
        && y < ctx.page_h - ctx.margin_t
    {
        *page_index = pdf.add_page(ctx.page_w, ctx.page_h);
        y = ctx.page_h - ctx.margin_t;
    }
    for (index, item) in list.items.iter().enumerate() {
        let marker = if list.ordered {
            format!("{}.", index + 1)
        } else {
            "•".to_string()
        };
        y = ensure_room(ctx, pdf, page_index, y, 17.0);
        let marker_baseline = y - 15.0;
        let page = pdf.page_mut(*page_index).expect("page");
        page.begin_artifact();
        page.text(
            ctx.margin_l + 2.0,
            marker_baseline,
            BODY_FONT_SIZE,
            PdfFont::Helvetica,
            &marker,
        );
        page.end_artifact();
        y = render_list_item(ctx, pdf, page_index, y, item);
        y -= LIST_ITEM_GAP;
    }
    y - LIST_TRAILING_GAP
}

fn render_list_item(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    mut y: f32,
    item: &ListItem,
) -> f32 {
    for block in &item.blocks {
        match block {
            Block::Paragraph(paragraph) => {
                y = render_paragraph(ctx, pdf, page_index, y, paragraph, 12.0, 18.0, "LI");
            }
            Block::List(list) => {
                let mut nested = ctx.clone();
                nested.margin_l += 18.0;
                y = render_list(&nested, pdf, page_index, y, list);
            }
            Block::Image(_) => {
                let mut nested = ctx.clone();
                nested.margin_l += 18.0;
                y = render_inline_image_group(
                    &nested,
                    pdf,
                    page_index,
                    y,
                    std::slice::from_ref(block),
                );
            }
            Block::Table(table) => {
                let mut nested = ctx.clone();
                nested.margin_l += 18.0;
                y = render_table(&nested, pdf, page_index, y, table);
            }
            Block::Heading(heading) => {
                y = render_heading(ctx, pdf, page_index, y, heading);
            }
            Block::PageBreak => {
                *page_index = pdf.add_page(ctx.page_w, ctx.page_h);
                y = ctx.page_h - ctx.margin_t;
            }
            _ => {}
        }
    }
    y
}

fn list_height(ctx: &LayoutContext, list: &ListBlock) -> f32 {
    let available_width = (ctx.page_w - ctx.margin_l - ctx.margin_r - 18.0).max(48.0);
    list.items
        .iter()
        .flat_map(|item| item.blocks.iter())
        .filter_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph_height(
                paragraph,
                BODY_FONT_SIZE,
                available_width,
                &ctx.fonts,
                1,
            )),
            _ => None,
        })
        .sum::<f32>()
        + list.items.len() as f32 * LIST_ITEM_GAP
        + LIST_TRAILING_GAP
}

fn following_keep_height(ctx: &LayoutContext, block: &Block) -> f32 {
    match block {
        Block::Paragraph(paragraph) => paragraph_height(
            paragraph,
            BODY_FONT_SIZE,
            ctx.page_w - ctx.margin_l - ctx.margin_r,
            &ctx.fonts,
            2,
        ),
        Block::List(list) => list_height(ctx, list),
        Block::Table(table) => first_table_row_height(ctx, table),
        Block::Image(image) => {
            let max_height = ctx.page_h - ctx.margin_t - ctx.margin_b - 28.0;
            image.size.height.as_pt().max(48.0).min(max_height) + 18.0
        }
        Block::Heading(heading) => heading_height(ctx, heading),
        Block::Section(_) => BODY_FONT_SIZE * 2.5,
        Block::HorizontalRule => 14.0,
        Block::PageBreak => 0.0,
    }
}

fn paragraph_margins(paragraph: &Paragraph) -> [f32; 4] {
    paragraph
        .style
        .odf_margins_mpt
        .map(|m| m.map(|v| v as f32 / 1000.0))
        .unwrap_or([
            mm_to_pt(paragraph.style.margin_top_mm),
            mm_to_pt(paragraph.style.margin_bottom_mm),
            mm_to_pt(paragraph.style.margin_left_mm),
            mm_to_pt(paragraph.style.margin_right_mm),
        ])
}

// hhea ascender - descender + lineGap, divided by unitsPerEm, for the
// bundled Liberation families. ODF percentage spacing multiplies this natural
// line box, not the nominal em size. Non-ODF layout keeps its existing policy.
fn odf_line_metric(font: PdfFont) -> f32 {
    if matches!(font, PdfFont::Courier) {
        2320.0 / 2048.0
    } else {
        2355.0 / 2048.0
    }
}
fn odf_descent(font: PdfFont) -> f32 {
    match font {
        PdfFont::Courier => 615.0 / 2048.0,
        PdfFont::TimesRoman | PdfFont::TimesBold | PdfFont::TimesItalic => 443.0 / 2048.0,
        _ => 434.0 / 2048.0,
    }
}
fn run_line_height(paragraph: &Paragraph, run: &StyledRun) -> f32 {
    run.size
        * paragraph_line_factor(paragraph)
        * if paragraph.style.source_spacing {
            odf_line_metric(run.font)
        } else {
            1.0
        }
}

fn paragraph_line_factor(paragraph: &Paragraph) -> f32 {
    if paragraph.style.line_height_percent > 0 {
        paragraph.style.line_height_percent as f32 / 100.0
    } else {
        1.25
    }
}
fn paragraph_empty_line_height(paragraph: &Paragraph, default_size: f32, empty: bool) -> f32 {
    if !empty {
        return 0.0;
    }
    let size = if paragraph.text_style.font_size_pt > 0 {
        paragraph.text_style.font_size_pt as f32
    } else {
        default_size
    };
    size * paragraph_line_factor(paragraph)
        * if paragraph.style.source_spacing {
            2355.0 / 2048.0
        } else {
            1.0
        }
}

fn draw_edges(
    page: &mut lo_core::PdfPage,
    x: f32,
    top: f32,
    width: f32,
    height: f32,
    borders: &[String; 4],
) {
    for (side, border) in borders.iter().enumerate() {
        let mut parts = border.split_whitespace();
        let Some(thickness) = parts
            .next()
            .and_then(lo_core::style::odf_length)
            .filter(|v| *v > 0.0)
        else {
            continue;
        };
        if parts.next() != Some("solid") {
            continue;
        }
        let (r, g, b) = parts
            .next()
            .and_then(parse_color)
            .unwrap_or((0.0, 0.0, 0.0));
        page.line_width(thickness.clamp(0.25, 100.0));
        let (x1, y1, x2, y2) = match side {
            0 => (x, top, x, top - height),
            1 => (x + width, top, x + width, top - height),
            2 => (x, top, x + width, top),
            _ => (x, top - height, x + width, top - height),
        };
        page.line_rgb(x1, y1, x2, y2, r, g, b);
    }
    page.line_width(1.0);
}

fn render_paragraph(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    mut y: f32,
    paragraph: &Paragraph,
    default_size: f32,
    extra_indent: f32,
    semantic_role: &str,
) -> f32 {
    let padding = paragraph
        .style
        .odf_box
        .as_ref()
        .map(|b| b.padding_mpt.map(|n| n as f32 / 1000.0))
        .unwrap_or([0.0; 4]);
    let margins = paragraph_margins(paragraph);
    let left = ctx.margin_l + margins[2] + extra_indent + padding[0];
    let right = ctx.margin_r + margins[3] + padding[1];
    let available_width = (ctx.page_w - left - right).max(48.0);
    let margin_top = margins[0];
    let minimum_bottom_mm = if paragraph.style.source_spacing {
        0
    } else {
        minimum_paragraph_bottom_mm(semantic_role)
    };
    let margin_bottom = margins[1].max(mm_to_pt(minimum_bottom_mm));
    let runs = paragraph_runs(paragraph, default_size, &ctx.fonts);
    let lines = layout_runs(&runs, available_width);
    let line_heights = lines
        .iter()
        .map(|line| {
            line.runs
                .iter()
                .map(|run| run_line_height(paragraph, run))
                .fold(
                    if paragraph.style.source_spacing {
                        paragraph_empty_line_height(paragraph, default_size, line.runs.is_empty())
                    } else {
                        default_size * 1.25
                    },
                    f32::max,
                )
        })
        .collect::<Vec<_>>();
    let total_height =
        margin_top + padding[2] + line_heights.iter().sum::<f32>() + padding[3] + margin_bottom;
    let content_height = ctx.page_h - ctx.margin_t - ctx.margin_b;
    if total_height <= content_height
        && y - total_height < ctx.margin_b
        && y < ctx.page_h - ctx.margin_t
    {
        *page_index = pdf.add_page(ctx.page_w, ctx.page_h);
        y = ctx.page_h - ctx.margin_t;
    }
    y -= margin_top;
    // Draw decoration per page fragment so long paragraphs remain bounded to a page.
    let mut fragment_start = true;
    for (line_index, line) in lines.iter().enumerate() {
        let line_height = line_heights[line_index];
        let old_page = *page_index;
        y = ensure_room(
            ctx,
            pdf,
            page_index,
            y,
            line_height + padding[2] + padding[3] + 2.0,
        );
        fragment_start |= *page_index != old_page;
        if fragment_start {
            let remaining: f32 = line_heights[line_index..].iter().sum();
            let height = (remaining + padding[2] + padding[3])
                .min(y - ctx.margin_b)
                .max(0.0);
            if let Some(b) = &paragraph.style.odf_box {
                let page = pdf.page_mut(*page_index).expect("paragraph page");
                page.begin_artifact();
                if let Some((r, g, blue)) = parse_color(&b.background) {
                    page.rect_fill_rgb(
                        left - padding[0],
                        y - height,
                        available_width + padding[0] + padding[1],
                        height,
                        r,
                        g,
                        blue,
                    );
                }
                draw_edges(
                    page,
                    left - padding[0],
                    y,
                    available_width + padding[0] + padding[1],
                    height,
                    &b.borders,
                );
                page.end_artifact();
            }
            y -= padding[2];
            fragment_start = false;
        }
        // `y` is the free block boundary; PDF text APIs take a baseline.
        // Descend by one line box before drawing so glyph ascenders never
        // intrude into the preceding table, paragraph, or page margin.
        y -= line_height;
        if let Some((r, g, b)) = parse_color(&paragraph.text_style.background)
            .filter(|_| !paragraph.style.source_spacing)
        {
            let page = pdf.page_mut(*page_index).expect("page");
            page.begin_artifact();
            page.rect_fill_rgb(
                left - 4.0,
                y - 3.0,
                available_width + 8.0,
                line_height,
                r,
                g,
                b,
            );
            page.end_artifact();
        }
        let justify =
            matches!(paragraph.style.alignment, Alignment::Justify) && line_index + 1 < lines.len();
        render_line(
            pdf.page_mut(*page_index).expect("page"),
            line,
            left,
            y + if paragraph.style.source_spacing {
                line.runs
                    .iter()
                    .map(|run| run.size * odf_descent(run.font))
                    .fold(0.0, f32::max)
            } else {
                0.0
            },
            paragraph.style.alignment.clone(),
            available_width,
            justify,
            semantic_role,
        );
    }

    y - margin_bottom - padding[3]
}

fn paragraph_height(
    paragraph: &Paragraph,
    default_size: f32,
    available_width: f32,
    fonts: &FontResolver,
    minimum_bottom_mm: u16,
) -> f32 {
    let margins = paragraph_margins(paragraph);
    let padding = paragraph
        .style
        .odf_box
        .as_ref()
        .map(|b| b.padding_mpt.map(|v| v as f32 / 1000.0))
        .unwrap_or([0.0; 4]);
    let lines = layout_runs(
        &paragraph_runs(paragraph, default_size, fonts),
        (available_width - margins[2] - margins[3] - padding[0] - padding[1]).max(48.0),
    );
    margins[0]
        + padding[2]
        + padding[3]
        + lines
            .iter()
            .map(|line| {
                line.runs
                    .iter()
                    .map(|run| run_line_height(paragraph, run))
                    .fold(
                        if paragraph.style.source_spacing {
                            paragraph_empty_line_height(
                                paragraph,
                                default_size,
                                line.runs.is_empty(),
                            )
                        } else {
                            default_size * 1.25
                        },
                        f32::max,
                    )
            })
            .sum::<f32>()
        + margins[1].max(if paragraph.style.source_spacing {
            0.0
        } else {
            mm_to_pt(minimum_bottom_mm)
        })
}

fn minimum_paragraph_bottom_mm(semantic_role: &str) -> u16 {
    if semantic_role == "P" || semantic_role.starts_with('H') {
        2
    } else {
        1
    }
}

fn render_odf_table(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    mut y: f32,
    table: &Table,
    layout: &lo_core::OdfTableLayout,
) -> f32 {
    let count = table.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    if count == 0 {
        return y;
    }
    let available = ctx.page_w - ctx.margin_l - ctx.margin_r;
    let mut widths: Vec<f32> = (0..count)
        .map(|c| {
            layout
                .columns
                .get(c)
                .copied()
                .unwrap_or(available / count as f32)
        })
        .collect();
    let total: f32 = widths.iter().sum();
    if total > available {
        for w in &mut widths {
            *w *= available / total;
        }
    }
    let mut rows = Vec::new();
    for (r, row) in table.rows.iter().enumerate() {
        let mut cells = Vec::new();
        let mut height = 16.0f32;
        for (c, cell) in row.cells.iter().enumerate() {
            let (span, _) = layout.spans.get(&(r, c)).copied().unwrap_or((1, 1));
            let width: f32 = widths[c..(c + span).min(count)].iter().sum();
            let lines = if span == 0 {
                Vec::new()
            } else {
                layout_cell(cell, (width - 4.0).max(1.0), &ctx.fonts, false)
            };
            height = height.max(lines.iter().map(table_line_height).sum::<f32>() + 4.0);
            cells.push(lines);
        }
        rows.push((cells, height));
    }
    for (r, (cells, height)) in rows.iter().enumerate() {
        y = ensure_room(ctx, pdf, page_index, y, *height);
        let page = pdf.page_mut(*page_index).expect("table page");
        let mut x = ctx.margin_l;
        for (c, lines) in cells.iter().enumerate() {
            let (span, row_span) = layout.spans.get(&(r, c)).copied().unwrap_or((1, 1));
            let width: f32 = widths[c..(c + span).min(count)].iter().sum();
            let cell_height: f32 = rows[r..(r + row_span).min(rows.len())]
                .iter()
                .map(|(_, h)| h)
                .sum();
            if span > 0 {
                if let Some((red, green, blue)) =
                    layout.backgrounds.get(&(r, c)).and_then(|s| parse_color(s))
                {
                    page.rect_fill_rgb(x, y - cell_height, width, cell_height, red, green, blue);
                }
                if let Some(edges) = layout.edge_borders.get(&(r, c)) {
                    draw_edges(page, x, y, width, cell_height, edges);
                } else if let Some(border) = layout.borders.get(&(r, c)) {
                    draw_edges(
                        page,
                        x,
                        y,
                        width,
                        cell_height,
                        &std::array::from_fn(|_| border.clone()),
                    );
                }
                let mut baseline = y
                    - 2.0
                    - lines
                        .first()
                        .map(|line| line.runs.iter().map(|r| r.size).fold(9.0, f32::max))
                        .unwrap_or(9.0);
                for line in lines {
                    render_line(
                        page,
                        line,
                        x + 2.0,
                        baseline,
                        table.rows[r].cells[c]
                            .paragraphs
                            .first()
                            .map(|p| p.style.alignment.clone())
                            .unwrap_or_default(),
                        width - 4.0,
                        false,
                        "TD",
                    );
                    baseline -= table_line_height(line);
                }
            }
            x += widths[c];
        }
        y -= height;
    }
    y
}

fn render_table(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    mut y: f32,
    table: &Table,
) -> f32 {
    if let Some(layout) = &table.odf_layout {
        return render_odf_table(ctx, pdf, page_index, y, table, layout);
    }
    let col_widths = table_column_widths(ctx, table);
    let row_layouts = table
        .rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| layout_table_row(row, &col_widths, &ctx.fonts, row_index == 0))
        .collect::<Vec<_>>();

    for (row_index, row) in row_layouts.iter().enumerate() {
        let previous_page = *page_index;
        y = ensure_room(ctx, pdf, page_index, y, row.height);
        if *page_index != previous_page && row_index > 0 {
            if let Some(header) = row_layouts.first() {
                y = draw_table_row(ctx, pdf, *page_index, y, &col_widths, header, true, false);
                y = ensure_room(ctx, pdf, page_index, y, row.height);
            }
        }
        y = draw_table_row(
            ctx,
            pdf,
            *page_index,
            y,
            &col_widths,
            row,
            row_index == 0,
            row_index > 0 && row_index % 2 == 0,
        );
    }

    y - TABLE_TRAILING_GAP
}

#[derive(Clone, Debug)]
struct TableRowLayout {
    cells: Vec<Vec<LineLayout>>,
    height: f32,
}

fn table_column_widths(ctx: &LayoutContext, table: &Table) -> Vec<f32> {
    let cols = table
        .rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let available_width = ctx.page_w - ctx.margin_l - ctx.margin_r;
    let mut minimums = vec![32.0_f32; cols];
    let mut preferred = vec![32.0_f32; cols];
    for row in &table.rows {
        for (index, cell) in row.cells.iter().enumerate() {
            let text = cell_plain_text(cell);
            let longest_token = cell
                .paragraphs
                .iter()
                .flat_map(|paragraph| paragraph_runs(paragraph, TABLE_FONT_SIZE, &ctx.fonts))
                .flat_map(|run| {
                    run.text
                        .split_whitespace()
                        .map(|token| measure_text(token, run.size, run.font))
                        .collect::<Vec<_>>()
                })
                .fold(0.0_f32, f32::max);
            let minimum = (longest_token + TABLE_CELL_HORIZONTAL_PADDING + TABLE_WORD_FIT_SLOP)
                .clamp(32.0, available_width * 0.22);
            let ideal = (measure_text(&text, TABLE_FONT_SIZE, PdfFont::Helvetica)
                + TABLE_CELL_HORIZONTAL_PADDING)
                .clamp(minimum, available_width * 0.42);
            minimums[index] = minimums[index].max(minimum);
            preferred[index] = preferred[index].max(ideal);
        }
    }

    let minimum_total = minimums.iter().sum::<f32>();
    if minimum_total >= available_width {
        let scale = available_width / minimum_total;
        for width in &mut minimums {
            *width *= scale;
        }
        return minimums;
    }

    let remaining = available_width - minimum_total;
    let needs = preferred
        .iter()
        .zip(&minimums)
        .map(|(ideal, minimum)| (ideal - minimum).max(0.0))
        .collect::<Vec<_>>();
    let total_need = needs.iter().sum::<f32>();
    if total_need <= f32::EPSILON {
        let extra = remaining / cols as f32;
        return minimums.into_iter().map(|width| width + extra).collect();
    }

    minimums
        .into_iter()
        .zip(needs)
        .map(|(minimum, need)| minimum + remaining * need / total_need)
        .collect()
}

fn layout_table_row(
    row: &lo_core::TableRow,
    col_widths: &[f32],
    fonts: &FontResolver,
    header: bool,
) -> TableRowLayout {
    let mut cells = Vec::new();
    let mut height = 18.0_f32;
    for (index, cell) in row.cells.iter().enumerate() {
        let width = col_widths.get(index).copied().unwrap_or(48.0) - TABLE_CELL_HORIZONTAL_PADDING;
        let lines = layout_cell(cell, width.max(16.0), fonts, header);
        let content_height = lines.iter().map(table_line_height).sum::<f32>();
        height = height.max(content_height + TABLE_CELL_VERTICAL_PADDING);
        cells.push(lines);
    }
    TableRowLayout { cells, height }
}

fn draw_table_row(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: usize,
    top: f32,
    col_widths: &[f32],
    row: &TableRowLayout,
    header: bool,
    shaded: bool,
) -> f32 {
    let bottom = top - row.height;
    let page = pdf.page_mut(page_index).expect("page");
    let mut x = ctx.margin_l;
    for (index, width) in col_widths.iter().enumerate() {
        page.begin_artifact();
        if header {
            page.rect_fill_rgb(x, bottom, *width, row.height, 0.12, 0.18, 0.30);
        } else if shaded {
            page.rect_fill_rgb(x, bottom, *width, row.height, 0.965, 0.972, 0.982);
        }
        page.rect_stroke_rgb(x, bottom, *width, row.height, 0.72, 0.75, 0.79);
        page.end_artifact();

        let mut line_y = top - TABLE_CELL_VERTICAL_PADDING / 2.0 - TABLE_FONT_SIZE;
        if let Some(lines) = row.cells.get(index) {
            for line in lines {
                render_line(
                    page,
                    line,
                    x + TABLE_CELL_HORIZONTAL_PADDING / 2.0,
                    line_y,
                    Alignment::Start,
                    *width - TABLE_CELL_HORIZONTAL_PADDING,
                    false,
                    if header { "TH" } else { "TD" },
                );
                line_y -= table_line_height(line);
            }
        }
        x += *width;
    }
    bottom
}

fn table_line_height(line: &LineLayout) -> f32 {
    line.runs
        .iter()
        .map(|run| run.size * 1.25)
        .fold(TABLE_FONT_SIZE * 1.25, f32::max)
}

fn first_table_row_height(ctx: &LayoutContext, table: &Table) -> f32 {
    let widths = table_column_widths(ctx, table);
    table
        .rows
        .iter()
        .take(2)
        .enumerate()
        .map(|(row_index, row)| layout_table_row(row, &widths, &ctx.fonts, row_index == 0).height)
        .sum()
}

fn layout_cell(
    cell: &TableCell,
    width: f32,
    fonts: &FontResolver,
    header: bool,
) -> Vec<LineLayout> {
    let mut out = Vec::new();
    for paragraph in &cell.paragraphs {
        let mut runs = paragraph_runs(paragraph, TABLE_FONT_SIZE, fonts);
        if header {
            for run in &mut runs {
                run.font = PdfFont::HelveticaBold;
                run.color = (1.0, 1.0, 1.0);
                run.underline = false;
            }
        }
        let lines = layout_runs(&runs, width);
        out.extend(lines);
    }
    if out.is_empty() {
        out.push(LineLayout::default());
    }
    out
}

fn render_line(
    page: &mut lo_core::PdfPage,
    line: &LineLayout,
    x: f32,
    y: f32,
    alignment: Alignment,
    available_width: f32,
    justify: bool,
    semantic_role: &str,
) {
    let base_x = match alignment {
        Alignment::Center => x + ((available_width - line.width).max(0.0) / 2.0),
        Alignment::End => x + (available_width - line.width).max(0.0),
        _ => x,
    };
    let mut cursor = base_x;
    let space_slots = if justify {
        line.runs
            .iter()
            .map(|run| run.text.matches(' ').count())
            .sum::<usize>()
    } else {
        0
    };
    let extra_per_space = if space_slots > 0 {
        (available_width - line.width).max(0.0) / space_slots as f32
    } else {
        0.0
    };
    // Merge consecutive same-style runs (including the space tokens
    // produced by `tokenize_run`) into a single text-show operator. If we
    // emit each space as its own Tj, `pdftotext -raw` cannot tell that the
    // adjacent words are separated and ends up gluing them together
    // (e.g. "PHPWord search" -> "PHPWordsearch").
    let mut idx = 0usize;
    page.begin_tag(semantic_role, None);
    while idx < line.runs.len() {
        let start = idx;
        let head = &line.runs[idx];
        let mut combined = String::new();
        while idx < line.runs.len() {
            let cur = &line.runs[idx];
            if cur.font != head.font
                || cur.size != head.size
                || cur.color != head.color
                || cur.background != head.background
                || cur.underline != head.underline
                || cur.link != head.link
            {
                break;
            }
            combined.push_str(&cur.text);
            idx += 1;
        }
        let group_width: f32 = line.runs[start..idx]
            .iter()
            .map(|r| measure_text(&r.text, r.size, r.font))
            .sum();
        if !combined.is_empty() {
            if let Some(background) = head.background {
                page.begin_artifact();
                page.rect_fill_rgb(
                    cursor - 1.0,
                    y - 2.0,
                    group_width + 2.0,
                    head.size + 4.0,
                    background.0,
                    background.1,
                    background.2,
                );
                page.end_artifact();
            }
            page.text_rgb(
                cursor,
                y,
                head.size,
                head.font,
                &combined,
                head.color.0,
                head.color.1,
                head.color.2,
            );
            if let Some(uri) = &head.link {
                page.link(cursor, y - 2.0, group_width, head.size + 4.0, uri.clone());
            }
            if head.underline {
                page.begin_artifact();
                page.line_rgb(
                    cursor,
                    y - 1.5,
                    cursor + group_width,
                    y - 1.5,
                    head.color.0,
                    head.color.1,
                    head.color.2,
                );
                page.end_artifact();
            }
        }
        cursor += group_width;
        if extra_per_space > 0.0 {
            cursor += combined.matches(' ').count() as f32 * extra_per_space;
        }
    }
    page.end_tag();
}

fn paragraph_runs(
    paragraph: &Paragraph,
    default_size: f32,
    fonts: &FontResolver,
) -> Vec<StyledRun> {
    let family = if paragraph.text_style.font_family.trim().is_empty() {
        None
    } else {
        Some(paragraph.text_style.font_family.as_str())
    };
    let size = if paragraph.text_style.font_size_pt == 0 {
        default_size
    } else {
        paragraph.text_style.font_size_pt as f32
    };
    let base_color = parse_color(&paragraph.text_style.color).unwrap_or((0.0, 0.0, 0.0));
    let mut runs = Vec::new();
    for inline in &paragraph.spans {
        match inline {
            Inline::Text(text) => runs.push(StyledRun {
                text: text.clone(),
                font: fonts.pick(
                    family,
                    paragraph.text_style.bold,
                    paragraph.text_style.italic,
                ),
                size,
                color: base_color,
                background: None,
                underline: paragraph.text_style.underline,
                link: None,
            }),
            Inline::Bold(text) => runs.push(StyledRun {
                text: text.clone(),
                font: fonts.pick(family, true, paragraph.text_style.italic),
                size,
                color: base_color,
                background: None,
                underline: paragraph.text_style.underline,
                link: None,
            }),
            Inline::Italic(text) => runs.push(StyledRun {
                text: text.clone(),
                font: fonts.pick(family, paragraph.text_style.bold, true),
                size,
                color: base_color,
                background: None,
                underline: paragraph.text_style.underline,
                link: None,
            }),
            Inline::Code(text) => runs.push(StyledRun {
                text: text.clone(),
                font: PdfFont::Courier,
                size: (size - 1.0).max(9.0),
                color: (0.10, 0.10, 0.10),
                background: Some((0.95, 0.94, 0.91)),
                underline: false,
                link: None,
            }),
            Inline::Link { label, url } => runs.push(StyledRun {
                text: label.clone(),
                font: fonts.pick(family, false, false),
                size,
                color: (0.10, 0.25, 0.65),
                background: None,
                underline: true,
                link: Some(url.clone()),
            }),
            Inline::Styled { text, style, url } => {
                let styled_family = if style.font_family.trim().is_empty() {
                    family
                } else {
                    Some(style.font_family.as_str())
                };
                runs.push(StyledRun {
                    text: text.clone(),
                    font: fonts.pick(
                        styled_family,
                        style.bold || paragraph.text_style.bold,
                        style.italic || paragraph.text_style.italic,
                    ),
                    size: if style.font_size_pt == 0 {
                        size
                    } else {
                        style.font_size_pt as f32
                    },
                    color: parse_color(&style.color).unwrap_or(base_color),
                    background: parse_color(&style.background),
                    underline: style.underline || paragraph.text_style.underline,
                    link: url.clone(),
                });
            }
            Inline::LineBreak => runs.push(StyledRun {
                text: "\n".to_string(),
                font: fonts.pick(family, false, false),
                size,
                color: base_color,
                background: None,
                underline: false,
                link: None,
            }),
        }
    }
    if runs.is_empty() {
        runs.push(StyledRun {
            text: String::new(),
            font: fonts.pick(family, false, false),
            size,
            color: base_color,
            background: None,
            underline: false,
            link: None,
        });
    }
    runs
}

fn layout_runs(runs: &[StyledRun], max_width: f32) -> Vec<LineLayout> {
    let mut lines = Vec::new();
    let mut current = LineLayout::default();
    for run in runs {
        for token in tokenize_run(run) {
            if token.text == "\n" {
                lines.push(std::mem::take(&mut current));
                continue;
            }
            let token_width = measure_text(&token.text, token.size, token.font);
            let is_space = token
                .text
                .chars()
                .all(|ch| ch.is_whitespace() && ch != '\u{a0}');
            if !is_space && token_width > max_width {
                if !current.runs.is_empty() {
                    trim_trailing_spaces(&mut current);
                    lines.push(std::mem::take(&mut current));
                }
                for fragment in split_run_to_width(&token, max_width) {
                    let fragment_width = measure_text(&fragment.text, fragment.size, fragment.font);
                    if !current.runs.is_empty() && current.width + fragment_width > max_width {
                        lines.push(std::mem::take(&mut current));
                    }
                    current.width += fragment_width;
                    current.runs.push(fragment);
                }
                continue;
            }
            if !current.runs.is_empty() && current.width + token_width > max_width && !is_space {
                trim_trailing_spaces(&mut current);
                lines.push(std::mem::take(&mut current));
            }
            if current.runs.is_empty() && is_space {
                continue;
            }
            current.width += token_width;
            current.runs.push(token);
        }
    }
    trim_trailing_spaces(&mut current);
    if !current.runs.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn split_run_to_width(run: &StyledRun, max_width: f32) -> Vec<StyledRun> {
    let mut fragments = Vec::new();
    let mut buffer = String::new();
    let mut width = 0.0_f32;
    for character in run.text.chars() {
        let character_width = measure_text(&character.to_string(), run.size, run.font);
        if !buffer.is_empty() && width + character_width > max_width {
            fragments.push(styled_run_with_text(run, std::mem::take(&mut buffer)));
            width = 0.0;
        }
        buffer.push(character);
        width += character_width;
    }
    if !buffer.is_empty() {
        fragments.push(styled_run_with_text(run, buffer));
    }
    fragments
}

fn styled_run_with_text(template: &StyledRun, text: String) -> StyledRun {
    StyledRun {
        text,
        font: template.font,
        size: template.size,
        color: template.color,
        background: template.background,
        underline: template.underline,
        link: template.link.clone(),
    }
}

fn tokenize_run(run: &StyledRun) -> Vec<StyledRun> {
    let mut out = Vec::new();
    let mut buffer = String::new();
    let flush = |out: &mut Vec<StyledRun>, buffer: &mut String, template: &StyledRun| {
        if !buffer.is_empty() {
            out.push(StyledRun {
                text: std::mem::take(buffer),
                font: template.font,
                size: template.size,
                color: template.color,
                background: template.background,
                underline: template.underline,
                link: template.link.clone(),
            });
        }
    };
    for ch in run.text.chars() {
        if ch == '\n' {
            flush(&mut out, &mut buffer, run);
            out.push(StyledRun {
                text: "\n".to_string(),
                font: run.font,
                size: run.size,
                color: run.color,
                background: run.background,
                underline: run.underline,
                link: run.link.clone(),
            });
            continue;
        }
        if ch.is_whitespace() && ch != '\u{a0}' {
            flush(&mut out, &mut buffer, run);
            out.push(StyledRun {
                text: ch.to_string(),
                font: run.font,
                size: run.size,
                color: run.color,
                background: run.background,
                underline: run.underline,
                link: run.link.clone(),
            });
            continue;
        }
        buffer.push(ch);
        if matches!(ch, '/' | '-' | '_' | '?' | '&' | '=') {
            flush(&mut out, &mut buffer, run);
        }
    }
    flush(&mut out, &mut buffer, run);
    out
}

fn trim_trailing_spaces(line: &mut LineLayout) {
    while matches!(line.runs.last(), Some(run) if run.text.chars().all(|ch| ch.is_whitespace() && ch != '\u{a0}'))
    {
        if let Some(run) = line.runs.pop() {
            line.width -= measure_text(&run.text, run.size, run.font);
        }
    }
}

fn cell_plain_text(cell: &TableCell) -> String {
    cell.paragraphs
        .iter()
        .map(|paragraph| {
            paragraph
                .spans
                .iter()
                .map(inline_text)
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn inline_text(inline: &Inline) -> String {
    match inline {
        Inline::Text(text)
        | Inline::Bold(text)
        | Inline::Italic(text)
        | Inline::Code(text)
        | Inline::Styled { text, .. } => text.clone(),
        Inline::Link { label, .. } => label.clone(),
        Inline::LineBreak => "\n".to_string(),
    }
}

fn register_pdf_image(pdf: &mut PdfDocument, image: &ImageBlock) -> Option<String> {
    match image.mime_type.as_str() {
        "image/png" => {
            let decoded = decode_png(&image.data).ok()?;
            Some(register_raster_image(pdf, decoded))
        }
        "image/webp" => {
            let decoded = decode_webp(&image.data).ok()?;
            Some(register_raster_image(pdf, decoded))
        }
        "image/jpeg" => {
            let (width, height) = jpeg_dimensions(&image.data)?;
            Some(pdf.add_jpeg_image(width, height, image.data.clone()))
        }
        _ => None,
    }
}

fn register_raster_image(pdf: &mut PdfDocument, decoded: lo_core::RasterImage) -> String {
    pdf.add_rgba_image(decoded.width, decoded.height, decoded.pixels)
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

fn render_svg_image(
    page: &mut lo_core::PdfPage,
    bytes: &[u8],
    x: f32,
    bottom: f32,
    width: f32,
    height: f32,
) -> bool {
    let Ok(svg) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Ok(root) = parse_xml_document(svg) else {
        return false;
    };
    if root.local_name() != "svg" {
        return false;
    }
    let view_box = root
        .attr("viewBox")
        .or_else(|| root.attr("viewbox"))
        .and_then(parse_number_list)
        .filter(|values| values.len() == 4);
    let (min_x, min_y, svg_width, svg_height) = if let Some(values) = view_box {
        (values[0], values[1], values[2].max(1.0), values[3].max(1.0))
    } else {
        (
            0.0,
            0.0,
            svg_length(root.attr("width")).unwrap_or(100.0).max(1.0),
            svg_length(root.attr("height")).unwrap_or(100.0).max(1.0),
        )
    };
    let transform = SvgTransform {
        x,
        bottom,
        width,
        height,
        min_x,
        min_y,
        svg_width,
        svg_height,
    };
    for child in &root.children {
        render_svg_node(page, child, transform);
    }
    true
}

#[derive(Clone, Copy)]
struct SvgTransform {
    x: f32,
    bottom: f32,
    width: f32,
    height: f32,
    min_x: f32,
    min_y: f32,
    svg_width: f32,
    svg_height: f32,
}

impl SvgTransform {
    fn x(self, value: f32) -> f32 {
        self.x + (value - self.min_x) * self.width / self.svg_width
    }

    fn y(self, value: f32) -> f32 {
        self.bottom + self.height - (value - self.min_y) * self.height / self.svg_height
    }

    fn w(self, value: f32) -> f32 {
        value * self.width / self.svg_width
    }

    fn h(self, value: f32) -> f32 {
        value * self.height / self.svg_height
    }
}

fn render_svg_node(page: &mut lo_core::PdfPage, node: &XmlNode, transform: SvgTransform) {
    if matches!(node.local_name(), "g" | "svg") {
        for child in &node.children {
            render_svg_node(page, child, transform);
        }
        return;
    }
    let fill = svg_paint(node, "fill", Some((0.12, 0.12, 0.12)));
    let stroke = svg_paint(node, "stroke", Some((0.20, 0.20, 0.20)));
    let stroke_width = svg_style_value(node, "stroke-width")
        .and_then(|value| svg_length(Some(value)))
        .unwrap_or(1.0);
    page.line_width(transform.w(stroke_width).max(0.4));
    match node.local_name() {
        "rect" => {
            let sx = transform.x(svg_number(node, "x", 0.0));
            let sy = transform.y(svg_number(node, "y", 0.0) + svg_number(node, "height", 0.0));
            let sw = transform.w(svg_number(node, "width", 0.0));
            let sh = transform.h(svg_number(node, "height", 0.0));
            match (fill, stroke) {
                (Some(fill), Some(stroke)) => {
                    page.rect_fill_stroke_rgb(sx, sy, sw, sh, fill, stroke)
                }
                (Some(fill), None) => page.rect_fill_rgb(sx, sy, sw, sh, fill.0, fill.1, fill.2),
                (None, Some(stroke)) => {
                    page.rect_stroke_rgb(sx, sy, sw, sh, stroke.0, stroke.1, stroke.2)
                }
                _ => {}
            }
        }
        "circle" | "ellipse" => {
            let cx = transform.x(svg_number(node, "cx", 0.0));
            let cy = transform.y(svg_number(node, "cy", 0.0));
            let rx = transform.w(if node.local_name() == "circle" {
                svg_number(node, "r", 0.0)
            } else {
                svg_number(node, "rx", 0.0)
            });
            let ry = transform.h(if node.local_name() == "circle" {
                svg_number(node, "r", 0.0)
            } else {
                svg_number(node, "ry", 0.0)
            });
            match (fill, stroke) {
                (Some(fill), Some(stroke)) => {
                    page.ellipse_fill_stroke_rgb(cx, cy, rx, ry, fill, stroke)
                }
                (_, Some(stroke)) => {
                    page.ellipse_stroke_rgb(cx, cy, rx, ry, stroke.0, stroke.1, stroke.2)
                }
                (Some(fill), None) => page.ellipse_fill_stroke_rgb(cx, cy, rx, ry, fill, fill),
                _ => {}
            }
        }
        "line" => {
            if let Some(stroke) = stroke {
                page.line_rgb(
                    transform.x(svg_number(node, "x1", 0.0)),
                    transform.y(svg_number(node, "y1", 0.0)),
                    transform.x(svg_number(node, "x2", 0.0)),
                    transform.y(svg_number(node, "y2", 0.0)),
                    stroke.0,
                    stroke.1,
                    stroke.2,
                );
            }
        }
        "polygon" | "polyline" => {
            if let Some(values) = node.attr("points").and_then(parse_number_list) {
                let points = values
                    .chunks_exact(2)
                    .map(|point| (transform.x(point[0]), transform.y(point[1])))
                    .collect::<Vec<_>>();
                if node.local_name() == "polygon" && points.len() >= 3 {
                    let fill = fill.unwrap_or((0.42, 0.35, 0.82));
                    page.polygon_fill_stroke_rgb(
                        &points,
                        fill,
                        stroke.unwrap_or((0.20, 0.17, 0.38)),
                    );
                } else if let Some(stroke) = stroke {
                    for pair in points.windows(2) {
                        page.line_rgb(
                            pair[0].0, pair[0].1, pair[1].0, pair[1].1, stroke.0, stroke.1,
                            stroke.2,
                        );
                    }
                }
            }
        }
        "text" => {
            let color = fill.or(stroke).unwrap_or((0.10, 0.10, 0.10));
            let size = transform
                .h(svg_style_value(node, "font-size")
                    .and_then(|v| svg_length(Some(v)))
                    .unwrap_or(14.0))
                .clamp(7.0, 32.0);
            let font = if svg_style_value(node, "font-weight")
                .is_some_and(|value| value.eq_ignore_ascii_case("bold") || value == "700")
            {
                PdfFont::HelveticaBold
            } else {
                PdfFont::Helvetica
            };
            page.text_rgb(
                transform.x(svg_number(node, "x", 0.0)),
                transform.y(svg_number(node, "y", 0.0)),
                size,
                font,
                &node.text_content(),
                color.0,
                color.1,
                color.2,
            );
        }
        _ => {}
    }
    page.line_width(1.0);
}

fn svg_number(node: &XmlNode, name: &str, fallback: f32) -> f32 {
    svg_length(node.attr(name)).unwrap_or(fallback)
}

fn svg_length(value: Option<&str>) -> Option<f32> {
    value?
        .trim()
        .trim_end_matches(|ch: char| ch.is_ascii_alphabetic() || ch == '%')
        .parse()
        .ok()
}

fn parse_number_list(value: &str) -> Option<Vec<f32>> {
    let numbers = value
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .map(str::parse::<f32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    (!numbers.is_empty()).then_some(numbers)
}

fn svg_style_value<'a>(node: &'a XmlNode, name: &str) -> Option<&'a str> {
    if let Some(value) = node.attr(name) {
        return Some(value);
    }
    node.attr("style")?.split(';').find_map(|declaration| {
        let (property, value) = declaration.split_once(':')?;
        property
            .trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn svg_paint(
    node: &XmlNode,
    name: &str,
    fallback: Option<(f32, f32, f32)>,
) -> Option<(f32, f32, f32)> {
    let Some(value) = svg_style_value(node, name) else {
        return fallback;
    };
    if value.eq_ignore_ascii_case("none") {
        return None;
    }
    parse_color(&normalize_named_color(value)).or(fallback)
}

fn normalize_named_color(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "red" => "#d32f2f",
        "green" => "#2e7d32",
        "blue" => "#1565c0",
        "purple" => "#6d4cc7",
        "orange" => "#ef8c32",
        "white" => "#ffffff",
        "black" => "#000000",
        "gray" | "grey" => "#777777",
        _ => value,
    }
    .to_string()
}

#[derive(Clone, Debug)]
struct MermaidNode {
    id: String,
    label: String,
    decision: bool,
}

fn render_mermaid(
    page: &mut lo_core::PdfPage,
    bytes: &[u8],
    x: f32,
    bottom: f32,
    width: f32,
    height: f32,
) -> bool {
    let source = String::from_utf8_lossy(bytes);
    let horizontal = source.lines().next().is_some_and(|line| {
        let upper = line.to_ascii_uppercase();
        upper.contains(" LR") || upper.ends_with("LR")
    });
    let mut nodes = Vec::<MermaidNode>::new();
    let mut edges = Vec::<(String, String)>::new();
    for line in source.lines().map(str::trim) {
        if line.is_empty()
            || line.starts_with("flowchart")
            || line.starts_with("graph")
            || line.starts_with("%%")
        {
            continue;
        }
        if let Some((left, right)) = line.split_once("-->") {
            let source_node = parse_mermaid_node(left.trim());
            let right = right.trim().trim_start_matches('|');
            let right = right.split_once('|').map_or(right, |(_, rest)| rest.trim());
            let target_node = parse_mermaid_node(right.trim());
            add_mermaid_node(&mut nodes, source_node.clone());
            add_mermaid_node(&mut nodes, target_node.clone());
            edges.push((source_node.id, target_node.id));
        } else {
            add_mermaid_node(&mut nodes, parse_mermaid_node(line));
        }
    }
    if nodes.is_empty() {
        return false;
    }
    page.rect_fill_stroke_rgb(
        x,
        bottom,
        width,
        height,
        (0.98, 0.975, 0.99),
        (0.78, 0.74, 0.88),
    );
    let count = nodes.len() as f32;
    let node_width = if horizontal {
        (width / count - 18.0).clamp(70.0, 125.0)
    } else {
        (width * 0.46).clamp(110.0, 190.0)
    };
    let node_height = 38.0;
    let positions = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let (cx, cy) = if horizontal {
                (
                    x + width * (index as f32 + 0.5) / count,
                    bottom + height / 2.0,
                )
            } else {
                (
                    x + width / 2.0,
                    bottom + height - height * (index as f32 + 0.5) / count,
                )
            };
            (node.id.clone(), cx, cy)
        })
        .collect::<Vec<_>>();
    for (source, target) in &edges {
        let Some((_, sx, sy)) = positions.iter().find(|(id, _, _)| id == source) else {
            continue;
        };
        let Some((_, tx, ty)) = positions.iter().find(|(id, _, _)| id == target) else {
            continue;
        };
        let (start_x, start_y, end_x, end_y) = if horizontal {
            (sx + node_width / 2.0, *sy, tx - node_width / 2.0, *ty)
        } else {
            (*sx, sy - node_height / 2.0, *tx, ty + node_height / 2.0)
        };
        page.line_rgb(start_x, start_y, end_x, end_y, 0.35, 0.31, 0.49);
        let angle = (end_y - start_y).atan2(end_x - start_x);
        let arrow = 7.0;
        page.polygon_fill_stroke_rgb(
            &[
                (end_x, end_y),
                (
                    end_x - arrow * (angle - 0.55).cos(),
                    end_y - arrow * (angle - 0.55).sin(),
                ),
                (
                    end_x - arrow * (angle + 0.55).cos(),
                    end_y - arrow * (angle + 0.55).sin(),
                ),
            ],
            (0.35, 0.31, 0.49),
            (0.35, 0.31, 0.49),
        );
    }
    for (node, (_, cx, cy)) in nodes
        .iter()
        .zip(positions.iter().map(|(_, x, y)| ((), *x, *y)))
    {
        let fill = if node.decision {
            (0.98, 0.76, 0.42)
        } else {
            (0.42, 0.35, 0.82)
        };
        if node.decision {
            page.polygon_fill_stroke_rgb(
                &[
                    (cx, cy + node_height / 2.0),
                    (cx + node_width / 2.0, cy),
                    (cx, cy - node_height / 2.0),
                    (cx - node_width / 2.0, cy),
                ],
                fill,
                (0.28, 0.23, 0.43),
            );
        } else {
            page.rect_fill_stroke_rgb(
                cx - node_width / 2.0,
                cy - node_height / 2.0,
                node_width,
                node_height,
                fill,
                (0.28, 0.23, 0.43),
            );
        }
        let text_color = if node.decision {
            (0.18, 0.13, 0.08)
        } else {
            (1.0, 1.0, 1.0)
        };
        let label_width = measure_text(&node.label, 10.0, PdfFont::HelveticaBold);
        page.text_rgb(
            cx - label_width / 2.0,
            cy - 3.5,
            10.0,
            PdfFont::HelveticaBold,
            &node.label,
            text_color.0,
            text_color.1,
            text_color.2,
        );
    }
    true
}

fn add_mermaid_node(nodes: &mut Vec<MermaidNode>, node: MermaidNode) {
    if !node.id.is_empty() && !nodes.iter().any(|existing| existing.id == node.id) {
        nodes.push(node);
    }
}

fn parse_mermaid_node(token: &str) -> MermaidNode {
    let token = token.trim().trim_end_matches(';');
    let boundary = token
        .find(|ch| matches!(ch, '[' | '{' | '('))
        .unwrap_or(token.len());
    let id = token[..boundary].trim().to_string();
    let decision = token[boundary..].starts_with('{');
    let label = if boundary < token.len() {
        token[boundary + 1..]
            .trim_end_matches(|ch| matches!(ch, ']' | '}' | ')'))
            .trim_matches(['\'', '\"'])
            .to_string()
    } else {
        id.clone()
    };
    MermaidNode {
        id,
        label,
        decision,
    }
}

fn render_formula(
    page: &mut lo_core::PdfPage,
    bytes: &[u8],
    x: f32,
    bottom: f32,
    width: f32,
    height: f32,
) -> bool {
    let source = String::from_utf8_lossy(bytes);
    page.rect_fill_stroke_rgb(
        x,
        bottom,
        width,
        height,
        (0.975, 0.97, 0.99),
        (0.75, 0.70, 0.86),
    );
    let formula = source.trim().replace(['\n', '\r'], " ");
    let Some(frac_start) = formula.find("\\frac{") else {
        let text = readable_latex(&formula);
        let size = 18.0;
        let text_width = measure_text(&text, size, PdfFont::TimesItalic);
        page.text_rgb(
            x + (width - text_width) / 2.0,
            bottom + height / 2.0 - 6.0,
            size,
            PdfFont::TimesItalic,
            &text,
            0.20,
            0.16,
            0.33,
        );
        return true;
    };
    let prefix = readable_latex(&formula[..frac_start]);
    let fraction_source = &formula[frac_start + "\\frac".len()..];
    let Some((numerator, after_numerator)) = take_braced(fraction_source) else {
        return false;
    };
    let Some((denominator, suffix)) = take_braced(after_numerator) else {
        return false;
    };
    let numerator = readable_latex(numerator);
    let denominator = readable_latex(denominator);
    let suffix = readable_latex(suffix);
    let text_size = 17.0;
    let fraction_size = 13.0;
    let prefix_width = measure_text(&prefix, text_size, PdfFont::TimesItalic);
    let numerator_width = measure_text(&numerator, fraction_size, PdfFont::TimesItalic);
    let denominator_width = measure_text(&denominator, fraction_size, PdfFont::TimesItalic);
    let fraction_width = numerator_width.max(denominator_width) + 12.0;
    let suffix_width = measure_text(&suffix, text_size, PdfFont::TimesItalic);
    let total_width = prefix_width + fraction_width + suffix_width;
    let start = x + (width - total_width) / 2.0;
    let center_y = bottom + height / 2.0;
    page.text_rgb(
        start,
        center_y - 5.0,
        text_size,
        PdfFont::TimesItalic,
        &prefix,
        0.20,
        0.16,
        0.33,
    );
    let fraction_x = start + prefix_width;
    page.text_rgb(
        fraction_x + (fraction_width - numerator_width) / 2.0,
        center_y + 7.0,
        fraction_size,
        PdfFont::TimesItalic,
        &numerator,
        0.20,
        0.16,
        0.33,
    );
    page.line_rgb(
        fraction_x + 3.0,
        center_y + 3.0,
        fraction_x + fraction_width - 3.0,
        center_y + 3.0,
        0.20,
        0.16,
        0.33,
    );
    page.text_rgb(
        fraction_x + (fraction_width - denominator_width) / 2.0,
        center_y - 12.0,
        fraction_size,
        PdfFont::TimesItalic,
        &denominator,
        0.20,
        0.16,
        0.33,
    );
    page.text_rgb(
        fraction_x + fraction_width,
        center_y - 5.0,
        text_size,
        PdfFont::TimesItalic,
        &suffix,
        0.20,
        0.16,
        0.33,
    );
    true
}

fn take_braced(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    let inner = input.strip_prefix('{')?;
    let mut depth = 1usize;
    for (index, ch) in inner.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&inner[..index], &inner[index + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn readable_latex(source: &str) -> String {
    source
        .replace("\\times", "x")
        .replace("\\cdot", "·")
        .replace("\\pi", "pi")
        .replace("\\alpha", "alpha")
        .replace("\\beta", "beta")
        .replace("\\sqrt", "sqrt")
        .replace("^2", "²")
        .replace("^3", "³")
        .replace(['{', '}'], "")
}

fn ensure_room(
    ctx: &LayoutContext,
    pdf: &mut PdfDocument,
    page_index: &mut usize,
    y: f32,
    needed: f32,
) -> f32 {
    if y - needed >= ctx.margin_b {
        y
    } else {
        *page_index = pdf.add_page(ctx.page_w, ctx.page_h);
        ctx.page_h - ctx.margin_t
    }
}

fn parse_color(input: &str) -> Option<(f32, f32, f32)> {
    let trimmed = input.trim().trim_start_matches('#');
    if trimmed.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&trimmed[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&trimmed[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&trimmed[4..6], 16).ok()? as f32 / 255.0;
    Some((r, g, b))
}

fn mm_to_pt(mm: u16) -> f32 {
    mm as f32 * 72.0 / 25.4
}

impl LayoutContext {
    fn from_page_style(style: &PageStyle) -> Self {
        let page_w = mm_to_pt(style.width_mm.max(1));
        let page_h = mm_to_pt(style.height_mm.max(1));
        let margin = mm_to_pt(style.margin_mm.max(10));
        Self {
            page_w,
            page_h,
            margin_l: margin,
            margin_r: margin,
            margin_t: margin,
            margin_b: margin,
            fonts: FontResolver::scan(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct FontResolver {
    installed: BTreeSet<String>,
}

impl FontResolver {
    fn scan() -> Self {
        let mut installed = BTreeSet::new();
        let mut roots = vec![
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            roots.push(PathBuf::from(home).join(".fonts"));
        }
        for root in roots {
            collect_font_names(&root, &mut installed);
        }
        Self { installed }
    }

    fn pick(&self, requested: Option<&str>, bold: bool, italic: bool) -> PdfFont {
        let request = requested.unwrap_or("").to_ascii_lowercase();
        let serif = request.contains("times")
            || request.contains("serif")
            || request.contains("georgia")
            || request.contains("cambria")
            || request.contains("garamond");
        let mono = request.contains("mono")
            || request.contains("code")
            || request.contains("courier")
            || request.contains("consolas");
        let known = if request.is_empty() {
            false
        } else {
            self.installed.iter().any(|name| name.contains(&request))
        };
        if mono {
            return PdfFont::Courier;
        }
        if serif || known && (request.contains("times") || request.contains("serif")) {
            return match (bold, italic) {
                (true, _) => PdfFont::TimesBold,
                (false, true) => PdfFont::TimesItalic,
                _ => PdfFont::TimesRoman,
            };
        }
        match (bold, italic) {
            (true, _) => PdfFont::HelveticaBold,
            (false, true) => PdfFont::HelveticaOblique,
            _ => PdfFont::Helvetica,
        }
    }
}

fn collect_font_names(root: &PathBuf, out: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_names(&path, out);
        } else if let Some(ext) = path.extension().and_then(|value| value.to_str()) {
            if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc") {
                if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                    out.insert(stem.to_ascii_lowercase());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{layout_runs, StyledRun};
    use lo_core::PdfFont;

    #[test]
    fn unbroken_urls_wrap_inside_the_available_width() {
        let run = StyledRun {
            text: concat!(
                "https://example.com/a/very/long/path/that/must/not/",
                "paint/outside/the/page",
            )
            .to_string(),
            font: PdfFont::Helvetica,
            size: 11.0,
            color: (0.0, 0.0, 0.0),
            background: None,
            underline: false,
            link: None,
        };
        let lines = layout_runs(&[run], 96.0);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.width <= 96.01));
    }
}

#[cfg(test)]
mod odf_box_tests {
    use super::*;

    #[test]
    fn explicit_indentation_is_not_trimmed_by_wrapping() {
        let run = StyledRun {
            text: "\u{a0}\u{a0}\u{a0}\u{a0}code".into(),
            font: PdfFont::Courier,
            size: 10.0,
            color: (0.0, 0.0, 0.0),
            background: None,
            underline: false,
            link: None,
        };
        let lines = layout_runs(&[run], 100.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].width, 48.0);
        assert!(lines[0].runs[0]
            .text
            .starts_with("\u{a0}\u{a0}\u{a0}\u{a0}"));
    }

    #[test]
    fn odf_heading_keeps_source_font_and_spacing() {
        let mut paragraph = Paragraph::default();
        paragraph.style.source_spacing = true;
        paragraph.text_style.font_size_pt = 13;
        paragraph.style.margin_bottom_mm = 0;
        let heading = Heading {
            level: 1,
            content: paragraph.clone(),
        };
        let styled = styled_heading_paragraph(&heading, 20.0);
        assert_eq!(styled, paragraph);
    }
}
