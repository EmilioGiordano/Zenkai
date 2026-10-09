use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::chart::{BarChart, LineChart, PieChart};
use gpui_kit::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, CellView, ColIdx, Range, RowIdx, SheetId};

use crate::actions::{
    ChartColumn, ChartLine, ChartPie, CloseChart, CopyChartMermaid, ExportChartSvg,
};
use crate::chart::{self, ChartData, ChartKind, MAX_POINTS};

const PIE_COLORS: [u32; 8] = [
    0x217346, 0x2F6FB3, 0xC4502B, 0x8E5CC2, 0xD19A22, 0x2A9D8F, 0xB5476F, 0x5B6B7A,
];

pub struct ChartPanel {
    pub sheet: SheetId,
    pub source: Range,
    pub kind: ChartKind,
    pub data: ChartData,
}

impl ChartPanel {
    pub fn new(workbook: &Workbook, sheet: SheetId, source: Range) -> ChartPanel {
        let mut panel = ChartPanel {
            sheet,
            source,
            kind: ChartKind::Column,
            data: ChartData {
                title: String::new(),
                points: Vec::new(),
            },
        };
        panel.refresh(workbook);
        panel
    }

    pub fn refresh(&mut self, workbook: &Workbook) {
        let end = workbook.used_end(self.sheet);
        let last_row = self
            .source
            .end
            .row
            .min(end.row)
            .min(self.source.start.row.offset(MAX_POINTS as i64));
        let columns: Vec<ColIdx> = if self.source.cols() >= 2 {
            vec![self.source.start.col, self.source.end.col]
        } else {
            vec![self.source.start.col]
        };
        let rows: Vec<Vec<CellView>> = (self.source.start.row.get()..=last_row.get())
            .map(|row| {
                columns
                    .iter()
                    .map(|col| {
                        workbook.cell(
                            self.sheet,
                            CellPos::new(RowIdx::clamped(i64::from(row)), *col),
                        )
                    })
                    .collect()
            })
            .collect();
        self.data = chart::from_rows(&rows, self.source.start.row.get() + 1);
    }
}

fn button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn kind_button(
    id: &'static str,
    kind: ChartKind,
    current: ChartKind,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .label(kind.label())
        .selected(kind == current)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

pub fn render(panel: &ChartPanel, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let points = panel.data.points.clone();
    let body: AnyElement = if points.is_empty() {
        div()
            .p_4()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Select a range with numbers to chart it.")
            .into_any_element()
    } else {
        match panel.kind {
            ChartKind::Column => BarChart::new(points)
                .band(|p| p.label.clone())
                .value(|p| p.value)
                .value_axis(true)
                .into_any_element(),
            ChartKind::Line => LineChart::new(points)
                .x(|p| p.label.clone())
                .y(|p| p.value)
                .y_axis(true)
                .dot()
                .into_any_element(),
            ChartKind::Pie => PieChart::new(points.into_iter().enumerate().collect::<Vec<_>>())
                .value(|(_, p)| p.value as f32)
                .color(|(index, _)| rgb(PIE_COLORS[index % PIE_COLORS.len()]))
                .label(|(_, p)| p.label.clone().into())
                .outer_radius(120.0)
                .into_any_element(),
        }
    };
    v_flex()
        .w(px(460.0))
        .h_full()
        .border_l_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            h_flex()
                .h(px(36.0))
                .px_2()
                .gap_1()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .overflow_hidden()
                        .child(format!("{}, {}", panel.data.title, panel.source)),
                )
                .child(kind_button(
                    "chart-column",
                    ChartKind::Column,
                    panel.kind,
                    ChartColumn,
                ))
                .child(kind_button(
                    "chart-line",
                    ChartKind::Line,
                    panel.kind,
                    ChartLine,
                ))
                .child(kind_button(
                    "chart-pie",
                    ChartKind::Pie,
                    panel.kind,
                    ChartPie,
                ))
                .child(button(
                    "chart-svg",
                    IconName::Download,
                    "Export as SVG image",
                    ExportChartSvg,
                ))
                .child(button(
                    "chart-mermaid",
                    IconName::Copy,
                    "Copy as Mermaid",
                    CopyChartMermaid,
                ))
                .child(button(
                    "chart-close",
                    IconName::X,
                    "Close chart",
                    CloseChart,
                )),
        )
        .child(div().flex_1().min_h_0().p_4().child(body))
        .child(
            div()
                .px_3()
                .pb_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Live: the chart follows edits to its range."),
        )
}
