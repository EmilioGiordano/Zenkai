use gpui_kit::component::Theme;
use gpui_kit::*;
use zenkai_agent::preferences::{DarkTheme, LightTheme, ThemeChoice};

pub(crate) struct Palette {
    background: u32,
    panel: u32,
    sidebar: u32,
    header: u32,
    foreground: u32,
    muted_foreground: u32,
    border: u32,
    accent: u32,
    accent_foreground: u32,
    warning: u32,
    success: u32,
    danger: u32,
}

fn palette(choice: ThemeChoice) -> Option<Palette> {
    let palette = match choice {
        ThemeChoice::Dark(DarkTheme::AyuDark) => Palette {
            background: 0x10_14_1C,
            panel: 0x14_18_21,
            sidebar: 0x0D_10_17,
            header: 0x14_18_21,
            foreground: 0xBF_BD_B6,
            muted_foreground: 0x96_A0_AF,
            border: 0x1B_1F_29,
            accent: 0xE6_B4_50,
            accent_foreground: 0x0D_10_17,
            warning: 0xFF_B4_54,
            success: 0x70_BF_56,
            danger: 0xD9_57_57,
        },
        ThemeChoice::Dark(DarkTheme::AyuMirage) => Palette {
            background: 0x24_29_36,
            panel: 0x28_2E_3B,
            sidebar: 0x1F_24_30,
            header: 0x28_2E_3B,
            foreground: 0xCC_CA_C2,
            muted_foreground: 0x94_9D_AC,
            border: 0x17_1B_24,
            accent: 0xFF_CC_66,
            accent_foreground: 0x1F_24_30,
            warning: 0xFF_CD_66,
            success: 0x87_D9_6C,
            danger: 0xFF_66_66,
        },
        ThemeChoice::Dark(DarkTheme::OneDark) => Palette {
            background: 0x28_2C_34,
            panel: 0x21_25_2B,
            sidebar: 0x21_25_2B,
            header: 0x21_25_2B,
            foreground: 0xAB_B2_BF,
            muted_foreground: 0x9D_A5_B4,
            border: 0x18_1A_1F,
            accent: 0x52_8B_FF,
            accent_foreground: 0xFF_FF_FF,
            warning: 0xE5_C0_7B,
            success: 0x98_C3_79,
            danger: 0xE0_6C_75,
        },
        ThemeChoice::Dark(DarkTheme::TokyoNight) => Palette {
            background: 0x1A_1B_26,
            panel: 0x16_16_1E,
            sidebar: 0x16_16_1E,
            header: 0x16_16_1E,
            foreground: 0xC0_CA_F5,
            muted_foreground: 0x96_99_A8,
            border: 0x36_3B_54,
            accent: 0x7A_A2_F7,
            accent_foreground: 0x1A_1B_26,
            warning: 0xE0_AF_68,
            success: 0x9E_CE_6A,
            danger: 0xF7_76_8E,
        },
        ThemeChoice::Dark(DarkTheme::Dracula) => Palette {
            background: 0x28_2A_36,
            panel: 0x21_22_2C,
            sidebar: 0x21_22_2C,
            header: 0x21_22_2C,
            foreground: 0xF8_F8_F2,
            muted_foreground: 0x8B_95_B5,
            border: 0x44_47_5A,
            accent: 0xBD_93_F9,
            accent_foreground: 0x28_2A_36,
            warning: 0xFF_B8_6C,
            success: 0x50_FA_7B,
            danger: 0xFF_55_55,
        },
        ThemeChoice::Dark(DarkTheme::Nord) => Palette {
            background: 0x2E_34_40,
            panel: 0x3B_42_52,
            sidebar: 0x3B_42_52,
            header: 0x3B_42_52,
            foreground: 0xD8_DE_E9,
            muted_foreground: 0xAE_B3_BF,
            border: 0x43_4C_5E,
            accent: 0x88_C0_D0,
            accent_foreground: 0x2E_34_40,
            warning: 0xEB_CB_8B,
            success: 0xA3_BE_8C,
            danger: 0xBF_61_6A,
        },
        ThemeChoice::Dark(DarkTheme::ModestDark) => Palette {
            background: 0x0F_12_19,
            panel: 0x0F_12_19,
            sidebar: 0x0F_12_19,
            header: 0x1E_24_2E,
            foreground: 0xAB_B2_BF,
            muted_foreground: 0x85_8F_A1,
            border: 0x1E_24_2E,
            accent: 0x5A_B0_F6,
            accent_foreground: 0x0F_12_19,
            warning: 0xEB_C2_75,
            success: 0xA5_E0_75,
            danger: 0xEF_5F_6B,
        },
        ThemeChoice::Dark(DarkTheme::Lumin) => Palette {
            background: 0x10_10_10,
            panel: 0x161616,
            sidebar: 0x10_10_10,
            header: 0x161616,
            foreground: 0xFF_FF_FF,
            muted_foreground: 0xA0_A0_A0,
            border: 0x28_28_28,
            accent: 0xFF_C7_99,
            accent_foreground: 0x1C_1C_1C,
            warning: 0xFF_C7_99,
            success: 0x99_FF_E4,
            danger: 0xFF_80_80,
        },
        ThemeChoice::Light(LightTheme::AyuLight) => Palette {
            background: 0xFC_FC_FC,
            panel: 0xEB_EE_F0,
            sidebar: 0xF8_F9_FA,
            header: 0xFA_FA_FA,
            foreground: 0x5C_61_66,
            muted_foreground: 0x6A_72_80,
            border: 0xEB_EE_F0,
            accent: 0xF2_97_18,
            accent_foreground: 0x4C_2A_0A,
            warning: 0xEB_A4_00,
            success: 0x6C_BF_43,
            danger: 0xE6_50_50,
        },
        ThemeChoice::Light(LightTheme::OneLight) => Palette {
            background: 0xFA_FA_FA,
            panel: 0xEB_EB_EB,
            sidebar: 0xEB_EB_EB,
            header: 0xEB_EB_EB,
            foreground: 0x38_3A_42,
            muted_foreground: 0x42_42_43,
            border: 0xDC_DC_DC,
            accent: 0x40_78_F2,
            accent_foreground: 0xFF_FF_FF,
            warning: 0xC1_84_01,
            success: 0x2D_B4_48,
            danger: 0xE4_56_49,
        },
        ThemeChoice::Light(LightTheme::CatppuccinLatte) => Palette {
            background: 0xEF_F1_F5,
            panel: 0xE6_E9_EF,
            sidebar: 0xE6_E9_EF,
            header: 0xE6_E9_EF,
            foreground: 0x4C_4F_69,
            muted_foreground: 0x5C_5F_77,
            border: 0xCC_D0_DA,
            accent: 0x1E_66_F5,
            accent_foreground: 0xDC_E0_E8,
            warning: 0xDF_8E_1E,
            success: 0x40_A0_2B,
            danger: 0xD2_0F_39,
        },
        ThemeChoice::Light(LightTheme::LuminLight) => Palette {
            background: 0xFF_FF_FF,
            panel: 0xF0_F0_F0,
            sidebar: 0xF0_F0_F0,
            header: 0xF0_F0_F0,
            foreground: 0x11_11_11,
            muted_foreground: 0x55_55_55,
            border: 0xCC_CC_CC,
            accent: 0xFF_C7_99,
            accent_foreground: 0x11_11_11,
            warning: 0xD4_87_4A,
            success: 0x2D_BF_99,
            danger: 0xFF_80_80,
        },
        ThemeChoice::Dark(DarkTheme::ZenkaiDark | DarkTheme::HighContrast)
        | ThemeChoice::Light(LightTheme::ZenkaiLight) => return None,
    };
    Some(palette)
}

