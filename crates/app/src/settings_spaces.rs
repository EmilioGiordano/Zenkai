use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::*;

use crate::settings_page::SettingsPage;
use crate::space_appearance::{Intensity, MAX_INTENSITY, NewSpaceColor};
use crate::space_controls::{apply_to_checkboxes, caption, intensity_row, style_cards, swatch_dot};
use crate::space_settings;
use crate::spaces::SpaceColor;

// The default every space follows; each change shows at once in the sidebar beside it.
pub struct SpacesSection {
    slider: Entity<SliderState>,
    shown: Option<Intensity>,
    _dragged: Subscription,
}

impl SpacesSection {
    pub fn new(cx: &mut Context<SettingsPage>) -> SpacesSection {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(f32::from(MAX_INTENSITY))
                .step(1.0)
        });
        let dragged = cx.subscribe(&slider, |_, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                let intensity = Intensity::from(value.start().round() as i64);
                space_settings::change(cx, |defaults| defaults.look.intensity = intensity);
            }
        });
        SpacesSection {
            slider,
            shown: None,
            _dragged: dragged,
        }
    }

    pub fn render(&mut self, window: &mut Window, cx: &mut Context<SettingsPage>) -> Div {
        let defaults = space_settings::current(cx);
        let look = defaults.look;
        if self.shown != Some(look.intensity) {
            self.shown = Some(look.intensity);
            let percent = f32::from(look.intensity.percent());
            self.slider
                .update(cx, |slider, cx| slider.set_value(percent, window, cx));
        }
        let rotation = h_flex().gap_1p5().children(
            SpaceColor::ALL[1..]
                .iter()
                .map(|color| swatch_dot(*color, false, cx)),
        );
        let new_color = RadioGroup::vertical("new-space-color")
            .selected_index(Some(match defaults.new_space_color {
                NewSpaceColor::Auto => 0,
                NewSpaceColor::None => 1,
            }))
            .on_change(|index, _, cx| {
                let choice = if *index == 0 {
                    NewSpaceColor::Auto
                } else {
                    NewSpaceColor::None
                };
                space_settings::change(cx, |defaults| defaults.new_space_color = choice);
            })
            .child(
                Radio::new("new-space-auto")
                    .label("Automatic: each new space takes the next color of the palette"),
            )
            .child(Radio::new("new-space-none").label("New spaces have no color"));
        v_flex()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Appearance: Spaces"),
            )
            .child(caption(
                "Applies to every space that has no appearance of its own. To change one space, right-click it and choose Customize.",
                cx,
            ))
            .child(style_cards(
                "default-space-style",
                look,
                |style, _, cx| space_settings::change(cx, |defaults| defaults.look.style = style),
                cx,
            ))
            .child(
                h_flex()
                    .gap_6()
                    .items_start()
                    .child(div().flex_1().child(intensity_row(
                        &self.slider,
                        look.intensity,
                        |up, _, cx| {
                            space_settings::change(cx, |defaults| {
                                defaults.look.intensity = defaults.look.intensity.stepped(up)
                            })
                        },
                        cx,
                    )))
                    .child(div().flex_1().child(apply_to_checkboxes(
                        "default-space-apply",
                        look.apply_to,
                        |apply_to, _, cx| {
                            space_settings::change(cx, |defaults| defaults.look.apply_to = apply_to)
                        },
                        cx,
                    ))),
            )
            .child(caption("Color of new spaces", cx))
            .child(rotation)
            .child(new_color)
    }
}
