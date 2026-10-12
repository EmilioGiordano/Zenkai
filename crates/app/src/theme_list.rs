use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::command::CommandItem;
use gpui_kit::*;
use zenkai_agent::preferences::ThemeChoice;

pub fn items(choices: &[ThemeChoice], current: ThemeChoice) -> Vec<CommandItem> {
    choices
        .iter()
        .map(|&choice| {
            CommandItem::new()
                .label(choice.name())
                .keywords([choice.tag()])
                .checked(choice == current)
                .child(move |_, cx| {
                    h_flex()
                        .flex_1()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().child(choice.name()))
                        .child(
                            div()
                                .px_1p5()
                                .rounded_md()
                                .text_xs()
                                .bg(cx.theme().secondary)
                                .text_color(cx.theme().muted_foreground)
                                .child(choice.tag()),
                        )
                })
        })
        .collect()
}
