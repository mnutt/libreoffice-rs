//! PDF export of a `Presentation`.
//!
//! Each slide is rendered onto its own page using the shared pure-Rust
//! PDF canvas. The renderer now respects shape fill/stroke colors,
//! textbox padding/backgrounds, and wraps textbox content to the frame.

use lo_core::{PdfDocument, PdfFont, Presentation, ShapeKind, SlideElement, TextBoxStyle};

use crate::chart::render_chart_rows_pdf;

pub fn to_pdf(presentation: &Presentation) -> Vec<u8> {
    let slide_w = presentation.page_size.width.as_pt().max(1.0);
    let slide_h = presentation.page_size.height.as_pt().max(1.0);
    let mut pdf = PdfDocument::new();

    if presentation.slides.is_empty() {
        let page_index = pdf.add_page(slide_w, slide_h);
        if let Ok(page) = pdf.page_mut(page_index) {
            page.rect_fill_rgb(0.0, 0.0, slide_w, slide_h, 1.0, 1.0, 1.0);
            page.text(24.0, slide_h - 32.0, 24.0, PdfFont::HelveticaBold, &presentation.meta.title);
        }
        return pdf.finish();
    }

    // Images shared by slide masters should occupy one PDF resource per document.
    // Borrow the original bytes so cache keys do not duplicate image allocations.
    let mut image_cache = std::collections::HashMap::new();
    for slide in &presentation.slides {
        let resources: std::collections::BTreeMap<usize, String> = slide
            .elements
            .iter()
            .enumerate()
            .filter_map(|(index, element)| {
                if let SlideElement::Image(image) = element {
                    let svg_size = if image.mime_type == "image/svg+xml" {
                        (image.frame.size.width.as_pt().to_bits(),image.frame.size.height.as_pt().to_bits())
                    } else {(0,0)};
                    let key=(image.mime_type.as_str(),image.data.as_slice(),image.opacity,svg_size);
                    if let Some(name)=image_cache.get(&key) { return Some((index,String::clone(name))); }
                    register_image(&mut pdf, image).map(|name| {image_cache.insert(key,name.clone());(index,name)})
                } else {
                    None
                }
            })
            .collect();
        let page_index = pdf.add_page(slide_w, slide_h);
        let page = pdf.page_mut(page_index).expect("slide page");
        let background = slide
            .background
            .as_deref()
            .and_then(parse_color)
            .unwrap_or((1.0, 1.0, 1.0));
        page.rect_fill_rgb(
            0.0,
            0.0,
            slide_w,
            slide_h,
            background.0,
            background.1,
            background.2,
        );

        for (element_index, element) in slide.elements.iter().enumerate() {
            let transform=match element{SlideElement::TextBox(t)=>t.style.transform,SlideElement::Shape(s)=>s.style.transform,SlideElement::Image(i)=>i.transform};
            if let Some([a,b,c,d,e,f])=transform {page.raw(&format!("q {a} {} {} {d} {} {} cm",-b,-c,e+c*slide_h,slide_h*(1.0-d)-f));}
            match element {
                SlideElement::TextBox(text_box) => {
                    render_text_box(page, slide_h, text_box);
                }
                SlideElement::Shape(shape) => {
                    let x = shape.frame.origin.x.as_pt();
                    let y_top = shape.frame.origin.y.as_pt();
                    let w = shape.frame.size.width.as_pt().max(0.0);
                    let h = shape.frame.size.height.as_pt().max(0.0);
                    let y_bottom = slide_h - y_top - h;
                    let angle = (shape.style.rotation_millidegrees as f32 / 1000.0).to_radians();
                    let (sin, cos) = angle.sin_cos();
                    let px = x+w/2.0;
                    let py = slide_h - y_top-h/2.0;
                    page.raw(&format!(
                        "q {cos} {sin} {} {cos} {} {} cm",
                        -sin,
                        px - cos * px + sin * py,
                        py - sin * px - cos * py
                    ));
                    let fill = parse_color(&shape.style.fill).unwrap_or((1.0, 1.0, 1.0));
                    let stroke = parse_color(&shape.style.stroke).unwrap_or((0.0, 0.0, 0.0));
                    let stroke_width = shape.style.stroke_width_mpt.map(|v|v as f32/1000.0).unwrap_or_else(||(shape.style.stroke_width_mm as f32 * 72.0 / 25.4).max(0.75));
                    page.line_width(stroke_width);
                    if !shape.style.stroke_dash_mpt.is_empty() {
                        let pattern = shape.style.stroke_dash_mpt.iter().map(|v| ( *v as f32 / 1000.0).to_string()).collect::<Vec<_>>().join(" ");
                        page.raw(&format!("[{pattern}] 0 d"));
                    }

                    match shape.kind {
                        ShapeKind::Custom(ref paths) => {
                            for path in paths {path.render_pdf(page,[x,slide_h-y_top,w,h],parse_color(&shape.style.fill),parse_color(&shape.style.stroke));}
                        }
                        ShapeKind::Rectangle => {
                            if shape.style.fill != "none" {
                                page.rect_fill_rgb(x, y_bottom, w, h, fill.0, fill.1, fill.2);
                            }
                            if shape.style.stroke != "none" {
                                page.rect_stroke_rgb(
                                    x, y_bottom, w, h, stroke.0, stroke.1, stroke.2,
                                );
                            }
                        }
                        ShapeKind::Ellipse => {
                            page.ellipse_fill_stroke_rgb(x + w / 2.0, y_bottom + h / 2.0, w / 2.0, h / 2.0, fill, stroke);
                        }
                        ShapeKind::Line => {
                            page.line_rgb(x, slide_h - y_top, x + w, slide_h - y_top - h, stroke.0, stroke.1, stroke.2);
                        }
                    }
                    page.line_width(1.0);
                    page.raw("Q");
                }
                SlideElement::Image(image) => {
                    let x = image.frame.origin.x.as_pt();
                    let y =
                        slide_h - image.frame.origin.y.as_pt() - image.frame.size.height.as_pt();
                    let w = image.frame.size.width.as_pt();
                    let h = image.frame.size.height.as_pt();
                    if let Some(resource) = resources.get(&element_index) {
                        page.begin_tag("Figure", Some(&image.alt));
                        page.raw("q");
                        let angle=-image.rotation.to_radians();let (sin,cos)=angle.sin_cos();
                        let sx=if image.flip_horizontal{-1.0}else{1.0};let sy=if image.flip_vertical{-1.0}else{1.0};
                        let (a,b,c,d)=(cos*sx,sin*sx,-sin*sy,cos*sy);let (cx,cy)=(x+w/2.0,y+h/2.0);
                        page.raw(&format!("{a} {b} {c} {d} {} {} cm",cx-a*cx-c*cy,cy-b*cx-d*cy));
                        page.raw(&format!("{x} {y} {w} {h} re W n"));
                        let [left,top,right,bottom]=image.crop;
                        let iw=w/(1.0-left-right).max(0.001);let ih=h/(1.0-top-bottom).max(0.001);
                        page.image(resource,x-left*iw,y-bottom*ih,iw,ih);
                        page.raw("Q");
                        page.end_tag();
                    } else {
                        page.rect_stroke(x, y, w, h);
                    }
                }
            }
            if transform.is_some(){page.raw("Q");}
        }

        for chart in &slide.odf_charts {
            chart.render(page, slide_h, 0.0, 0.0);
        }
        if !slide.chart_tokens.is_empty() {
            render_chart_rows_pdf(page, slide_h, slide_w, &slide.chart_tokens);
        }
        if !slide.notes.is_empty() {
            let band_h = 48.0f32.min(slide_h * 0.18);
            page.rect_fill_rgb(0.0, 0.0, slide_w, band_h, 0.96, 0.96, 0.96);
            let mut y = band_h - 14.0;
            for note in slide.notes.iter().take(3) {
                page.text_rgb(18.0, y, 10.5, PdfFont::HelveticaOblique, note, 0.20, 0.20, 0.20);
                y -= 12.0;
            }
        }

    }

    pdf.finish()
}

