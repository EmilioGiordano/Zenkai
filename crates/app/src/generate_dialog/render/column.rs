use super::*;
use zenkai_i18n::t;

impl GenerateDialog {
    pub(super) fn render_column(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let column = &self.draft.columns()[index];
        let letter =
            ColIdx::clamped(i64::from(self.draft.layout().first_col().get()) + index as i64)
                .letters();
        let inputs = &self.columns[index];
        let kind_label = KindChoice::of(&column.kind).label();
        let type_open = matches!(self.panel, Some(Panel::Type(open)) if open == index);
        let options_open =
            matches!(&self.panel, Some(Panel::Options(open)) if open.column == index);
        let type_cell = v_flex()
            .w(px(220.0))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Button::new(("type", index))
                                .w_full()
                                .label(kind_label)
                                .dropdown_caret(true)
                                .accessibility_label(t!("gen.aria_type", letter = letter))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_type_menu(index, cx)
                                })),
                        ),
                    )
                    .when(column.kind_source == KindSource::Detected, |row| {
                        row.child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .text_xs()
                                .bg(theme.secondary)
                                .text_color(theme.secondary_foreground)
                                .child(t!("gen.auto")),
                        )
                    }),
            )
            .when(type_open, |cell| {
                cell.child(below_trigger(self.render_type_menu(index, cx)))
            });
        let options_cell =
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    Button::new(("options", index))
                        .w_full()
                        .label(ellipsize(&options::summary(&column.kind), SUMMARY_CHARS))
                        .accessibility_label(t!("gen.aria_options", letter = letter))
                        .selected(options_open)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_options(index, window, cx)
                        })),
                )
                .when(options_open, |cell| {
                    cell.child(below_trigger(self.render_options(index, cx)))
                });
        let removable = self.draft.is_added(index) && index + 1 == self.draft.columns().len();
        let unique_cell = h_flex()
            .w(px(64.0))
            .gap_1()
            .items_center()
            .child(
                Checkbox::new(("unique", index))
                    .checked(column.unique)
                    .accessibility_label(t!("gen.aria_unique", letter = letter))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_unique(index, cx))),
            )
            .when(removable, |cell| {
                cell.child(
                    Button::new(("remove", index))
                        .ghost()
                        .compact()
                        .label("✕")
                        .accessibility_label(t!("gen.aria_remove", letter = letter))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_column(index, window, cx)
                        })),
                )
            });
        v_flex()
            .p_1()
            .rounded_md()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        h_flex()
                            .w(px(176.0))
                            .gap_2()
                            .items_center()
                            .child(div().w(px(18.0)).text_sm().child(letter.clone()))
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&inputs.header)
                                        .aria_label(t!("gen.aria_header", letter = letter)),
                                ),
                            ),
                    )
                    .child(type_cell)
                    .child(options_cell)
                    .child(
                        div().w(px(84.0)).child(
                            Input::new(&inputs.blanks)
                                .aria_label(t!("gen.aria_blanks", letter = letter)),
                        ),
                    )
                    .child(unique_cell),
            )
            .when(column.header_changed(), |row| {
                row.child(
                    h_flex()
                        .pl(px(30.0))
                        .gap_2()
                        .items_center()
                        .child(div().size(px(8.0)).rounded_full().bg(theme.warning))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.warning)
                                .child(t!("gen.unsaved_header")),
                        )
                        .child(
                            Button::new(("discard", index))
                                .ghost()
                                .compact()
                                .label(t!("button.discard"))
                                .accessibility_label(t!("gen.aria_discard", letter = letter))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.discard_header(index, window, cx)
                                })),
                        ),
                )
            })
            .when_some(self.column_error(index), |row, message| {
                row.child(
                    div()
                        .pl(px(30.0))
                        .text_xs()
                        .text_color(theme.danger)
                        .child(format!("⚠ {message}")),
                )
            })
    }

    fn render_type_menu(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let current = KindChoice::of(&self.draft.columns()[index].kind);
        menu_surface(220.0, cx)
            .gap_0p5()
            .children(
                KindChoice::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(position, choice)| {
                        choice_button(("kind", position), choice.label(), choice == current)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose_kind(index, choice, cx)
                            }))
                    }),
            )
    }

    fn render_options(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let column = &self.draft.columns()[index];
        let Some(Panel::Options(panel)) = &self.panel else {
            return menu_surface(OPTIONS_WIDTH, cx);
        };
        let groups = options::choice_groups(&column.kind)
            .into_iter()
            .enumerate()
            .map(|(group_index, group)| {
                let buttons =
                    group
                        .choices
                        .into_iter()
                        .enumerate()
                        .map(|(position, choice)| {
                            let kind = choice.kind;
                            choice_button(
                                ("choice", group_index * 10 + position),
                                choice.label,
                                choice.selected,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.set_kind(index, kind.clone(), cx),
                            ))
                        });
                labelled(
                    group.label,
                    h_flex().flex_wrap().gap_1().children(buttons),
                    cx,
                )
            });
        let sources = matches!(column.kind, ColumnKind::Email { .. }).then(|| {
            v_flex()
                .gap_2()
                .child(self.render_sources(index, NameRole::First, t!("gen.first_name_from"), cx))
                .child(self.render_sources(index, NameRole::Last, t!("gen.last_name_from"), cx))
        });
        let fields = options::fields(&column.kind);
        let inputs = fields.iter().zip(&panel.inputs).map(|(field, input)| {
            labelled(field.label, Input::new(input).aria_label(field.label), cx)
        });
        let accents = matches!(column.kind, ColumnKind::Email { .. }).then(|| {
            Checkbox::new("strip-accents")
                .checked(true)
                .disabled(true)
                .label(t!("gen.strip_accents"))
        });
        menu_surface(OPTIONS_WIDTH, cx)
            .p_3()
            .gap_3()
            .children(groups)
            .children(sources)
            .children(inputs)
            .children(accents)
            .when_some(column.options_issue.clone(), |surface, issue| {
                surface.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(format!("⚠ {issue}")),
                )
            })
    }

    fn render_sources(
        &self,
        index: usize,
        role: NameRole,
        label: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let chosen = self.draft.resolved_source(index, role);
        let role_id = role as usize;
        let none = choice_button(("source-none", role_id), t!("gen.none"), chosen.is_none())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_source(index, role, Source::NoColumn, cx)
            }));
        let candidates = self
            .draft
            .source_candidates(index, role)
            .into_iter()
            .map(|candidate| {
                let header = self.draft.columns()[candidate].header.clone();
                let name = if header.is_empty() {
                    t!("gen.no_header").to_string()
                } else {
                    header
                };
                choice_button(
                    ("source", role_id * 1000 + candidate),
                    &ellipsize(&name, 20),
                    chosen == Some(candidate),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_source(index, role, Source::Column(candidate), cx)
                }))
            });
        labelled(
            label,
            h_flex()
                .flex_wrap()
                .gap_1()
                .child(none)
                .children(candidates),
            cx,
        )
    }
}