fn lift(color: Hsla, is_dark: bool, amount: f32) -> Hsla {
    let l = if is_dark {
        (color.l + amount).min(1.0)
    } else {
        (color.l - amount).max(0.0)
    };
    Hsla { l, ..color }
}

pub(crate) fn apply(choice: ThemeChoice, cx: &mut App) {
    let Some(palette) = palette(choice) else {
        return;
    };
    let is_dark = choice.is_dark();
    let color = |hex: u32| -> Hsla { rgb(hex).into() };
    let background = color(palette.background);
    let foreground = color(palette.foreground);
    let panel = color(palette.panel);
    let sidebar = color(palette.sidebar);
    let header = color(palette.header);
    let border = color(palette.border);
    let muted_foreground = color(palette.muted_foreground);
    let primary = color(palette.accent);
    let primary_foreground = color(palette.accent_foreground);
    let warning = color(palette.warning);
    let success = color(palette.success);
    let danger = color(palette.danger);
    let hover = lift(panel, is_dark, 0.06);
    let active = lift(panel, is_dark, 0.12);
    let primary_hover = lift(primary, is_dark, 0.1);
    let primary_active = lift(primary, is_dark, 0.2);
    let even = lift(background, is_dark, 0.02);
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        colors.background = background;
        colors.foreground = foreground;
        colors.border = border;
        colors.input = border;
        colors.sidebar_border = border;
        colors.title_bar_border = border;
        colors.status_bar_border = border;
        colors.window_border = border;
        colors.table_row_border = border;
        colors.sidebar = sidebar;
        colors.sidebar_foreground = foreground;
        colors.sidebar_accent = panel;
        colors.sidebar_accent_foreground = foreground;
        colors.sidebar_primary = primary;
        colors.sidebar_primary_foreground = primary_foreground;
        colors.secondary = panel;
        colors.secondary_foreground = foreground;
        colors.secondary_hover = hover;
        colors.secondary_active = active;
        colors.muted = panel;
        colors.muted_foreground = muted_foreground;
        colors.popover = panel;
        colors.popover_foreground = foreground;
        colors.list = background;
        colors.list_head = header;
        colors.list_even = even;
        colors.list_hover = primary.opacity(0.12);
        colors.list_active = primary.opacity(0.2);
        colors.list_active_border = primary.opacity(0.5);
        colors.table = background;
        colors.table_head = header;
        colors.table_head_foreground = muted_foreground;
        colors.table_even = even;
        colors.table_hover = primary.opacity(0.1);
        colors.table_active = primary.opacity(0.16);
        colors.table_active_border = primary.opacity(0.5);
        colors.title_bar = panel;
        colors.status_bar = panel;
        colors.tab_bar = panel;
        colors.tab_bar_segmented = panel;
        colors.tab = panel;
        colors.tab_active = background;
        colors.tab_foreground = muted_foreground;
        colors.tab_active_foreground = foreground;
        colors.accent = hover;
        colors.accent_foreground = foreground;
        colors.primary = primary;
        colors.primary_foreground = primary_foreground;
        colors.primary_hover = primary_hover;
        colors.primary_active = primary_active;
        colors.button_primary = primary;
        colors.button_primary_foreground = primary_foreground;
        colors.button_primary_hover = primary_hover;
        colors.button_primary_active = primary_active;
        colors.button = panel;
        colors.button_foreground = foreground;
        colors.button_hover = hover;
        colors.button_active = active;
        colors.button_secondary = panel;
        colors.button_secondary_foreground = foreground;
        colors.button_secondary_hover = hover;
        colors.button_secondary_active = active;
        colors.button_danger = danger.opacity(0.2);
        colors.button_danger_foreground = danger;
        colors.button_danger_hover = danger.opacity(0.3);
        colors.button_danger_active = danger.opacity(0.4);
        colors.button_warning = warning.opacity(0.2);
        colors.button_warning_foreground = warning;
        colors.button_warning_hover = warning.opacity(0.3);
        colors.button_warning_active = warning.opacity(0.4);
        colors.button_success = success.opacity(0.2);
        colors.button_success_foreground = success;
        colors.button_success_hover = success.opacity(0.3);
        colors.button_success_active = success.opacity(0.4);
        colors.danger = danger;
        colors.danger_foreground = danger;
        colors.warning = warning;
        colors.warning_foreground = warning;
        colors.success = success;
        colors.success_foreground = success;
        colors.info = primary;
        colors.info_foreground = primary;
        colors.info_hover = primary_hover;
        colors.info_active = primary_active;
        colors.button_info = primary.opacity(0.2);
        colors.button_info_foreground = primary;
        colors.button_info_hover = primary.opacity(0.3);
        colors.button_info_active = primary.opacity(0.4);
        colors.ring = primary;
        colors.caret = primary;
        colors.link = primary;
        colors.link_active = primary_hover;
        colors.link_hover = primary_hover;
        colors.selection = primary.opacity(0.3);
        colors.progress_bar = primary;
        colors.slider_bar = primary;
        colors.slider_thumb = primary_foreground;
        colors.skeleton = panel;
        colors.switch = panel;
        colors.switch_thumb = background;
        colors.drag_border = primary.opacity(0.65);
        colors.drop_target = primary.opacity(0.2);
        colors.group_box = panel;
        colors.group_box_foreground = foreground;
        colors.red = danger;
        colors.red_light = lift(danger, is_dark, 0.15);
        colors.green = success;
        colors.green_light = lift(success, is_dark, 0.15);
        colors.yellow = warning;
        colors.yellow_light = lift(warning, is_dark, 0.15);
        colors.blue = primary;
        colors.blue_light = lift(primary, is_dark, 0.15);
    });
}