fn render_text_box(page: &mut lo_core::PdfPage, slide_h: f32, text_box: &lo_core::TextBox) {
    let x = text_box.frame.origin.x.as_pt();
    let y_top = text_box.frame.origin.y.as_pt();
    let w = text_box.frame.size.width.as_pt().max(16.0);
    let h = text_box.frame.size.height.as_pt().max(12.0);
    let y_bottom = slide_h - y_top - h;
    let style = &text_box.style;
    let padding = style.padding.as_pt().max(0.0);
    if style.rotation != 0.0 {
        let (sin, cos) = style.rotation.sin_cos();
        let (px,origin_y)=style.rotation_origin.unwrap_or((x,y_top));
        let py = slide_h - origin_y;
        page.raw(&format!(
            "q {cos} {sin} {} {cos} {} {} cm",
            -sin,
            px - cos * px + sin * py,
            py - sin * px - cos * py
        ));
    }
    if let Some(background) = parse_color(&style.background) {
        page.rect_fill_stroke_rgb(x, y_bottom, w, h, background, (0.75, 0.75, 0.75));
    }
    if !style.runs.is_empty() {
        crate::pptx_text::render(page, slide_h-y_top-padding, x+padding, (w-padding*2.0).max(1.0), (h-padding*2.0).max(1.0), style);
        if style.rotation != 0.0 { page.raw("Q"); }
        return;
    }
    let (font, size) = if style.font_size_pt > 0.0 {
        (text_box_font(style), style.font_size_pt)
    } else {
        pick_text_box_font(text_box.text.lines().next().unwrap_or(""), y_top, h)
    };
    let color = parse_color(&style.foreground).unwrap_or((0.0, 0.0, 0.0));
    let line_count: usize = text_box
        .text
        .split('\n')
        .enumerate()
        .map(|(p, text)| {
            let indent = style.paragraph_indents.get(p).copied().unwrap_or(0.0);
            wrap_text_lines(text, (w - padding * 2.0 - indent).max(1.0), size, font).len()
        })
        .sum();
    let text_height = size + line_count.saturating_sub(1) as f32 * size * 1.2;
    let remaining = (h - padding * 2.0 - text_height).max(0.0);
    let vertical_offset = match style.vertical_alignment {
        lo_core::style::Alignment::Center => remaining / 2.0,
        lo_core::style::Alignment::End => remaining,
        _ => 0.0,
    };
    let mut line_y = slide_h - y_top - padding - size - vertical_offset;
    'paragraphs: for (paragraph, raw) in text_box.text.split('\n').enumerate() {
        let indent = style
            .paragraph_indents
            .get(paragraph)
            .copied()
            .unwrap_or(0.0);
        let available = (w - padding * 2.0 - indent).max(1.0);
        let lines = wrap_text_lines(raw, available, size, font);
        for (line_index, line) in lines.iter().enumerate() {
            if line_y < y_bottom + padding {
                break 'paragraphs;
            }
            let estimated_width = lo_core::font_metrics::measure_base_font(line, size, font);
            let offset = match style.alignment {
                lo_core::style::Alignment::Center => ((available - estimated_width) / 2.0).max(0.0),
                lo_core::style::Alignment::End => (available - estimated_width).max(0.0),
                _ => 0.0,
            };
            if line_index == 0 {
                if let Some(Some(marker)) = style.paragraph_markers.get(paragraph) {
                    page.text_rgb(
                        x + padding + indent - 12.0,
                        line_y,
                        size,
                        font,
                        marker,
                        color.0,
                        color.1,
                        color.2,
                    );
                }
            }
            page.text_rgb(
                x + padding + indent + offset,
                line_y,
                size,
                font,
                line,
                color.0,
                color.1,
                color.2,
            );
            line_y -= size * 1.2;
        }
    }
    if style.border && style.background.is_empty() {
        page.rect_stroke_rgb(x, y_bottom, w, h, 0.82, 0.82, 0.82);
    }
    if style.rotation != 0.0 {
        page.raw("Q");
    }
}

