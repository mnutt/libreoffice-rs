//! Positioned PPTX import. No formula execution, external resources, or speaker notes on slides.
use lo_core::style::Alignment;
use lo_core::{
    parse_xml_document, Length, Presentation, Rect, Result, Shape, ShapeKind, ShapeStyle, Slide,
    SlideElement, TextBox, TextBoxStyle, XmlNode,
};
use lo_zip::{rels_path_for, resolve_part_target, ZipArchive};
use std::collections::BTreeMap;

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
            let mut budget = 100_000usize;
            let mut ctx = Context {
                zip: &zip,
                slide_number: i + 1,
                table_styles: table_styles.as_ref(),
                background,
                theme: &theme,
                theme_root,
                master: master_root,
                layout: layout_root,
                default_text: root.child("defaultTextStyle"),
                budget: &mut budget,
                diagram_depth: 0,
            };
            if let Some(bg) = background {
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
                    self.shape(&chain, slide, t);
                }
                "pic" => self.picture(n, rels, slide, t)?,
                "graphicFrame" => {
                    let data = n.child("graphic").and_then(|n| n.child("graphicData"));
                    let f = frame(&[n], t);
                    if let Some(ids) = data.and_then(|n| n.child("relIds")) {
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
        let Some(drawing) = drawing_id(&model)
            .and_then(|id| rels.paths.get(id).or_else(|| model_rels.paths.get(id)))
        else {
            return Ok(());
        };
        if !self.zip.contains(drawing) {
            return Ok(());
        }
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
        if let Some(color) = self.theme.color(fill) {
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
        } else if fill.local_name() == "gradFill" {
            let mut stops: Vec<_> = fill
                .child("gsLst")
                .into_iter()
                .flat_map(|n| n.children_named("gs"))
                .filter_map(|n| {
                    self.theme
                        .color(n)
                        .and_then(|c| crate::pdf::parse_color(&c))
                        .map(|c| ((num(Some(n), "pos", 0.0) / 100000.0).clamp(0.0, 1.0), c))
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
                    let upper = stops.partition_point(|s| s.0 < t).min(stops.len() - 1);
                    let lower = upper.saturating_sub(1);
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
                }
            }
            slide
                .elements
                .push(SlideElement::Image(lo_core::ImageElement {
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
            .find(|n| matches!(n.local_name(), "solidFill" | "gradFill"))
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
        let f = frame(chain, t);
        let props = chain
            .iter()
            .filter_map(|n| n.child("spPr"))
            .collect::<Vec<_>>();
        let fill = props
            .iter()
            .find_map(|n| {
                if n.child("noFill").is_some() {
                    Some("none".into())
                } else {
                    n.child("solidFill").and_then(|n| self.theme.color(n))
                }
            })
            .or_else(|| {
                chain
                    .iter()
                    .find_map(|n| n.child("style").and_then(|n| n.child("fillRef")))
                    .and_then(|n| self.theme_fill(n))
                    .and_then(|n| {
                        if n.local_name() == "solidFill" {
                            self.theme.color(&n)
                        } else {
                            None
                        }
                    })
            });
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
                self.paint_fill(fill, slide, f);
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
        } else if let Some(gradient) = props.iter().find_map(|n| n.child("gradFill")) {
            // Until curved path clipping is supported, only rectangular gradients are painted.
            if props
                .iter()
                .find_map(|n| n.child("prstGeom"))
                .and_then(|n| n.attr("prst"))
                .is_none_or(|v| matches!(v, "rect" | "roundRect"))
            {
                self.paint_fill(gradient, slide, f);
            }
        }
        let theme_line = chain
            .iter()
            .find_map(|n| n.child("style").and_then(|n| n.child("lnRef")))
            .and_then(|n| self.theme_fill(n));
        let line = props
            .iter()
            .find_map(|n| n.child("ln"))
            .or(theme_line.as_ref());
        let stroke = line
            .and_then(|n| {
                if n.child("noFill").is_some() {
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
        if fill.is_some() || stroke != "none" {
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
                },
            }));
        }
        if let Some(body) = chain[0].child("txBody") {
            self.text(body, chain, slide, f, angle);
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
        let mut crop =
            ["l", "t", "r", "b"].map(|key| (num(crop_node, key, 0.0) / 100000.0).clamp(0.0, 0.999));
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
        let name = n.child("nvPicPr").and_then(|n| n.child("cNvPr"));
        slide
            .elements
            .push(SlideElement::Image(lo_core::ImageElement {
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
                opacity: 255,
            }));
        Ok(())
    }
    fn text(&self, body: &XmlNode, chain: &[&XmlNode], slide: &mut Slide, f: Rect, angle: f32) {
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
        let scale = bodypr
            .and_then(|n| n.child("normAutofit"))
            .map(|n| num(Some(n), "fontScale", 100000.0) / 100000.0)
            .unwrap_or(1.0)
            .clamp(0.01, 1.0);
        let mut paragraphs = vec![];
        for p in body.children_named("p") {
            let pp = p.child("pPr");
            let level = num(pp, "lvl", 0.0).clamp(0.0, 8.0) as usize;
            let key = format!("lvl{}pPr", level + 1);
            let mut properties = vec![];
            if let Some(n) = self.default_text.and_then(|n| n.child(&key)) {
                properties.push(n);
            }
            if let Some(n) = master_style.and_then(|n| n.child(&key)) {
                properties.push(n);
            }
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
            let mut alignment = Alignment::Start;
            let mut indent = 0.0;
            let mut before = 0.0;
            let mut after = 0.0;
            let mut spacing = 1.2;
            let mut marker = None;
            for pr in properties {
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
                if let Some(n) = pr.child("lnSpc").and_then(|n| n.child("spcPct")) {
                    spacing = (num(Some(n), "val", 100000.0) / 100000.0 * 1.2).clamp(0.5, 5.0);
                }
                if let Some(n) = pr.child("spcBef").and_then(|n| n.child("spcPts")) {
                    before = num(Some(n), "val", 0.0) / 100.0;
                }
                if let Some(n) = pr.child("spcAft").and_then(|n| n.child("spcPts")) {
                    after = num(Some(n), "val", 0.0) / 100.0;
                }
                if let Some(n) = pr.child("buChar") {
                    marker = n.attr("char").map(str::to_string);
                }
                if pr.child("buNone").is_some() {
                    marker = None;
                }
                if let Some(n) = pr.child("buAutoNum") {
                    marker = Some(format!(
                        "{}.",
                        num(Some(n), "startAt", 1.0) as u32 + paragraphs.len() as u32
                    ));
                }
            }
            let mut runs = vec![];
            for n in &p.children {
                if n.local_name() == "br" {
                    runs.push(lo_core::calc::CellTextRun {
                        text: "\n".into(),
                        style: base.clone(),
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
                style.font_size_mpt =
                    Some((style.font_size_mpt.unwrap_or(18000) as f32 * scale).round() as u32);
                let text = if n.local_name() == "fld" && n.attr("type") == Some("slidenum") {
                    self.slide_number.to_string()
                } else {
                    n.child("t").map(|n| n.text_content()).unwrap_or_default()
                };
                runs.push(lo_core::calc::CellTextRun { text, style });
            }
            if runs.is_empty() {
                runs.push(lo_core::calc::CellTextRun {
                    text: " ".into(),
                    style: base.clone(),
                });
            }
            if let Some(marker) = marker.filter(|_| runs.iter().any(|r| !r.text.trim().is_empty()))
            {
                runs.insert(
                    0,
                    lo_core::calc::CellTextRun {
                        text: format!("{marker} "),
                        style: base.clone(),
                    },
                );
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
            let h = crate::pptx_text::height(&runs, text_width, spacing)
                .max(base.font_size_mpt.unwrap_or(18000) as f32 / 1000.0 * scale);
            let style = TextBoxStyle {
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
        let total: f32 = paragraphs.iter().map(|(_, _, h, _, b, a)| h + b + a).sum();
        let anchor = bodypr.and_then(|n| n.attr("anchor"));
        let mut y = f.origin.y.as_pt()
            + top
            + match anchor {
                Some("ctr") => (height - total).max(0.0) / 2.0,
                Some("b") => (height - total).max(0.0),
                _ => 0.0,
            };
        for (text, style, h, indent, before, after) in paragraphs {
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
}
