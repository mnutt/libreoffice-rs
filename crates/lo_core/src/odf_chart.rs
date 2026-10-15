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
    pie_start: Option<f32>,
    radar: Option<String>,
    pie_vary_colors: bool,
    category_title: (String, f32, bool),
    value_title: (String, f32, bool),
    gradients: std::collections::BTreeMap<usize, LinearGradient>,
}
#[derive(Clone, Debug, PartialEq)]
struct LinearGradient {
    angle: f32,
    scaled: bool,
    stops: Vec<(f32, String)>,
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
        if !matches!(kind, "barChart" | "lineChart" | "scatterChart" | "pieChart" | "radarChart") {
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
        let mut gradients = std::collections::BTreeMap::new();
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
                let cat=ser.child("cat");
                // Hierarchical category placement is retained by the Impress fallback renderer.
                if cat.is_some_and(|n|n.child("multiLvlStrRef").is_some()) {return None;}
                categories = cat.map(cached).unwrap_or_default();
                let format=cat.and_then(|n|n.child("numRef")).and_then(|n|n.child("numCache"))
                    .or_else(||cat.and_then(|n|n.child("numLit"))).and_then(|n|n.child("formatCode")).map(|n|n.text_content());
                if let Some(format)=format {for label in &mut categories {
                    if let Ok(value)=label.parse::<f64>() {if value.is_finite(){*label=crate::xlsx_format::number(value,&format,false);}}
                }}
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
            if let Some(gradient) = linear_gradient(ser.child("spPr"), theme) {
                gradients.insert(i, gradient);
            }
            let paint = gradients
                .get(&i)
                .map(|g| g.stops[0].1.clone())
                .or_else(|| chart_paint(ser.child("spPr"), theme))
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
        // Basic 2-D pies have one series; exploded/negative slices require
        // separate placement/semantics and must not be silently approximated.
        if kind == "pieChart"
            && (series.len() != 1
                || series[0].1.iter().flatten().any(|v| *v < 0.0)
                || data.children_named("ser").any(|ser| {
                    ser.child("explosion")
                        .and_then(|n| n.attr("val"))
                        .is_some_and(|v| v != "0")
                        || ser.children_named("dPt").any(|p| {
                            p.child("explosion")
                                .and_then(|n| n.attr("val"))
                                .is_some_and(|v| v != "0")
                        })
                }))
        {
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
        let axis_title = |axis: Option<&XmlNode>| {
            let title = axis.and_then(|a| a.child("title"));
            (
                title.map(text).unwrap_or_default(),
                text_size(title, 10.0),
                title.and_then(text_properties).and_then(|n| n.attr("b")) == Some("1"),
            )
        };
        Some(Self {
            frame,
            ooxml_layout: Some(OoxmlLayout {
                stacked,
                radar: (kind=="radarChart").then(||data.child("radarStyle").and_then(|n|n.attr("val")).unwrap_or("standard").to_string()),
                gradients,
                pie_vary_colors: data.child("varyColors").and_then(|n| n.attr("val")) == Some("1"),
                pie_start: (kind == "pieChart").then(|| {
                    data.child("firstSliceAng")
                        .and_then(|n| n.attr("val"))
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                        .unwrap_or(0.0)
                        .rem_euclid(360.0)
                }),
                category_title: axis_title(category_axis),
                value_title: axis_title(axis),
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
            font_size: text_size(
                axis.and_then(|n| n.child("txPr")),
                text_size(chart.child("legend").and_then(|n| n.child("txPr")), 10.0),
            ),
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
    fn bar_fill(
        &self,
        page: &mut PdfPage,
        series: usize,
        point: usize,
        rect: [f32; 4],
        paint: (f32, f32, f32),
    ) {
        if !self.point_colors.contains_key(&(series, point)) {
            if let Some(gradient) = self
                .ooxml_layout
                .as_ref()
                .and_then(|l| l.gradients.get(&series))
            {
                gradient.render(page, rect);
                return;
            }
        }
        page.rect_fill_rgb(
            rect[0], rect[1], rect[2], rect[3], paint.0, paint.1, paint.2,
        );
    }
    fn render_radar(&self, page: &mut PdfPage, rect: [f32; 4], legend: [f32; 2], style: &str) {
        let count = self.categories.len();
        if count < 3 {
            return;
        }
        let [x, y, w, h] = rect;
        let radius = w.min(h) * 0.40;
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let maximum = self
            .axis_max
            .unwrap_or_else(|| {
                self.series
                    .iter()
                    .flat_map(|(_, v, _)| v.iter().flatten())
                    .copied()
                    .fold(0.0, f64::max)
            })
            .max(1.0);
        let minimum = self.axis_min.unwrap_or(0.0).min(maximum - 1e-9);
        let raw=(maximum-minimum)/2.0;
        let power=10f64.powf(raw.log10().floor());
        let step=self.axis_step.unwrap_or(power*if raw/power<=1.0{1.0}else if raw/power<=2.0{2.0}else if raw/power<=5.0{5.0}else{10.0});
        let maximum=self.axis_max.unwrap_or_else(||(maximum/step).ceil()*step).max(minimum+1e-9);
        let rings=(((maximum-minimum)/step).ceil() as usize).clamp(1,100);
        let point = |i: usize, r: f32| {
            let angle = i as f32 * std::f32::consts::TAU / count as f32;
            (cx + r * angle.sin(), cy + r * angle.cos())
        };
        for ring in 0..=rings {
            let value=(minimum+ring as f64*step).min(maximum);
            let fraction=((value-minimum)/(maximum-minimum)) as f32;
            if ring>0 {for i in 0..count {let a=point(i,radius*fraction);let b=point((i+1)%count,radius*fraction);page.line_rgb(a.0,a.1,b.0,b.1,0.8,0.8,0.8);}}
            let label=crate::xlsx_format::number(value,&self.axis_format,false);
            page.text(cx+3.0,cy+radius*fraction-self.font_size,self.font_size,PdfFont::Helvetica,&label);
        }
        for (i, label) in self.categories.iter().enumerate() {
            let p = point(i, radius);
            page.line_rgb(cx, cy, p.0, p.1, 0.8, 0.8, 0.8);
            let p = point(i, radius + self.font_size * 1.3);
            let tw = crate::font_metrics::measure_helvetica(label, self.font_size, false);
            page.text(
                p.0 - tw / 2.0,
                p.1 - self.font_size / 3.0,
                self.font_size,
                PdfFont::Helvetica,
                label,
            );
        }
        for (si, (name, values, paint)) in self.series.iter().enumerate() {
            let (r, g, b) = color(paint);
            let points: Vec<_> = (0..count)
                .map(|i| {
                    values.get(i).copied().flatten().map(|v| {
                        point(
                            i,
                            (((v - minimum) / (maximum - minimum)).clamp(0.0, 1.0) as f32) * radius,
                        )
                    })
                })
                .collect();
            if style == "filled" && points.iter().all(Option::is_some) {
                page.raw(&format!("q {r} {g} {b} rg"));
                for (i, p) in points.iter().enumerate() {
                    let p = p.unwrap();
                    page.raw(&format!(
                        "{} {} {}",
                        p.0,
                        p.1,
                        if i == 0 { "m" } else { "l" }
                    ));
                }
                page.raw("h f Q");
            }
            for i in 0..count {
                if let (Some(a), Some(bpt)) = (points[i], points[(i + 1) % count]) {
                    page.line_rgb(a.0, a.1, bpt.0, bpt.1, r, g, b);
                }
            }
            if style == "marker" {
                for p in points.iter().flatten() {
                    page.ellipse_fill_stroke_rgb(p.0, p.1, 2.5, 2.5, (r, g, b), (r, g, b));
                }
            }
            if self.legend {
                let ly = legend[1] - (si as f32 + 1.0) * self.font_size * 1.4;
                page.line_rgb(legend[0], ly, legend[0] + 8.0, ly, r, g, b);
                page.text(
                    legend[0] + 12.0,
                    ly - self.font_size / 3.0,
                    self.font_size,
                    PdfFont::Helvetica,
                    name,
                );
            }
        }
    }

    fn render_pie(
        &self,
        page: &mut PdfPage,
        rect: [f32; 4],
        start: f32,
        legend: [f32; 2],
        offsets: &[[f32; 2]],
    ) {
        let radius = rect[2].min(rect[3]) / 2.0;
        let cx = rect[0] + rect[2] / 2.0;
        let cy = rect[1] + rect[3] / 2.0;
        let data = &self.series[0].1;
        let palette = [
            "#4472C4", "#ED7D31", "#A5A5A5", "#FFC000", "#5B9BD5", "#70AD47",
        ];
        let paint = |i| {
            color(
                self.point_colors
                    .get(&(0, i))
                    .map(String::as_str)
                    .unwrap_or(
                        if self
                            .ooxml_layout
                            .as_ref()
                            .is_some_and(|l| l.pie_vary_colors)
                        {
                            palette[i % palette.len()]
                        } else {
                            &self.series[0].2
                        },
                    ),
            )
        };
        for (i, angle, sweep) in pie_slices(data, start) {
            let (r, g, b) = paint(i);
            page.raw(&format!(
                "q {r} {g} {b} rg {cx} {cy} m {} {} l",
                cx + radius * angle.cos(),
                cy + radius * angle.sin()
            ));
            let segments = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
            let step = sweep / segments as f32;
            for j in 0..segments {
                let a = angle + j as f32 * step;
                let z = a + step;
                let k = 4.0 / 3.0 * (step / 4.0).tan();
                page.raw(&format!(
                    "{} {} {} {} {} {} c",
                    cx + radius * (a.cos() - k * a.sin()),
                    cy + radius * (a.sin() + k * a.cos()),
                    cx + radius * (z.cos() + k * z.sin()),
                    cy + radius * (z.sin() - k * z.cos()),
                    cx + radius * z.cos(),
                    cy + radius * z.sin()
                ));
            }
            page.raw("h f Q");
            if self.value_labels.first() == Some(&true) {
                let label = crate::xlsx_format::number(
                    data[i].unwrap(),
                    self.data_formats.first().map(String::as_str).unwrap_or(""),
                    false,
                );
                let mid = angle + sweep / 2.0;
                let width = crate::font_metrics::measure_helvetica(&label, self.font_size, false);
                page.text(
                    cx + radius * 0.65 * mid.cos() - width / 2.0,
                    cy + radius * 0.65 * mid.sin(),
                    self.font_size,
                    PdfFont::Helvetica,
                    &label,
                );
            }
        }
        if self.legend {
            for (i, name) in self.categories.iter().enumerate() {
                let offset = offsets
                    .get(i)
                    .copied()
                    .unwrap_or([0.0, i as f32 * self.font_size * 1.4]);
                let lx = legend[0] + offset[0];
                let ly = legend[1] - self.font_size - offset[1];
                let (r, g, b) = paint(i);
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
    pub fn render(&self, page: &mut PdfPage, page_height: f32, dx: f32, dy: f32) {
        let x = self.frame.origin.x.as_pt() + dx;
        let y = self.frame.origin.y.as_pt() + dy;
        let w = self.frame.size.width.as_pt();
        let h = self.frame.size.height.as_pt();
        if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
            return;
        }
        let pie = self.ooxml_layout.as_ref().and_then(|l| l.pie_start);
        let legend_names: Vec<&str> = if pie.is_some() {
            self.categories.iter().map(String::as_str).collect()
        } else {
            self.series.iter().map(|s| s.0.as_str()).collect()
        };
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
            let legend_width = legend_names
                .iter()
                .map(|s| measure(s) + self.font_size * 2.0)
                .fold(0.0_f32, f32::max);
            let horizontal_legend = matches!(layout.legend_side.as_str(), "b" | "t");
            let mut legend_height = self.font_size * 1.4;
            if horizontal_legend {
                let widths: Vec<_> = legend_names
                    .iter()
                    .map(|s| measure(s) + self.font_size * 2.0)
                    .collect();
                legend_offsets = legend_rows(&widths, (w - 16.0).max(1.0), self.font_size * 1.4);
                legend_height += legend_offsets.last().map_or(0.0, |p| p[1]);
            }
            let mut left_pad = (if self.horizontal {
                category_width
            } else {
                max_label
            }) + 10.0;
            if pie.is_some() {
                left_pad = 10.0;
            }
            let (x_title, y_title) = if self.horizontal {
                (&layout.value_title, &layout.category_title)
            } else {
                (&layout.category_title, &layout.value_title)
            };
            if !y_title.0.is_empty() {
                left_pad += y_title.1 + 12.0;
            }
            let mut right_pad = 10.0;
            let mut top_pad = if self.title.is_empty() {
                12.0
            } else {
                self.title_size + 20.0
            };
            let mut bottom_pad = if pie.is_some() {
                10.0
            } else {
                self.font_size * 2.0 + 10.0
            };
            if !x_title.0.is_empty() {
                bottom_pad += x_title.1 + 8.0;
            }
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
        if let Some(style)=self.ooxml_layout.as_ref().and_then(|l|l.radar.as_deref()) {
            self.render_radar(page,[left,bottom,pw,ph],[x+legend_position[0]*w,page_height-y-legend_position[1]*h],style);return;
        }
        if let Some(start) = pie {
            self.render_pie(
                page,
                [left, bottom, pw, ph],
                start,
                [
                    x + legend_position[0] * w,
                    page_height - y - legend_position[1] * h,
                ],
                &legend_offsets,
            );
            return;
        }
        if let Some(layout) = &self.ooxml_layout {
            let (xt, yt) = if self.horizontal {
                (&layout.value_title, &layout.category_title)
            } else {
                (&layout.category_title, &layout.value_title)
            };
            let font = |bold| {
                if bold {
                    PdfFont::HelveticaBold
                } else {
                    PdfFont::Helvetica
                }
            };
            if !xt.0.is_empty() {
                let width = crate::font_metrics::measure_helvetica(&xt.0, xt.1, xt.2);
                page.text(
                    left + (pw - width) / 2.0,
                    bottom - self.font_size * 2.0 - xt.1 - 3.0,
                    xt.1,
                    font(xt.2),
                    &xt.0,
                );
            }
            if !yt.0.is_empty() {
                let width = crate::font_metrics::measure_helvetica(&yt.0, yt.1, yt.2);
                let tx = x + yt.1 + 4.0;
                let ty = bottom + (ph - width) / 2.0;
                page.raw(&format!("q 0 1 -1 0 {tx} {ty} cm"));
                page.text(0.0, 0.0, yt.1, font(yt.2), &yt.0);
                page.raw("Q");
            }
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
        let max = if min == 0.0 && max == 0.0 { 1.0 } else { max };
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
        let (auto_lower, auto_upper) = if self.ooxml_layout.is_some() {
            chart_bounds(min, max, step)
        } else {
            ((min / step).floor() * step, (max / step).ceil() * step)
        };
        let lower = self.axis_min.unwrap_or(auto_lower);
        let upper = self.axis_max.unwrap_or(auto_upper).max(lower + step);
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
                    self.bar_fill(
                        page,
                        si,
                        i,
                        [start.min(end), cy, (end - start).abs(), bh],
                        (r, g, b),
                    );
                } else {
                    let band = pw / count as f32;
                    let bw = band * bar_fraction / groups;
                    let cx =
                        left + band * (i as f32 + (1.0 - bar_fraction) / 2.0) + group_index * bw;
                    let start = bottom + ph * fraction(base_value);
                    let end = bottom + ph * fraction(end_value);
                    self.bar_fill(
                        page,
                        si,
                        i,
                        [cx, start.min(end), bw, (end - start).abs()],
                        (r, g, b),
                    );
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

fn chart_bounds(min: f64, max: f64, step: f64) -> (f64, f64) {
    let mut lower = (min / step).floor() * step;
    let mut upper = (max / step).ceil() * step;
    // Leave room above/below bars and markers that land exactly on a tick.
    // Explicit source bounds are applied by the caller and remain authoritative.
    if min < 0.0 && min - lower < step * 1e-9 {
        lower -= step;
    }
    if max > 0.0 && upper - max < step * 1e-9 {
        upper += step;
    }
    (lower, upper)
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
    #[test]
    fn radar_uses_polar_geometry_and_cached_categories() {
        let xml=r#"<chartSpace><chart><plotArea><radarChart><radarStyle val="marker"/><ser><tx><v>Series</v></tx><cat><strLit><pt idx="0"><v>North</v></pt><pt idx="1"><v>East</v></pt><pt idx="2"><v>West</v></pt></strLit></cat><val><numLit><pt idx="0"><v>1</v></pt><pt idx="1"><v>3</v></pt><pt idx="2"><v>2</v></pt></numLit></val></ser></radarChart></plotArea></chart></chartSpace>"#;
        let root=crate::parse_xml_document(xml).unwrap();
        let chart=super::OdfChart::from_ooxml(&root,crate::Rect::new(crate::Length::pt(0.0),crate::Length::pt(0.0),crate::Length::pt(400.0),crate::Length::pt(300.0))).unwrap();
        assert_eq!(chart.ooxml_layout.as_ref().unwrap().radar.as_deref(),Some("marker"));
        assert_eq!(chart.categories,["North","East","West"]);
        let mut pdf=crate::PdfDocument::new();let i=pdf.add_page(400.0,300.0);chart.render(pdf.page_mut(i).unwrap(),300.0,0.0,0.0);assert!(pdf.finish().len()>1000);
    }

    use super::*;
    #[test]
    fn pie_slices_preserve_proportions_start_angle_and_missing_indexes() {
        let slices = pie_slices(&[Some(1.0), None, Some(0.0), Some(3.0)], 90.0);
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].0, 0);
        assert_eq!(slices[1].0, 3);
        assert!(slices[0].1.abs() < 1e-6);
        assert!((slices[0].2 + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((slices.iter().map(|s| s.2).sum::<f32>() + std::f32::consts::TAU).abs() < 1e-6);
        assert!(pie_slices(&[None, Some(0.0)], 0.0).is_empty());
    }
    #[test]
    fn linear_gradient_uses_fill_stops_instead_of_black_outline() {
        let root = crate::parse_xml_document(r#"<spPr><gradFill><gsLst><gs pos="0"><srgbClr val="FF0000"/></gs><gs pos="100000"><srgbClr val="0000FF"/></gs></gsLst><lin ang="5400000" scaled="1"/></gradFill><ln><solidFill><srgbClr val="000000"/></solidFill></ln></spPr>"#).unwrap();
        let g = linear_gradient(Some(&root), None).unwrap();
        assert_eq!(g.angle, 90.0);
        assert!(g.scaled);
        assert_eq!(g.sample(0.0), (1.0, 0.0, 0.0));
        assert_eq!(g.sample(0.5), (0.5, 0.0, 0.5));
        assert_eq!(g.sample(1.0), (0.0, 0.0, 1.0));
    }
    #[test]
    fn pie_import_retains_category_legend_and_rejects_exploded_slices() {
        let xml = r#"<chartSpace><chart><plotArea><pieChart><firstSliceAng val="90"/><varyColors val="1"/><ser><cat><strLit><pt idx="0"><v>First</v></pt><pt idx="1"><v>Second</v></pt></strLit></cat><val><numLit><pt idx="0"><v>1</v></pt><pt idx="1"><v>3</v></pt></numLit></val></ser></pieChart></plotArea><legend><txPr><p><pPr><defRPr sz="800"/></pPr></p></txPr></legend></chart></chartSpace>"#;
        let root = crate::parse_xml_document(xml).unwrap();
        let c = OdfChart::from_ooxml(&root, Rect::default()).unwrap();
        assert_eq!(c.categories, ["First", "Second"]);
        assert_eq!(c.ooxml_layout.as_ref().unwrap().pie_start, Some(90.0));
        assert_eq!(c.font_size, 8.0);
        let exploded =
            crate::parse_xml_document(&xml.replace("<ser>", r#"<ser><explosion val="10"/>"#))
                .unwrap();
        assert!(OdfChart::from_ooxml(&exploded, Rect::default()).is_none());
    }
    #[test]
    fn automatic_chart_bounds_leave_space_at_exact_ticks() {
        assert_eq!(chart_bounds(0.0, 3.0, 0.5), (0.0, 3.5));
        assert_eq!(chart_bounds(-3.0, 2.0, 0.5), (-3.5, 2.5));
        assert_eq!(chart_bounds(0.0, 3750.0, 500.0), (0.0, 4000.0));
    }
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
    chart_color(fill.children.first()?, theme)
}
fn chart_color(n: &XmlNode, theme: Option<&XmlNode>) -> Option<String> {
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

fn pie_slices(data: &[Option<f64>], start_degrees: f32) -> Vec<(usize, f32, f32)> {
    let sum: f64 = data.iter().flatten().filter(|v| **v > 0.0).sum();
    if sum <= 0.0 || !sum.is_finite() {
        return Vec::new();
    }
    let mut angle = std::f32::consts::FRAC_PI_2 - start_degrees.to_radians();
    let mut slices = Vec::new();
    for (i, v) in data.iter().enumerate() {
        if let Some(v) = v.filter(|v| *v > 0.0) {
            let sweep = -std::f32::consts::TAU * (v / sum) as f32;
            slices.push((i, angle, sweep));
            angle += sweep;
        }
    }
    slices
}
fn linear_gradient(node: Option<&XmlNode>, theme: Option<&XmlNode>) -> Option<LinearGradient> {
    let gradient = node?.child("gradFill")?;
    let line = gradient.child("lin")?;
    let angle = line.attr("ang").unwrap_or("0").parse::<f32>().ok()? / 60000.0;
    if !angle.is_finite() {
        return None;
    }
    let mut stops = Vec::new();
    for stop in gradient.child("gsLst")?.children_named("gs").take(16) {
        let position = stop.attr("pos")?.parse::<f32>().ok()? / 100000.0;
        if !position.is_finite() {
            return None;
        }
        stops.push((
            position.clamp(0.0, 1.0),
            chart_color(stop.children.first()?, theme)?,
        ));
    }
    if stops.len() < 2 {
        return None;
    }
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(LinearGradient {
        angle: angle.rem_euclid(360.0),
        scaled: line.attr("scaled") == Some("1"),
        stops,
    })
}
impl LinearGradient {
    fn sample(&self, t: f32) -> (f32, f32, f32) {
        if t <= self.stops[0].0 {
            return color(&self.stops[0].1);
        }
        for pair in self.stops.windows(2) {
            if t <= pair[1].0 {
                let a = color(&pair[0].1);
                let b = color(&pair[1].1);
                let f =
                    ((t - pair[0].0) / (pair[1].0 - pair[0].0).max(f32::EPSILON)).clamp(0.0, 1.0);
                return (
                    a.0 + (b.0 - a.0) * f,
                    a.1 + (b.1 - a.1) * f,
                    a.2 + (b.2 - a.2) * f,
                );
            }
        }
        color(&self.stops.last().unwrap().1)
    }
    fn render(&self, page: &mut PdfPage, rect: [f32; 4]) {
        let [x, y, w, h] = rect;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        page.raw(&format!("q {x} {y} {w} {h} re W n"));
        let (width, height) = if self.scaled {
            page.raw(&format!("{w} 0 0 {h} {x} {y} cm"));
            (1.0, 1.0)
        } else {
            page.raw(&format!("1 0 0 1 {x} {y} cm"));
            (w, h)
        };
        let angle = -self.angle.to_radians();
        let (dy, dx) = angle.sin_cos();
        let projections = [0.0, width * dx, height * dy, width * dx + height * dy];
        let lo = projections.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = projections
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let extent = width + height;
        // Fixed subdivision bounds PDF growth independently of gradient resolution.
        for i in 0..64 {
            let a = lo + (hi - lo) * i as f32 / 64.0;
            let b = lo + (hi - lo) * (i + 1) as f32 / 64.0;
            let (r, g, blue) = self.sample((i as f32 + 0.5) / 64.0);
            let points = [(a, -extent), (b, -extent), (b, extent), (a, extent)]
                .map(|(u, v)| (u * dx - v * dy, u * dy + v * dx));
            page.raw(&format!(
                "{r} {g} {blue} rg {} {} m {} {} l {} {} l {} {} l h f",
                points[0].0,
                points[0].1,
                points[1].0,
                points[1].1,
                points[2].0,
                points[2].1,
                points[3].0,
                points[3].1
            ));
        }
        page.raw("Q");
    }
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
