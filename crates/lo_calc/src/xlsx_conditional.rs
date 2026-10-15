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
fn scale_threshold(
    node: &XmlNode,
    values: &[f64],
    resolve: &impl Fn(&str) -> Option<String>,
) -> Option<f64> {
    let lo = *values.first()?;
    let hi = *values.last()?;
    let value = || {
        resolve(node.attr("val")?)?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    };
    match node.attr("type")? {
        "min" => Some(lo),
        "max" => Some(hi),
        "num" | "formula" => value(),
        "percent" => Some(lo + (hi - lo) * value()?.clamp(0.0, 100.0) / 100.0),
        "percentile" => {
            let index = (values.len() - 1) as f64 * value()?.clamp(0.0, 100.0) / 100.0;
            let a = values[index.floor() as usize];
            let b = values[index.ceil() as usize];
            Some(a + (b - a) * index.fract())
        }
        _ => None,
    }
}
fn select_icon(
    node: &XmlNode,
    values: &[f64],
    value: f64,
    resolve: &impl Fn(&str) -> Option<String>,
) -> Option<lo_core::calc::ConditionalIcon> {
    let set = node.attr("iconSet").unwrap_or("3TrafficLights1");
    if !matches!(
        set,
        "3Arrows" | "3ArrowsGray" | "3TrafficLights1" | "3Flags"
    ) || crate::xlsx_styles::truth(node.attr("custom"))
    {
        return None;
    }
    let limits: Vec<_> = node.children_named("cfvo").collect();
    if limits.len() != 3 {
        return None;
    }
    // The first threshold is not a lower cutoff: every numeric cell gets an icon.
    let low = scale_threshold(limits[1], values, resolve)?;
    let high = scale_threshold(limits[2], values, resolve)?;
    if low > high {
        return None;
    }
    let passes = |n: &XmlNode, bound| {
        if matches!(n.attr("gte"), Some("0" | "false")) {
            value > bound
        } else {
            value >= bound
        }
    };
    let mut rank = if passes(limits[2], high) {
        2
    } else if passes(limits[1], low) {
        1
    } else {
        0
    };
    if crate::xlsx_styles::truth(node.attr("reverse")) {
        rank = 2 - rank;
    }
    Some(lo_core::calc::ConditionalIcon {
        set: set.into(),
        rank,
        show_value: !matches!(node.attr("showValue"), Some("0" | "false")),
    })
}
fn bar_fraction(
    node: &XmlNode,
    values: &[f64],
    value: f64,
    resolve: &impl Fn(&str) -> Option<String>,
) -> Option<f32> {
    let limits: Vec<_> = node.children_named("cfvo").collect();
    if limits.len() != 2 {
        return None;
    }
    let lo = scale_threshold(limits[0], values, resolve)?;
    let hi = scale_threshold(limits[1], values, resolve)?;
    if hi <= lo {
        return None;
    }
    let length = |key, default| {
        node.attr(key)
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
            .clamp(0.0, 100.0)
            / 100.0
    };
    let min = length("minLength", 10.0);
    let max = length("maxLength", 90.0);
    if max < min {
        return None;
    }
    Some(min + ((value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 * (max - min))
}
fn scale_color(
    scale: &XmlNode,
    values: &[f64],
    value: f64,
    theme: &[String],
    resolve: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let limits: Vec<_> = scale.children_named("cfvo").collect();
    let colors: Vec<_> = scale
        .children_named("color")
        .map(|n| color(Some(n), theme, &[]))
        .collect();
    if !(2..=3).contains(&limits.len()) || colors.len() != limits.len() {
        return None;
    }
    let limits: Vec<f64> = limits
        .into_iter()
        .map(|n| scale_threshold(n, values, resolve))
        .collect::<Option<_>>()?;
    if limits.windows(2).any(|w| w[0] > w[1]) {
        return None;
    }
    let i = if limits.len() == 3 && value > limits[1] {
        1
    } else {
        0
    };
    let t = if limits[i + 1] > limits[i] {
        ((value - limits[i]) / (limits[i + 1] - limits[i])).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let rgb = |s: &str| -> Option<[u8; 3]> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 || !s.is_ascii() {
            return None;
        }
        Some([
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ])
    };
    let a = rgb(&colors[i])?;
    let b = rgb(&colors[i + 1])?;
    let c: Vec<_> = (0..3)
        .map(|i| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * t).round() as u8)
        .collect();
    Some(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]))
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
    let mut numeric = BTreeMap::new();
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
                if matches!(c.attr("t"), None | Some("n")) {
                    if let Some(v) = value.parse::<f64>().ok().filter(|v| v.is_finite()) {
                        numeric.insert(a, v);
                    }
                }
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
    let mut distributions = BTreeMap::new();
    for (_, range, rule) in &rules {
        if !["colorScale", "iconSet", "dataBar"]
            .iter()
            .any(|key| rule.child(key).is_some())
            || distributions.contains_key(range)
        {
            continue;
        }
        if numeric.len() > budget {
            eprintln!("XLSX preview: conditional scale work limit reached");
            break;
        }
        budget -= numeric.len();
        let mut values: Vec<f64> = numeric
            .iter()
            .filter(|(a, _)| within(range, **a))
            .map(|(_, v)| *v)
            .collect();
        values.sort_by(f64::total_cmp);
        distributions.insert(*range, values);
    }
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
            if let Some(scale) = rule.child("colorScale") {
                if let (Some(values), Some(value)) = (distributions.get(range), numeric.get(a)) {
                    if let Some(background) = scale_color(scale, values, *value, &colors, &resolve)
                    {
                        matched.push((None, Some(background)));
                        if crate::xlsx_styles::truth(rule.attr("stopIfTrue")) {
                            break;
                        }
                    }
                }
                continue;
            }
            if let Some(icons) = rule.child("iconSet") {
                if let (Some(values), Some(value)) = (distributions.get(range), numeric.get(a)) {
                    if let Some(icon) = select_icon(icons, values, *value, &resolve) {
                        sheet
                            .layout
                            .as_mut()
                            .unwrap()
                            .conditional_icons
                            .entry(*a)
                            .or_insert(icon);
                        if crate::xlsx_styles::truth(rule.attr("stopIfTrue")) {
                            break;
                        }
                    }
                }
                continue;
            }
            if let Some(bar) = rule.child("dataBar") {
                if let (Some(values), Some(value)) = (distributions.get(range), numeric.get(a)) {
                    if let Some(fraction) = bar_fraction(bar, values, *value, &resolve) {
                        sheet
                            .layout
                            .as_mut()
                            .unwrap()
                            .data_bars
                            .entry(*a)
                            .or_insert((
                                fraction,
                                color(bar.child("color"), &colors, &[]),
                                !matches!(bar.attr("showValue"), Some("0" | "false")),
                            ));
                        if crate::xlsx_styles::truth(rule.attr("stopIfTrue")) {
                            break;
                        }
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
                    matched.push((Some(*dxf), None));
                }
                if crate::xlsx_styles::truth(rule.attr("stopIfTrue")) {
                    break;
                }
            }
        }
        for (dxf, background) in matched.into_iter().rev() {
            let s = cell.style.get_or_insert_with(Default::default);
            if let Some(background) = background {
                s.background = background;
            }
            let Some(dxf) = dxf else {
                continue;
            };
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
