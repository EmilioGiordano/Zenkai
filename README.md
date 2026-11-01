# Zenkai

A native, free, cross-platform spreadsheet that opens, explores and cleans files too large
or too messy for the usual tools.

- **Opens big files.** Built for workbooks with hundreds of thousands of rows, with compact
  cell storage and idle workbooks unloaded to free memory.
- **Feels like Excel.** Same shortcuts, formulas and error codes, so there is little to learn.
- **Does not lose your work.** Saves are atomic, unsaved work survives a crash, and Zenkai
  warns you before dropping anything it cannot keep.
- **Light.** A native app, not a browser in disguise.
- **Free and open source.** Apache-2.0, no account, no cloud.

## What it does

- Opens, edits and saves `.xlsx`, `.csv` and `.tsv`, with Excel formulas that recalculate as
  you type.
- Shows formatting faithfully: number formats, fonts, colors, borders, merged cells and
  frozen panes.
- Keeps several workbooks open, grouped into spaces in a sidebar, and restores your session
  on the next start.
- Sort, fill, find and replace, a quick chart of the selection, light and dark themes.
- Every command has a shortcut and shows up in the command palette (Ctrl+Shift+P).
- Optional AI chat and MCP bridge: let an agent you already have installed, such as Claude
  Code, read and edit the open workbook. Zenkai ships no model, and writes ask for approval
  by default.

## Download

Get the latest build from [GitHub Releases](https://github.com/EmilioGiordano/Zenkai/releases).

| Platform | Status |
| --- | --- |
| Windows x64 | Supported |
| Linux x64 | Experimental |
| macOS, Apple silicon (arm64) | Experimental |

The Windows build is unsigned, so SmartScreen shows "Windows protected your PC". Click
"More info", then "Run anyway". Viewing PDF files works on Windows only for now.

## Known limitations

- Saving keeps what Zenkai cannot edit, such as charts, images, comments, Excel tables and
  pivot tables, wherever the edit allows. When a save would drop any of it, Zenkai lists what
  and offers a copy with a new name instead.
- Content of a kind Zenkai does not detect may still be dropped when its sheet is rewritten.
- `.xlsm` files open, but their macros are not kept and the original is never overwritten.
- `.xls`, `.ods` and `.xlsb` files open read-only.
- No pivot tables, macros, real-time collaboration or web version. See the non-goals in the
  [roadmap](docs/ROADMAP.md).

## Built with AI agents

Zenkai is developed with AI coding agents, under the rules in [AGENTS.md](AGENTS.md), with
tests and review on every change. The maintainer answers for each one.

## Contributing

Bug reports, files that break the app and pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md). To build from source, package or run the benchmark, see
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Where the project is going is in
[docs/ROADMAP.md](docs/ROADMAP.md).

## Credits

Formulas are calculated by [IronCalc](https://github.com/ironcalc/IronCalc), and the
interface is built with [GPUI](https://www.gpui.rs).

## License

[Apache-2.0](LICENSE)