#[cfg(test)]
mod tests {
    use super::{DarkTheme, ThemeChoice, palette};
    use gpui_kit::component::ThemeColor;
    use gpui_kit::{Hsla, Rgba, rgb};

    fn channel(value: f32) -> f32 {
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(color: Rgba) -> f32 {
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    fn over(foreground: Hsla, background: Hsla) -> Rgba {
        let fg = foreground.to_rgb();
        let bg = background.to_rgb();
        let mix = |f: f32, b: f32| f * fg.a + b * (1.0 - fg.a);
        Rgba {
            r: mix(fg.r, bg.r),
            g: mix(fg.g, bg.g),
            b: mix(fg.b, bg.b),
            a: 1.0,
        }
    }

    fn ratio(text: Hsla, background: Hsla) -> f32 {
        let a = luminance(over(text, background)) + 0.05;
        let b = luminance(background.to_rgb()) + 0.05;
        a.max(b) / a.min(b)
    }

    fn text_pairs(choice: ThemeChoice) -> [(Hsla, Hsla); 2] {
        let hex = |value: u32| -> Hsla { rgb(value).into() };
        match palette(choice) {
            Some(p) => [
                (hex(p.foreground), hex(p.background)),
                (hex(p.muted_foreground), hex(p.header)),
            ],
            None if choice == ThemeChoice::Dark(DarkTheme::HighContrast) => {
                [(hex(0xFFFFFF), hex(0x000000)); 2]
            }
            None => {
                let colors = if choice.is_dark() {
                    ThemeColor::dark()
                } else {
                    ThemeColor::light()
                };
                [
                    (colors.foreground, colors.background),
                    (colors.muted_foreground, colors.table_head),
                ]
            }
        }
    }

    #[test]
    fn text_meets_wcag_aa_in_every_theme() {
        for choice in ThemeChoice::ALL {
            for (text, background) in text_pairs(choice) {
                let value = ratio(text, background);
                assert!(value >= 4.5, "{choice:?}: contrast {value:.2}");
            }
        }
    }
}
