//! Positioned PPTX import. No formula execution, external resources, or speaker notes on slides.
use lo_core::style::Alignment;
use lo_core::{
    parse_xml_document, Length, Presentation, Rect, Result, Shape, ShapeKind, ShapeStyle, Slide,
    SlideElement, TextBox, TextBoxStyle, XmlNode,
};
use lo_zip::{rels_path_for, resolve_part_target, ZipArchive};
use std::collections::BTreeMap;
#[path = "pptx_smartart.rs"]
mod smartart;

#[derive(Default)]
struct Relations {
    paths: BTreeMap<String, String>,
    kinds: BTreeMap<String, String>,
}
fn relationships(zip: &ZipArchive, part: &str) -> Result<Relations> {
    let path = rels_path_for(part);
    let mut out = Relations::default();
    if !zip.contains(&path) {
        return Ok(out);
    }
    let root = parse_xml_document(&zip.read_string(&path)?)?;
    for n in root.children_named("Relationship") {
        if n.attr("TargetMode") == Some("External") {
            continue;
        }
        if let (Some(id), Some(target)) = (n.attr("Id"), n.attr("Target")) {
            let path = resolve_part_target(part, target);
            out.paths.insert(id.into(), path.clone());
            if let Some(kind) = n.attr("Type").and_then(|s| s.rsplit('/').next()) {
                out.kinds.insert(kind.into(), path);
            }
        }
    }
    Ok(out)
}
fn related(
    zip: &ZipArchive,
    rels: &Relations,
    kind: &str,
) -> Result<Option<(String, XmlNode, Relations)>> {
    let Some(path) = rels.kinds.get(kind).filter(|p| zip.contains(p)) else {
        return Ok(None);
    };
    Ok(Some((
        path.clone(),
        parse_xml_document(&zip.read_string(path)?)?,
        relationships(zip, path)?,
    )))
}
fn num(n: Option<&XmlNode>, key: &str, default: f32) -> f32 {
    n.and_then(|n| n.attr(key))
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}
fn pt(n: Option<&XmlNode>, key: &str, default: f32) -> f32 {
    num(n, key, default * 12700.0) / 12700.0
}
fn color_alpha(n: &XmlNode) -> f32 {
    let color = if matches!(n.local_name(), "srgbClr" | "schemeClr" | "sysClr") {
        Some(n)
    } else {
        n.children
            .iter()
            .find(|c| matches!(c.local_name(), "srgbClr" | "schemeClr" | "sysClr"))
            .map(|n| n.as_ref())
    };
    let mut alpha = 1.0;
    if let Some(color) = color {
        for transform in &color.children {
            let value = num(Some(transform), "val", 100000.0) / 100000.0;
            match transform.local_name() {
                "alpha" => alpha = value,
                "alphaMod" => alpha *= value,
                "alphaOff" => alpha += value,
                _ => {}
            }
        }
    }
    alpha.clamp(0.0, 1.0)
}
fn png_density(data: &[u8]) -> Option<(f32, f32)> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut offset = 8usize;
    while let Some(header) = data.get(offset..offset.checked_add(8)?) {
        let len = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let end = offset.checked_add(12)?.checked_add(len)?;
        let chunk = data.get(offset + 8..end - 4)?;
        if &header[4..] == b"pHYs" && chunk.len() == 9 && chunk[8] == 1 {
            let x = u32::from_be_bytes(chunk[..4].try_into().ok()?) as f32 * 0.0254;
            let y = u32::from_be_bytes(chunk[4..8].try_into().ok()?) as f32 * 0.0254;
            return (x > 0.0 && y > 0.0).then_some((x, y));
        }
        offset = end;
    }
    None
}
fn line_dashes(line: Option<&XmlNode>) -> Vec<u32> {
    let width = pt(line, "w", 0.75).clamp(0.01, 100.0);
    let pattern: Vec<f32> = if let Some(custom) = line.and_then(|n| n.child("custDash")) {
        custom
            .children_named("ds")
            .take(64)
            .flat_map(|n| {
                [
                    num(Some(n), "d", 100000.0) / 100000.0,
                    num(Some(n), "sp", 100000.0) / 100000.0,
                ]
            })
            .collect()
    } else {
        match line
            .and_then(|n| n.child("prstDash"))
            .and_then(|n| n.attr("val"))
        {
            Some("dot") => vec![1.0, 3.0],
            Some("sysDot") => vec![1.0, 1.0],
            Some("dash") => vec![4.0, 3.0],
            Some("sysDash") => vec![3.0, 1.0],
            Some("lgDash") => vec![8.0, 3.0],
            Some("dashDot") => vec![4.0, 3.0, 1.0, 3.0],
            Some("lgDashDot") => vec![8.0, 3.0, 1.0, 3.0],
            Some("lgDashDotDot") => vec![8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
            Some("sysDashDot") => vec![3.0, 1.0, 1.0, 1.0],
            Some("sysDashDotDot") => vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            _ => Vec::new(),
        }
    };
    pattern
        .iter()
        .map(|v| (v.clamp(0.001, 1000.0) * width * 1000.0).max(1.0) as u32)
        .collect()
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(
        Length::pt(x),
        Length::pt(y),
        Length::pt(w.max(0.0)),
        Length::pt(h.max(0.0)),
    )
}
fn tree(n: &XmlNode) -> Option<&XmlNode> {
    n.child("cSld").and_then(|n| n.child("spTree"))
}
// Only use the stored raster/vector preview; embedded object payloads are never opened.
fn ole_preview(data: &XmlNode) -> Option<&XmlNode> {
    if let Some(pic) = data.child("oleObj").and_then(|n| n.child("pic")) {
        return Some(pic);
    }
    let alternate = data.child("AlternateContent")?;
    alternate
        .children_named("Choice")
        .chain(alternate.children_named("Fallback"))
        .find_map(|branch| branch.child("oleObj").and_then(|n| n.child("pic")))
}
fn ole_object(data: &XmlNode) -> Option<&XmlNode> {
    data.child("oleObj").or_else(|| {
        data.child("AlternateContent")?
            .children
            .iter()
            .find_map(|branch| branch.child("oleObj"))
    })
}
fn vml_preview<'a>(root: &'a XmlNode, id: &str) -> Option<&'a XmlNode> {
    let mut stack = vec![(root, 0)];
    let mut visited = 0;
    while let Some((node, depth)) = stack.pop() {
        visited += 1;
        if visited > 4096 {
            break;
        }
        if node.local_name() == "shape"
            && (node.attr("spid") == Some(id) || node.attr("id") == Some(id))
        {
            return node.child("imagedata");
        }
        if depth < 32 {
            stack.extend(node.children.iter().rev().map(|n| (n.as_ref(), depth + 1)));
        }
    }
    None
}
fn placeholder(n: &XmlNode) -> Option<&XmlNode> {
    n.child("nvSpPr")
        .or_else(|| n.child("nvPicPr"))
        .and_then(|n| n.child("nvPr"))
        .and_then(|n| n.child("ph"))
}
fn matching<'a>(n: &XmlNode, parent: Option<&'a XmlNode>) -> Option<&'a XmlNode> {
    let ph = placeholder(n)?;
    let idx = ph.attr("idx").unwrap_or("0");
    let kind = ph.attr("type").unwrap_or("body");
    let shapes = &tree(parent?)?.children;
    shapes
        .iter()
        .find(|s| placeholder(s).is_some_and(|p| p.attr("idx").unwrap_or("0") == idx))
        .or_else(|| {
            shapes
                .iter()
                .find(|s| placeholder(s).is_some_and(|p| p.attr("type").unwrap_or("body") == kind))
        })
        .map(|s| s.as_ref())
}
fn multiply(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}
#[derive(Clone, Copy)]
struct Transform {
    actual: [f32; 6],
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
}
impl Transform {
    fn identity() -> Self {
        Self {
            actual: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            sx: 1.0,
            sy: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }
    fn frame(self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        rect(
            self.tx + x * self.sx,
            self.ty + y * self.sy,
            w * self.sx,
            h * self.sy,
        )
    }
    fn post(self) -> Option<[f32; 6]> {
        if self.sx.abs() < 1e-8 || self.sy.abs() < 1e-8 {
            return None;
        }
        let m = multiply(
            self.actual,
            [
                1.0 / self.sx,
                0.0,
                0.0,
                1.0 / self.sy,
                -self.tx / self.sx,
                -self.ty / self.sy,
            ],
        );
        if !m.iter().all(|v| v.is_finite()) {
            return None;
        }
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        m.iter()
            .zip(identity)
            .any(|(a, b)| (*a - b).abs() > 0.0001)
            .then_some(m)
    }
    fn group(self, n: &XmlNode) -> Self {
        let t = n.child("grpSpPr").and_then(|n| n.child("xfrm"));
        let off = t.and_then(|n| n.child("off"));
        let ext = t.and_then(|n| n.child("ext"));
        let ch_off = t.and_then(|n| n.child("chOff"));
        let ch_ext = t.and_then(|n| n.child("chExt"));
        let sx = pt(ext, "cx", 1.0) / pt(ch_ext, "cx", 1.0).max(0.001);
        let sy = pt(ext, "cy", 1.0) / pt(ch_ext, "cy", 1.0).max(0.001);
        let tx = pt(off, "x", 0.0) - pt(ch_off, "x", 0.0) * sx;
        let ty = pt(off, "y", 0.0) - pt(ch_off, "y", 0.0) * sy;
        let center = (
            pt(off, "x", 0.0) + pt(ext, "cx", 1.0) / 2.0,
            pt(off, "y", 0.0) + pt(ext, "cy", 1.0) / 2.0,
        );
        let angle = (num(t, "rot", 0.0) / 60000.0).to_radians();
        let (sin, cos) = angle.sin_cos();
        let fx = if t
            .and_then(|n| n.attr("flipH"))
            .is_some_and(|v| matches!(v, "1" | "true"))
        {
            -1.0
        } else {
            1.0
        };
        let fy = if t
            .and_then(|n| n.attr("flipV"))
            .is_some_and(|v| matches!(v, "1" | "true"))
        {
            -1.0
        } else {
            1.0
        };
        let (a, b, c, d) = (cos * fx, sin * fx, -sin * fy, cos * fy);
        let rotation = [
            a,
            b,
            c,
            d,
            center.0 - a * center.0 - c * center.1,
            center.1 - b * center.0 - d * center.1,
        ];
        Self {
            sx: self.sx * sx,
            sy: self.sy * sy,
            tx: self.tx + self.sx * tx,
            ty: self.ty + self.sy * ty,
            actual: multiply(self.actual, multiply(rotation, [sx, 0.0, 0.0, sy, tx, ty])),
        }
    }
}
fn xfrm(n: &XmlNode) -> Option<&XmlNode> {
    n.child("spPr")
        .and_then(|n| n.child("xfrm"))
        .or_else(|| n.child("xfrm"))
}
fn frame(chain: &[&XmlNode], t: Transform) -> Rect {
    let off = chain
        .iter()
        .filter_map(|n| xfrm(n))
        .find_map(|n| n.child("off"));
    let ext = chain
        .iter()
        .filter_map(|n| xfrm(n))
        .find_map(|n| n.child("ext"));
    t.frame(
        pt(off, "x", 0.0),
        pt(off, "y", 0.0),
        pt(ext, "cx", 300.0),
        pt(ext, "cy", 60.0),
    )
}
struct Theme {
    colors: BTreeMap<String, String>,
    major: String,
    minor: String,
}
impl Theme {
    fn new(root: Option<&XmlNode>, map: Option<&XmlNode>) -> Self {
        let elements = root.and_then(|n| n.child("themeElements"));
        let scheme = elements.and_then(|n| n.child("clrScheme"));
        let mut colors: BTreeMap<String, String> = [
            ("dk1", "000000"),
            ("lt1", "FFFFFF"),
            ("dk2", "1F497D"),
            ("lt2", "EEECE1"),
            ("accent1", "4F81BD"),
            ("accent2", "C0504D"),
            ("accent3", "9BBB59"),
            ("accent4", "8064A2"),
            ("accent5", "4BACC6"),
            ("accent6", "F79646"),
        ]
        .into_iter()
        .map(|(a, b)| (a.into(), b.into()))
        .collect();
        if let Some(scheme) = scheme {
            for n in &scheme.children {
                if let Some(v) = n
                    .children
                    .first()
                    .and_then(|n| n.attr("lastClr").or_else(|| n.attr("val")))
                {
                    colors.insert(n.local_name().into(), v.into());
                }
            }
        }
        let source_colors = colors.clone();
        for (alias, default) in [
            ("accent1", "accent1"),
            ("accent2", "accent2"),
            ("accent3", "accent3"),
            ("accent4", "accent4"),
            ("accent5", "accent5"),
            ("accent6", "accent6"),
            ("hlink", "hlink"),
            ("folHlink", "folHlink"),
            ("bg1", "lt1"),
            ("tx1", "dk1"),
            ("bg2", "lt2"),
            ("tx2", "dk2"),
        ] {
            let key = map.and_then(|n| n.attr(alias)).unwrap_or(default);
            if let Some(v) = source_colors.get(key).cloned() {
                colors.insert(alias.into(), v);
            }
        }
        let font = |kind| {
            elements
                .and_then(|n| n.child("fontScheme"))
                .and_then(|n| n.child(kind))
                .and_then(|n| n.child("latin"))
                .and_then(|n| n.attr("typeface"))
                .unwrap_or("Arial")
                .to_string()
        };
        Self {
            colors,
            major: font("majorFont"),
            minor: font("minorFont"),
        }
    }
    fn font(&self, s: &str) -> String {
        match s {
            "+mj-lt" | "+mj-ea" | "+mj-cs" => self.major.clone(),
            "+mn-lt" | "+mn-ea" | "+mn-cs" => self.minor.clone(),
            _ => s.into(),
        }
    }
    fn color(&self, n: &XmlNode) -> Option<String> {
        let c = if matches!(n.local_name(), "srgbClr" | "schemeClr" | "sysClr") {
            n
        } else {
            n.children
                .iter()
                .find(|c| matches!(c.local_name(), "srgbClr" | "schemeClr" | "sysClr"))?
        };
        let raw = if c.local_name() == "schemeClr" {
            self.colors.get(c.attr("val")?)?.as_str()
        } else {
            c.attr("lastClr").or_else(|| c.attr("val"))?
        };
        if raw.len() != 6 || !raw.is_ascii() {
            return None;
        }
        let rgb = u32::from_str_radix(raw, 16).ok()?;
        let channels = [(rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255].map(|v| {
            let mut v = v as f32 / 255.0;
            for m in &c.children {
                let a = num(Some(m), "val", 100000.0) / 100000.0;
                v = match m.local_name() {
                    "tint" | "shade" => {
                        let linear = if v <= 0.04045 {
                            v / 12.92
                        } else {
                            ((v + 0.055) / 1.055).powf(2.4)
                        };
                        let linear = if m.local_name() == "tint" {
                            linear * a + 1.0 - a
                        } else {
                            linear * a
                        };
                        if linear <= 0.0031308 {
                            linear * 12.92
                        } else {
                            1.055 * linear.powf(1.0 / 2.4) - 0.055
                        }
                    }
                    "lumMod" => v * a,
                    "lumOff" => v + a,
                    _ => v,
                };
            }
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        });
        Some(format!(
            "#{:02X}{:02X}{:02X}",
            channels[0], channels[1], channels[2]
        ))
    }
}

pub(crate) fn load(title: String, bytes: &[u8]) -> Result<Presentation> {
    let zip = ZipArchive::new(bytes)?;
    let root = parse_xml_document(&zip.read_string("ppt/presentation.xml")?)?;
    let rels = relationships(&zip, "ppt/presentation.xml")?;
    let table_styles = if zip.contains("ppt/tableStyles.xml") {
        Some(parse_xml_document(
            &zip.read_string("ppt/tableStyles.xml")?,
        )?)
    } else {
        None
    };
    let mut deck = Presentation::new(title);
    let size = root.child("sldSz");
    deck.page_size = lo_core::Size::new(
        Length::pt(pt(size, "cx", 720.0).clamp(1.0, 14400.0)),
        Length::pt(pt(size, "cy", 540.0).clamp(1.0, 14400.0)),
    );
    if let Some(list) = root.child("sldIdLst") {
        for (i, id) in list.children_named("sldId").take(2048).enumerate() {
            let Some(path) = id
                .attr("r:id")
                .or_else(|| id.attr("id"))
                .and_then(|id| rels.paths.get(id))
            else {
                continue;
            };
            let slide_root = parse_xml_document(&zip.read_string(path)?)?;
            let sr = relationships(&zip, path)?;
            let layout = related(&zip, &sr, "slideLayout")?;
            let master = if let Some((_, _, r)) = &layout {
                related(&zip, r, "slideMaster")?
            } else {
                None
            };
            let theme = if let Some((_, _, r)) = &master {
                related(&zip, r, "theme")?
            } else {
                None
            };
            let master_root = master.as_ref().map(|(_, n, _)| n);
            let layout_root = layout.as_ref().map(|(_, n, _)| n);
            let theme_root = theme.as_ref().map(|(_, n, _)| n);
            let color_map = [Some(&slide_root), layout_root]
                .into_iter()
                .flatten()
                .find_map(|n| {
                    n.child("clrMapOvr")
                        .and_then(|n| n.child("overrideClrMapping"))
                })
                .or_else(|| master_root.and_then(|n| n.child("clrMap")));
            let theme = Theme::new(theme_root, color_map);
            let mut slide = Slide {
                name: format!("Slide {}", i + 1),
                ..Default::default()
            };
            slide.background = [Some(&slide_root), layout_root, master_root]
                .into_iter()
                .flatten()
                .find_map(|n| {
                    n.child("cSld")
                        .and_then(|n| n.child("bg"))
                        .and_then(|n| n.child("bgPr"))
                        .and_then(|n| n.child("solidFill"))
                        .and_then(|n| theme.color(n))
                });
            // Background properties inherit as a unit; a local background suppresses the master.
            let background = [Some(&slide_root), layout_root, master_root]
                .into_iter()
                .flatten()
                .find_map(|n| n.child("cSld").and_then(|n| n.child("bg")));
            // Inherited backgrounds retain the color map of their defining part.
            let has_background =
                |n: &XmlNode| n.child("cSld").and_then(|n| n.child("bg")).is_some();
            let background_map = if has_background(&slide_root) {
                color_map
            } else if layout_root.is_some_and(has_background) {
                layout_root
                    .and_then(|n| n.child("clrMapOvr"))
                    .and_then(|n| n.child("overrideClrMapping"))
                    .or_else(|| master_root.and_then(|n| n.child("clrMap")))
            } else {
                master_root.and_then(|n| n.child("clrMap"))
            };
            let background_theme = Theme::new(theme_root, background_map);
            let mut budget = 100_000usize;
            let mut ctx = Context {
                zip: &zip,
                slide_number: i + 1,
                table_styles: table_styles.as_ref(),
                background,
                background_theme: Some(&background_theme),
                theme: &theme,
                theme_root,
                master: master_root,
                layout: layout_root,
                default_text: root.child("defaultTextStyle"),
                budget: &mut budget,
                diagram_depth: 0,
            };
            if let Some(bg) = background {
                ctx.theme = &background_theme;
                let frame = rect(
                    0.0,
                    0.0,
                    deck.page_size.width.as_pt(),
                    deck.page_size.height.as_pt(),
                );
                if let Some(pr) = bg.child("bgPr") {
                    let br = if slide_root
                        .child("cSld")
                        .and_then(|n| n.child("bg"))
                        .is_some()
                    {
                        &sr
                    } else if layout_root
                        .and_then(|n| n.child("cSld"))
                        .and_then(|n| n.child("bg"))
                        .is_some()
                    {
                        &layout.as_ref().unwrap().2
                    } else {
                        master.as_ref().map(|m| &m.2).unwrap_or(&sr)
                    };
                    ctx.background(pr, br, &mut slide, frame)?;
                } else if let Some(reference) = bg.child("bgRef") {
                    if let Some(fill) = ctx.theme_fill(reference) {
                        ctx.paint_fill(&fill, &mut slide, frame);
                    }
                }
            }
            ctx.theme = &theme;
            if slide_root.attr("showMasterSp") != Some("0")
                && layout_root.and_then(|n| n.attr("showMasterSp")) != Some("0")
            {
                if let Some((_, m, r)) = &master {
                    if let Some(t) = tree(m) {
                        ctx.walk(t, r, &mut slide, Transform::identity(), true)?;
                    }
                }
            }
            if let Some((_, l, r)) = &layout {
                if let Some(t) = tree(l) {
                    ctx.walk(t, r, &mut slide, Transform::identity(), true)?;
                }
            }
            if let Some(t) = tree(&slide_root) {
                ctx.walk(t, &sr, &mut slide, Transform::identity(), false)?;
            }
            // Speaker notes are metadata, not painted onto slide content.
            deck.slides.push(slide);
        }
    }
    Ok(deck)
}
struct Context<'a> {
    zip: &'a ZipArchive,
    slide_number: usize,
    table_styles: Option<&'a XmlNode>,
    background: Option<&'a XmlNode>,
    background_theme: Option<&'a Theme>,
    theme: &'a Theme,
    theme_root: Option<&'a XmlNode>,
    master: Option<&'a XmlNode>,
    layout: Option<&'a XmlNode>,
    default_text: Option<&'a XmlNode>,
    budget: &'a mut usize,
    diagram_depth: usize,
}
impl Context<'_> {
    fn walk(
        &mut self,
        node: &XmlNode,
        rels: &Relations,
        slide: &mut Slide,
        t: Transform,
        inherited: bool,
    ) -> Result<()> {
        for n in &node.children {
            if *self.budget == 0 {
                break;
            }
            *self.budget -= 1;
            if inherited && placeholder(n).is_some() {
                continue;
            }
            // Visibility belongs to this object's nonvisual properties. Do not
            // search descendants: a hidden child must not hide its visible parent.
            if n.children
                .iter()
                .filter_map(|pr| pr.child("cNvPr"))
                .any(|pr| pr.attr("hidden").is_some_and(|v| matches!(v, "1" | "true")))
            {
                continue;
            }
            let element_start = slide.elements.len();
            let chart_start = slide.odf_charts.len();
            match n.local_name() {
                "AlternateContent" => {
                    let chosen = n
                        .children_named("Choice")
                        .find(|c| {
                            c.children.iter().any(|s| {
                                matches!(
                                    s.local_name(),
                                    "sp" | "grpSp" | "pic" | "graphicFrame" | "cxnSp"
                                )
                            })
                        })
                        .or_else(|| n.child("Fallback"));
                    if let Some(chosen) = chosen {
                        self.walk(chosen, rels, slide, t, inherited)?;
                    }
                }
                "grpSp" => self.walk(n, rels, slide, t.group(n), inherited)?,
                "sp" | "cxnSp" => {
                    let layout = if inherited {
                        None
                    } else {
                        matching(n, self.layout)
                    };
                    let master = matching(layout.unwrap_or(n), self.master);
                    let mut chain = vec![n.as_ref()];
                    if let Some(l) = layout {
                        chain.push(l);
                    }
                    if let Some(m) = master {
                        chain.push(m);
                    }
                    if let Some(fill) = n.child("spPr").and_then(|pr| pr.child("blipFill")) {
                        let mut picture = n.as_ref().clone();
                        picture.children.push(fill.clone().into());
                        self.picture(&picture, rels, slide, t)?;
                    }
                    self.shape(&chain, slide, t);
                }
                "pic" => self.picture(n, rels, slide, t)?,
                "graphicFrame" => {
                    let data = n.child("graphic").and_then(|n| n.child("graphicData"));
                    let f = frame(&[n], t);
                    if let Some(picture) = data.and_then(ole_preview) {
                        let mut picture = picture.clone();
                        if xfrm(&picture).is_none() {
                            if let Some(transform) = n.child("xfrm") {
                                let mut properties =
                                    picture.child("spPr").cloned().unwrap_or_default();
                                properties.name = "spPr".into();
                                properties.children.push(transform.clone().into());
                                picture.children.retain(|n| n.local_name() != "spPr");
                                picture.children.push(properties.into());
                            }
                        }
                        self.picture(&picture, rels, slide, t)?;
                    } else if let Some(ole) = data.and_then(ole_object) {
                        if let Some(id) = ole.attr("spid") {
                            if let Some((_, vml, vml_rels)) =
                                related(self.zip, rels, "legacyDrawing")?
                            {
                                if let Some(preview) = vml_preview(&vml, id) {
                                    if let Some(image_id) =
                                        preview.attr("relid").or_else(|| preview.attr("id"))
                                    {
                                        let mut picture = n.as_ref().clone();
                                        picture.name = "pic".into();
                                        picture.children.retain(|n| n.local_name() == "xfrm");
                                        let blip = XmlNode {
                                            name: "blip".into(),
                                            attributes: BTreeMap::from([(
                                                "r:embed".into(),
                                                image_id.into(),
                                            )]),
                                            ..Default::default()
                                        };
                                        let fill = XmlNode {
                                            name: "blipFill".into(),
                                            children: vec![blip.into()],
                                            ..Default::default()
                                        };
                                        picture.children.push(fill.into());
                                        self.picture(&picture, &vml_rels, slide, t)?;
                                    }
                                }
                            }
                        }
                    } else if let Some(ids) = data.and_then(|n| n.child("relIds")) {
                        self.diagram(ids, rels, slide, f)?;
                    } else if let Some(table) = data.and_then(|n| n.child("tbl")) {
                        self.table(table, slide, f);
                    } else if let Some(path) = data
                        .and_then(|n| n.child("chart"))
                        .and_then(|n| n.attr("r:id").or_else(|| n.attr("id")))
                        .and_then(|id| rels.paths.get(id))
                    {
                        if self.zip.contains(path) {
                            let xml = parse_xml_document(&self.zip.read_string(path)?)?;
                            if let Some(chart) = lo_core::odf_chart::OdfChart::from_ooxml_with_theme(
                                &xml,
                                f,
                                self.theme_root,
                            ) {
                                slide.odf_charts.push(chart);
                            } else if let Some(row) = crate::chart::fallback_chart(&xml, f) {
                                slide.chart_tokens.push(row);
                            }
                        }
                    }
                }
                _ => {}
            }
            if !matches!(n.local_name(), "grpSp" | "AlternateContent") {
                if let Some(matrix) = t.post() {
                    for element in &mut slide.elements[element_start..] {
                        let target = match element {
                            SlideElement::Shape(s) => &mut s.style.transform,
                            SlideElement::TextBox(s) => &mut s.style.transform,
                            SlideElement::Image(s) => &mut s.transform,
                        };
                        *target = Some(target.map(|m| multiply(matrix, m)).unwrap_or(matrix));
                    }
                    for chart in &mut slide.odf_charts[chart_start..] {
                        chart.transform = Some(
                            chart
                                .transform
                                .map(|m| multiply(matrix, m))
                                .unwrap_or(matrix),
                        );
                    }
                }
            }
        }
        Ok(())
    }
    fn diagram(
        &mut self,
        ids: &XmlNode,
        rels: &Relations,
        slide: &mut Slide,
        f: Rect,
    ) -> Result<()> {
        if self.diagram_depth >= 16 {
            return Ok(());
        }
        let Some(path) = ids
            .attr("r:dm")
            .or_else(|| ids.attr("dm"))
            .and_then(|id| rels.paths.get(id))
        else {
            return Ok(());
        };
        if !self.zip.contains(path) {
            return Ok(());
        }
        let model = parse_xml_document(&self.zip.read_string(path)?)?;
        if let Some(bg) = model.child("bg").and_then(|n| {
            n.children
                .iter()
                .find(|n| matches!(n.local_name(), "solidFill" | "gradFill"))
        }) {
            self.paint_fill(bg, slide, f);
        }
        fn drawing_id(n: &XmlNode) -> Option<&str> {
            if n.local_name() == "dataModelExt" {
                return n.attr("relId");
            }
            n.children.iter().find_map(|c| drawing_id(c))
        }
        let model_rels = relationships(self.zip, path)?;
        let drawing = drawing_id(&model)
            .and_then(|id| rels.paths.get(id).or_else(|| model_rels.paths.get(id)))
            .filter(|path| self.zip.contains(path));
        let Some(drawing) = drawing else {
            let read = |key| -> Result<Option<XmlNode>> {
                let Some(path) = ids
                    .attr(key)
                    .and_then(|id| rels.paths.get(id))
                    .filter(|p| self.zip.contains(p))
                else {
                    return Ok(None);
                };
                Ok(Some(parse_xml_document(&self.zip.read_string(path)?)?))
            };
            if let Some(layout) = read("lo")? {
                let colors = read("cs")?;
                let styles = read("qs")?;
                smartart::render(
                    self,
                    &model,
                    &layout,
                    colors.as_ref(),
                    styles.as_ref(),
                    slide,
                    f,
                );
            }
            return Ok(());
        };
        let root = parse_xml_document(&self.zip.read_string(drawing)?)?;
        let dr = relationships(self.zip, drawing)?;
        if let Some(tree) = root.child("spTree") {
            let transform = Transform {
                actual: [1.0, 0.0, 0.0, 1.0, f.origin.x.as_pt(), f.origin.y.as_pt()],
                sx: 1.0,
                sy: 1.0,
                tx: f.origin.x.as_pt(),
                ty: f.origin.y.as_pt(),
            };
            self.diagram_depth += 1;
            let result = self.walk(tree, &dr, slide, transform, false);
            self.diagram_depth -= 1;
            result?;
        }
        Ok(())
    }
    fn theme_fill(&self, reference: &XmlNode) -> Option<XmlNode> {
        let index = reference.attr("idx")?.parse::<usize>().ok()?;
        if index == 0 {
            return None;
        }
        let (list, index) = if reference.local_name() == "lnRef" {
            ("lnStyleLst", index - 1)
        } else if index >= 1001 {
            ("bgFillStyleLst", index - 1001)
        } else {
            ("fillStyleLst", index - 1)
        };
        let mut fill = self
            .theme_root?
            .child("themeElements")?
            .child("fmtScheme")?
            .child(list)?
            .children
            .get(index)?
            .as_ref()
            .clone();
        // phClr is supplied by the referencing shape, preserving its own transforms.
        fn replace(n: &mut XmlNode, reference: &XmlNode) {
            if n.local_name() == "schemeClr" && n.attr("val") == Some("phClr") {
                if let Some(c) = reference
                    .children
                    .iter()
                    .find(|n| matches!(n.local_name(), "schemeClr" | "srgbClr" | "sysClr"))
                {
                    n.name = c.name.clone();
                    n.attributes = c.attributes.clone();
                    let mut children = c.children.clone();
                    children.extend(n.children.clone());
                    n.children = children;
                }
            } else {
                for c in &mut n.children {
                    replace(std::sync::Arc::make_mut(c), reference);
                }
            }
        }
        replace(&mut fill, reference);
        Some(fill)
    }
    fn paint_fill(&self, fill: &XmlNode, slide: &mut Slide, f: Rect) {
        self.paint_fill_with_theme(fill, slide, f, self.theme);
    }
    fn paint_fill_with_theme(&self, fill: &XmlNode, slide: &mut Slide, f: Rect, theme: &Theme) {
        if let Some(color) = theme.color(fill) {
            slide.elements.push(SlideElement::Shape(Shape {
                frame: f,
                kind: ShapeKind::Rectangle,
                style: ShapeStyle {
                    transform: None,
                    fill: color,
                    stroke: "none".into(),
                    ..Default::default()
                },
            }));
        } else if fill.local_name() == "pattFill" {
            let foreground = fill
                .child("fgClr")
                .and_then(|n| theme.color(n))
                .unwrap_or_else(|| "#000000".into());
            let background = fill
                .child("bgClr")
                .and_then(|n| theme.color(n))
                .unwrap_or_else(|| "#FFFFFF".into());
            slide.elements.push(SlideElement::Shape(Shape {
                frame: f,
                kind: ShapeKind::Rectangle,
                style: ShapeStyle {
                    fill: background,
                    stroke: "none".into(),
                    hatch: crate::pptx_pattern::hatch(fill.attr("prst").unwrap_or(""), foreground),
                    ..Default::default()
                },
            }));
        } else if fill.local_name() == "gradFill" {
            let mut stops: Vec<_> = fill
                .child("gsLst")
                .into_iter()
                .flat_map(|n| n.children_named("gs"))
                .filter_map(|n| {
                    theme
                        .color(n)
                        .and_then(|c| crate::pdf::parse_color(&c))
                        .map(|c| {
                            (
                                (num(Some(n), "pos", 0.0) / 100000.0).clamp(0.0, 1.0),
                                c,
                                color_alpha(n),
                            )
                        })
                })
                .collect();
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            if stops.is_empty() {
                return;
            }
            let angle = (num(fill.child("lin"), "ang", 0.0) / 60000.0).to_radians();
            let (dy, dx) = angle.sin_cos();
            let mut image = lo_core::RasterImage::new(256, 256, lo_core::Rgba::WHITE);
            for y in 0..256 {
                for x in 0..256 {
                    let t = (((x as f32 / 255.0 - 0.5) * dx + (y as f32 / 255.0 - 0.5) * dy)
                        / (dx.abs() + dy.abs()).max(0.001)
                        + 0.5)
                        .clamp(0.0, 1.0);
                    let t = if let Some(path) = fill.child("path") {
                        let target = path.child("fillToRect");
                        let cx = (num(target, "l", 0.0) / 100000.0 + 1.0
                            - num(target, "r", 0.0) / 100000.0)
                            / 2.0;
                        let cy = (num(target, "t", 0.0) / 100000.0 + 1.0
                            - num(target, "b", 0.0) / 100000.0)
                            / 2.0;
                        let (px, py) = (x as f32 / 255.0 - cx, y as f32 / 255.0 - cy);
                        if path.attr("path") == Some("rect") {
                            (px.abs().max(py.abs()) * 2.0).clamp(0.0, 1.0)
                        } else {
                            (px.hypot(py) * std::f32::consts::SQRT_2).clamp(0.0, 1.0)
                        }
                    } else {
                        t
                    };
                    // Equal-position stops create a hard edge; samples beyond the
                    // final stop must use its color, not the preceding stop's color.
                    let next = stops.partition_point(|s| s.0 <= t);
                    let upper = next.min(stops.len() - 1);
                    let lower = next.saturating_sub(1).min(stops.len() - 1);
                    let (a, b) = (stops[lower], stops[upper]);
                    let t = if b.0 > a.0 {
                        ((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let rgb = [
                        a.1 .0 + (b.1 .0 - a.1 .0) * t,
                        a.1 .1 + (b.1 .1 - a.1 .1) * t,
                        a.1 .2 + (b.1 .2 - a.1 .2) * t,
                    ];
                    let offset = (y * 256 + x) * 4;
                    for (i, v) in rgb.iter().enumerate() {
                        image.pixels[offset + i] = (v * 255.0).round() as u8;
                    }
                    image.pixels[offset + 3] = ((a.2 + (b.2 - a.2) * t) * 255.0).round() as u8;
                }
            }
            slide
                .elements
                .push(SlideElement::Image(lo_core::ImageElement {
                    tile: None,
                    clip_paths: Vec::new(),
                    transform: None,
                    name: "background-gradient.png".into(),
                    mime_type: "image/png".into(),
                    data: image.encode_png(),
                    frame: f,
                    alt: String::new(),
                    opacity: 255,
                    crop: [0.0; 4],
                    rotation: 0.0,
                    flip_horizontal: false,
                    flip_vertical: false,
                }));
        }
    }
    fn background(&self, pr: &XmlNode, rels: &Relations, slide: &mut Slide, f: Rect) -> Result<()> {
        if let Some(fill) = pr
            .children
            .iter()
            .find(|n| matches!(n.local_name(), "solidFill" | "gradFill" | "pattFill"))
        {
            self.paint_fill(fill, slide, f);
        }
        if let Some(fill) = pr.child("blipFill") {
            let mut pic = XmlNode {
                name: "pic".into(),
                ..Default::default()
            };
            pic.children.push(fill.clone().into());
            let before = slide.elements.len();
            self.picture(&pic, rels, slide, Transform::identity())?;
            if let Some(SlideElement::Image(image)) = slide.elements.get_mut(before) {
                image.frame = f;
            }
        }
        Ok(())
    }
    fn shape(&self, chain: &[&XmlNode], slide: &mut Slide, t: Transform) {
        self.shape_with_fit(chain, slide, t, false);
    }
    fn shape_with_fit(
        &self,
        chain: &[&XmlNode],
        slide: &mut Slide,
        t: Transform,
        generated_layout: bool,
    ) {
        let f = frame(chain, t);
        let props = chain
            .iter()
            .filter_map(|n| n.child("spPr"))
            .collect::<Vec<_>>();
        let effective_fill = chain.iter().find_map(|n| {
            n.child("spPr")
                .and_then(|n| {
                    n.children
                        .iter()
                        .find(|n| {
                            matches!(
                                n.local_name(),
                                "noFill" | "solidFill" | "gradFill" | "blipFill" | "pattFill"
                            )
                        })
                        .map(|n| n.as_ref().clone())
                })
                .or_else(|| {
                    n.child("style")
                        .and_then(|n| n.child("fillRef"))
                        .and_then(|n| self.theme_fill(n))
                })
        });
        let effective_fill = effective_fill.as_ref();
        let fill = effective_fill.map(|n| {
            if n.local_name() == "solidFill" {
                self.theme.color(n).unwrap_or_else(|| "none".into())
            } else {
                "none".into()
            }
        });
        let fill_start = slide.elements.len();
        let uses_background = chain[0]
            .attr("useBgFill")
            .is_some_and(|v| matches!(v, "1" | "true"));
        let fill = if uses_background { None } else { fill };
        if chain[0]
            .attr("useBgFill")
            .is_some_and(|v| matches!(v, "1" | "true"))
        {
            let referenced = self
                .background
                .and_then(|n| n.child("bgRef"))
                .and_then(|n| self.theme_fill(n));
            let background_fill = self
                .background
                .and_then(|n| n.child("bgPr"))
                .and_then(|n| {
                    n.children
                        .iter()
                        .find(|n| matches!(n.local_name(), "solidFill" | "gradFill"))
                })
                .map(|n| n.as_ref())
                .or(referenced.as_ref());
            if let Some(fill) = background_fill {
                self.paint_fill_with_theme(
                    fill,
                    slide,
                    f,
                    self.background_theme.unwrap_or(self.theme),
                );
            } else if self.background.is_none() {
                // The implicit slide background is opaque white. A background-filled
                // shape must cover earlier elements even when no bg node is stored.
                slide.elements.push(SlideElement::Shape(Shape {
                    frame: f,
                    kind: ShapeKind::Rectangle,
                    style: ShapeStyle {
                        fill: "#FFFFFF".into(),
                        stroke: "none".into(),
                        ..Default::default()
                    },
                }));
            }
        } else if let Some(gradient) =
            effective_fill.filter(|n| matches!(n.local_name(), "gradFill" | "pattFill"))
        {
            self.paint_fill(gradient, slide, f);
        }
        let line = chain.iter().find_map(|n| {
            n.child("spPr")
                .and_then(|n| n.child("ln"))
                .cloned()
                .or_else(|| {
                    n.child("style")
                        .and_then(|n| n.child("lnRef"))
                        .and_then(|n| self.theme_fill(n))
                })
        });
        let line = line.as_ref();
        let stroke = line
            .and_then(|n| {
                if n.child("noFill").is_some() || n.child("blipFill").is_some() {
                    Some("none".into())
                } else {
                    n.child("solidFill").and_then(|n| self.theme.color(n))
                }
            })
            .unwrap_or_else(|| "none".into());
        let mut kind = props
            .iter()
            .find_map(|n| n.child("custGeom").or_else(|| n.child("prstGeom")))
            .and_then(|n| {
                crate::pptx_geometry::geometry(n, f.size.width.as_pt(), f.size.height.as_pt())
            })
            .unwrap_or(ShapeKind::Rectangle);
        let xf = chain.iter().find_map(|n| xfrm(n));
        crate::pptx_geometry::flip(
            &mut kind,
            xf.and_then(|n| n.attr("flipH"))
                .is_some_and(|v| matches!(v, "1" | "true")),
            xf.and_then(|n| n.attr("flipV"))
                .is_some_and(|v| matches!(v, "1" | "true")),
        );
        let angle = chain
            .iter()
            .find_map(|n| xfrm(n))
            .map(|n| num(Some(n), "rot", 0.0) / 60000.0)
            .unwrap_or(0.0);
        for element in &mut slide.elements[fill_start..] {
            match element {
                SlideElement::Image(image) => {
                    image.clip_paths = if let ShapeKind::Custom(paths) = &kind {
                        paths.clone()
                    } else {
                        Vec::new()
                    };
                    image.rotation = angle;
                }
                SlideElement::Shape(shape) => {
                    shape.kind = kind.clone();
                    shape.style.rotation_millidegrees = (-angle * 1000.0) as i32;
                    // Hatch directions in the reference are page-relative even
                    // when the filled shape rotates.
                    if let Some(hatch) = &mut shape.style.hatch {
                        hatch.angle_degrees = (hatch.angle_degrees as f32 + angle)
                            .rem_euclid(360.0)
                            .round() as i16;
                    }
                }
                _ => {}
            }
        }
        if fill.is_some() || stroke != "none" {
            let arrowheads = line
                .filter(|_| stroke != "none")
                .map(|line| {
                    crate::pptx_geometry::line_ends(
                        &kind,
                        line,
                        f.size.width.as_pt(),
                        f.size.height.as_pt(),
                        pt(Some(line), "w", 0.75).clamp(0.0, 100.0),
                    )
                })
                .unwrap_or_default();
            let arrow_color = stroke.clone();
            slide.elements.push(SlideElement::Shape(Shape {
                frame: f,
                kind,
                style: ShapeStyle {
                    transform: None,
                    fill: fill.unwrap_or_else(|| "none".into()),
                    stroke,
                    rotation_millidegrees: (-angle * 1000.0) as i32,
                    stroke_width_mm: 0,
                    stroke_width_mpt: Some((pt(line, "w", 0.75).clamp(0.0, 100.0) * 1000.0) as u32),
                    stroke_dash_mpt: line_dashes(line),
                    hatch: None,
                    stroke_cap: match line.and_then(|n| n.attr("cap")) {
                        Some("rnd") => 1,
                        Some("sq") => 2,
                        _ => 0,
                    },
                },
            }));
            if !arrowheads.is_empty() {
                slide.elements.push(SlideElement::Shape(Shape {
                    frame: rect(
                        f.origin.x.as_pt(),
                        f.origin.y.as_pt(),
                        f.size.width.as_pt().max(1.0),
                        f.size.height.as_pt().max(1.0),
                    ),
                    kind: ShapeKind::Custom(arrowheads),
                    style: ShapeStyle {
                        fill: arrow_color.clone(),
                        stroke: arrow_color,
                        rotation_millidegrees: (-angle * 1000.0) as i32,
                        stroke_width_mpt: Some(
                            (pt(line, "w", 0.75).clamp(0.0, 100.0) * 1000.0) as u32,
                        ),
                        ..Default::default()
                    },
                }));
            }
        }
        if let Some(body) = chain[0].child("txBody") {
            self.text_with_fit(body, chain, slide, f, angle, generated_layout);
        }
    }
    fn picture(
        &self,
        n: &XmlNode,
        rels: &Relations,
        slide: &mut Slide,
        t: Transform,
    ) -> Result<()> {
        let fill = n.child("blipFill");
        let blip = fill.and_then(|n| n.child("blip"));
        let Some(path) = blip
            .and_then(|n| n.attr("r:embed").or_else(|| n.attr("embed")))
            .and_then(|id| rels.paths.get(id))
        else {
            return Ok(());
        };
        if !self.zip.contains(path) {
            return Ok(());
        }
        let data = self.zip.read(path)?;
        let mime = match path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            "svg" => "image/svg+xml",
            _ => return Ok(()),
        };
        let crop_node = fill.and_then(|n| n.child("srcRect"));
        let mut crop = ["l", "t", "r", "b"]
            .map(|key| (num(crop_node, key, 0.0) / 100000.0).clamp(-1000.0, 0.999));
        if crop[0] + crop[2] >= 1.0 || crop[1] + crop[3] >= 1.0 {
            crop = [0.0; 4];
        }
        let xf = xfrm(n);
        let rotation = num(xf, "rot", 0.0) / 60000.0;
        let flip_horizontal = xf
            .and_then(|n| n.attr("flipH"))
            .is_some_and(|v| matches!(v, "1" | "true"));
        let flip_vertical = xf
            .and_then(|n| n.attr("flipV"))
            .is_some_and(|v| matches!(v, "1" | "true"));
        let f = frame(&[n], t);
        let tiled = fill.is_some_and(|fill| {
            fill.child("tile").is_some()
                || (n.local_name() == "sp" && fill.child("stretch").is_none())
        });
        let tile = if tiled {
            let dimensions = if mime == "image/png" && data.len() >= 24 {
                Some((
                    u32::from_be_bytes(data[16..20].try_into().unwrap()),
                    u32::from_be_bytes(data[20..24].try_into().unwrap()),
                ))
            } else if mime == "image/jpeg" {
                crate::pdf::jpeg_dimensions(&data)
            } else {
                None
            };
            dimensions.and_then(|(w, h)| {
                let props = fill.and_then(|n| n.child("tile"));
                let density = png_density(&data).unwrap_or((96.0, 96.0));
                let explicit_dpi = num(fill, "dpi", 0.0);
                let (dpi_x, dpi_y) = if explicit_dpi > 0.0 {
                    (explicit_dpi, explicit_dpi)
                } else {
                    density
                };
                let width = w as f32 * 72.0 / dpi_x.max(1.0)
                    * (num(props, "sx", 100000.0) / 100000.0).clamp(0.001, 1000.0);
                let height = h as f32 * 72.0 / dpi_y.max(1.0)
                    * (num(props, "sy", 100000.0) / 100000.0).clamp(0.001, 1000.0);
                if width <= 0.0 || height <= 0.0 {
                    return None;
                }
                let align = props.and_then(|n| n.attr("algn")).unwrap_or("tl");
                let x = match align {
                    "tr" | "r" | "br" => f.size.width.as_pt() - width,
                    "t" | "ctr" | "b" => (f.size.width.as_pt() - width) / 2.0,
                    _ => 0.0,
                };
                let y = match align {
                    "bl" | "b" | "br" => f.size.height.as_pt() - height,
                    "l" | "ctr" | "r" => (f.size.height.as_pt() - height) / 2.0,
                    _ => 0.0,
                };
                let flip = props.and_then(|n| n.attr("flip")).unwrap_or("none");
                Some(lo_core::ImageTile {
                    width,
                    height,
                    offset_x: (x + pt(props, "tx", 0.0)).rem_euclid(width),
                    offset_y: (y + pt(props, "ty", 0.0)).rem_euclid(height),
                    flip_x: matches!(flip, "x" | "xy"),
                    flip_y: matches!(flip, "y" | "xy"),
                })
            })
        } else {
            None
        };
        let clip_paths = n
            .child("spPr")
            .and_then(|pr| pr.child("custGeom").or_else(|| pr.child("prstGeom")))
            .filter(|g| g.attr("prst") != Some("rect"))
            .and_then(|g| {
                crate::pptx_geometry::geometry(g, f.size.width.as_pt(), f.size.height.as_pt())
            })
            .and_then(|kind| {
                if let ShapeKind::Custom(paths) = kind {
                    Some(paths)
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let name = n.child("nvPicPr").and_then(|n| n.child("cNvPr"));
        slide
            .elements
            .push(SlideElement::Image(lo_core::ImageElement {
                clip_paths,
                tile,
                transform: None,
                crop,
                rotation,
                flip_horizontal,
                flip_vertical,
                name: path.clone(),
                mime_type: mime.into(),
                data,
                frame: f,
                alt: name.and_then(|n| n.attr("descr")).unwrap_or("").into(),
                opacity: ((num(blip.and_then(|n| n.child("alphaModFix")), "amt", 100000.0)
                    / 100000.0)
                    .clamp(0.0, 1.0)
                    * 255.0)
                    .round() as u8,
            }));
        Ok(())
    }
    fn text(&self, body: &XmlNode, chain: &[&XmlNode], slide: &mut Slide, f: Rect, angle: f32) {
        self.text_with_fit(body, chain, slide, f, angle, false);
    }
    fn text_with_fit(
        &self,
        body: &XmlNode,
        chain: &[&XmlNode],
        slide: &mut Slide,
        f: Rect,
        angle: f32,
        generated_layout: bool,
    ) {
        let ph = chain
            .iter()
            .find_map(|n| placeholder(n))
            .map(|n| n.attr("type").unwrap_or("body"))
            .unwrap_or("other");
        let title = matches!(ph, "title" | "ctrTitle");
        let master_style = self.master.and_then(|n| n.child("txStyles")).and_then(|n| {
            n.child(if title {
                "titleStyle"
            } else if matches!(ph, "body" | "subTitle" | "obj") {
                "bodyStyle"
            } else {
                "otherStyle"
            })
        });
        let mut merged_body = XmlNode::default();
        for node in chain
            .iter()
            .rev()
            .filter_map(|n| n.child("txBody").and_then(|n| n.child("bodyPr")))
        {
            merged_body.attributes.extend(node.attributes.clone());
        }
        merged_body.children = chain
            .iter()
            .filter_map(|n| n.child("txBody").and_then(|n| n.child("bodyPr")))
            .flat_map(|n| n.children.clone())
            .collect();
        let bodypr = Some(&merged_body);
        let vertical = bodypr.and_then(|n| n.attr("vert"));
        let angle = angle
            + match vertical {
                Some("vert") => 90.0,
                Some("vert270") => -90.0,
                _ => 0.0,
            };
        let f = if matches!(vertical, Some("vert" | "vert270")) {
            let (w, h) = (f.size.width.as_pt(), f.size.height.as_pt());
            rect(
                f.origin.x.as_pt() + (w - h) / 2.0,
                f.origin.y.as_pt() + (h - w) / 2.0,
                h,
                w,
            )
        } else {
            f
        };
        let left = pt(bodypr, "lIns", 7.2);
        let right = pt(bodypr, "rIns", 7.2);
        let top = pt(bodypr, "tIns", 3.6);
        let bottom = pt(bodypr, "bIns", 3.6);
        let width = (f.size.width.as_pt() - left - right).max(1.0);
        let height = (f.size.height.as_pt() - top - bottom).max(1.0);
        let columns = num(bodypr, "numCol", 1.0).clamp(1.0, 16.0) as usize;
        let column_gap = pt(bodypr, "spcCol", 0.0).clamp(0.0, width / columns as f32);
        let width = ((width - column_gap * (columns - 1) as f32) / columns as f32).max(1.0);
        let rtl_columns = merged_body
            .attr("rtlCol")
            .is_some_and(|v| matches!(v, "1" | "true"));
        let scale = bodypr
            .and_then(|n| n.child("normAutofit"))
            .map(|n| num(Some(n), "fontScale", 100000.0) / 100000.0)
            .unwrap_or(1.0)
            .clamp(0.01, 1.0);
        // LibreOffice's normal autofit rounds the resulting font size to whole
        // points (SdrTextObj::setupAutoFitText). Measure and draw the same size.
        let round_autofit = merged_body.child("normAutofit").is_some();
        let scaled_font = |size: u32| {
            let size = size as f32 * scale;
            if round_autofit {
                (size / 1000.0).round().max(1.0) as u32 * 1000
            } else {
                size.round().max(1.0) as u32
            }
        };
        let mut paragraphs = vec![];
        let mut numbering: [Option<(String, u32)>; 9] = std::array::from_fn(|_| None);
        let paragraph_count = body.children_named("p").count();
        for (paragraph_index, p) in body.children_named("p").enumerate() {
            let pp = p.child("pPr");
            let level = num(pp, "lvl", 0.0).clamp(0.0, 8.0) as usize;
            let key = format!("lvl{}pPr", level + 1);
            let mut properties = vec![];
            if master_style.and_then(|n| n.child(&key)).is_none() {
                if let Some(n) = self.default_text.and_then(|n| n.child(&key)) {
                    properties.push(n);
                }
            }
            if let Some(n) = master_style.and_then(|n| n.child(&key)) {
                properties.push(n);
            }
            let inherited_count = properties.len();
            for n in chain.iter().rev() {
                if let Some(tx) = n.child("txBody") {
                    if let Some(n) = tx.child("lstStyle").and_then(|n| n.child(&key)) {
                        properties.push(n);
                    }
                    if !std::ptr::eq(*n, chain[0]) {
                        if let Some(n) = tx.child("p").and_then(|n| n.child("pPr")) {
                            properties.push(n);
                        }
                    }
                }
            }
            if let Some(n) = pp {
                properties.push(n);
            }
            let mut base = lo_core::style::CellStyle::default();
            base.font_family = if title {
                self.theme.major.clone()
            } else {
                self.theme.minor.clone()
            };
            base.font_size_mpt = Some(if title { 32000 } else { 18000 });
            base.foreground = self
                .theme
                .colors
                .get("tx1")
                .map(|s| format!("#{s}"))
                .unwrap_or_else(|| "#000000".into());
            let apply_font_ref = |base: &mut lo_core::style::CellStyle| {
                for n in chain
                    .iter()
                    .rev()
                    .filter_map(|n| n.child("style").and_then(|n| n.child("fontRef")))
                {
                    if let Some(color) = self.theme.color(n) {
                        base.foreground = color;
                    }
                    if let Some(index) = n.attr("idx") {
                        base.font_family = if index == "major" {
                            self.theme.major.clone()
                        } else {
                            self.theme.minor.clone()
                        };
                    }
                }
            };
            let mut alignment = Alignment::Start;
            let mut indent = 0.0;
            let mut before = 0.0;
            let mut after = 0.0;
            let mut before_percent = None;
            let mut after_percent = None;
            let mut spacing = 1.2;
            let mut percentage_spacing = false;
            let mut point_spacing = None;
            let mut marker = None;
            let mut auto_number = None;
            let mut hanging_indent = 0.0;
            let property_count = properties.len();
            for (index, pr) in properties.into_iter().enumerate() {
                if index == inherited_count {
                    apply_font_ref(&mut base);
                }
                if let Some(n) = pr.child("defRPr") {
                    self.run_style(n, &mut base);
                }
                if let Some(a) = pr.attr("algn") {
                    alignment = match a {
                        "ctr" => Alignment::Center,
                        "r" => Alignment::End,
                        _ => Alignment::Start,
                    };
                }
                if pr.attr("marL").is_some() {
                    indent = pt(Some(pr), "marL", 0.0);
                }
                if pr.attr("indent").is_some() {
                    hanging_indent = pt(Some(pr), "indent", 0.0);
                }
                if let Some(n) = pr.child("lnSpc").and_then(|n| n.child("spcPct")) {
                    spacing = (num(Some(n), "val", 100000.0) / 100000.0 * 1.2).clamp(0.5, 5.0);
                    percentage_spacing = true;
                    point_spacing = None;
                }
                if let Some(n) = pr.child("lnSpc").and_then(|n| n.child("spcPts")) {
                    point_spacing = Some((num(Some(n), "val", 100.0) / 100.0).clamp(0.01, 1000.0));
                    percentage_spacing = false;
                }
                if let Some(n) = pr.child("spcBef").and_then(|n| n.child("spcPts")) {
                    before = num(Some(n), "val", 0.0) / 100.0;
                    before_percent = None;
                }
                if let Some(n) = pr.child("spcAft").and_then(|n| n.child("spcPts")) {
                    after = num(Some(n), "val", 0.0) / 100.0;
                    after_percent = None;
                }
                if let Some(n) = pr.child("spcBef").and_then(|n| n.child("spcPct")) {
                    before_percent = Some((num(Some(n), "val", 0.0) / 100000.0).clamp(0.0, 10.0));
                }
                if let Some(n) = pr.child("spcAft").and_then(|n| n.child("spcPct")) {
                    after_percent = Some((num(Some(n), "val", 0.0) / 100000.0).clamp(0.0, 10.0));
                }
                if let Some(n) = pr.child("buChar") {
                    marker = n.attr("char").map(str::to_string);
                    auto_number = None;
                }
                if pr.child("buBlip").is_some() {
                    marker = Some(String::new()); // Reserve the picture bullet's text indentation.
                    auto_number = None;
                }
                if pr.child("buNone").is_some() {
                    marker = None;
                    auto_number = None;
                }
                if let Some(n) = pr.child("buAutoNum") {
                    auto_number = Some((
                        n.attr("type").unwrap_or("arabicPeriod").to_string(),
                        num(Some(n), "startAt", 1.0).clamp(1.0, 32767.0) as u32,
                    ));
                }
            }
            if let Some((kind, start)) = auto_number {
                let value = numbering[level]
                    .as_ref()
                    .filter(|(k, _)| k == &kind)
                    .map(|(_, n)| n.saturating_add(1).max(start))
                    .unwrap_or(start);
                marker = Some(crate::pptx_text::number_marker(value, &kind));
                numbering[level] = Some((kind, value));
            } else {
                numbering[level] = None;
            }
            for entry in &mut numbering[level + 1..] {
                *entry = None;
            }
            if inherited_count == property_count {
                apply_font_ref(&mut base);
            }
            if percentage_spacing {
                if let Some(fit) = bodypr.and_then(|n| n.child("normAutofit")) {
                    spacing = (spacing
                        - 1.2 * (num(Some(fit), "lnSpcReduction", 0.0) / 100000.0).clamp(0.0, 0.8))
                    .max(0.2);
                }
            }
            let mut runs = vec![];
            for n in &p.children {
                if n.local_name() == "br" {
                    let mut style = base.clone();
                    if let Some(pr) = n.child("rPr") {
                        self.run_style(pr, &mut style);
                    }
                    style.font_size_mpt = Some(scaled_font(style.font_size_mpt.unwrap_or(18000)));
                    runs.push(lo_core::calc::CellTextRun {
                        text: "\n".into(),
                        style,
                    });
                    continue;
                }
                if !matches!(n.local_name(), "r" | "fld") {
                    continue;
                }
                let mut style = base.clone();
                if let Some(pr) = n.child("rPr") {
                    self.run_style(pr, &mut style);
                }
                style.font_size_mpt = Some(scaled_font(style.font_size_mpt.unwrap_or(18000)));
                let text = if n.local_name() == "fld" && n.attr("type") == Some("slidenum") {
                    self.slide_number.to_string()
                } else {
                    n.child("t").map(|n| n.text_content()).unwrap_or_default()
                };
                runs.push(lo_core::calc::CellTextRun { text, style });
            }
            if runs.is_empty() {
                let mut empty_style = base.clone();
                if let Some(end) = p.child("endParaRPr") {
                    self.run_style(end, &mut empty_style);
                }
                empty_style.font_size_mpt =
                    Some(scaled_font(empty_style.font_size_mpt.unwrap_or(18000)));
                runs.push(lo_core::calc::CellTextRun {
                    text: " ".into(),
                    style: empty_style,
                });
            }
            let mut hanging_marker = None;
            let paragraph_size = runs
                .iter()
                .map(|r| r.style.font_size_mpt.unwrap_or(18000) as f32 / 1000.0)
                .fold(0.0_f32, f32::max);
            if paragraph_size > 0.0 {
                spacing = if let Some(points) = point_spacing {
                    points / paragraph_size
                } else if percentage_spacing
                    || !bodypr
                        .and_then(|n| n.attr("compatLnSpc"))
                        .is_some_and(|v| matches!(v, "1" | "true"))
                {
                    spacing
                } else {
                    spacing * crate::pptx_text::natural_line_height(&runs) / paragraph_size / 1.2
                };
            }
            if let Some(percent) = before_percent {
                before = paragraph_size * percent;
            }
            if let Some(percent) = after_percent {
                after = paragraph_size * percent;
            }
            if paragraphs.is_empty() {
                before = if percentage_spacing {
                    paragraph_size * (spacing - 1.2).max(0.0)
                } else {
                    0.0
                };
            }
            if paragraph_index + 1 == paragraph_count
                && matches!(bodypr.and_then(|n| n.attr("anchor")), Some("ctr" | "b"))
            {
                after = crate::pptx_text::descent(&runs);
            }
            // The textbox height ends at its last baseline. Preserve the normal
            // baseline-to-baseline leading before the next paragraph as well.
            if paragraph_index + 1 < paragraph_count {
                after += paragraph_size * (spacing - 1.0);
            }
            if let Some(marker) = marker.filter(|_| runs.iter().any(|r| !r.text.trim().is_empty()))
            {
                let mut marker_style = base.clone();
                marker_style.font_size_mpt =
                    Some(scaled_font(marker_style.font_size_mpt.unwrap_or(18000)));
                if hanging_indent < 0.0 {
                    hanging_marker = Some(marker);
                } else {
                    runs.insert(
                        0,
                        lo_core::calc::CellTextRun {
                            text: format!("{marker} "),
                            style: marker_style,
                        },
                    );
                }
            }
            let text = runs.iter().map(|r| r.text.as_str()).collect::<String>();
            let wrap_text = bodypr.and_then(|n| n.attr("wrap")) != Some("none")
                || bodypr.is_some_and(|n| {
                    n.child("spAutoFit").is_some() || n.child("normAutofit").is_some()
                });
            let text_width = if wrap_text {
                (width - indent).max(1.0)
            } else {
                1e8
            };
            let first_indent = if hanging_marker.is_some() {
                0.0
            } else {
                hanging_indent
            };
            let h = crate::pptx_text::height_indented(&runs, text_width, spacing, first_indent)
                .max(paragraph_size);
            let style = TextBoxStyle {
                paragraph_markers: vec![hanging_marker],
                paragraph_indents: vec![hanging_indent],
                wrap_text,
                runs,
                line_spacing: spacing,
                font_size_pt: base.font_size_mpt.unwrap_or(18000) as f32 / 1000.0 * scale,
                font_family: base.font_family.clone(),
                alignment,
                padding: Length::pt(0.0),
                border: false,
                rotation: -angle.to_radians(),
                rotation_origin: Some((
                    f.origin.x.as_pt() + f.size.width.as_pt() / 2.0,
                    f.origin.y.as_pt() + f.size.height.as_pt() / 2.0,
                )),
                ..Default::default()
            };
            paragraphs.push((text, style, h, indent, before, after));
        }
        if columns > 1
            || generated_layout
            || merged_body.child("normAutofit").is_some_and(|n| {
                n.attr("fontScale").is_some() || n.attr("lnSpcReduction").is_some()
            })
        {
            if columns > 1 {
                if let Some(first) = paragraphs.first_mut() {
                    first.4 = 0.0;
                }
            }
            if merged_body.child("normAutofit").is_some() {
                // Stored fontScale can be stale after a font substitution. Fit
                // the measured lines into the requested column count as well.
                let original = paragraphs.clone();
                let fits = |paragraphs: &Vec<(String, TextBoxStyle, f32, f32, f32, f32)>| {
                    let mut column = 0;
                    let mut used = 0.0;
                    // A trailing empty paragraph contributes to anchoring, but
                    // must not force visible text into a smaller font size.
                    let visible_end = paragraphs
                        .iter()
                        .rposition(|(text, ..)| !text.trim().is_empty())
                        .map_or(0, |index| index + 1);
                    for (_, style, _, indent, before, after) in &paragraphs[..visible_end] {
                        used += before;
                        let lines = crate::pptx_text::wrapped_runs(
                            &style.runs,
                            (width - indent).max(1.0),
                            crate::pptx_text::first_line_indent(style),
                        );
                        for (index, (_, size)) in lines.iter().enumerate() {
                            if used + *size > height && used > 0.0 {
                                column += 1;
                                used = 0.0;
                            }
                            if column >= columns || *size > height {
                                return false;
                            }
                            used += size
                                * if index + 1 == lines.len() {
                                    1.0
                                } else {
                                    style.line_spacing
                                };
                        }
                        used += after;
                    }
                    true
                };
                if !fits(&paragraphs) {
                    let (mut low, mut high) = (0.01_f32, 1.0_f32);
                    for _ in 0..12 {
                        let factor = (low + high) / 2.0;
                        paragraphs = original.clone();
                        for (_, style, _, _, _, _) in &mut paragraphs {
                            for run in &mut style.runs {
                                run.style.font_size_mpt = Some(
                                    (run.style.font_size_mpt.unwrap_or(18000) as f32 * factor)
                                        .max(1.0) as u32,
                                );
                            }
                        }
                        if fits(&paragraphs) {
                            low = factor;
                        } else {
                            high = factor;
                        }
                    }
                    paragraphs = original;
                    for (_, style, h, indent, _, _) in &mut paragraphs {
                        style.font_size_pt *= low;
                        for run in &mut style.runs {
                            run.style.font_size_mpt = Some(
                                (run.style.font_size_mpt.unwrap_or(18000) as f32 * low).max(1.0)
                                    as u32,
                            );
                        }
                        *h = crate::pptx_text::height_indented(
                            &style.runs,
                            (width - *indent).max(1.0),
                            style.line_spacing,
                            crate::pptx_text::first_line_indent(style),
                        );
                    }
                }
            }
        }
        let total: f32 = paragraphs.iter().map(|(_, _, h, _, b, a)| h + b + a).sum();
        let anchor = bodypr.and_then(|n| n.attr("anchor"));
        // Normal autofit anchors the scaled content, including trailing empty
        // paragraphs. Keep the existing overflow handling for other modes.
        let anchor_space = if round_autofit {
            height - total
        } else {
            (height - total).max(0.0)
        };
        let mut y = f.origin.y.as_pt()
            + top
            + match anchor {
                Some("ctr") => anchor_space / 2.0,
                Some("b") => anchor_space,
                _ => 0.0,
            };
        for (text, style, h, indent, before, after) in paragraphs {
            if columns > 1 {
                // A paragraph can span columns. Splitting whole paragraphs would
                // leave the first column overflowing for a single long paragraph.
                let mut column =
                    ((y - f.origin.y.as_pt() - top) / height).floor().max(0.0) as usize;
                column = column.min(columns - 1);
                let mut offset = (y - f.origin.y.as_pt() - top) - column as f32 * height;
                offset += before;
                let lines = crate::pptx_text::wrapped_runs(
                    &style.runs,
                    if style.wrap_text {
                        (width - indent).max(1.0)
                    } else {
                        1e8
                    },
                    crate::pptx_text::first_line_indent(&style),
                );
                let line_count = lines.len();
                for (line_index, (runs, line_height)) in lines.into_iter().enumerate() {
                    if offset + line_height > height && offset > 0.0 && column + 1 < columns {
                        column += 1;
                        offset = 0.0;
                    }
                    let physical_column = if rtl_columns {
                        columns - 1 - column
                    } else {
                        column
                    };
                    let mut line_style = style.clone();
                    if line_index > 0 {
                        line_style.paragraph_markers.clear();
                        line_style.paragraph_indents.clear();
                    }
                    line_style.runs = runs;
                    line_style.wrap_text = false;
                    slide.elements.push(SlideElement::TextBox(TextBox {
                        frame: rect(
                            f.origin.x.as_pt()
                                + left
                                + physical_column as f32 * (width + column_gap)
                                + indent,
                            f.origin.y.as_pt() + top + offset,
                            (width - indent).max(1.0),
                            line_height + 0.5,
                        ),
                        text: line_style.runs.iter().map(|r| r.text.as_str()).collect(),
                        style: line_style,
                    }));
                    offset += line_height
                        * if line_index + 1 == line_count {
                            1.0
                        } else {
                            style.line_spacing
                        };
                }
                y = f.origin.y.as_pt() + top + column as f32 * height + offset + after;
                continue;
            }
            y += before;
            slide.elements.push(SlideElement::TextBox(TextBox {
                frame: rect(
                    f.origin.x.as_pt() + left + indent,
                    y,
                    (width - indent).max(1.0),
                    h + 0.5,
                ),
                text,
                style,
            }));
            y += h + after;
        }
    }
    fn run_style(&self, n: &XmlNode, s: &mut lo_core::style::CellStyle) {
        if n.attr("sz").is_some() {
            s.font_size_mpt =
                Some((num(Some(n), "sz", 1800.0).clamp(100.0, 40000.0) * 10.0).round() as u32);
        }
        if let Some(v) = n.attr("b") {
            s.bold = matches!(v, "1" | "true");
        }
        if let Some(v) = n.attr("i") {
            s.italic = matches!(v, "1" | "true");
        }
        if let Some(v) = n.attr("u") {
            s.underline = v != "none";
        }
        if let Some(v) = n.child("latin").and_then(|n| n.attr("typeface")) {
            s.font_family = self.theme.font(v);
        }
        if let Some(v) = n.child("solidFill").and_then(|n| self.theme.color(n)) {
            s.foreground = v;
        }
        if let Some(v) = n.child("highlight").and_then(|n| self.theme.color(n)) {
            s.background = v;
        }
        if n.child("hlinkClick").is_some() {
            if n.child("solidFill").is_none() {
                s.foreground = self
                    .theme
                    .colors
                    .get("hlink")
                    .map(|c| format!("#{c}"))
                    .unwrap_or_else(|| "#0000FF".into());
            }
            if n.attr("u").is_none() {
                s.underline = true;
            }
        }
    }
    fn table(&self, table: &XmlNode, slide: &mut Slide, f: Rect) {
        let widths: Vec<f32> = table
            .child("tblGrid")
            .map(|n| {
                n.children_named("gridCol")
                    .map(|n| pt(Some(n), "w", 72.0).max(0.0))
                    .collect()
            })
            .unwrap_or_default();
        let rows: Vec<_> = table.children_named("tr").collect();
        let heights: Vec<_> = rows
            .iter()
            .map(|n| pt(Some(n), "h", 24.0).max(1.0))
            .collect();
        let props = table.child("tblPr");
        let enabled = |key| {
            props
                .and_then(|n| n.attr(key))
                .is_some_and(|v| matches!(v, "1" | "true"))
        };
        let style_id = props
            .and_then(|n| n.child("tableStyleId"))
            .map(|n| n.text_content());
        let style = self.table_styles.and_then(|n| {
            n.children_named("tblStyle")
                .find(|n| n.attr("styleId") == style_id.as_deref())
        });
        let mut y = f.origin.y.as_pt();
        for (ri, row) in rows.iter().enumerate() {
            let h = heights[ri];
            let mut x = f.origin.x.as_pt();
            for (ci, cell) in row.children_named("tc").enumerate() {
                let w = widths.get(ci).copied().unwrap_or(72.0);
                let span = num(Some(cell), "gridSpan", 1.0).clamp(1.0, 256.0) as usize;
                let rowspan = num(Some(cell), "rowSpan", 1.0).clamp(1.0, 65536.0) as usize;
                let cw = widths.iter().skip(ci).take(span).sum::<f32>().max(w);
                let ch = heights.iter().skip(ri).take(rowspan).sum::<f32>().max(h);
                if !["hMerge", "vMerge"]
                    .iter()
                    .any(|key| cell.attr(key).is_some_and(|v| matches!(v, "1" | "true")))
                {
                    let mut layers = vec![];
                    if let Some(st) = style {
                        if let Some(n) = st.child("wholeTbl") {
                            layers.push(n);
                        }
                        for (flag, index, a, b) in [
                            (
                                "bandRow",
                                ri.saturating_sub(usize::from(enabled("firstRow"))),
                                "band1H",
                                "band2H",
                            ),
                            (
                                "bandCol",
                                ci.saturating_sub(usize::from(enabled("firstCol"))),
                                "band1V",
                                "band2V",
                            ),
                        ] {
                            if enabled(flag) {
                                if let Some(n) = st.child(if index % 2 == 0 { a } else { b }) {
                                    layers.push(n);
                                }
                            }
                        }
                        for (flag, edge) in [
                            ("firstCol", ci == 0),
                            ("lastCol", ci + span >= widths.len()),
                            ("firstRow", ri == 0),
                            ("lastRow", ri + rowspan >= rows.len()),
                        ] {
                            if edge && enabled(flag) {
                                if let Some(n) = st.child(flag) {
                                    layers.push(n);
                                }
                            }
                        }
                    }
                    let direct = cell.child("tcPr");
                    let mut fill = "none".to_string();
                    for n in layers
                        .iter()
                        .filter_map(|n| n.child("tcStyle"))
                        .chain(direct)
                    {
                        let resolved = n.child("fillRef").and_then(|r| self.theme_fill(r));
                        let paint = resolved
                            .as_ref()
                            .unwrap_or_else(|| n.child("fill").unwrap_or(n));
                        if paint.child("noFill").is_some() || paint.local_name() == "noFill" {
                            fill = "none".into();
                        } else if let Some(c) = if paint.local_name() == "solidFill" {
                            self.theme.color(paint)
                        } else {
                            paint.child("solidFill").and_then(|n| self.theme.color(n))
                        } {
                            fill = c;
                        }
                    }
                    let cf = rect(x, y, cw, ch);
                    slide.elements.push(SlideElement::Shape(Shape {
                        frame: cf,
                        kind: ShapeKind::Rectangle,
                        style: ShapeStyle {
                            transform: None,
                            fill,
                            stroke: "none".into(),
                            ..Default::default()
                        },
                    }));
                    // Borders are narrow filled rectangles: retain fractional point widths.
                    for (side, key, inside) in [
                        ("left", "lnL", ci > 0),
                        ("right", "lnR", ci + span < widths.len()),
                        ("top", "lnT", ri > 0),
                        ("bottom", "lnB", ri + rowspan < rows.len()),
                    ] {
                        let mut line = None;
                        for layer in &layers {
                            if let Some(b) = layer.child("tcStyle").and_then(|n| n.child("tcBdr")) {
                                if let Some(n) = b
                                    .child(if inside {
                                        if side == "left" || side == "right" {
                                            "insideV"
                                        } else {
                                            "insideH"
                                        }
                                    } else {
                                        side
                                    })
                                    .or_else(|| b.child(side))
                                {
                                    line = n.child("ln").or_else(|| n.child("lnRef"));
                                }
                            }
                        }
                        if let Some(n) = direct.and_then(|n| n.child(key)) {
                            line = Some(n);
                        }
                        if let Some(n) = line.filter(|n| n.child("noFill").is_none()) {
                            let color = n
                                .child("solidFill")
                                .and_then(|n| self.theme.color(n))
                                .or_else(|| self.theme.color(n));
                            if let Some(color) = color {
                                let bw = pt(Some(n), "w", 1.0).clamp(0.0, 100.0);
                                let bf = match side {
                                    "left" => rect(x - bw / 2.0, y, bw, ch),
                                    "right" => rect(x + cw - bw / 2.0, y, bw, ch),
                                    "top" => rect(x, y - bw / 2.0, cw, bw),
                                    _ => rect(x, y + ch - bw / 2.0, cw, bw),
                                };
                                slide.elements.push(SlideElement::Shape(Shape {
                                    frame: bf,
                                    kind: ShapeKind::Rectangle,
                                    style: ShapeStyle {
                                        transform: None,
                                        fill: color,
                                        stroke: "none".into(),
                                        ..Default::default()
                                    },
                                }));
                            }
                        }
                    }
                    if let Some(body) = cell.child("txBody") {
                        let mut inherited = XmlNode {
                            name: "tc".into(),
                            ..Default::default()
                        };
                        let mut tx = XmlNode {
                            name: "txBody".into(),
                            ..Default::default()
                        };
                        let mut bp = XmlNode {
                            name: "bodyPr".into(),
                            ..Default::default()
                        };
                        for (source, target) in [
                            ("marL", "lIns"),
                            ("marR", "rIns"),
                            ("marT", "tIns"),
                            ("marB", "bIns"),
                            ("anchor", "anchor"),
                        ] {
                            if let Some(v) = direct.and_then(|n| n.attr(source)) {
                                bp.attributes.insert(target.into(), v.into());
                            }
                        }
                        tx.children.push(bp.into());
                        let mut defaults = XmlNode {
                            name: "defRPr".into(),
                            ..Default::default()
                        };
                        for n in layers.iter().filter_map(|n| n.child("tcTxStyle")) {
                            for key in ["b", "i"] {
                                if let Some(v) = n.attr(key) {
                                    defaults.attributes.insert(
                                        key.into(),
                                        if matches!(v, "on" | "1" | "true") {
                                            "1"
                                        } else {
                                            "0"
                                        }
                                        .into(),
                                    );
                                }
                            }
                            if let Some(c) = self.theme.color(n) {
                                defaults.children.retain(|n| n.local_name() != "solidFill");
                                defaults.children.push(
                                    parse_xml_document(&format!(
                                        "<solidFill><srgbClr val=\"{}\"/></solidFill>",
                                        &c[1..]
                                    ))
                                    .expect("validated color")
                                    .into(),
                                );
                            }
                        }
                        let mut ppr = XmlNode {
                            name: "lvl1pPr".into(),
                            ..Default::default()
                        };
                        ppr.children.push(defaults.into());
                        let mut list = XmlNode {
                            name: "lstStyle".into(),
                            ..Default::default()
                        };
                        list.children.push(ppr.into());
                        tx.children.push(list.into());
                        inherited.children.push(tx.into());
                        self.text(body, &[cell, &inherited], slide, cf, 0.0);
                    }
                }
                x += w;
            }
            y += h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        lo_zip::write_zip_to_vec(
            &parts
                .iter()
                .map(|(p, s)| lo_zip::ZipEntry::new(*p, s.as_bytes().to_vec()))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
    #[test]
    fn embedded_object_uses_preview_fallback_without_opening_payload() {
        let xml = parse_xml_document(r#"<graphicData><AlternateContent><Choice Requires="v"><oleObj id="payload"><embed/></oleObj></Choice><Fallback><oleObj id="payload"><embed/><pic><blipFill><blip embed="preview"/></blipFill></pic></oleObj></Fallback></AlternateContent></graphicData>"#).unwrap();
        let pic = ole_preview(&xml).unwrap();
        assert_eq!(
            pic.child("blipFill")
                .unwrap()
                .child("blip")
                .unwrap()
                .attr("embed"),
            Some("preview")
        );
        let no_preview =
            parse_xml_document("<graphicData><oleObj><embed/></oleObj></graphicData>").unwrap();
        assert!(ole_preview(&no_preview).is_none());
    }
    #[test]
    fn legacy_ole_uses_vml_preview_without_reading_embedded_object() {
        let bytes = package(&[
            (
                "ppt/presentation.xml",
                r#"<presentation><sldIdLst><sldId id="slide"/></sldIdLst></presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="slide" Target="slides/slide.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide.xml",
                r#"<sld><cSld><spTree><graphicFrame><xfrm><off x="127000" y="254000"/><ext cx="1270000" cy="635000"/></xfrm><graphic><graphicData><oleObj spid="object1" id="missing-payload"/></graphicData></graphic></graphicFrame></spTree></cSld></sld>"#,
            ),
            (
                "ppt/slides/_rels/slide.xml.rels",
                r#"<Relationships><Relationship Id="vml" Type="legacyDrawing" Target="../drawings/preview.vml"/></Relationships>"#,
            ),
            (
                "ppt/drawings/preview.vml",
                r#"<xml><shape id="unrelated"><imagedata relid="missing"/></shape><shape spid="object1"><imagedata relid="image"/></shape></xml>"#,
            ),
            (
                "ppt/drawings/_rels/preview.vml.rels",
                r#"<Relationships><Relationship Id="image" Type="image" Target="../media/preview.png"/></Relationships>"#,
            ),
            ("ppt/media/preview.png", "stored-preview"),
        ]);
        let deck = load("OLE".into(), &bytes).unwrap();
        let image = deck.slides[0]
            .elements
            .iter()
            .find_map(|e| match e {
                SlideElement::Image(i) => Some(i),
                _ => None,
            })
            .unwrap();
        assert_eq!(image.data, b"stored-preview");
        assert_eq!(image.frame.origin.x.as_pt(), 10.0);
        assert_eq!(image.frame.size.width.as_pt(), 100.0);
    }
    #[test]
    fn relationship_order_dimensions_and_fields() {
        let bytes = package(&[
            (
                "ppt/presentation.xml",
                r#"<presentation><sldSz cx="12192000" cy="6858000"/><sldIdLst><sldId id="256" r:id="second"/><sldId id="257" r:id="first"/></sldIdLst></presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="first" Target="slides/a.xml"/><Relationship Id="second" Target="slides/b.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/a.xml",
                r#"<sld><cSld><spTree><sp><txBody><p><r><t>A</t></r></p></txBody></sp></spTree></cSld></sld>"#,
            ),
            (
                "ppt/slides/b.xml",
                r#"<sld><cSld><spTree><sp><txBody><p><r><t>B</t></r><fld type="slidenum"><t>‹#›</t></fld></p></txBody></sp></spTree></cSld></sld>"#,
            ),
        ]);
        let p = load("test".into(), &bytes).unwrap();
        assert_eq!(p.page_size.width.as_pt(), 960.0);
        assert_eq!(p.page_size.height.as_pt(), 540.0);
        let text = |s: &Slide| {
            s.elements
                .iter()
                .filter_map(|e| {
                    if let SlideElement::TextBox(t) = e {
                        Some(t.text.clone())
                    } else {
                        None
                    }
                })
                .collect::<String>()
        };
        assert_eq!(text(&p.slides[0]), "B1");
        assert_eq!(text(&p.slides[1]), "A");
    }
    #[test]
    fn empty_transform_inherits_and_group_coordinates_scale() {
        let a = parse_xml_document("<sp><spPr><xfrm rot=\"0\"/></spPr></sp>").unwrap();
        let b=parse_xml_document("<sp><spPr><xfrm><off x=\"127000\" y=\"254000\"/><ext cx=\"1270000\" cy=\"635000\"/></xfrm></spPr></sp>").unwrap();
        let g=parse_xml_document("<grpSp><grpSpPr><xfrm><off x=\"1270000\" y=\"2540000\"/><ext cx=\"2540000\" cy=\"1270000\"/><chOff x=\"127000\" y=\"254000\"/><chExt cx=\"1270000\" cy=\"635000\"/></xfrm></grpSpPr></grpSp>").unwrap();
        let f = frame(&[&a, &b], Transform::identity().group(&g));
        assert_eq!(
            (
                f.origin.x.as_pt(),
                f.origin.y.as_pt(),
                f.size.width.as_pt(),
                f.size.height.as_pt()
            ),
            (100.0, 200.0, 200.0, 100.0)
        );
    }
    #[test]
    fn table_styles_merge_and_direct_run_override() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let theme_root=parse_xml_document(r#"<theme><themeElements><fmtScheme><fillStyleLst><solidFill><schemeClr val="phClr"/></solidFill></fillStyleLst></fmtScheme></themeElements></theme>"#).unwrap();
        let mut budget = 100;
        let styles=parse_xml_document(r#"<tblStyleLst><tblStyle styleId="test"><wholeTbl><tcTxStyle b="on"><srgbClr val="FFFFFF"/></tcTxStyle><tcStyle><fillRef idx="1"><srgbClr val="123456"/></fillRef></tcStyle></wholeTbl></tblStyle></tblStyleLst>"#).unwrap();
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: Some(&theme_root),
            master: None,
            layout: None,
            default_text: None,
            table_styles: Some(&styles),
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let t=parse_xml_document(r#"<tbl><tblPr><tableStyleId>test</tableStyleId></tblPr><tblGrid><gridCol w="1270000"/><gridCol w="1270000"/></tblGrid><tr h="254000"><tc gridSpan="2" rowSpan="2"><txBody><bodyPr/><p><r><rPr b="0"/><t>Merged</t></r></p></txBody><tcPr marL="12700" anchor="b"/></tc><tc hMerge="1"/></tr><tr h="381000"><tc vMerge="1"/><tc hMerge="1" vMerge="1"/></tr></tbl>"#).unwrap();
        let mut slide = Slide::default();
        ctx.table(&t, &mut slide, rect(0.0, 0.0, 200.0, 50.0));
        assert_eq!(slide.elements.len(), 2);
        let SlideElement::Shape(s) = &slide.elements[0] else {
            panic!()
        };
        assert_eq!(s.style.fill, "#123456");
        assert_eq!(s.frame.size.height.as_pt(), 50.0);
        assert_eq!(s.frame.size.width.as_pt(), 200.0);
        let SlideElement::TextBox(t) = &slide.elements[1] else {
            panic!()
        };
        assert_eq!(t.frame.origin.x.as_pt(), 1.0);
        assert!(t.frame.origin.y.as_pt() > 20.0);
        assert!(!t.style.runs[0].style.bold);
        assert_eq!(t.style.runs[0].style.foreground, "#FFFFFF");
    }
    #[test]
    fn nonplaceholder_does_not_inherit_body_bullets() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let master=parse_xml_document(r#"<sldMaster><txStyles><bodyStyle><lvl1pPr><buChar char="•"/></lvl1pPr></bodyStyle></txStyles></sldMaster>"#).unwrap();
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: Some(&master),
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let shape=parse_xml_document(r#"<sp><txBody><bodyPr/><p><r><rPr sz="3200"><solidFill><srgbClr val="FF0000"/></solidFill></rPr><t>Title</t></r></p></txBody></sp>"#).unwrap();
        let mut slide = Slide::default();
        ctx.shape(&[&shape], &mut slide, Transform::identity());
        let SlideElement::TextBox(t) = &slide.elements[0] else {
            panic!()
        };
        assert_eq!(t.text, "Title");
        assert_eq!(t.style.runs[0].style.font_size_mpt, Some(32000));
        assert_eq!(t.style.runs[0].style.foreground, "#FF0000");
    }
    #[test]
    fn implicit_background_fill_covers_preceding_shapes() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let shape = parse_xml_document(r#"<sp useBgFill="1"><spPr><xfrm><off x="0" y="0"/><ext cx="1270000" cy="1270000"/></xfrm><ln><noFill/></ln></spPr></sp>"#).unwrap();
        let mut slide = Slide::default();
        ctx.shape(&[&shape], &mut slide, Transform::identity());
        let SlideElement::Shape(s) = &slide.elements[0] else {
            panic!()
        };
        assert_eq!(s.style.fill, "#FFFFFF");
        assert_eq!(s.frame.size.width.as_pt(), 100.0);
    }
    #[test]
    fn group_rotation_and_flip_preserve_center() {
        let group = parse_xml_document(r#"<grpSp><grpSpPr><xfrm rot="5400000" flipH="1"><off x="1270000" y="2540000"/><ext cx="2540000" cy="1270000"/><chOff x="0" y="0"/><chExt cx="2540000" cy="1270000"/></xfrm></grpSpPr></grpSp>"#).unwrap();
        let t = Transform::identity().group(&group);
        let m = t.post().unwrap();
        let apply = |x, y| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let center = apply(200.0, 250.0);
        assert!((center.0 - 200.0).abs() < 0.001 && (center.1 - 250.0).abs() < 0.001);
        let corner = apply(100.0, 200.0);
        assert!((corner.0 - 250.0).abs() < 0.001 && (corner.1 - 350.0).abs() < 0.001);
        assert!(Transform::identity().post().is_none());
    }
    #[test]
    fn hidden_groups_are_skipped_but_hidden_children_do_not_hide_parent() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let mut ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let tree = parse_xml_document(r#"<spTree>
          <grpSp><nvGrpSpPr><cNvPr hidden="1"/></nvGrpSpPr><sp><txBody><bodyPr/><p><r><t>Hidden group</t></r></p></txBody></sp></grpSp>
          <grpSp><nvGrpSpPr><cNvPr/></nvGrpSpPr>
            <sp><nvSpPr><cNvPr hidden="true"/></nvSpPr><txBody><bodyPr/><p><r><t>Hidden child</t></r></p></txBody></sp>
            <sp><txBody><bodyPr/><p><r><t>Visible child</t></r></p></txBody></sp>
          </grpSp></spTree>"#).unwrap();
        let mut slide = Slide::default();
        ctx.walk(
            &tree,
            &Relations::default(),
            &mut slide,
            Transform::identity(),
            false,
        )
        .unwrap();
        assert_eq!(slide.elements.len(), 1);
        let SlideElement::TextBox(t) = &slide.elements[0] else {
            panic!()
        };
        assert_eq!(t.text, "Visible child");
    }
    #[test]
    fn dash_lengths_follow_line_width_and_bound_custom_patterns() {
        let line = parse_xml_document(r#"<ln w="25400"><prstDash val="dashDot"/></ln>"#).unwrap();
        assert_eq!(line_dashes(Some(&line)), vec![8000, 6000, 2000, 6000]);
        let line = parse_xml_document(r#"<ln w="12700"><custDash><ds d="200000" sp="50000"/><ds d="-10" sp="NaN"/></custDash></ln>"#).unwrap();
        assert_eq!(line_dashes(Some(&line)), vec![2000, 500, 1, 1000]);
        assert!(line_dashes(None).is_empty());
    }
    #[test]
    fn color_map_swaps_resolve_against_original_theme_colors() {
        let map = parse_xml_document(
            r#"<overrideClrMapping bg1="dk1" tx1="lt1" accent1="accent2" accent2="accent1"/>"#,
        )
        .unwrap();
        let theme = Theme::new(None, Some(&map));
        assert_eq!(theme.colors["bg1"], "000000");
        assert_eq!(theme.colors["tx1"], "FFFFFF");
        assert_eq!(theme.colors["accent1"], "C0504D");
        assert_eq!(theme.colors["accent2"], "4F81BD");
    }
    #[test]
    fn bitmap_shape_preserves_clip_padding_and_opacity() {
        let zip = ZipArchive::new(&package(&[("image.png", "bytes")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let mut ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let tree=parse_xml_document(r#"<spTree><sp><spPr><xfrm><off x="0" y="0"/><ext cx="1270000" cy="1270000"/></xfrm><prstGeom prst="ellipse"/><blipFill><blip embed="image"><alphaModFix amt="50000"/></blip><srcRect l="-10000"/></blipFill></spPr></sp></spTree>"#).unwrap();
        let mut rels = Relations::default();
        rels.paths.insert("image".into(), "image.png".into());
        let mut slide = Slide::default();
        ctx.walk(&tree, &rels, &mut slide, Transform::identity(), false)
            .unwrap();
        let SlideElement::Image(image) = &slide.elements[0] else {
            panic!()
        };
        assert_eq!(image.clip_paths.len(), 1);
        assert_eq!(image.opacity, 128);
        assert!((image.crop[0] + 0.1).abs() < 0.0001);
        assert_eq!(image.frame.size.width.as_pt(), 100.0);
        assert!(slide
            .elements
            .iter()
            .all(|e| !matches!(e,SlideElement::Shape(s) if s.style.fill != "none")));
    }
    #[test]
    fn png_tile_density_uses_physical_units_and_rejects_truncation() {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend(9u32.to_be_bytes());
        data.extend(b"pHYs");
        data.extend(3543u32.to_be_bytes());
        data.extend(3543u32.to_be_bytes());
        data.push(1);
        data.extend([0; 4]);
        let (x, y) = png_density(&data).unwrap();
        assert!((x - 90.0).abs() < 0.02);
        assert_eq!(x, y);
        assert!(png_density(&data[..20]).is_none());
        data[24] = 0;
        assert!(png_density(&data).is_none());
    }
    #[test]
    fn inherited_background_keeps_master_map_while_slide_shapes_use_override() {
        let bytes = package(&[
            (
                "ppt/presentation.xml",
                r#"<presentation><sldIdLst><sldId id="1" r:id="s"/></sldIdLst></presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="s" Target="slides/s.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/s.xml",
                r#"<sld><cSld><spTree><sp><spPr><solidFill><schemeClr val="bg1"/></solidFill></spPr></sp></spTree></cSld><clrMapOvr><overrideClrMapping bg1="dk1"/></clrMapOvr></sld>"#,
            ),
            (
                "ppt/slides/_rels/s.xml.rels",
                r#"<Relationships><Relationship Id="l" Type="x/slideLayout" Target="../slideLayouts/l.xml"/></Relationships>"#,
            ),
            (
                "ppt/slideLayouts/l.xml",
                r#"<sldLayout><cSld><spTree/></cSld></sldLayout>"#,
            ),
            (
                "ppt/slideLayouts/_rels/l.xml.rels",
                r#"<Relationships><Relationship Id="m" Type="x/slideMaster" Target="../slideMasters/m.xml"/></Relationships>"#,
            ),
            (
                "ppt/slideMasters/m.xml",
                r#"<sldMaster><cSld><bg><bgRef idx="1001"><schemeClr val="bg1"/></bgRef></bg><spTree/></cSld><clrMap bg1="lt1"/></sldMaster>"#,
            ),
            (
                "ppt/slideMasters/_rels/m.xml.rels",
                r#"<Relationships><Relationship Id="t" Type="x/theme" Target="../theme/t.xml"/></Relationships>"#,
            ),
            (
                "ppt/theme/t.xml",
                r#"<theme><themeElements><fmtScheme><bgFillStyleLst><solidFill><schemeClr val="phClr"/></solidFill></bgFillStyleLst></fmtScheme></themeElements></theme>"#,
            ),
        ]);
        let deck = load("test".into(), &bytes).unwrap();
        let colors: Vec<_> = deck.slides[0]
            .elements
            .iter()
            .filter_map(|e| {
                if let SlideElement::Shape(s) = e {
                    Some(s.style.fill.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(colors, vec!["#FFFFFF", "#000000"]);
    }
    #[test]
    fn local_theme_fill_beats_inherited_no_fill_and_numbering_restarts() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let root=parse_xml_document(r#"<theme><themeElements><fmtScheme><fillStyleLst><solidFill><srgbClr val="0000FF"/></solidFill></fillStyleLst></fmtScheme></themeElements></theme>"#).unwrap();
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: Some(&root),
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let parent = parse_xml_document(r#"<sp><spPr><noFill/></spPr></sp>"#).unwrap();
        let child=parse_xml_document(r#"<sp><style><fillRef idx="1"/></style><txBody><bodyPr/><p><r><t>Heading</t></r></p><p><pPr><buAutoNum type="arabicPeriod"/></pPr><r><t>One</t></r></p><p><pPr><buAutoNum type="arabicPeriod"/></pPr><r><t>Two</t></r></p><p><pPr><buNone/></pPr><r><t>Break</t></r></p><p><pPr><buAutoNum type="arabicPeriod"/></pPr><r><t>Restart</t></r></p></txBody></sp>"#).unwrap();
        let mut slide = Slide::default();
        ctx.shape(&[&child, &parent], &mut slide, Transform::identity());
        assert!(slide
            .elements
            .iter()
            .any(|e| matches!(e,SlideElement::Shape(s) if s.style.fill=="#0000FF")));
        let texts: Vec<_> = slide
            .elements
            .iter()
            .filter_map(|e| match e {
                SlideElement::TextBox(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec!["Heading", "1. One", "2. Two", "Break", "1. Restart"]
        );
    }
    #[test]
    fn autofit_reduction_applies_only_to_percentage_line_spacing() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        for (properties, multiplier, points) in [
            ("", 1.0, None),
            (r#"<lnSpc><spcPct val="200000"/></lnSpc>"#, 1.8, None),
            (r#"<lnSpc><spcPts val="2000"/></lnSpc>"#, 1.0, Some(20.0)),
        ] {
            let shape=parse_xml_document(&format!(r#"<sp><txBody><bodyPr><normAutofit fontScale="100000" lnSpcReduction="20000"/></bodyPr><p><pPr>{properties}</pPr><r><rPr sz="2000"/><t>One</t></r><br/><r><rPr sz="2000"/><t>Two</t></r></p></txBody></sp>"#)).unwrap();
            let mut slide = Slide::default();
            ctx.text(
                shape.child("txBody").unwrap(),
                &[&shape],
                &mut slide,
                rect(0.0, 0.0, 400.0, 400.0),
                0.0,
            );
            let text = slide
                .elements
                .iter()
                .find_map(|e| match e {
                    SlideElement::TextBox(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            let expected = points.unwrap_or(20.0 * 1.2 * multiplier) / 20.0;
            assert!((text.style.line_spacing - expected).abs() < 0.001);
        }
    }
    #[test]
    fn stored_autofit_rounds_sizes_and_preserves_centered_overflow() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let shape = parse_xml_document(r#"<sp><txBody><bodyPr anchor="ctr" tIns="0" bIns="0"><normAutofit fontScale="62500"/></bodyPr><p><r><rPr sz="4400"/><t>One</t></r><br><rPr sz="4400"/></br><r><rPr sz="4400"/><t>Two</t></r><br><rPr sz="4400"/></br><r><rPr sz="4400"/><t>Three</t></r></p><p><endParaRPr sz="4400"/></p></txBody></sp>"#).unwrap();
        let mut slide = Slide::default();
        ctx.text(
            shape.child("txBody").unwrap(),
            &[&shape],
            &mut slide,
            rect(0.0, 50.0, 400.0, 110.0),
            0.0,
        );
        let text = slide
            .elements
            .iter()
            .find_map(|element| match element {
                SlideElement::TextBox(text) => Some(text),
                _ => None,
            })
            .unwrap();
        assert!(text
            .style
            .runs
            .iter()
            .all(|run| run.style.font_size_mpt == Some(28000)));
        assert!(text.frame.origin.y.as_pt() < 50.0);
        assert_eq!(slide.elements.len(), 2);
    }
    #[test]
    fn columns_flow_rich_text_and_fit_without_discarding_words() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        for rtl in [false, true] {
            let text =
                "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu ".repeat(4);
            let shape = parse_xml_document(&format!(r#"<sp><txBody><bodyPr numCol="2" spcCol="127000" rtlCol="{}"><normAutofit/></bodyPr><p><r><rPr sz="1800"/><t>{}</t></r></p></txBody></sp>"#, u8::from(rtl), text)).unwrap();
            let mut slide = Slide::default();
            ctx.text(
                shape.child("txBody").unwrap(),
                &[&shape],
                &mut slide,
                rect(0.0, 0.0, 200.0, 120.0),
                0.0,
            );
            let boxes: Vec<_> = slide
                .elements
                .iter()
                .filter_map(|e| match e {
                    SlideElement::TextBox(t) => Some(t),
                    _ => None,
                })
                .collect();
            let actual = boxes.iter().map(|t| t.text.as_str()).collect::<String>();
            assert_eq!(
                actual.split_whitespace().collect::<Vec<_>>(),
                text.split_whitespace().collect::<Vec<_>>()
            );
            assert!(boxes.len() > 2);
            assert!(boxes
                .iter()
                .all(|t| t.frame.origin.y.as_pt() + t.frame.size.height.as_pt() <= 117.0));
            let first = boxes.first().unwrap().frame.origin.x.as_pt();
            let last = boxes.last().unwrap().frame.origin.x.as_pt();
            assert!(if rtl { first > last } else { first < last });
        }
    }
    #[test]
    fn local_gradient_clips_to_shape_and_suppresses_theme_solid_fill() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let root=parse_xml_document(r#"<theme><themeElements><fmtScheme><fillStyleLst><solidFill><srgbClr val="0000FF"/></solidFill></fillStyleLst></fmtScheme></themeElements></theme>"#).unwrap();
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: Some(&root),
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let shape=parse_xml_document(r#"<sp><spPr><prstGeom prst="ellipse"/><gradFill><gsLst><gs pos="0"><srgbClr val="FF0000"/></gs><gs pos="100000"><srgbClr val="FFFF00"/></gs></gsLst><lin ang="0"/></gradFill></spPr><style><fillRef idx="1"/></style></sp>"#).unwrap();
        let mut slide = Slide::default();
        ctx.shape(&[&shape], &mut slide, Transform::identity());
        let SlideElement::Image(image) = &slide.elements[0] else {
            panic!()
        };
        assert_eq!(image.clip_paths.len(), 1);
        assert!(!slide
            .elements
            .iter()
            .any(|e| matches!(e,SlideElement::Shape(s) if s.style.fill!="none")));
    }
    #[test]
    fn gradient_alpha_and_equal_position_stops_are_preserved() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: None,
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let fill=parse_xml_document(r#"<gradFill><gsLst><gs pos="50000"><srgbClr val="000000"><alpha val="0"/></srgbClr></gs><gs pos="50000"><srgbClr val="0000FF"/></gs></gsLst><lin ang="0"/></gradFill>"#).unwrap();
        let mut slide = Slide::default();
        ctx.paint_fill(&fill, &mut slide, rect(0.0, 0.0, 100.0, 100.0));
        let SlideElement::Image(image) = &slide.elements[0] else {
            panic!()
        };
        let raster = lo_core::decode_png(&image.data).unwrap();
        assert_eq!(&raster.pixels[..4], &[0, 0, 0, 0]);
        assert_eq!(&raster.pixels[255 * 4..256 * 4], &[0, 0, 255, 255]);
        let color=parse_xml_document(r#"<srgbClr val="000000"><alpha val="50000"/><alphaMod val="50000"/><alphaOff val="10000"/></srgbClr>"#).unwrap();
        assert!((color_alpha(&color) - 0.35).abs() < 0.00001);
        let radial=parse_xml_document(r#"<gradFill><gsLst><gs pos="50000"><srgbClr val="000000"><alpha val="0"/></srgbClr></gs><gs pos="50000"><srgbClr val="0000FF"/></gs></gsLst><path path="circle"><fillToRect l="100000" t="100000"/></path></gradFill>"#).unwrap();
        let mut slide = Slide::default();
        ctx.paint_fill(&radial, &mut slide, rect(0.0, 0.0, 100.0, 100.0));
        let SlideElement::Image(image) = &slide.elements[0] else {
            panic!()
        };
        let raster = lo_core::decode_png(&image.data).unwrap();
        assert_eq!(&raster.pixels[..4], &[0, 0, 255, 255]);
        assert_eq!(&raster.pixels[raster.pixels.len() - 4..], &[0, 0, 0, 0]);
    }
    #[test]
    fn shape_font_reference_overrides_global_defaults_but_not_paragraph_color() {
        let zip = ZipArchive::new(&package(&[("a", "b")])).unwrap();
        let theme = Theme::new(None, None);
        let mut budget = 100;
        let defaults=parse_xml_document(r#"<defaultTextStyle><lvl1pPr><defRPr><solidFill><srgbClr val="000000"/></solidFill></defRPr></lvl1pPr></defaultTextStyle>"#).unwrap();
        let ctx = Context {
            zip: &zip,
            theme: &theme,
            theme_root: None,
            master: None,
            layout: None,
            default_text: Some(&defaults),
            table_styles: None,
            background: None,
            background_theme: None,
            slide_number: 1,
            budget: &mut budget,
            diagram_depth: 0,
        };
        let shape=parse_xml_document(r#"<sp><style><fontRef idx="minor"><srgbClr val="FFFFFF"/></fontRef></style><txBody><bodyPr/><p><r><t>White</t></r></p><p><pPr><defRPr><solidFill><srgbClr val="FF0000"/></solidFill></defRPr></pPr><r><t>Red</t></r></p></txBody></sp>"#).unwrap();
        let mut slide = Slide::default();
        ctx.shape(&[&shape], &mut slide, Transform::identity());
        let colors: Vec<_> = slide
            .elements
            .iter()
            .filter_map(|e| {
                if let SlideElement::TextBox(t) = e {
                    Some(t.style.runs[0].style.foreground.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(colors, vec!["#FFFFFF", "#FF0000"]);
    }
}
