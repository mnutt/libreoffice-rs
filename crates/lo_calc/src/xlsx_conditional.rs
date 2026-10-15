//! Conditional preview formatting reads cached scalar values, never recalculates workbooks.
use crate::xlsx_styles::{color, theme_colors};
use lo_core::{CellAddr, Sheet, XmlNode};
use std::collections::BTreeMap;

fn address(s: &str) -> Option<CellAddr> {
    if s.len() > 16 {
        return None;
    }
    let a = CellAddr::from_a1(&s.replace('$', "")).ok()?;
    (a.col < 16384 && a.row < 1048576).then_some(a)
}
fn within(s: &str, a: CellAddr) -> bool {
    s.split_whitespace().take(256).any(|r| {
        let (start, end) = r.split_once(':').unwrap_or((r, r));
        match (address(start), address(end)) {
            (Some(l), Some(h)) => {
                a.col >= l.col && a.col <= h.col && a.row >= l.row && a.row <= h.row
            }
            _ => false,
        }
    })
}
fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(a), Ok(b)) => a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.to_uppercase().cmp(&b.to_uppercase()),
    }
}
fn condition(op: &str, a: &str, b: &str, c: &str) -> bool {
    use std::cmp::Ordering::*;
    let order = compare(a, b);
    match op {
        "greaterThan" => order == Greater,
        "lessThan" => order == Less,
        "equal" => order == Equal,
        "notEqual" => order != Equal,
        "greaterThanOrEqual" => order != Less,
        "lessThanOrEqual" => order != Greater,
        "between" => order != Less && compare(a, c) != Greater,
        "notBetween" => order == Less || compare(a, c) == Greater,
        _ => false,
    }
}
pub(crate) fn apply(
    sheet: &mut Sheet,
    root: &XmlNode,
    styles: Option<&XmlNode>,
    theme: Option<&XmlNode>,
) {
    let colors = theme_colors(theme);
    let dxfs: Vec<_> = styles
        .and_then(|n| n.child("dxfs"))
        .map(|n| n.children_named("dxf").collect())
        .unwrap_or_default();
    let mut cached = BTreeMap::new();
    let mut named = BTreeMap::new();
    if let Some(data) = root.child("sheetData") {
        for row in data.children_named("row") {
            for c in row.children_named("c") {
                let Some(a) = c.attr("r").and_then(address) else {
                    continue;
                };
                let value = c.child("v").map(|n| n.text_content()).unwrap_or_default();
                if let Some(f) = c.child("f") {
                    let name = f.text_content();
                    if name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        && value.parse::<f64>().is_ok()
                    {
                        named.insert(name, value.clone());
                    }
                }
                let value = if matches!(c.attr("t"), Some("s" | "inlineStr")) {
                    sheet.display_values.get(&a).cloned().unwrap_or_default()
                } else {
                    value
                };
                cached.insert(a, value);
            }
        }
    }
    let scalar = |s: &str| -> Option<String> {
        if let Some(a) = address(s) {
            cached.get(&a).cloned()
        } else if let Some(v) = named.get(s) {
            Some(v.clone())
        } else if s.starts_with('"') && s.ends_with('"') {
            Some(s.trim_matches('"').into())
        } else {
            s.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|_| s.to_string())
        }
    };
    let mut rules = Vec::new();
    for cf in root.children_named("conditionalFormatting").take(256) {
        for rule in cf.children_named("cfRule").take(64) {
            rules.push((
                rule.attr("priority")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(u32::MAX),
                cf.attr("sqref").unwrap_or(""),
                rule,
            ));
        }
    }
    rules.sort_by_key(|r| r.0);
    let mut budget = 1_000_000usize;
    for (a, cell) in &mut sheet.cells {
        let value = cached.get(a).map(String::as_str).unwrap_or("");
        let mut matched = Vec::new();
        for (_, range, rule) in &rules {
            if budget == 0 {
                eprintln!("XLSX preview: conditional-format work limit reached");
                return;
            }
            budget -= 1;
            let base = range
                .split_whitespace()
                .next()
                .and_then(|r| r.split(':').next())
                .and_then(address);
            let resolve = |s: &str| -> Option<String> {
                let s = s.trim();
                if let (Some(mut target), Some(base)) = (address(s), base) {
                    if !s.starts_with('$') {
                        target.col = u32::try_from(
                            i64::from(target.col) + i64::from(a.col) - i64::from(base.col),
                        )
                        .ok()?;
                    }
                    if !s.trim_start_matches('$').contains('$') {
                        target.row = u32::try_from(
                            i64::from(target.row) + i64::from(a.row) - i64::from(base.row),
                        )
                        .ok()?;
                    }
                    cached.get(&target).cloned()
                } else {
                    scalar(s)
                }
            };
            if !within(range, *a) {
                continue;
            }
            if let Some(bar) = rule.child("dataBar") {
                let limits: Vec<_> = bar.children_named("cfvo").collect();
                let bound = |n: &&XmlNode| -> Option<f32> {
                    resolve(n.attr("val")?)?
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                };
                if let (Some(lo), Some(hi), Ok(v)) = (
                    limits.first().and_then(bound),
                    limits.get(1).and_then(bound),
                    value.parse::<f32>(),
                ) {
                    if hi > lo && v.is_finite() {
                        sheet.layout.as_mut().unwrap().data_bars.insert(
                            *a,
                            (
                                ((v - lo) / (hi - lo)).clamp(0.0, 1.0),
                                color(bar.child("color"), &colors, &[]),
                                bar.attr("showValue") != Some("0"),
                            ),
                        );
                    }
                }
                continue;
            }
            let formulas: Vec<_> = rule
                .children_named("formula")
                .map(|n| n.text_content())
                .collect();
            let applies = match rule.attr("type") {
                Some("cellIs") => {
                    let b = formulas.first().and_then(|s| resolve(s));
                    let c = formulas.get(1).and_then(|s| resolve(s));
                    b.is_some_and(|b| {
                        condition(
                            rule.attr("operator").unwrap_or(""),
                            value,
                            &b,
                            c.as_deref().unwrap_or(""),
                        )
                    })
                }
                Some("expression") => formulas.first().is_some_and(|f| {
                    [
                        (">=", "greaterThanOrEqual"),
                        ("<=", "lessThanOrEqual"),
                        ("<>", "notEqual"),
                        (">", "greaterThan"),
                        ("<", "lessThan"),
                        ("=", "equal"),
                    ]
                    .iter()
                    .find_map(|(token, op)| f.split_once(token).map(|(l, r)| (l, r, op)))
                    .is_some_and(|(l, r, op)| match (resolve(l), resolve(r)) {
                        (Some(l), Some(r)) => condition(op, &l, &r, ""),
                        _ => false,
                    })
                }),
                _ => false,
            };
            if applies {
                if let Some(dxf) = rule
                    .attr("dxfId")
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|i| dxfs.get(i))
                {
                    matched.push(*dxf);
                }
                if crate::xlsx_styles::truth(rule.attr("stopIfTrue")) {
                    break;
                }
            }
        }
        for dxf in matched.into_iter().rev() {
            let s = cell.style.get_or_insert_with(Default::default);
            if let Some(font) = dxf.child("font") {
                for (key, slot) in [
                    ("b", &mut s.bold),
                    ("i", &mut s.italic),
                    ("u", &mut s.underline),
                ] {
                    if let Some(n) = font.child(key) {
                        *slot = !matches!(n.attr("val"), Some("0" | "false" | "none"));
                    }
                }
                if font.child("color").is_some() {
                    s.foreground = color(font.child("color"), &colors, &[]);
                }
                if let Some(name) = font.child("name").and_then(|n| n.attr("val")) {
                    s.font_family = name.into();
                }
            }
            if let Some(fill) = dxf.child("fill").and_then(|n| n.child("patternFill")) {
                s.background = color(
                    fill.child("fgColor").or_else(|| fill.child("bgColor")),
                    &colors,
                    &[],
                );
            }
        }
    }
}
