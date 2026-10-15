//! Cached ODF bar/line charts. Unsupported chart types are left unsupported.
use crate::style::{odf_length, OdfStyles};
use crate::{PdfFont, PdfPage, Rect, XmlNode};

#[derive(Clone, Debug, PartialEq)]
pub struct OdfChart {
    pub frame: Rect,
    ooxml_layout: Option<OoxmlLayout>,
    plot: [f32; 4],
    title: String,
    categories: Vec<String>,
    series: Vec<(String, Vec<Option<f64>>, String)>,
    x_values: Option<Vec<Vec<Option<f64>>>>,
    x_axis: [Option<f64>; 3],
    connect_points: bool,
    series_lines: Vec<bool>,
    markers: Vec<(String, f32, String)>,
    value_labels: Vec<bool>,
    point_colors: std::collections::BTreeMap<(usize, usize), String>,
    axis_format: String,
    data_formats: Vec<String>,
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
#[derive(Clone, Debug, PartialEq)]
struct OoxmlLayout {
    legend_side: String,
    value_grid: bool,
    category_grid: bool,
    gap_ratio: f32,
    title_bold: bool,
    stacked: bool,
}
fn text_properties(node: &XmlNode) -> Option<&XmlNode> {
    if matches!(node.local_name(), "defRPr" | "rPr") {
        return Some(node);
    }
    node.children.iter().find_map(|n| text_properties(n))
}
fn text_size(node: Option<&XmlNode>, fallback: f32) -> f32 {
    node.and_then(text_properties)
        .and_then(|n| n.attr("sz"))
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v / 100.0)
        .unwrap_or(fallback)
        .clamp(1.0, 72.0)
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
            ooxml_layout: None,
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
            x_axis: [None; 3],
            connect_points: true,
            series_lines: Vec::new(),
            markers: Vec::new(),
            value_labels: Vec::new(),
            point_colors: Default::default(),
            axis_format: String::new(),
            data_formats: Vec::new(),
            horizontal,
            line: kind == "line",
            legend: chart.child("legend").is_some(),
        })
    }
    /// Import cached OOXML chart values without executing formulas or external links.
    pub fn from_ooxml(root: &XmlNode, frame: Rect) -> Option<Self> {
        Self::from_ooxml_with_theme(root, frame, None)
    }
    pub fn from_ooxml_with_theme(
        root: &XmlNode,
        frame: Rect,
        theme: Option<&XmlNode>,
    ) -> Option<Self> {
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
        let grouping = data
            .child("grouping")
            .and_then(|n| n.attr("val"))
            .unwrap_or("standard");
        let stacked = grouping == "stacked" && kind == "barChart";
        if !matches!(grouping, "standard" | "clustered") && !stacked {
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
        let mut markers = Vec::new();
        let mut value_labels = Vec::new();
        let mut point_colors = std::collections::BTreeMap::new();
        let mut data_formats = Vec::new();
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
            let paint = chart_paint(ser.child("spPr"), theme)
                .unwrap_or_else(|| palette[i % palette.len()].into());
            for point in ser.children_named("dPt").take(10000) {
                if let (Some(idx), Some(paint)) = (
                    point
                        .child("idx")
                        .and_then(|n| n.attr("val"))
                        .and_then(|v| v.parse::<usize>().ok())
                        .filter(|v| *v < 10000),
                    chart_paint(point.child("spPr"), theme),
                ) {
                    point_colors.insert((i, idx), paint);
                }
            }
            let format = ser
                .child("dLbls")
                .and_then(|n| n.child("numFmt"))
                .and_then(|n| n.attr("formatCode"))
                .map(str::to_string)
                .or_else(|| {
                    ser.child("val")
                        .and_then(|n| n.child("numRef"))
                        .and_then(|n| n.child("numCache"))
                        .and_then(|n| n.child("formatCode"))
                        .map(|n| n.text_content())
                })
                .unwrap_or_default();
            data_formats.push(format);
            let marker = ser.child("marker");
            markers.push((
                marker
                    .and_then(|n| n.child("symbol"))
                    .and_then(|n| n.attr("val"))
                    .unwrap_or("square")
                    .into(),
                marker
                    .and_then(|n| n.child("size"))
                    .and_then(|n| n.attr("val"))
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                    .unwrap_or(4.0)
                    .clamp(2.0, 72.0),
                chart_paint(marker.and_then(|n| n.child("spPr")), theme)
                    .unwrap_or_else(|| paint.clone()),
            ));
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
        let category_axis = if kind == "scatterChart" {
            plot.children_named("valAx").next()
        } else {
            plot.child("catAx")
        };
        Some(Self {
            frame,
            ooxml_layout: Some(OoxmlLayout {
                stacked,
                legend_side: chart
                    .child("legend")
                    .and_then(|n| n.child("legendPos"))
                    .and_then(|n| n.attr("val"))
                    .unwrap_or("r")
                    .into(),
                value_grid: axis.is_some_and(|n| n.child("majorGridlines").is_some()),
                category_grid: category_axis.is_some_and(|n| n.child("majorGridlines").is_some()),
                gap_ratio: data
                    .child("gapWidth")
                    .and_then(|n| n.attr("val"))
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                    .unwrap_or(150.0)
                    .clamp(0.0, 500.0)
                    / 100.0,
                title_bold: chart
                    .child("title")
                    .and_then(text_properties)
                    .and_then(|n| n.attr("b"))
                    == Some("1"),
            }),
            series_lines,
            markers,
            value_labels,
            point_colors,
            data_formats,
            axis_format: axis
                .and_then(|n| n.child("numFmt"))
                .and_then(|n| n.attr("formatCode"))
                .unwrap_or("")
                .into(),
            plot: [0.15, 0.12, 0.65, 0.7],
            title,
            categories,
            series,
            x_axis: {
                let axis = plot.children_named("valAx").next();
                ["min", "max", "majorUnit"].map(|key| {
                    axis.and_then(|a| {
                        if key == "majorUnit" {
                            a.child(key)
                        } else {
                            a.child("scaling").and_then(|n| n.child(key))
                        }
                    })
                    .and_then(|n| n.attr("val"))
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && v.abs() < 1e100)
                })
            },
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
            font_size: text_size(axis.and_then(|n| n.child("txPr")), 10.0),
            title_size: text_size(chart.child("title"), 14.0),
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
    fn stack_span(&self, series: usize, category: usize, value: f64) -> (f64, f64) {
        let base = self.series[..series]
            .iter()
            .filter_map(|(_, data, _)| data.get(category).copied().flatten())
            .filter(|previous| (*previous >= 0.0) == (value >= 0.0))
            .sum::<f64>();
        (base, base + value)
    }
    pub fn render(&self, page: &mut PdfPage, page_height: f32, dx: f32, dy: f32) {
        let x = self.frame.origin.x.as_pt() + dx;
        let y = self.frame.origin.y.as_pt() + dy;
        let w = self.frame.size.width.as_pt();
        let h = self.frame.size.height.as_pt();
        if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
            return;
        }
        let mut plot = self.plot;
        let mut legend_position = self.legend_position;
        let mut legend_offsets = Vec::new();
        let mut title_x = x + self.title_position[0] * w;
        let mut title_y = page_height - y - self.title_position[1] * h - self.title_size;
        if let Some(layout) = &self.ooxml_layout {
            let measure =
                |s: &str| crate::font_metrics::measure_helvetica(s, self.font_size, false);
            let max_label = self
                .series
                .iter()
                .flat_map(|s| s.1.iter().flatten())
                .map(|v| crate::xlsx_format::number(*v, &self.axis_format, false))
                .map(|s| measure(&s))
                .fold(20.0_f32, f32::max);
            let category_width = self
                .categories
                .iter()
                .map(|s| measure(s))
                .fold(0.0_f32, f32::max);
            let legend_width = self
                .series
                .iter()
                .map(|s| measure(&s.0) + self.font_size * 2.0)
                .fold(0.0_f32, f32::max);
            let horizontal_legend = matches!(layout.legend_side.as_str(), "b" | "t");
            let mut legend_height = self.font_size * 1.4;
            if horizontal_legend {
                let widths: Vec<_> = self
                    .series
                    .iter()
                    .map(|s| measure(&s.0) + self.font_size * 2.0)
                    .collect();
                legend_offsets = legend_rows(&widths, (w - 16.0).max(1.0), self.font_size * 1.4);
                legend_height += legend_offsets.last().map_or(0.0, |p| p[1]);
            }
            let mut left_pad = (if self.horizontal {
                category_width
            } else {
                max_label
            }) + 10.0;
            let mut right_pad = 10.0;
            let mut top_pad = if self.title.is_empty() {
                12.0
            } else {
                self.title_size + 20.0
            };
            let mut bottom_pad = self.font_size * 2.0 + 10.0;
            if self.legend {
                match layout.legend_side.as_str() {
                    "b" => bottom_pad += legend_height + 8.0,
                    "t" => top_pad += legend_height + 8.0,
                    "l" => left_pad += legend_width + 10.0,
                    _ => right_pad += legend_width + 10.0,
                }
            }
            left_pad = left_pad.min(w * 0.35);
            right_pad = right_pad.min(w * 0.35);
            top_pad = top_pad.min(h * 0.3);
            bottom_pad = bottom_pad.min(h * 0.3);
            plot = [
                left_pad / w,
                top_pad / h,
                (w - left_pad - right_pad) / w,
                (h - top_pad - bottom_pad) / h,
            ];
            legend_position = match layout.legend_side.as_str() {
                "b" => [8.0 / w, 1.0 - (legend_height + 8.0) / h],
                "t" => [
                    8.0 / w,
                    if self.title.is_empty() {
                        4.0 / h
                    } else {
                        (self.title_size + 10.0) / h
                    },
                ],
                "l" => [4.0 / w, 0.45],
                _ => [(w - right_pad + 10.0) / w, 0.45],
            };
            title_x = x
                + (w - crate::font_metrics::measure_helvetica(
                    &self.title,
                    self.title_size,
                    layout.title_bold,
                )) / 2.0;
            title_y = page_height - y - 8.0 - self.title_size;
        }
        let left = x + plot[0] * w;
        let top = page_height - y - plot[1] * h;
        let pw = plot[2] * w;
        let ph = plot[3] * h;
        let bottom = top - ph;
        page.rect_fill_rgb(x, page_height - y - h, w, h, 1.0, 1.0, 1.0);
        if !self.title.is_empty() {
            page.text(
                title_x,
                title_y,
                self.title_size,
                if self.ooxml_layout.as_ref().is_none_or(|l| l.title_bold) {
                    PdfFont::HelveticaBold
                } else {
                    PdfFont::Helvetica
                },
                &self.title,
            );
        }
        if self.ooxml_layout.is_some() {
            page.line_rgb(left, bottom, left + pw, bottom, 0.55, 0.55, 0.55);
            page.line_rgb(left, bottom, left, top, 0.55, 0.55, 0.55);
        }
        let stacked = self.ooxml_layout.as_ref().is_some_and(|l| l.stacked);
        let values: Vec<_> = self
            .series
            .iter()
            .flat_map(|s| s.1.iter().flatten())
            .copied()
            .collect();
        let values = if stacked {
            self.series
                .iter()
                .enumerate()
                .flat_map(|(si, (_, data, _))| {
                    data.iter()
                        .enumerate()
                        .filter_map(move |(i, value)| value.map(|v| self.stack_span(si, i, v).1))
                })
                .collect::<Vec<_>>()
        } else {
            values
        };
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
            let label = if self.axis_format.is_empty() || self.axis_format == "General" {
                label
            } else {
                crate::xlsx_format::number(value, &self.axis_format, false)
            };
            if self.horizontal {
                let tx = left + pw * f;
                if self.ooxml_layout.as_ref().is_none_or(|l| l.value_grid) {
                    page.line_rgb(tx, bottom, tx, top, 0.65, 0.65, 0.65);
                }
                page.text(
                    tx - self.font_size * 0.25,
                    bottom - self.font_size - 3.0,
                    self.font_size,
                    PdfFont::Helvetica,
                    &label,
                );
            } else {
                let ty = bottom + ph * f;
                if self.ooxml_layout.as_ref().is_none_or(|l| l.value_grid) {
                    page.line_rgb(left, ty, left + pw, ty, 0.8, 0.8, 0.8);
                }
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
        let (xmin, xmax, xstep) = if let Some(xs) = &self.x_values {
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
                scatter_axis(min, max, self.x_axis)
            } else {
                (0.0, 1.0, 0.2)
            }
        } else {
            (0.0, 1.0, 0.2)
        };
        if self.x_values.is_some() {
            for i in 0..=((xmax - xmin) / xstep).round().min(32.0) as usize {
                let v = xmin + xstep * i as f64;
                let label = format!("{v:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string();
                let cx = left + pw * ((v - xmin) / (xmax - xmin)) as f32;
                if self.ooxml_layout.as_ref().is_none_or(|l| l.category_grid) {
                    page.line_rgb(cx, bottom, cx, top, 0.8, 0.8, 0.8);
                }
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
        let groups = if stacked {
            1.0
        } else {
            self.series.len() as f32
        };
        let bar_fraction = self
            .ooxml_layout
            .as_ref()
            .map(|l| groups / (groups + l.gap_ratio))
            .unwrap_or(0.6);
        for (si, (name, data, paint)) in self.series.iter().enumerate() {
            let (r, g, b) = color(paint);
            let mut previous = None;
            page.raw(&format!("q {left} {bottom} {pw} {ph} re W n"));
            for (i, value) in data.iter().enumerate() {
                let (r, g, b) = self
                    .point_colors
                    .get(&(si, i))
                    .map(|s| color(s))
                    .unwrap_or((r, g, b));
                let Some(value) = value else {
                    previous = None;
                    continue;
                };
                let (base_value, end_value) = if stacked {
                    self.stack_span(si, i, *value)
                } else {
                    (0.0, *value)
                };
                let group_index = if stacked { 0.0 } else { si as f32 };
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
                    let marker = self.markers.get(si);
                    let size = marker.map(|m| m.1).unwrap_or(4.0);
                    let paint = marker.map(|m| color(&m.2)).unwrap_or((r, g, b));
                    match marker.map(|m| m.0.as_str()).unwrap_or("square") {
                        "none" => {}
                        "circle" => page.ellipse_fill_stroke_rgb(
                            cx,
                            cy,
                            size / 2.0,
                            size / 2.0,
                            paint,
                            paint,
                        ),
                        _ => page.rect_fill_rgb(
                            cx - size / 2.0,
                            cy - size / 2.0,
                            size,
                            size,
                            paint.0,
                            paint.1,
                            paint.2,
                        ),
                    }
                } else if self.horizontal {
                    let band = ph / count as f32;
                    let bh = band * bar_fraction / groups;
                    let cy =
                        bottom + band * (i as f32 + (1.0 - bar_fraction) / 2.0) + group_index * bh;
                    let start = left + pw * fraction(base_value);
                    let end = left + pw * fraction(end_value);
                    page.rect_fill_rgb(start.min(end), cy, (end - start).abs(), bh, r, g, b);
                } else {
                    let band = pw / count as f32;
                    let bw = band * bar_fraction / groups;
                    let cx =
                        left + band * (i as f32 + (1.0 - bar_fraction) / 2.0) + group_index * bw;
                    let start = bottom + ph * fraction(base_value);
                    let end = bottom + ph * fraction(end_value);
                    page.rect_fill_rgb(cx, start.min(end), bw, (end - start).abs(), r, g, b);
                }
            }
            page.raw("Q");
            if self.value_labels.get(si) == Some(&true) && !self.horizontal && !self.line {
                for (i, value) in data.iter().enumerate() {
                    if let Some(value) = value {
                        let label = crate::xlsx_format::number(
                            *value,
                            self.data_formats.get(si).map(String::as_str).unwrap_or(""),
                            false,
                        );
                        let cx = left + pw * (i as f32 + 0.5) / count as f32;
                        page.text(
                            cx - crate::font_metrics::measure_helvetica(
                                &label,
                                self.font_size,
                                false,
                            ) / 2.0,
                            bottom
                                + ph * fraction(if stacked {
                                    let (a, b) = self.stack_span(si, i, *value);
                                    (a + b) / 2.0
                                } else {
                                    *value
                                })
                                + 3.0,
                            self.font_size,
                            PdfFont::Helvetica,
                            &label,
                        );
                    }
                }
            }
            if self.legend {
                let offset = legend_offsets
                    .get(si)
                    .copied()
                    .unwrap_or([0.0, si as f32 * self.font_size * 1.4]);
                let lx = x + legend_position[0] * w + offset[0];
                let ly = page_height - y - legend_position[1] * h - self.font_size - offset[1];
                let swatch = self.font_size * 0.6;
                if self.ooxml_layout.is_some() && self.line {
                    let marker = self.markers.get(si);
                    let paint = marker.map(|m| color(&m.2)).unwrap_or((r, g, b));
                    if self.connect_points && self.series_lines.get(si).copied().unwrap_or(true) {
                        page.line_rgb(
                            lx,
                            ly + swatch / 2.0,
                            lx + swatch,
                            ly + swatch / 2.0,
                            r,
                            g,
                            b,
                        );
                    }
                    match marker.map(|m| m.0.as_str()).unwrap_or("square") {
                        "none" => {}
                        "circle" => page.ellipse_fill_stroke_rgb(
                            lx + swatch / 2.0,
                            ly + swatch / 2.0,
                            swatch / 2.0,
                            swatch / 2.0,
                            paint,
                            paint,
                        ),
                        _ => page.rect_fill_rgb(lx, ly, swatch, swatch, paint.0, paint.1, paint.2),
                    }
                } else {
                    page.rect_fill_rgb(lx, ly, swatch, swatch, r, g, b);
                }
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

// Pack horizontal legends into centered rows using the same advances as rendering.
fn legend_rows(widths: &[f32], available: f32, row_height: f32) -> Vec<[f32; 2]> {
    let mut positions: Vec<[f32; 2]> = Vec::with_capacity(widths.len());
    let mut start = 0;
    let mut used = 0.0;
    let mut y = 0.0;
    for &width in widths {
        if used > 0.0 && used + width > available {
            for p in &mut positions[start..] {
                p[0] += (available - used).max(0.0) / 2.0;
            }
            start = positions.len();
            used = 0.0;
            y += row_height;
        }
        positions.push([used, y]);
        used += width;
    }
    for p in &mut positions[start..] {
        p[0] += (available - used).max(0.0) / 2.0;
    }
    positions
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
    fn stacked_bars_accumulate_positive_and_negative_values_separately() {
        let root = crate::parse_xml_document(r#"<chartSpace><chart><plotArea><barChart><grouping val="stacked"/><ser><val><numLit><pt idx="0"><v>2</v></pt></numLit></val></ser></barChart></plotArea></chart></chartSpace>"#).unwrap();
        let mut c = OdfChart::from_ooxml(&root, Rect::default()).unwrap();
        assert!(c.ooxml_layout.as_ref().unwrap().stacked);
        c.series = vec![
            ("a".into(), vec![Some(2.0)], "".into()),
            ("b".into(), vec![Some(-3.0)], "".into()),
            ("c".into(), vec![None], "".into()),
        ];
        assert_eq!(c.stack_span(3, 0, 4.0), (2.0, 6.0));
        assert_eq!(c.stack_span(3, 0, -5.0), (-3.0, -8.0));
    }
    #[test]
    fn horizontal_legends_wrap_and_center_each_row() {
        assert_eq!(
            legend_rows(&[30.0, 40.0, 50.0], 100.0, 14.0),
            vec![[15.0, 0.0], [45.0, 0.0], [25.0, 14.0]]
        );
    }
    #[test]
    fn ooxml_layout_retains_font_grid_and_bar_spacing() {
        let root=crate::parse_xml_document(r#"<chartSpace><chart><plotArea><barChart><gapWidth val="37"/><ser><val><numLit><pt idx="0"><v>10</v></pt></numLit></val></ser></barChart><valAx><txPr><p><pPr><defRPr sz="900"/></pPr></p></txPr></valAx></plotArea><legend><legendPos val="b"/></legend></chart></chartSpace>"#).unwrap();
        let c = OdfChart::from_ooxml(&root, Rect::default()).unwrap();
        let layout = c.ooxml_layout.unwrap();
        assert_eq!(c.font_size, 9.0);
        assert!(!layout.value_grid);
        assert_eq!(layout.legend_side, "b");
        assert!((layout.gap_ratio - 0.37).abs() < 0.001);
    }
    #[test]
    fn scatter_axis_has_headroom_and_honors_explicit_bounds() {
        let (low, high, step) = scatter_axis(0.0, 1.0, [None; 3]);
        assert_eq!(low, 0.0);
        assert!((high - 1.2).abs() < 1e-8);
        assert!((step - 0.2).abs() < 1e-8);
        assert_eq!(
            scatter_axis(0.0, 1.0, [Some(-2.0), Some(2.0), Some(1.0)]),
            (-2.0, 2.0, 1.0)
        );
    }
    #[test]
    fn chart_points_use_workbook_theme_and_luminance() {
        let theme=crate::parse_xml_document(r#"<theme><themeElements><clrScheme><accent1><srgbClr val="80C020"/></accent1></clrScheme></themeElements></theme>"#).unwrap();
        let n=crate::parse_xml_document(r#"<spPr><solidFill><schemeClr val="accent1"><lumMod val="40000"/><lumOff val="60000"/></schemeClr></solidFill></spPr>"#).unwrap();
        assert_eq!(chart_paint(Some(&n), Some(&theme)), Some("#CCE6A6".into()));
    }
    #[test]
    fn ooxml_scatter_preserves_sparse_numeric_coordinates_and_no_fill() {
        let xml = r#"<chartSpace><chart><autoTitleDeleted val="0"/><plotArea><scatterChart><ser><tx><v>Series</v></tx><marker><symbol val="circle"/><size val="5"/></marker><spPr><ln><noFill/></ln></spPr><xVal><numLit><pt idx="1"><v>9</v></pt><pt idx="0"><v>2</v></pt></numLit></xVal><yVal><numRef><numCache><pt idx="0"><v>5</v></pt><pt idx="2"><v>NaN</v></pt></numCache></numRef></yVal></ser></scatterChart></plotArea></chart></chartSpace>"#;
        let root = crate::parse_xml_document(xml).unwrap();
        let c = OdfChart::from_ooxml(&root, Rect::default()).unwrap();
        assert_eq!(c.x_values.unwrap()[0], vec![Some(2.0), Some(9.0)]);
        assert_eq!(c.series[0].1, vec![Some(5.0), None, None]);
        assert_eq!(c.title, "Series");
        assert!(!c.series_lines[0]);
        assert_eq!(c.markers[0].0, "circle");
        assert_eq!(c.markers[0].1, 5.0);
        let root = crate::parse_xml_document(&xml.replace("scatterChart", "pieChart")).unwrap();
        assert!(OdfChart::from_ooxml(&root, Rect::default()).is_none());
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

fn chart_paint(node: Option<&XmlNode>, theme: Option<&XmlNode>) -> Option<String> {
    let fill = node?
        .child("solidFill")
        .or_else(|| node?.child("ln")?.child("solidFill"))?;
    let n = fill.children.first()?;
    let fallback = [
        ("accent1", "4472C4"),
        ("accent2", "ED7D31"),
        ("accent3", "A5A5A5"),
        ("accent4", "FFC000"),
        ("accent5", "5B9BD5"),
        ("accent6", "70AD47"),
    ];
    let raw = if n.local_name() == "schemeClr" {
        let name = n.attr("val")?;
        theme
            .and_then(|n| n.child("themeElements"))
            .and_then(|n| n.child("clrScheme"))
            .and_then(|n| n.child(name))
            .and_then(|n| n.children.first())
            .and_then(|n| n.attr("val").or_else(|| n.attr("lastClr")))
            .or_else(|| fallback.iter().find(|(k, _)| *k == name).map(|(_, v)| *v))?
    } else if n.local_name() == "srgbClr" {
        n.attr("val")?
    } else {
        return None;
    };
    if raw.len() != 6 || !raw.is_ascii() {
        return None;
    }
    let rgb = u32::from_str_radix(raw, 16).ok()?;
    let factor = |key, default| {
        n.child(key)
            .and_then(|n| n.attr("val"))
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map(|v| (v / 100000.0).clamp(0.0, 1.0))
            .unwrap_or(default)
    };
    let m = factor("lumMod", 1.0);
    let o = factor("lumOff", 0.0);
    let c = |shift| {
        ((((rgb >> shift) & 255u32) as f32 * m + 255.0 * o)
            .clamp(0.0, 255.0)
            .round()) as u8
    };
    Some(format!("#{:02X}{:02X}{:02X}", c(16), c(8), c(0)))
}

fn scatter_axis(min: f64, max: f64, explicit: [Option<f64>; 3]) -> (f64, f64, f64) {
    let span = (max - min).max(1e-12);
    let power = 10f64.powf((span / 5.0).log10().floor());
    let raw = span / 5.0 / power;
    let step = explicit[2].filter(|v| *v > 0.0).unwrap_or(
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
    let low = explicit[0].unwrap_or((min.min(0.0) / step).floor() * step);
    let high = explicit[1]
        .unwrap_or(((max.max(0.0) + span * 0.05) / step).ceil() * step)
        .max(low + step);
    (low, high, step)
}