fn text_box_font(style: &TextBoxStyle) -> PdfFont {
    let family = style.font_family.to_ascii_lowercase();
    if family.contains("mono") || family.contains("courier") {
        return PdfFont::Courier;
    }
    if family.contains("times") || family.contains("serif") && !family.contains("sans") {
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

fn pick_text_box_font(first_line: &str, y_top: f32, height: f32) -> (PdfFont, f32) {
    if y_top < 70.0 && height <= 110.0 && first_line.chars().count() <= 80 {
        (PdfFont::HelveticaBold, 26.0)
    } else {
        (PdfFont::Helvetica, 16.0)
    }
}

fn wrap_text_lines(text: &str, width: f32, font_size: f32, font: PdfFont) -> Vec<String> {
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if lo_core::font_metrics::measure_base_font(&candidate, font_size, font) <= width {
                current = candidate;
            } else {
                if !current.is_empty() {
                    out.push(current);
                }
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        } else if raw_line.is_empty() {
            out.push(String::new());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub(crate) fn parse_color(input: &str) -> Option<(f32, f32, f32)> {
    let trimmed = input.trim().trim_start_matches('#');
    if trimmed.len() != 6 || !trimmed.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&trimmed[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&trimmed[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&trimmed[4..6], 16).ok()? as f32 / 255.0;
    Some((r, g, b))
}

#[allow(dead_code)]
fn _style(_style: &TextBoxStyle) {}

fn register_image(pdf: &mut PdfDocument, image: &lo_core::ImageElement) -> Option<String> {
    if image.mime_type == "image/jpeg" {
        let (w, h) = jpeg_dimensions(&image.data)?;
        return Some(pdf.add_jpeg_image(w, h, image.data.clone()));
    }
    let mut decoded = match image.mime_type.as_str() {
        #[cfg(feature = "svg")]
        "image/svg+xml" => raster_svg(image)?,
        "image/png" => lo_core::decode_png(&image.data).ok()?,
        "image/webp" => lo_core::decode_webp(&image.data).ok()?,
        "image/gif" | "image/bmp" => lo_core::decode_legacy_image(&image.data,&image.mime_type).ok()?,
        _ => return None,
    };
    if image.opacity != 255 {
        for pixel in decoded.pixels.chunks_exact_mut(4) {
            pixel[3] = ((pixel[3] as u16 * image.opacity as u16 + 127) / 255) as u8;
        }
    }
    Some(pdf.add_rgba_image(decoded.width, decoded.height, decoded.pixels))
}
#[cfg(feature = "svg")]
fn raster_svg(image: &lo_core::ImageElement) -> Option<lo_core::RasterImage> {
    use resvg::{tiny_skia, usvg};
    if image.data.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut options = usvg::Options::default();
    // Embedded SVG is self-contained. Never resolve filesystem or network URLs.
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options.image_href_resolver.resolve_data = Box::new(|_, _, _| None);
    let tree = usvg::Tree::from_data(&image.data, &options).ok()?;
    let w = (image.frame.size.width.as_pt() * 1.5).clamp(1.0, 2048.0);
    let h = (image.frame.size.height.as_pt() * 1.5).clamp(1.0, 2048.0);
    let scale = (2_000_000.0 / (w * h)).sqrt().min(1.0);
    let (w, h) = ((w * scale).round() as u32, (h * scale).round() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            w as f32 / tree.size().width(),
            h as f32 / tree.size().height(),
        ),
        &mut pixmap.as_mut(),
    );
    let mut pixels = pixmap.data().to_vec();
    // tiny-skia uses premultiplied channels; PDF soft masks require straight RGB.
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    Some(lo_core::RasterImage {
        width: w,
        height: h,
        pixels,
    })
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

#[cfg(all(test, feature = "svg"))]
mod svg_tests {
    use super::*;
    #[test]
    fn svg_raster_keeps_straight_alpha_and_rejects_external_images() {
        let image = lo_core::ImageElement {
                transform: None,
                crop: [0.0; 4], rotation: 0.0, flip_horizontal: false, flip_vertical: false,
            name: "test.svg".into(), mime_type: "image/svg+xml".into(), opacity: 255,
            data: br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="5" height="10" fill="#ff0000" opacity="0.5"/><image href="file:///etc/passwd" x="5" width="5" height="10"/></svg>"##.to_vec(),
            frame: lo_core::Rect::new(lo_core::Length::pt(0.0),lo_core::Length::pt(0.0),lo_core::Length::pt(10.0),lo_core::Length::pt(10.0)), alt: String::new(),
        };
        let raster = raster_svg(&image).unwrap();
        assert_eq!(raster.pixels[0], 255);
        assert!((127..=128).contains(&raster.pixels[3]));
        assert_eq!(raster.pixels[((raster.width - 1) * 4 + 3) as usize], 0);
        assert!((raster.width as usize * raster.height as usize) <= 2_000_000);
    }
}

#[cfg(test)]
mod shared_image_tests {
    use super::*;
    #[test]
    fn repeated_master_images_share_one_pdf_resource() {
        let data=lo_core::RasterImage::new(10,10,lo_core::Rgba::WHITE).encode_png();
        let image=lo_core::ImageElement{transform:None,name:"master.png".into(),mime_type:"image/png".into(),data,frame:lo_core::Rect::default(),alt:String::new(),opacity:255,crop:[0.0;4],rotation:0.0,flip_horizontal:false,flip_vertical:false};
        let mut deck=Presentation::new("Shared image");
        let mut slide=lo_core::Slide::default();slide.elements.push(SlideElement::Image(image));deck.slides.push(slide.clone());
        let count=|bytes:Vec<u8>|String::from_utf8_lossy(&bytes).matches("/Subtype /Image").count();
        let single=count(to_pdf(&deck));assert!(single>0);
        deck.slides.extend([slide.clone(),slide]);assert_eq!(count(to_pdf(&deck)),single);
    }
}
