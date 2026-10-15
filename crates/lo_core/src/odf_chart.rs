//! Cached ODF bar/line charts. Unsupported chart types are left unsupported.
use crate::style::{odf_length, OdfStyles};
use crate::{PdfFont, PdfPage, Rect, XmlNode};

#[derive(Clone, Debug, PartialEq)]
pub struct OdfChart {
    pub frame: Rect,
    plot: [f32; 4],
    title: String,
    categories: Vec<String>,
    series: Vec<(String, Vec<Option<f64>>, String)>,
    x_values: Option<Vec<Vec<Option<f64>>>>,
    connect_points: bool,
    series_lines: Vec<bool>,
    value_labels: Vec<bool>,
    horizontal: bool,
    line: bool,
    legend: bool,
    font_size: f32,
    title_size: f32,
    title_position: [f32; 2],
    legend_position: [f32; 2],
    axis_min: Option<f64>,
    axis_max: Option<f64>,
    axis_step: Option<f64>,
}
fn rows<'a>(node: &'a XmlNode, out: &mut Vec<&'a XmlNode>) {
    for child in &node.children {
        if child.local_name() == "table-row" {
            out.push(child);
        } else if matches!(child.local_name(), "table-rows" | "table-header-rows") {
            rows(child, out);
        }
    }
}
fn color(value: &str) -> (f32, f32, f32) {
    let s = value.trim_start_matches('#');
    if s.len() == 6 && s.is_ascii() {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&s[0..2], 16),
            u8::from_str_radix(&s[2..4], 16),
            u8::from_str_radix(&s[4..6], 16),
        ) {
            return (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        }
    }
    (0.31, 0.51, 0.74)
}
impl OdfChart {
    pub fn parse(root: &XmlNode, frame: Rect) -> Option<Self> {
        let chart = root.child("body")?.child("chart")?.child("chart")?;
        let kind = chart.attr("class")?.rsplit(':').next()?;
        if !matches!(kind, "bar" | "line") {
            return None;
        }
        let mut styles = OdfStyles::default();
        styles.collect(root);
        let plot = chart.child("plot-area")?;
        let style = plot.attr("style-name").unwrap_or("");
        // Stacking, percentages and log scales need different axis/series math.
        if ["stacked", "percentage"]
            .iter()
            .any(|p| styles.property(style, "chart-properties", p) == Some("true"))
        {
            return None;
        }
        for axis in plot.children_named("axis") {
            if styles.property(
                axis.attr("style-name").unwrap_or(""),
                "chart-properties",
                "logarithmic",
            ) == Some("true")
            {
                return None;
            }
        }
        let horizontal = styles.property(style, "chart-properties", "vertical") == Some("true");
        let table = chart.child("table")?;
        let mut table_rows = Vec::new();
        rows(table, &mut table_rows);
        if table_rows.len() < 2 || table_rows.len() > 10_001 {
            return None;
        }
        let cells: Vec<Vec<_>> = table_rows
            .iter()
            .map(|r| r.children_named("table-cell").collect())
            .collect();
        let categories: Vec<_> = cells
            .iter()
            .skip(1)
            .map(|r| {
                r.first()
                    .and_then(|c| c.child("p"))
                    .map(|n| crate::style::odf_text(n))
                    .unwrap_or_default()
            })
            .collect();
        let mut series = Vec::new();
        for (index, node) in plot.children_named("series").enumerate() {
            if index >= 64 {
                return None;
            }
            let range = node.attr("values-cell-range-address")?;
            let column = if range.starts_with("local-table.") {
                let (start, end) = range.split_once(':')?;
                let start = start.rsplit('.').next()?.replace('$', "");
                let end = end.rsplit('.').next()?.replace('$', "");
                let start = crate::CellAddr::from_a1(&start).ok()?;
                let end = crate::CellAddr::from_a1(&end).ok()?;
                if start.col != end.col || start.row != 1 || end.row as usize != cells.len() - 1 {
                    return None;
                }
                start.col as usize
            } else {
                cells[1].iter().position(|cell| {
                    cell.child("g")
                        .and_then(|n| n.child("desc"))
                        .map(|n| crate::style::odf_text(n))
                        .as_deref()
                        == Some(range)
                })?
            };
            let name = node
                .attr("label-string")
                .map(|s| s.trim_matches('"').to_string())
                .or_else(|| {
                    cells[0]
                        .get(column)
                        .and_then(|n| n.child("p"))
                        .map(|n| crate::style::odf_text(n))
                })
                .unwrap_or_default();
            let values: Vec<_> = cells
                .iter()
                .skip(1)
                .map(|r| {
                    r.get(column)
                        .and_then(|c| c.attr("value"))
                        .and_then(|v| v.parse::<f64>().ok())
                        .filter(|v| v.is_finite() && v.abs() < 1e100)
                })
                .collect();
            if !values.iter().any(Option::is_some) {
                continue;
            }
            let style = node.attr("style-name").unwrap_or("");
            let color = styles
                .property(style, "graphic-properties", "fill-color")
                .or_else(|| styles.property(style, "graphic-properties", "stroke-color"))
                .unwrap_or("#4f81bd")
                .to_string();
            series.push((name, values, color));
        }
        if series.is_empty() {
            return None;
        }
        let object_w = chart
            .attr("width")
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .unwrap_or(frame.size.width.as_pt());
        let object_h = chart
            .attr("height")
            .and_then(odf_length)
            .filter(|v| *v > 0.0)
            .unwrap_or(frame.size.height.as_pt());
        let region = plot.child("coordinate-region").unwrap_or(plot);
        let plot = [
            region
                .attr("x")
                .and_then(odf_length)
                .unwrap_or(object_w * 0.15)
                / object_w,
            region
                .attr("y")
                .and_then(odf_length)
                .unwrap_or(object_h * 0.15)
                / object_h,
            region
                .attr("width")
                .and_then(odf_length)
                .unwrap_or(object_w * 0.65)
                / object_w,
            region
                .attr("height")
                .and_then(odf_length)
                .unwrap_or(object_h * 0.7)
                / object_h,
        ];
        if plot[2] <= 0.0 || plot[3] <= 0.0 {
            return None;
        }
        let font_size = plot_axis_font(chart, &styles).unwrap_or(10.0);
        let title_node = chart.child("title");
        let mut title_size = title_node
            .and_then(|n| n.attr("style-name"))
            .and_then(|name| styles.property(name, "text-properties", "font-size"))
            .and_then(odf_length)
            .unwrap_or(14.0);
        if let Some(span) = title_node
            .and_then(|n| n.child("p"))
            .and_then(|n| n.child("span"))
        {
            if let Some(size) = span
                .attr("style-name")
                .and_then(|name| styles.property(name, "text-properties", "font-size"))
                .and_then(odf_length)
            {
                title_size = size;
            }
        }
        let position = |node: Option<&XmlNode>, fallback: [f32; 2]| {
            [
                node.and_then(|n| n.attr("x"))
                    .and_then(odf_length)
                    .unwrap_or(fallback[0])
                    / object_w,
                node.and_then(|n| n.attr("y"))
                    .and_then(odf_length)
                    .unwrap_or(fallback[1])
                    / object_h,
            ]
        };
        let title_position = position(title_node, [object_w * 0.4, 5.0]);
        let legend_position = position(chart.child("legend"), [object_w * 0.85, object_h * 0.5]);
        let axis_name = chart
            .child("plot-area")
            .and_then(|n| {
                n.children_named("axis")
                    .find(|a| a.attr("dimension") == Some("y"))
            })
            .and_then(|n| n.attr("style-name"))
            .unwrap_or("");
        let axis_number = |name| {
            styles
                .property(axis_name, "chart-properties", name)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && v.abs() < 1e100)
        };
        Some(Self {
            frame,
            plot,
            font_size,
            title_size: title_size.clamp(1.0, 512.0),
            title_position,
            legend_position,
            axis_min: axis_number("minimum"),
            axis_max: axis_number("maximum"),
            axis_step: axis_number("interval-major").filter(|v| *v > 0.0),
            title: chart
                .child("title")
                .and_then(|n| n.child("p"))
                .map(|n| crate::style::odf_text(n))
                .unwrap_or_default(),
            categories,
            series,
            x_values: None,
            connect_points: true,
            series_lines: Vec::new(),
            value_labels: Vec::new(),
            horizontal,
            line: kind == "line",
            legend: chart.child("legend").is_some(),
        })
    }
    /// Import cached OOXML chart values without executing formulas or external links.
    pub fn from_ooxml(root: &XmlNode, frame: Rect) -> Option<Self> {
        fn cached(node: &XmlNode) -> Vec<String> {
            let cache = ["numCache", "strCache", "numLit", "strLit"]
                .iter()
                .find_map(|k| node.child(k));
            if let Some(cache) = cache {
                let mut values = Vec::new();
                for point in cache.children_named("pt").take(10000) {
                    let Some(i) = point
                        .attr("idx")
                        .and_then(|s| s.parse::<usize>().ok())
                        .filter(|i| *i < 10000)
                    else {
                        continue;
                    };
                    values.resize(values.len().max(i + 1), String::new());
                    values[i] = point
                        .child("v")
                        .map(|n| n.text_content())
                        .unwrap_or_default();
                }
                return values;
            }
            for key in ["numRef", "strRef"] {
                if let Some(n) = node.child(key) {
                    return cached(n);
                }
            }
            node.child("v")
                .map(|n| vec![n.text_content()])
                .unwrap_or_default()
        }
        fn text(node: &XmlNode) -> String {
            if node.local_name() == "t" {
                return node.text_content();
            }
            node.children.iter().map(|n| text(n)).collect()
        }
        let chart = root.child("chart")?;
        let plot = chart.child("plotArea")?;
        let kinds: Vec<_> = plot
            .children
            .iter()
            .filter(|n| n.local_name().ends_with("Chart"))
            .collect();
        if kinds.len() != 1 {
            return None;
        }
        let data = kinds[0];
        let kind = data.local_name();
        if !matches!(kind, "barChart" | "lineChart" | "scatterChart") {
            return None;
        }
        if data
            .child("grouping")
            .and_then(|n| n.attr("val"))
            .is_some_and(|v| !matches!(v, "standard" | "clustered"))
        {
            return None;
        }
        if plot.children_named("valAx").any(|a| {
            a.child("scaling")
                .is_some_and(|n| n.child("logBase").is_some())
        }) {
            return None;
        }
        let mut series = Vec::new();
        let mut categories = Vec::new();
        let mut xs = Vec::new();
        let mut series_lines = Vec::new();
        let mut value_labels = Vec::new();
        let palette = [
            "#4472C4", "#ED7D31", "#A5A5A5", "#FFC000", "#5B9BD5", "#70AD47",
        ];
        for (i, ser) in data.children_named("ser").take(64).enumerate() {
            let name = ser.child("tx").map(cached).unwrap_or_default().join("");
            let values = cached(ser.child(if kind == "scatterChart" {
                "yVal"
            } else {
                "val"
            })?)
            .iter()
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && v.abs() < 1e100)
            })
            .collect::<Vec<_>>();
            if categories.is_empty() {
                categories = ser.child("cat").map(cached).unwrap_or_default();
            }
            if kind == "scatterChart" {
                xs.push(
                    cached(ser.child("xVal")?)
                        .iter()
                        .map(|v| {
                            v.parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite() && v.abs() < 1e100)
                        })
                        .collect(),
                );
            }
            let paint = ser
                .child("spPr")
                .and_then(|n| {
                    n.child("solidFill")
                        .or_else(|| n.child("ln").and_then(|n| n.child("solidFill")))
                })
                .and_then(|n| n.child("srgbClr"))
                .and_then(|n| n.attr("val"))
                .filter(|s| s.len() == 6 && s.is_ascii())
                .map(|s| format!("#{s}"))
                .unwrap_or_else(|| palette[i % palette.len()].into());
            series_lines.push(
                !ser.child("spPr")
                    .and_then(|n| n.child("ln"))
                    .is_some_and(|n| n.child("noFill").is_some()),
            );
            value_labels.push(
                ser.child("dLbls")
                    .or_else(|| data.child("dLbls"))
                    .and_then(|n| n.child("showVal"))
                    .and_then(|n| n.attr("val"))
                    == Some("1"),
            );
            series.push((name, values, paint));
        }
        if series.is_empty() {
            return None;
        }
        if categories.is_empty() && kind != "scatterChart" {
            categories = (1..=series.iter().map(|s| s.1.len()).max().unwrap_or(0))
                .map(|i| i.to_string())
                .collect();
        }
        let axis = plot.children_named("valAx").last();
        let axis_number = |key| {
            axis.and_then(|n| n.child("scaling"))
                .and_then(|n| n.child(key))
                .and_then(|n| n.attr("val"))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && v.abs() < 1e100)
        };
        let mut title = chart.child("title").map(text).unwrap_or_default();
        if title.is_empty()
            && chart.child("autoTitleDeleted").and_then(|n| n.attr("val")) == Some("0")
            && series.len() == 1
        {
            title = series[0].0.clone();
        }
        let legend_position = match chart
            .child("legend")
            .and_then(|n| n.child("legendPos"))
            .and_then(|n| n.attr("val"))
        {
            Some("b") => [0.45, 0.9],
            Some("t") => [0.45, 0.04],
            Some("l") => [0.01, 0.4],
            _ => [0.82, 0.4],
        };
        Some(Self {
            frame,
            series_lines,
            value_labels,
            plot: [0.15, 0.12, 0.65, 0.7],
            title,
            categories,
            series,
            x_values: if kind == "scatterChart" {
                Some(xs)
            } else {
                None
            },
            connect_points: data.child("scatterStyle").and_then(|n| n.attr("val"))
                != Some("marker"),
            horizontal: data.child("barDir").and_then(|n| n.attr("val")) == Some("bar"),
            line: kind != "barChart",
            legend: chart.child("legend").is_some(),
            font_size: 10.0,
            title_size: 14.0,
            title_position: [0.3, 0.02],
            legend_position,
            axis_min: axis_number("min"),
            axis_max: axis_number("max"),
            axis_step: axis
                .and_then(|n| n.child("majorUnit"))
                .and_then(|n| n.attr("val"))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0),
        })
    }
    pub fn render(&self, page: &mut PdfPage, page_height: f32, dx: f32, dy: f32) {
        let x = self.frame.origin.x.as_pt() + dx;
        let y = self.frame.origin.y.as_pt() + dy;
        let w = self.frame.size.width.as_pt();
        let h = self.frame.size.height.as_pt();
        let left = x + self.plot[0] * w;
        let top = page_height - y - self.plot[1] * h;
        let pw = self.plot[2] * w;
        let ph = self.plot[3] * h;
        let bottom = top - ph;
        page.rect_fill_rgb(x, page_height - y - h, w, h, 1.0, 1.0, 1.0);
        if !self.title.is_empty() {
            page.text(
                x + self.title_position[0] * w,
                page_height - y - self.title_position[1] * h - self.title_size,
                self.title_size,
                PdfFont::HelveticaBold,
                &self.title,
            );
        }
        let values: Vec<_> = self
            .series
            .iter()
            .flat_map(|s| s.1.iter().flatten())
            .copied()
            .collect();
        let min = values.iter().copied().fold(0.0f64, f64::min);
        let max = values.iter().copied().fold(0.0f64, f64::max);
        let span = (max - min).max(1e-12);
        let power = 10f64.powf((span / 10.0).log10().floor());
        let raw = span / 10.0 / power;
        let step = self.axis_step.unwrap_or(
            power
                * if raw <= 1.0 {
                    1.0
                } else if raw <= 2.0 {
                    2.0
                } else if raw <= 5.0 {
                    5.0
                } else {
                    10.0
                },
        );
        let lower = self.axis_min.unwrap_or((min / step).floor() * step);
        let upper = self
            .axis_max
            .unwrap_or((max / step).ceil() * step)
            .max(lower + step);
        let ticks = ((upper - lower) / step).round() as usize;
        let fraction = |v: f64| ((v - lower) / (upper - lower)) as f32;
        for tick in 0..=ticks.min(32) {
            let value = lower + step * tick as f64;
            let f = fraction(value);
            let label = if step >= 1.0 {
                format!("{value:.0}")
            } else {
                format!("{value:.3}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            };
            if self.horizontal {
                let tx = left + pw * f;
                page.line_rgb(tx, bottom, tx, top, 0.65, 0.65, 0.65);
                page.text(
                    tx - self.font_size * 0.25,
                    bottom - self.font_size - 3.0,
                    self.font_size,
                    PdfFont::Helvetica,
                    &label,
                );
            } else {
                let ty = bottom + ph * f;
                page.line_rgb(left, ty, left + pw, ty, 0.8, 0.8, 0.8);
                page.text(
                    left - 8.0
                        - crate::font_metrics::measure_helvetica(&label, self.font_size, false),
                    ty - self.font_size * 0.25,
                    self.font_size,
                    PdfFont::Helvetica,
                    &label,
                );
            }
        }
        let (xmin, xmax) = if let Some(xs) = &self.x_values {
            let min = xs
                .iter()
                .flatten()
                .flatten()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let max = xs
                .iter()
                .flatten()
                .flatten()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            if min.is_finite() && max.is_finite() {
                (min, max.max(min + 1e-12))
            } else {
                (0.0, 1.0)
            }
        } else {
            (0.0, 1.0)
        };
        if self.x_values.is_some() {
            for i in 0..=5 {
                let v = xmin + (xmax - xmin) * i as f64 / 5.0;
                let label = format!("{v:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string();
                let cx = left + pw * i as f32 / 5.0;
                page.text(
                    cx - 5.0,
                    bottom - self.font_size - 3.0,
                    self.font_size,
                    PdfFont::Helvetica,
                    &label,
                );
            }
        }
        let count = self.categories.len().max(1);
        for (i, label) in self.categories.iter().enumerate() {
            if self.horizontal {
                let cy = bottom + ph * (i as f32 + 0.5) / count as f32;
                page.text(
                    left - 5.0
                        - crate::font_metrics::measure_helvetica(label, self.font_size, false),
                    cy - self.font_size * 0.25,
                    self.font_size,
                    PdfFont::Helvetica,
                    label,
                );
            } else {
                let cx = left + pw * (i as f32 + 0.5) / count as f32;
                page.text(
                    cx - crate::font_metrics::measure_helvetica(label, self.font_size, false) / 2.0,
                    bottom - self.font_size - 3.0,
                    self.font_size,
                    PdfFont::Helvetica,
                    label,
                );
            }
        }
        for (si, (name, data, paint)) in self.series.iter().enumerate() {
            let (r, g, b) = color(paint);
            let mut previous = None;
            page.raw(&format!("q {left} {bottom} {pw} {ph} re W n"));
            for (i, value) in data.iter().enumerate() {
                let Some(value) = value else {
                    previous = None;
                    continue;
                };
                if self.line {
                    let cx = if let Some(xs) = &self.x_values {
                        let Some(Some(v)) = xs.get(si).and_then(|v| v.get(i)) else {
                            previous = None;
                            continue;
                        };
                        left + pw * ((v - xmin) / (xmax - xmin)) as f32
                    } else {
                        left + pw * (i as f32 + 0.5) / count as f32
                    };
                    let cy = bottom + ph * fraction(*value);
                    if let Some((px, py)) = previous.filter(|_| {
                        self.connect_points && self.series_lines.get(si).copied().unwrap_or(true)
                    }) {
                        page.line_rgb(px, py, cx, cy, r, g, b);
                    }
                    previous = Some((cx, cy));
                    page.rect_fill_rgb(cx - 2.0, cy - 2.0, 4.0, 4.0, r, g, b);
                } else if self.horizontal {
                    let band = ph / count as f32;
                    let bh = band * 0.6 / self.series.len() as f32;
                    let cy = bottom + band * (i as f32 + 0.2) + si as f32 * bh;
                    let start = left + pw * fraction(0.0);
                    let end = left + pw * fraction(*value);
                    page.rect_fill_rgb(start.min(end), cy, (end - start).abs(), bh, r, g, b);
                } else {
                    let band = pw / count as f32;
                    let bw = band * 0.6 / self.series.len() as f32;
                    let cx = left + band * (i as f32 + 0.2) + si as f32 * bw;
                    let start = bottom + ph * fraction(0.0);
                    let end = bottom + ph * fraction(*value);
                    page.rect_fill_rgb(cx, start.min(end), bw, (end - start).abs(), r, g, b);
                }
            }
            page.raw("Q");
            if self.value_labels.get(si) == Some(&true) && !self.horizontal && !self.line {
                for (i, value) in data.iter().enumerate() {
                    if let Some(value) = value {
                        let label = format!("{value}");
                        let cx = left + pw * (i as f32 + 0.5) / count as f32;
                        page.text(
                            cx - crate::font_metrics::measure_helvetica(
                                &label,
                                self.font_size,
                                false,
                            ) / 2.0,
                            bottom + ph * fraction(*value) + 3.0,
                            self.font_size,
                            PdfFont::Helvetica,
                            &label,
                        );
                    }
                }
            }
            if self.legend {
                let lx = x + self.legend_position[0] * w;
                let ly = page_height
                    - y
                    - self.legend_position[1] * h
                    - self.font_size
                    - si as f32 * self.font_size * 1.4;
                page.rect_fill_rgb(lx, ly, self.font_size * 0.6, self.font_size * 0.6, r, g, b);
                page.text(
                    lx + self.font_size * 0.8,
                    ly,
                    self.font_size,
                    PdfFont::Helvetica,
                    name,
                );
            }
        }
    }
}

fn plot_axis_font(chart: &XmlNode, styles: &OdfStyles) -> Option<f32> {
    let axis = chart.child("plot-area")?.children_named("axis").next()?;
    styles
        .property(axis.attr("style-name")?, "text-properties", "font-size")
        .and_then(odf_length)
        .filter(|v| *v > 0.0 && *v <= 512.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ooxml_scatter_preserves_sparse_numeric_coordinates_and_no_fill() {
        let xml=r#"<chartSpace><chart><autoTitleDeleted val="0"/><plotArea><scatterChart><ser><tx><v>Series</v></tx><spPr><ln><noFill/></ln></spPr><xVal><numLit><pt idx="1"><v>9</v></pt><pt idx="0"><v>2</v></pt></numLit></xVal><yVal><numRef><numCache><pt idx="0"><v>5</v></pt><pt idx="2"><v>NaN</v></pt></numCache></numRef></yVal></ser></scatterChart></plotArea></chart></chartSpace>"#;
        let root=crate::parse_xml_document(xml).unwrap();
        let c=OdfChart::from_ooxml(&root,Rect::default()).unwrap();
        assert_eq!(c.x_values.unwrap()[0],vec![Some(2.0),Some(9.0)]);
        assert_eq!(c.series[0].1,vec![Some(5.0),None,None]);
        assert_eq!(c.title,"Series");assert!(!c.series_lines[0]);
        let root=crate::parse_xml_document(&xml.replace("scatterChart","pieChart")).unwrap();
        assert!(OdfChart::from_ooxml(&root,Rect::default()).is_none());
    }
    #[test]
    fn cached_ranges_select_the_actual_series_column() {
        let xml = r#"<document><body><chart><chart class="chart:bar"><plot-area><series values-cell-range-address="local-table.$C$2:.$C$3"/></plot-area><table><table-header-rows><table-row><table-cell/><table-cell><p>Other</p></table-cell><table-cell><p>Selected</p></table-cell></table-row></table-header-rows><table-rows><table-row><table-cell><p>A</p></table-cell><table-cell value="99"/><table-cell value="-2"/></table-row><table-row><table-cell><p>B</p></table-cell><table-cell value="98"/><table-cell value="5"/></table-row></table-rows></table></chart></chart></body></document>"#;
        let root = crate::parse_xml_document(xml).unwrap();
        let frame = Rect::new(
            crate::Length::pt(0.0),
            crate::Length::pt(0.0),
            crate::Length::pt(400.0),
            crate::Length::pt(300.0),
        );
        let chart = OdfChart::parse(&root, frame).unwrap();
        assert_eq!(chart.series[0].0, "Selected");
        assert_eq!(chart.series[0].1, [Some(-2.0), Some(5.0)]);
        assert_eq!(chart.categories, ["A", "B"]);
        let root = crate::parse_xml_document(&xml.replace("chart:bar", "chart:pie")).unwrap();
        assert!(OdfChart::parse(&root, frame).is_none());
    }
}
