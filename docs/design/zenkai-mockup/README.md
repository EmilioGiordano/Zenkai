# Zenkai UI mockup

The approved UI design canvas. The live version is
https://claude.ai/artifact/XYUMggmNUzgnraYvWceNNh

The `*.dc.html` files load `./support.js`, which is not kept here, so they do not render
on their own: open the live link to see them. The HTML and the `renderVals` scripts remain
the reference for exact values (colors, alphas, sizes, copy). `canvas.json` lays the
artboards out on the canvas. Labels in the mockup are in Spanish; the app uses the English
equivalents.

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

`SpaceStyle` is one component with three views (`menu`, `custom`, `defaults`); the three
space appearance artboards are wrappers around it.
