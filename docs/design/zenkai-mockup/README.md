# Zenkai UI mockup

The approved UI design canvas. The live version is
https://claude.ai/artifact/XYUMggmNUzgnraYvWceNNh

The `*.dc.html` files load `./support.js`, which is not kept here, so they do not render
on their own: open the live link to see them. The HTML and the `renderVals` scripts remain
the reference for exact values (colors, alphas, sizes, copy). `canvas.json` lays the
artboards out on the canvas. Labels in the older artboards are in Spanish; the app uses the
English equivalents. The settings window artboards are already in English.

## Artboards by feature

| Feature | Artboards |
| --- | --- |
| Sidebar | `Main` (dark), `Light`, `Collapsed` |
| Agent panel | `Agent` |
| Command palette | `Palette` |
| File search | `Files` |
| Settings | `Settings` (shortcuts), `SettingsAgents`, `SettingsLook` |
| Generate data | `GenerateMenu`, `Generate`, `GenerateResult`, `GenerateEmpty` |
| Space appearance | `SpaceStyle` (quick menu and four styles), `SpaceStyleCustom`, `SpaceStyleDefaults` |
| Settings window (Phase 7) | `PrefsGeneral`, `Prefs` (Appearance, dark, theme list open), `PrefsLight`, `PrefsModified`, `PrefsAI`, `PaletteTheme` (Select theme, live preview) |

`SpaceStyle` is one component with three views (`menu`, `custom`, `defaults`); the three
space appearance artboards are wrappers around it. `Prefs` is one component with four sections
(`appearance`, `general`, `ai`, `modified`) and a `light` flag; the `Prefs*` artboards are
wrappers around it. `PaletteTheme` is `Main` with the `theme` overlay. The sticky note
`prefs_notes` in `canvas.json` describes the behavior of the settings window.
