use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState, ColorSelect};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use super::sidebar::WIDTH;
use super::{Severity, Workspace};
use crate::actions::{CloseSpacePanel, ResetSpaceAppearance};
use crate::space_appearance::{
    ApplyTo, Custom, Intensity, MAX_INTENSITY, Opacity, Resolved, Rgba, SpaceOverride, resolve,
};
use crate::space_controls::{self, apply_to_checkboxes, caption, intensity_row, style_cards};
use crate::space_settings;
use crate::spaces::{SpaceColor, SpaceId};

const PANEL_WIDTH: f32 = 640.0;

// What the controls show, so the render knows when the model moved under them.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shown {
    intensity: Intensity,
    tint: Option<Rgba>,
}

pub(super) struct SpacePanel {
    space: SpaceId,
    focus: FocusHandle,
    picker: Entity<ColorPickerState>,
    slider: Entity<SliderState>,
    shown: Option<Shown>,
    _subscriptions: Vec<Subscription>,
}

fn within(
    entity: &WeakEntity<Workspace>,
    cx: &mut App,
    update: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
) {
    if let Err(error) = entity.update(cx, update) {
        tracing::debug!(%error, "workspace dropped");
    }
}

fn hsla_of(rgba: Rgba) -> Hsla {
    Hsla::from(rgb(rgba.rgb())).opacity(f32::from(rgba.opacity.percent()) / 100.0)
}

fn rgba_of(color: Hsla) -> Rgba {
    let channels = color.to_rgb();
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    let rgb = byte(channels.r) << 16 | byte(channels.g) << 8 | byte(channels.b);
    Rgba::new(rgb, Opacity::from((color.a * 100.0).round() as i64))
}

