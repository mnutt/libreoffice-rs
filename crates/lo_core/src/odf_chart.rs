//! Cached ODF bar/line charts. Unsupported chart types are left unsupported.
use crate::{XmlNode, Rect, PdfPage, PdfFont};
use crate::style::{OdfStyles, odf_length};

#[derive(Clone, Debug, PartialEq)]
pub struct OdfChart {
    pub frame: Rect,
    plot: [f32; 4],
    title: String,
    categories: Vec<String>,
    series: Vec<(String, Vec<Option<f64>>, String)>,
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
            horizontal,
            line: kind == "line",
            legend: chart.child("legend").is_some(),
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
                    let cx = left + pw * (i as f32 + 0.5) / count as f32;
                    let cy = bottom + ph * fraction(*value);
                    if let Some((px, py)) = previous {
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
