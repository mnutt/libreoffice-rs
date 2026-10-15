//! Bounded layout of uncached SmartArt presentation trees.
//! The data model already contains presentation-node instances, including their
//! ordering; this avoids guessing nodes from visible text or duplicating samples.
use super::{num, Context, Transform};
use lo_core::{Rect, Slide, XmlNode};
use std::collections::{BTreeMap, BTreeSet};

fn node(name: &str, attrs: &[(&str, String)]) -> XmlNode {
    XmlNode {
        name: name.into(),
        attributes: attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        ..Default::default()
    }
}
fn named<'a>(root: &'a XmlNode, name: &str, depth: usize) -> Option<&'a XmlNode> {
    if depth > 32 {
        return None;
    }
    if root.local_name() == "layoutNode" && root.attr("name") == Some(name) {
        return Some(root);
    }
    root.children.iter().find_map(|n| named(n, name, depth + 1))
}
fn algorithm(root: &XmlNode, count: usize, reverse: bool, depth: usize) -> Option<&XmlNode> {
    if depth > 16 {
        return None;
    }
    if let Some(alg) = root.child("alg") {
        return Some(alg);
    }
    let choose = root.child("choose")?;
    for branch in &choose.children {
        let matches = if branch.local_name() == "else" {
            true
        } else if branch.attr("func") == Some("var") && branch.attr("arg") == Some("dir") {
            (branch.attr("val") == Some("rev")) == reverse
        } else if branch.attr("func") == Some("cnt") {
            let value = num(Some(branch), "val", 0.0) as usize;
            match branch.attr("op").unwrap_or("equ") {
                "equ" => count == value,
                "neq" => count != value,
                "gt" => count > value,
                "gte" => count >= value,
                "lt" => count < value,
                "lte" => count <= value,
                _ => false,
            }
        } else {
            false
        };
        if matches {
            return algorithm(branch, count, reverse, depth + 1);
        }
    }
    None
}
struct Tree<'a> {
    point: &'a XmlNode,
    layout: &'a XmlNode,
    children: Vec<Tree<'a>>,
}
impl Tree<'_> {
    fn algorithm(&self) -> Option<&XmlNode> {
        let count = self
            .children
            .iter()
            .filter(|c| {
                c.layout
                    .child("shape")
                    .and_then(|n| n.attr("type"))
                    .is_some_and(|v| v != "conn")
            })
            .count();
        let reverse = self
            .point
            .child("prSet")
            .and_then(|n| n.child("presLayoutVars"))
            .and_then(|n| n.child("dir"))
            .and_then(|n| n.attr("val"))
            == Some("rev");
        algorithm(self.layout, count, reverse, 0)
    }
}
fn tree<'a>(
    id: &'a str,
    points: &BTreeMap<&'a str, &'a XmlNode>,
    edges: &BTreeMap<&'a str, Vec<(u32, &'a str)>>,
    layout: &'a XmlNode,
    seen: &mut BTreeSet<&'a str>,
    budget: &mut usize,
    depth: usize,
) -> Option<Tree<'a>> {
    if depth > 32 || *budget == 0 || !seen.insert(id) {
        return None;
    }
    *budget -= 1;
    let point = *points.get(id)?;
    let definition = named(layout, point.child("prSet")?.attr("presName")?, 0)?;
    let children = edges
        .get(id)
        .into_iter()
        .flatten()
        .filter_map(|(_, id)| tree(id, points, edges, layout, seen, budget, depth + 1))
        .collect();
    Some(Tree {
        point,
        layout: definition,
        children,
    })
}
#[derive(Clone, Copy)]
struct BoxSpec {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    font: f32,
}
impl BoxSpec {
    fn get(self, key: &str) -> Option<f32> {
        Some(match key {
            "w" => self.w,
            "h" => self.h,
            "l" => self.x,
            "t" => self.y,
            "r" => self.x + self.w,
            "b" => self.y + self.h,
            "ctrX" => self.x + self.w / 2.0,
            "ctrY" => self.y + self.h / 2.0,
            "primFontSz" => self.font,
            _ => return None,
        })
    }
    fn set(&mut self, key: &str, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(-100000.0, 100000.0);
        match key {
            "w" => self.w = value.max(0.0),
            "h" => self.h = value.max(0.0),
            "l" => self.x = value,
            "t" => self.y = value,
            "ctrX" => self.x = value - self.w / 2.0,
            "ctrY" => self.y = value - self.h / 2.0,
            "primFontSz" => self.font = value.clamp(1.0, 400.0),
            _ => {}
        }
    }
}
fn constraints<'a>(layout: &'a XmlNode) -> impl Iterator<Item = &'a XmlNode> {
    layout
        .child("constrLst")
        .into_iter()
        .flat_map(|n| n.children_named("constr"))
}
fn child_boxes(parent: &Tree<'_>, frame: BoxSpec) -> Vec<BoxSpec> {
    let local = BoxSpec {
        x: 0.0,
        y: 0.0,
        ..frame
    };
    let mut boxes = vec![local; parent.children.len()];
    // A few passes resolve sibling references such as h = sibling.w * 0.6.
    for _ in 0..4 {
        for c in constraints(parent.layout).filter(|c| matches!(c.attr("for"), Some("ch" | "des")))
        {
            let key = c.attr("type").unwrap_or("");
            let reference = c
                .attr("refForName")
                .and_then(|name| {
                    parent
                        .children
                        .iter()
                        .position(|n| n.layout.attr("name") == Some(name))
                })
                .map(|i| boxes[i])
                .unwrap_or(local);
            let Some(value) = c
                .attr("val")
                .and_then(|v| v.parse::<f32>().ok())
                .or_else(|| {
                    reference
                        .get(c.attr("refType").unwrap_or(key))
                        .map(|v| v * num(Some(c), "fact", 1.0))
                })
            else {
                continue;
            };
            for (child, b) in parent.children.iter().zip(&mut boxes) {
                if c.attr("forName")
                    .is_none_or(|name| child.layout.attr("name") == Some(name))
                {
                    b.set(key, value);
                }
            }
        }
    }
    let alg = parent.algorithm();
    let kind = alg.and_then(|n| n.attr("type")).unwrap_or("composite");
    let parameter = |name| {
        alg.and_then(|n| {
            n.children_named("param")
                .find(|n| n.attr("type") == Some(name))
        })
        .and_then(|n| n.attr("val"))
    };
    match kind {
        "lin" => {
            let direction = parameter("linDir").unwrap_or("fromL");
            let vertical = matches!(direction, "fromT" | "fromB");
            let reverse = matches!(direction, "fromR" | "fromB");
            let sum: f32 = boxes.iter().map(|b| if vertical { b.h } else { b.w }).sum();
            let available = if vertical { frame.h } else { frame.w };
            let scale = if sum > 0.0 { available / sum } else { 1.0 };
            let mut position = 0.0;
            for b in &mut boxes {
                if vertical {
                    b.h *= scale;
                    b.y = if reverse {
                        available - position - b.h
                    } else {
                        position
                    };
                    position += b.h;
                } else {
                    b.w *= scale;
                    b.x = if reverse {
                        available - position - b.w
                    } else {
                        position
                    };
                    position += b.w;
                }
            }
        }
        "snake" => {
            let visible: Vec<_> = parent
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.layout
                        .child("shape")
                        .and_then(|s| s.attr("type"))
                        .is_some()
                })
                .map(|(i, _)| i)
                .collect();
            let n = visible.len();
            if n > 0 {
                let cols = ((n as f32 * frame.w / frame.h.max(1.0) * 0.6).sqrt().ceil() as usize)
                    .clamp(1, n);
                let rows = n.div_ceil(cols);
                let w = (frame.w / (cols as f32 * 1.1 - 0.1))
                    .min(frame.h / (rows as f32 * 0.66 - 0.06));
                let h = w * 0.6;
                for (index, i) in visible.into_iter().enumerate() {
                    let row = index / cols;
                    let col = index % cols;
                    let row_len = (n - row * cols).min(cols);
                    boxes[i].w = w;
                    boxes[i].h = h;
                    boxes[i].x =
                        (frame.w - w * (row_len as f32 * 1.1 - 0.1)) / 2.0 + col as f32 * w * 1.1;
                    boxes[i].y =
                        (frame.h - h * (rows as f32 * 1.1 - 0.1)) / 2.0 + row as f32 * h * 1.1;
                }
            }
        }
        "cycle" => {
            let visible: Vec<_> = parent
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.layout
                        .child("shape")
                        .and_then(|s| s.attr("type"))
                        .is_some_and(|t| t != "conn")
                })
                .map(|(i, _)| i)
                .collect();
            let center = parameter("ctrShpMap") == Some("fNode");
            let count = visible.len().saturating_sub(usize::from(center));
            let start = parameter("stAng")
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
                - 90.0;
            let span = parameter("spanAng")
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(360.0)
                .clamp(-360.0, 360.0);
            let sine = (std::f32::consts::PI / count.max(2) as f32).sin();
            let size = (sine / (1.8 + sine)).min(if center { 0.3 } else { 0.4 });
            let radius = (1.0 - size) / 2.0;
            for (index, i) in visible.into_iter().enumerate() {
                let angle = (start
                    + span * index.saturating_sub(usize::from(center)) as f32
                        / count.max(1) as f32)
                    .to_radians();
                let (x, y) = if center && index == 0 {
                    (0.5, 0.5)
                } else {
                    (0.5 + radius * angle.cos(), 0.5 + radius * angle.sin())
                };
                // Solve the cycle in a unit square, then stretch with its frame.
                // This preserves the diagram's elliptical layout on wide slides.
                boxes[i].x = 0.5 + (x - 0.5) * frame.w.min(frame.h) / frame.w.max(1.0) - size / 2.0;
                boxes[i].y = 0.5 + (y - 0.5) * frame.w.min(frame.h) / frame.h.max(1.0) - size / 2.0;
                boxes[i].w = size;
                boxes[i].h = size;
            }
        }
        _ => {}
    }
    for (child, b) in parent.children.iter().zip(&mut boxes) {
        for c in constraints(child.layout).filter(|c| c.attr("for").is_none_or(|v| v == "self")) {
            let key = c.attr("type").unwrap_or("");
            if !matches!(key, "w" | "h" | "primFontSz") {
                continue;
            }
            let Some(value) = c
                .attr("val")
                .and_then(|v| v.parse::<f32>().ok())
                .or_else(|| {
                    b.get(c.attr("refType").unwrap_or(key))
                        .map(|v| v * num(Some(c), "fact", 1.0))
                })
            else {
                continue;
            };
            b.set(key, value);
        }
        if kind == "cycle" {
            b.x *= frame.w;
            b.y *= frame.h;
            b.w *= frame.w;
            b.h *= frame.h;
        }
        b.x += frame.x;
        b.y += frame.y;
    }
    boxes
}
struct Scene<'a, 'b> {
    ctx: &'a Context<'b>,
    points: BTreeMap<&'a str, &'a XmlNode>,
    colors: Option<&'a XmlNode>,
    styles: Option<&'a XmlNode>,
    source_edges: BTreeMap<&'a str, Vec<(u32, &'a str)>>,
    text_budget: std::cell::Cell<usize>,
}
impl Scene<'_, '_> {
    fn descendants(&self, id: &str) -> Vec<&XmlNode> {
        let mut out = vec![];
        let mut stack = vec![(id, 0)];
        let mut seen = BTreeSet::new();
        while let Some((id, depth)) = stack.pop() {
            if depth > 32 || !seen.insert(id) || out.len() >= 256 || self.text_budget.get() == 0 {
                continue;
            }
            if depth > 0 {
                if let Some(point) = self.points.get(id) {
                    out.push(*point);
                    self.text_budget.set(self.text_budget.get() - 1);
                }
            }
            if let Some(children) = self.source_edges.get(id) {
                stack.extend(children.iter().rev().map(|(_, id)| (*id, depth + 1)));
            }
        }
        out
    }
    fn paint(&self, tree: &Tree<'_>, frame: BoxSpec, slide: &mut Slide, depth: usize) {
        if depth > 32 {
            return;
        }
        let alg = tree
            .algorithm()
            .and_then(|n| n.attr("type"))
            .unwrap_or("composite");
        // Algorithms requiring a constraint solver are deliberately left out;
        // drawing all of their children on top of each other is not a fallback.
        if !matches!(alg, "composite" | "lin" | "snake" | "cycle" | "sp" | "tx") {
            return;
        }
        if let Some(shape) = tree
            .layout
            .child("shape")
            .filter(|s| s.attr("type").is_some_and(|t| t != "conn"))
        {
            if frame.w > 0.0 && frame.h > 0.0 {
                let props = tree.point.child("prSet");
                let source = props
                    .and_then(|n| n.attr("presAssocID"))
                    .and_then(|id| self.points.get(id))
                    .copied();
                let label = props
                    .and_then(|n| n.attr("presStyleLbl"))
                    .or_else(|| tree.layout.attr("styleLbl"))
                    .unwrap_or("node1");
                let color_style = self.colors.and_then(|n| {
                    n.children_named("styleLbl")
                        .find(|n| n.attr("name") == Some(label))
                });
                let quick = self.styles.and_then(|n| {
                    n.children_named("styleLbl")
                        .find(|n| n.attr("name") == Some(label))
                });
                let mut sp = node("sp", &[]);
                let mut pr = source
                    .and_then(|n| n.child("spPr"))
                    .cloned()
                    .unwrap_or_else(|| node("spPr", &[]));
                pr.children
                    .retain(|n| !matches!(n.local_name(), "xfrm" | "prstGeom" | "custGeom"));
                let mut xf = node("xfrm", &[]);
                xf.children.push(
                    node(
                        "off",
                        &[
                            ("x", (frame.x * 12700.0).round().to_string()),
                            ("y", (frame.y * 12700.0).round().to_string()),
                        ],
                    )
                    .into(),
                );
                xf.children.push(
                    node(
                        "ext",
                        &[
                            ("cx", (frame.w * 12700.0).round().to_string()),
                            ("cy", (frame.h * 12700.0).round().to_string()),
                        ],
                    )
                    .into(),
                );
                pr.children.push(xf.into());
                pr.children.push(
                    node(
                        "prstGeom",
                        &[("prst", shape.attr("type").unwrap_or("rect").into())],
                    )
                    .into(),
                );
                if shape.attr("hideGeom") == Some("1") {
                    pr.children.retain(|n| {
                        !matches!(
                            n.local_name(),
                            "solidFill" | "gradFill" | "pattFill" | "blipFill" | "ln"
                        )
                    });
                    pr.children.push(node("noFill", &[]).into());
                    let mut line = node("ln", &[]);
                    line.children.push(node("noFill", &[]).into());
                    pr.children.push(line.into());
                } else if !pr
                    .children
                    .iter()
                    .any(|n| matches!(n.local_name(), "solidFill" | "gradFill" | "noFill"))
                {
                    let mut fill = node("solidFill", &[]);
                    let color = color_style
                        .and_then(|n| n.child("fillClrLst"))
                        .and_then(|n| {
                            n.children.get(
                                (num(props, "presStyleIdx", 0.0).max(0.0) as usize)
                                    % n.children.len().max(1),
                            )
                        })
                        .cloned()
                        .unwrap_or_else(|| node("schemeClr", &[("val", "accent1".into())]).into());
                    fill.children.push(color);
                    pr.children.push(fill.into());
                }
                sp.children.push(pr.into());
                if let Some(style) = quick.and_then(|n| n.child("style")) {
                    sp.children.push(style.clone().into());
                }
                if alg == "tx" {
                    if let Some(text) = source.and_then(|n| n.child("t")) {
                        let mut body = text.clone();
                        body.name = "txBody".into();
                        if tree
                            .layout
                            .child("presOf")
                            .and_then(|n| n.attr("axis"))
                            .is_some_and(|s| {
                                s.split_whitespace()
                                    .any(|v| matches!(v, "desOrSelf" | "des"))
                            })
                        {
                            if let Some(id) = source.and_then(|n| n.attr("modelId")) {
                                for child in self.descendants(id) {
                                    if let Some(text) = child.child("t") {
                                        body.children.extend(
                                            text.children
                                                .iter()
                                                .filter(|n| n.local_name() == "p")
                                                .cloned(),
                                        );
                                    }
                                }
                            }
                        }
                        body.children.retain(|n| n.local_name() != "bodyPr");
                        let mut bodypr = node(
                            "bodyPr",
                            &[
                                ("anchor", "ctr".into()),
                                ("lIns", "91440".into()),
                                ("rIns", "91440".into()),
                                ("tIns", "45720".into()),
                                ("bIns", "45720".into()),
                            ],
                        );
                        bodypr.children.push(node("normAutofit", &[]).into());
                        body.children.insert(0, bodypr.into());
                        // Fit a bounded source font into the node; explicit run
                        // formatting continues to override paragraph defaults.
                        let chars = body
                            .children_named("p")
                            .map(|p| p.text_content().chars().count())
                            .sum::<usize>()
                            .max(1) as f32;
                        let size = frame
                            .font
                            .min(frame.h * 0.55)
                            .min(frame.w / (chars.min(20.0) * 0.6))
                            .max(5.0);
                        for p in &mut body.children {
                            if p.local_name() != "p" {
                                continue;
                            }
                            let mut paragraph = p.as_ref().clone();
                            let mut pp = paragraph
                                .child("pPr")
                                .cloned()
                                .unwrap_or_else(|| node("pPr", &[]));
                            pp.attributes
                                .entry("algn".into())
                                .or_insert_with(|| "ctr".into());
                            if pp.child("defRPr").is_none() {
                                pp.children.push(
                                    node("defRPr", &[("sz", (size * 100.0).round().to_string())])
                                        .into(),
                                );
                            }
                            paragraph.children.retain(|n| n.local_name() != "pPr");
                            paragraph.children.insert(0, pp.into());
                            *p = paragraph.into();
                        }
                        sp.children.push(body.into());
                    }
                }
                self.ctx
                    .shape_with_fit(&[&sp], slide, Transform::identity(), true);
            }
        }
        for (child, frame) in tree.children.iter().zip(child_boxes(tree, frame)) {
            self.paint(child, frame, slide, depth + 1);
        }
    }
}
pub(super) fn render(
    ctx: &mut Context<'_>,
    model: &XmlNode,
    layout: &XmlNode,
    colors: Option<&XmlNode>,
    styles: Option<&XmlNode>,
    slide: &mut Slide,
    frame: Rect,
) {
    let points: BTreeMap<_, _> = model
        .child("ptLst")
        .into_iter()
        .flat_map(|n| n.children_named("pt"))
        .take(4096)
        .filter_map(|n| Some((n.attr("modelId")?, n)))
        .collect();
    let mut edges: BTreeMap<&str, Vec<(u32, &str)>> = BTreeMap::new();
    let mut source_edges: BTreeMap<&str, Vec<(u32, &str)>> = BTreeMap::new();
    let mut child_ids = BTreeSet::new();
    for c in model
        .child("cxnLst")
        .into_iter()
        .flat_map(|n| n.children_named("cxn"))
        .take(8192)
    {
        if let (Some(src), Some(dest)) = (c.attr("srcId"), c.attr("destId")) {
            if c.attr("type").is_none_or(|v| v == "parOf") {
                source_edges
                    .entry(src)
                    .or_default()
                    .push((num(Some(c), "srcOrd", 0.0).max(0.0) as u32, dest));
            }
            if c.attr("type") != Some("presParOf") {
                continue;
            }
            edges
                .entry(src)
                .or_default()
                .push((num(Some(c), "srcOrd", 0.0).max(0.0) as u32, dest));
            child_ids.insert(dest);
        }
    }
    for children in edges.values_mut() {
        children.sort_by_key(|(order, _)| *order);
    }
    for children in source_edges.values_mut() {
        children.sort_by_key(|(order, _)| *order);
    }
    let mut seen = BTreeSet::new();
    let initial = 1024.min(*ctx.budget);
    let mut budget = initial;
    let roots: Vec<_> = points
        .iter()
        .filter(|(id, n)| n.attr("type") == Some("pres") && !child_ids.contains(**id))
        .filter_map(|(id, _)| tree(id, &points, &edges, layout, &mut seen, &mut budget, 0))
        .collect();
    *ctx.budget -= initial - budget;
    let scene = Scene {
        ctx,
        points,
        colors,
        styles,
        source_edges,
        text_budget: std::cell::Cell::new(2048),
    };
    let frame = BoxSpec {
        x: frame.origin.x.as_pt(),
        y: frame.origin.y.as_pt(),
        w: frame.size.width.as_pt(),
        h: frame.size.height.as_pt(),
        font: 65.0,
    };
    for tree in roots {
        scene.paint(&tree, frame, slide, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lo_core::parse_xml_document;
    #[test]
    fn presentation_tree_rejects_cycles_and_obeys_budget() {
        let model=parse_xml_document(r#"<ptLst><pt modelId="root"><prSet presName="list"/></pt><pt modelId="child"><prSet presName="item"/></pt></ptLst>"#).unwrap();
        let points: BTreeMap<_, _> = model
            .children_named("pt")
            .map(|n| (n.attr("modelId").unwrap(), n))
            .collect();
        let edges = BTreeMap::from([("root", vec![(0, "child")]), ("child", vec![(0, "root")])]);
        let layout=parse_xml_document(r#"<layoutNode name="list"><alg type="lin"/><layoutNode name="item"><alg type="tx"/><shape type="rect"/></layoutNode></layoutNode>"#).unwrap();
        let mut budget = 2;
        let tree = tree(
            "root",
            &points,
            &edges,
            &layout,
            &mut BTreeSet::new(),
            &mut budget,
            0,
        )
        .unwrap();
        assert_eq!(tree.children.len(), 1);
        assert!(tree.children[0].children.is_empty());
        assert_eq!(budget, 0);
        let boxes = child_boxes(
            &tree,
            BoxSpec {
                x: 10.0,
                y: 20.0,
                w: 200.0,
                h: 100.0,
                font: 18.0,
            },
        );
        assert_eq!((boxes[0].x, boxes[0].y, boxes[0].w), (10.0, 20.0, 200.0));
    }
    #[test]
    fn unsupported_constraint_reference_preserves_allocated_text_box() {
        let point = parse_xml_document("<pt/>").unwrap();
        let layout = parse_xml_document(r#"<layoutNode><alg type="composite"/><layoutNode><alg type="tx"/><constrLst><constr type="h" refType="userH"/></constrLst></layoutNode></layoutNode>"#).unwrap();
        let child = Tree {
            point: &point,
            layout: layout.child("layoutNode").unwrap(),
            children: vec![],
        };
        let parent = Tree {
            point: &point,
            layout: &layout,
            children: vec![child],
        };
        let boxes = child_boxes(
            &parent,
            BoxSpec {
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 100.0,
                font: 18.0,
            },
        );
        assert_eq!(boxes[0].h, 100.0);
    }
    #[test]
    fn cycle_layout_keeps_nodes_inside_wide_frame_and_starts_at_top() {
        let point = parse_xml_document("<pt/>").unwrap();
        let layout = parse_xml_document(r#"<layoutNode><alg type="cycle"><param type="stAng" val="0"/><param type="spanAng" val="360"/></alg><layoutNode><shape type="ellipse"/><constrLst><constr type="h" refType="w"/></constrLst></layoutNode></layoutNode>"#).unwrap();
        let parent = Tree {
            point: &point,
            layout: &layout,
            children: (0..5)
                .map(|_| Tree {
                    point: &point,
                    layout: layout.child("layoutNode").unwrap(),
                    children: vec![],
                })
                .collect(),
        };
        let boxes = child_boxes(
            &parent,
            BoxSpec {
                x: 10.0,
                y: 20.0,
                w: 900.0,
                h: 600.0,
                font: 18.0,
            },
        );
        assert!((boxes[0].y - 20.0).abs() < 0.001);
        for b in boxes {
            assert!(b.x >= 10.0 && b.y >= 20.0);
            assert!(b.x + b.w <= 910.001 && b.y + b.h <= 620.001);
            assert!((b.w / b.h - 1.5).abs() < 0.001);
        }
    }
    #[test]
    fn nested_choose_respects_count_and_direction() {
        let layout=parse_xml_document(r#"<layoutNode><choose><if func="var" arg="dir" val="norm"><choose><if func="cnt" op="gt" val="2"><alg type="cycle"/></if><else><alg type="lin"/></else></choose></if><else><alg type="snake"/></else></choose></layoutNode>"#).unwrap();
        assert_eq!(
            algorithm(&layout, 3, false, 0).unwrap().attr("type"),
            Some("cycle")
        );
        assert_eq!(
            algorithm(&layout, 1, false, 0).unwrap().attr("type"),
            Some("lin")
        );
        assert_eq!(
            algorithm(&layout, 3, true, 0).unwrap().attr("type"),
            Some("snake")
        );
    }
}