impl Workspace {
    pub(super) fn customize_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .sidebar_cursor_space()
            .unwrap_or_else(|| self.documents.current_space());
        if self.documents.spaces().get(target).is_none() {
            return;
        }
        self.sidebar.visible = true;
        let picker = cx.new(|cx| ColorPickerState::new(window, cx));
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(f32::from(MAX_INTENSITY))
                .step(1.0)
        });
        let picked = cx.subscribe(&picker, move |this, _, event: &ColorPickerEvent, cx| {
            let ColorPickerEvent::Change(color) = event;
            let color = color.map(rgba_of);
            this.edit_custom(cx, |custom| custom.color = color);
        });
        let dragged = cx.subscribe(&slider, move |this, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                let intensity = Intensity::from(value.start().round() as i64);
                this.edit_custom(cx, |custom| custom.look.intensity = intensity);
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.space_panel = Some(SpacePanel {
            space: target,
            focus,
            picker,
            slider,
            shown: None,
            _subscriptions: vec![picked, dragged],
        });
        cx.notify();
    }

    pub(super) fn close_space_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.space_panel.take().is_none() {
            return;
        }
        let focus = if self.sidebar.visible {
            self.sidebar.focus.clone()
        } else {
            self.grid.focus_handle(cx)
        };
        window.focus(&focus, cx);
        self.persist_session(cx);
        cx.notify();
    }

    pub(super) fn reset_space_appearance(&mut self, cx: &mut Context<Self>) {
        let target = self
            .space_panel
            .as_ref()
            .map(|panel| panel.space)
            .or_else(|| self.sidebar_cursor_space())
            .unwrap_or_else(|| self.documents.current_space());
        self.documents
            .set_space_appearance(target, SpaceOverride::Default);
        self.notify(
            Severity::Info,
            "The space follows the default appearance.",
            cx,
        );
    }

    fn panel_space(&self) -> Option<(SpaceId, SpaceColor, SpaceOverride)> {
        let id = self.space_panel.as_ref()?.space;
        let space = self.documents.spaces().get(id)?;
        Some((id, space.color, space.appearance))
    }

    fn resolved_in_panel(&self, cx: &App) -> Option<Resolved> {
        let (_, color, appearance) = self.panel_space()?;
        Some(resolve(&space_settings::current(cx), color, &appearance))
    }

    // Leaving the default for a custom look starts from the look the space had.
    fn edit_custom(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Custom)) {
        let Some((id, _, appearance)) = self.panel_space() else {
            return;
        };
        let mut custom = match appearance {
            SpaceOverride::Custom(custom) => custom,
            SpaceOverride::Default => Custom {
                look: space_settings::current(cx).look,
                color: None,
            },
        };
        change(&mut custom);
        self.documents
            .set_space_appearance(id, SpaceOverride::Custom(custom));
        cx.notify();
    }

    fn use_default_appearance(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _, _)) = self.panel_space() {
            self.documents
                .set_space_appearance(id, SpaceOverride::Default);
            cx.notify();
        }
    }

    fn pick_palette(&mut self, color: SpaceColor, cx: &mut Context<Self>) {
        if let Some((id, _, _)) = self.panel_space() {
            self.documents.set_space_color(id, color);
            cx.notify();
        }
    }

    fn sync_space_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resolved) = self.resolved_in_panel(cx) else {
            return;
        };
        let now = Shown {
            intensity: resolved.look.intensity,
            tint: resolved.tint,
        };
        let Some(panel) = self.space_panel.as_mut() else {
            return;
        };
        if panel.shown == Some(now) {
            return;
        }
        panel.shown = Some(now);
        let percent = f32::from(now.intensity.percent());
        panel
            .slider
            .update(cx, |slider, cx| slider.set_value(percent, window, cx));
        panel.picker.update(cx, |picker, cx| match now.tint {
            Some(tint) => picker.set_value(hsla_of(tint), window, cx),
            None => picker.clear_value(window, cx),
        });
    }

    pub(super) fn render_space_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        self.sync_space_panel(window, cx);
        let (_, color, appearance) = self.panel_space()?;
        let name = self
            .documents
            .spaces()
            .get(self.space_panel.as_ref()?.space)?
            .name
            .clone();
        let panel = self.space_panel.as_ref()?;
        let (focus, picker, slider) = (
            panel.focus.clone(),
            panel.picker.clone(),
            panel.slider.clone(),
        );
        let custom = appearance.custom().copied();
        let theme = cx.theme();
        let (popover, text, border, muted) = (
            theme.popover,
            theme.popover_foreground,
            theme.border,
            theme.muted_foreground,
        );
        let title_dot = div().size(px(12.0)).rounded_full().bg(resolve(
            &space_settings::current(cx),
            color,
            &appearance,
        )
        .tint
        .map_or(muted, hsla_of));
        let entity = cx.entity().downgrade();
        let (act_style, act_apply, act_step, act_palette) =
            (entity.clone(), entity.clone(), entity.clone(), entity);
        let featured: Vec<Hsla> = SpaceColor::ALL[1..]
            .iter()
            .filter_map(|color| color.rgb())
            .map(|value| rgb(value).into())
            .collect();
        let palette = h_flex()
            .flex_wrap()
            .gap_1()
            .children(SpaceColor::ALL.map(|option| {
                let selected =
                    custom.is_none_or(|custom| custom.color.is_none()) && color == option;
                let act = act_palette.clone();
                space_controls::swatch(
                    ("space-color", option as usize),
                    option,
                    selected,
                    move |_, cx| within(&act, cx, move |this, cx| this.pick_palette(option, cx)),
                    cx,
                )
            }));
        let is_custom = custom.is_some();
        let mode_button = |id: &'static str, label: &'static str, chosen: bool| {
            Button::new(id)
                .label(if chosen {
                    format!("✓ {label}")
                } else {
                    label.to_string()
                })
                .selected(chosen)
        };
        let mode = h_flex()
            .gap_1()
            .child(
                mode_button("space-mode-default", "Default", !is_custom)
                    .on_click(cx.listener(|this, _, _, cx| this.use_default_appearance(cx))),
            )
            .child(
                mode_button("space-mode-custom", "Custom", is_custom)
                    .on_click(cx.listener(|this, _, _, cx| this.edit_custom(cx, |_| {}))),
            );
        let custom_sections = custom.map(|custom| {
            let look = custom.look;
            v_flex()
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(caption("Style", cx))
                        .child(style_cards(
                            "space-style",
                            look,
                            move |style, _, cx| {
                                within(&act_style, cx, move |this, cx| {
                                    this.edit_custom(cx, |custom| custom.look.style = style)
                                })
                            },
                            cx,
                        )),
                )
                .child(
                    v_flex().gap_2().child(caption("Custom color", cx)).child(
                        ColorSelect::new(&picker)
                            .featured_colors(featured)
                            .placeholder("No color")
                            .accessibility_label("Custom color"),
                    ),
                )
                .child(
                    h_flex()
                        .gap_6()
                        .items_start()
                        .child(div().flex_1().child(intensity_row(
                            &slider,
                            look.intensity,
                            move |up, _, cx| {
                                within(&act_step, cx, move |this, cx| {
                                    this.edit_custom(cx, |custom| {
                                        custom.look.intensity = custom.look.intensity.stepped(up)
                                    })
                                })
                            },
                            cx,
                        )))
                        .child(div().flex_1().child(apply_to_checkboxes(
                            "space-apply",
                            look.apply_to,
                            move |apply_to: ApplyTo, _, cx| {
                                within(&act_apply, cx, move |this, cx| {
                                    this.edit_custom(cx, |custom| custom.look.apply_to = apply_to)
                                })
                            },
                            cx,
                        ))),
                )
        });
        let note = if is_custom {
            format!("Only {name} uses this. The other spaces follow the default in Settings.")
        } else {
            format!("{name} follows the default appearance from Settings (Ctrl+,).")
        };
        let left = if self.sidebar.visible {
            WIDTH + 16.0
        } else {
            48.0
        };
        Some(
            div()
                .key_context("SpacePanel")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.dispatch_action(Box::new(CloseSpacePanel), cx)
                })
                .child(
                    v_flex()
                        .id("space-panel")
                        .track_focus(&focus)
                        .absolute()
                        .top(px(72.0))
                        .left(px(left))
                        .w(px(PANEL_WIDTH))
                        .max_h(px(640.0))
                        .overflow_y_scroll()
                        .p_4()
                        .gap_4()
                        .bg(popover)
                        .text_color(text)
                        .border_1()
                        .border_color(border)
                        .rounded_lg()
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(title_dot)
                                .child(
                                    div()
                                        .flex_1()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!("Customize {name}")),
                                )
                                .child(
                                    Button::new("space-panel-close")
                                        .ghost()
                                        .icon(Icon::new(IconName::Close))
                                        .accessibility_label("Close")
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(CloseSpacePanel), cx)
                                        }),
                                ),
                        )
                        .child(v_flex().gap_2().child(mode).child(caption(note, cx)))
                        .children(custom_sections)
                        .child(
                            v_flex()
                                .gap_2()
                                .child(caption("Palette", cx))
                                .child(palette),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("space-reset")
                                        .label("Back to default (Ctrl+Alt+Shift+R)")
                                        .disabled(!is_custom)
                                        .on_click(|_, window, cx| {
                                            window
                                                .dispatch_action(Box::new(ResetSpaceAppearance), cx)
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("Changes show live in the sidebar."),
                                )
                                .child(Button::new("space-done").primary().label("Done").on_click(
                                    |_, window, cx| {
                                        window.dispatch_action(Box::new(CloseSpacePanel), cx)
                                    },
                                )),
                        ),
                ),
        )
    }
}
