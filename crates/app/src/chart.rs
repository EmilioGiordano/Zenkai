use zenkai_i18n::t;
use zenkai_types::{CellView, ValueKind};

pub const MAX_POINTS: usize = 2_000;
const SVG_WIDTH: f64 = 640.0;
const SVG_HEIGHT: f64 = 400.0;
const MARGIN: f64 = 48.0;
const PALETTE: [&str; 8] = [
    "#217346", "#2F6FB3", "#C4502B", "#8E5CC2", "#D19A22", "#2A9D8F", "#B5476F", "#5B6B7A",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartKind {
    Column,
    Line,
    Pie,
}

impl ChartKind {
    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Column => t!("chart.kind_column"),
            ChartKind::Line => t!("chart.kind_line"),
            ChartKind::Pie => t!("chart.kind_pie"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub label: String,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartData {
    pub title: String,
    pub points: Vec<Point>,
}

// Same layout rules Excel uses for a quick chart: with two or more columns the
// first holds the category labels and the next the values; a text first row is
// the series title.
pub fn from_rows(rows: &[Vec<CellView>], first_row_number: u32) -> ChartData {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let header = rows.first().is_some_and(|row| {
        row.iter()
            .last()
            .is_some_and(|cell| cell.kind == ValueKind::Text)
    });
    let title = match (header, width) {
        (true, _) => rows
            .first()
            .and_then(|row| row.last())
            .map(|cell| cell.text.clone())
            .unwrap_or_default(),
        _ => t!("chart.series_one").to_string(),
    };
    let body = if header { &rows[1..] } else { rows };
    let offset = first_row_number + u32::from(header);
    let points = (offset..)
        .zip(body)
        .filter_map(|(row_number, row)| {
            let value = row.last()?.number?;
            let label = if width >= 2 {
                row.first().map(|c| c.text.clone()).unwrap_or_default()
            } else {
                row_number.to_string()
            };
            Some(Point { label, value })
        })
        .take(MAX_POINTS)
        .collect();
    ChartData { title, points }
}

fn without_controls(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn escape_xml(text: &str) -> String {
    without_controls(text)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_mermaid(text: &str) -> String {
    without_controls(text).replace('"', "'")
}

pub fn to_mermaid(data: &ChartData, kind: ChartKind) -> String {
    let mut lines = Vec::new();
    match kind {
        ChartKind::Pie => {
            lines.push(format!("pie title {}", escape_mermaid(&data.title)));
            for point in data.points.iter().filter(|p| p.value > 0.0) {
                lines.push(format!(
                    "    \"{}\" : {}",
                    escape_mermaid(&point.label),
                    point.value
                ));
            }
        }
        ChartKind::Column | ChartKind::Line => {
            let labels: Vec<String> = data
                .points
                .iter()
                .map(|p| format!("\"{}\"", escape_mermaid(&p.label)))
                .collect();
            let values: Vec<String> = data.points.iter().map(|p| p.value.to_string()).collect();
            let series = if kind == ChartKind::Column {
                "bar"
            } else {
                "line"
            };
            lines.push("xychart-beta".to_string());
            lines.push(format!("    title \"{}\"", escape_mermaid(&data.title)));
            lines.push(format!("    x-axis [{}]", labels.join(", ")));
            lines.push(format!("    {series} [{}]", values.join(", ")));
        }
    }
    lines.join(
        "
",
    ) + "
"
}

pub fn to_svg(data: &ChartData, kind: ChartKind) -> String {
    let mut out = String::new();
    out.push_str(&format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{SVG_WIDTH}" height="{SVG_HEIGHT}" viewBox="0 0 {SVG_WIDTH} {SVG_HEIGHT}" font-family="Segoe UI, Arial, sans-serif"><rect width="100%" height="100%" fill="white"/><text x="{}" y="28" text-anchor="middle" font-size="16" fill="#222">{}</text>"##,
        SVG_WIDTH / 2.0,
        escape_xml(&data.title)
    ));
    match kind {
        ChartKind::Pie => pie_svg(data, &mut out),
        ChartKind::Column | ChartKind::Line => xy_svg(data, kind, &mut out),
    }
    out.push_str("</svg>");
    out
}

fn xy_svg(data: &ChartData, kind: ChartKind, out: &mut String) {
    let count = data.points.len().max(1) as f64;
    let max = data.points.iter().map(|p| p.value).fold(0.0_f64, f64::max);
    let min = data.points.iter().map(|p| p.value).fold(0.0_f64, f64::min);
    let span = (max - min).max(f64::EPSILON);
    let plot_w = SVG_WIDTH - 2.0 * MARGIN;
    let plot_h = SVG_HEIGHT - 2.0 * MARGIN;
    let y_of = |v: f64| MARGIN + plot_h * (1.0 - (v - min) / span);
    let band = plot_w / count;
    let zero = y_of(0.0);
    out.push_str(&format!(
        r##"<line x1="{MARGIN}" y1="{zero}" x2="{}" y2="{zero}" stroke="#999"/>"##,
        SVG_WIDTH - MARGIN
    ));
    let mut line_points = Vec::new();
    for (index, point) in data.points.iter().enumerate() {
        let x = MARGIN + band * index as f64;
        let y = y_of(point.value);
        if kind == ChartKind::Column {
            {
                let (top, height) = if y < zero {
                    (y, zero - y)
                } else {
                    (zero, y - zero)
                };
                out.push_str(&format!(r#"<rect x="{:.1}" y="{top:.1}" width="{:.1}" height="{height:.1}" fill="{}"/>"#,
                    x + band * 0.15,
                    band * 0.7,
                    PALETTE[0]
                ));
            }
        } else {
            line_points.push(format!("{:.1},{y:.1}", x + band / 2.0));
        }
        if data.points.len() <= 40 {
            out.push_str(&format!(r##"<text x="{:.1}" y="{}" text-anchor="middle" font-size="11" fill="#555">{}</text>"##,
                x + band / 2.0,
                SVG_HEIGHT - MARGIN + 16.0,
                escape_xml(&point.label)
            ));
        }
    }
    if !line_points.is_empty() {
        out.push_str(&format!(
            r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="2"/>"#,
            line_points.join(" "),
            PALETTE[0]
        ));
    }
    out.push_str(&format!(r##"<text x="{}" y="{:.1}" text-anchor="end" font-size="11" fill="#555">{max}</text><text x="{}" y="{:.1}" text-anchor="end" font-size="11" fill="#555">{min}</text>"##,
        MARGIN - 6.0,
        y_of(max) + 4.0,
        MARGIN - 6.0,
        y_of(min) + 4.0
    ));
}

fn pie_svg(data: &ChartData, out: &mut String) {
    let total: f64 = data.points.iter().map(|p| p.value.max(0.0)).sum();
    if total <= 0.0 {
        return;
    }
    let (cx, cy, r) = (SVG_WIDTH / 2.0 - 80.0, SVG_HEIGHT / 2.0 + 10.0, 140.0);
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (index, point) in data
        .points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.value > 0.0)
    {
        let sweep = point.value / total * std::f64::consts::TAU;
        let color = PALETTE[index % PALETTE.len()];
        if sweep >= std::f64::consts::TAU - 1e-9 {
            out.push_str(&format!(
                r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{color}"/>"#
            ));
        } else {
            let (x0, y0) = (cx + r * angle.cos(), cy + r * angle.sin());
            let end = angle + sweep;
            let (x1, y1) = (cx + r * end.cos(), cy + r * end.sin());
            let large = u8::from(sweep > std::f64::consts::PI);
            out.push_str(&format!(r#"<path d="M{cx},{cy} L{x0:.2},{y0:.2} A{r},{r} 0 {large} 1 {x1:.2},{y1:.2} Z" fill="{color}"/>"#
            ));
        }
        let legend_y = 70.0 + 20.0 * index as f64;
        out.push_str(&format!(r##"<rect x="{}" y="{}" width="12" height="12" fill="{color}"/><text x="{}" y="{}" font-size="12" fill="#333">{}</text>"##,
            SVG_WIDTH - 180.0,
            legend_y - 10.0,
            SVG_WIDTH - 162.0,
            legend_y,
            escape_xml(&point.label)
        ));
        angle += sweep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(n: f64) -> CellView {
        CellView {
            text: n.to_string(),
            kind: ValueKind::Number,
            number: Some(n),
            ..CellView::default()
        }
    }

    fn text(t: &str) -> CellView {
        CellView {
            text: t.to_string(),
            kind: ValueKind::Text,
            ..CellView::default()
        }
    }

    #[test]
    fn two_columns_with_header_become_labels_values_and_title() {
        let rows = vec![
            vec![text("Month"), text("Sales")],
            vec![text("Jan"), number(10.0)],
            vec![text("Feb"), number(20.0)],
        ];
        let data = from_rows(&rows, 1);
        assert_eq!(data.title, "Sales");
        assert_eq!(data.points.len(), 2);
        assert_eq!(data.points[1].label, "Feb");
        assert_eq!(data.points[1].value, 20.0);
    }

    #[test]
    fn single_column_uses_row_numbers_and_skips_non_numbers() {
        let rows = vec![vec![number(3.0)], vec![text("x")], vec![number(5.0)]];
        let data = from_rows(&rows, 4);
        assert_eq!(data.title, "Series 1");
        let labels: Vec<&str> = data.points.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["4", "6"]);
    }

    #[test]
    fn mermaid_and_svg_escape_user_text() {
        let data = ChartData {
            title: "A \"quoted\" <title>".to_string(),
            points: vec![Point {
                label: "x&y".to_string(),
                value: 1.0,
            }],
        };
        let mermaid = to_mermaid(&data, ChartKind::Column);
        assert!(mermaid.starts_with("xychart-beta"));
        assert!(mermaid.contains("title \"A 'quoted' <title>\""));
        assert!(to_mermaid(&data, ChartKind::Pie).contains("\"x&y\" : 1"));
        let svg = to_svg(&data, ChartKind::Pie);
        assert!(svg.contains("A &quot;quoted&quot; &lt;title&gt;"));
        assert!(svg.contains("x&amp;y"));
        assert!(svg.ends_with("</svg>"));
        let hostile = ChartData {
            title: "t".to_string(),
            points: vec![Point {
                label: "x\"\nclick A call evil()\u{1}".to_string(),
                value: 1.0,
            }],
        };
        assert_eq!(to_mermaid(&hostile, ChartKind::Pie).lines().count(), 2);
        assert!(!to_svg(&hostile, ChartKind::Column).contains('\u{1}'));
    }
}
