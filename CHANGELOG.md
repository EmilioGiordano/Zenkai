# Changelog

All notable changes to Zenkai are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Spreadsheet

- XLOOKUP with a return range of several columns (or rows) spills the matching row (or
  column), as Excel does, instead of `#VALUE!` ([#334](https://github.com/EmilioGiordano/Zenkai/issues/334)).
- A spilling formula that uses `@`, such as `=B1:B3+@C1:E1`, keeps the `@` when the file is
  saved instead of reopening without it.
- Select Visible Cells Only (Alt+;) keeps hidden rows and columns out of the next copy, so
  copying `A1:A10` with rows 3 to 5 hidden pastes 7 rows
  ([#339](https://github.com/EmilioGiordano/Zenkai/issues/339)).

### Safety

- A damaged `.xlsx` whose cell markup breaks XML rules, such as attributes run together
  (`r="A1"t="s"`) or a mismatched `</sheetData>`, is treated like any other damaged file
  instead of opening as if it were whole, which could drop a cell's type or style
  ([#348](https://github.com/EmilioGiordano/Zenkai/issues/348)).

## [0.1.2]

### Spreadsheet

- Date and time formats with fractional seconds, such as `yyyy-mm-dd hh:mm:ss.000`, show the
  time as Excel does instead of `#VALUE!` ([#316](https://github.com/EmilioGiordano/Zenkai/issues/316)).
- A CSV field that starts with `+` or `-` and is not a number, such as a phone number like
  `+34 600 111 222`, opens as text instead of being read as a formula.

### Updates

- A copy of Zenkai that is not installed, such as a loose `zenkai.exe`, now says it cannot
  update itself and points to getzenkai.com, instead of silently opening the Releases page.

### License

- Zenkai is now licensed under AGPL-3.0-only. Versions 0.1.0 and 0.1.1 stay under Apache-2.0.

## [0.1.1]

### Spreadsheet

- A `.csv` or `.tsv` now opens as the file itself, not as an untitled Book: Save writes back
  to it in its own separator, encoding and line endings, and cells you did not edit are kept
  exactly as the file had them. The options preview still shows when the file opens.
- Before a save would drop something a CSV cannot hold (other sheets, formulas, formatting),
  Zenkai asks once: keep the CSV, or save as `.xlsx`.
- The Windows installer adds Zenkai to "Open with" for `.tsv` files too.

### Security

- Update the `url` dependency for RUSTSEC-2026-0592.

## [0.1.0]

First release. Windows x64; the Linux and macOS (Apple silicon) builds are experimental.

### Spreadsheet

- Open, edit, recalculate and save `.xlsx` with an Excel-compatible formula engine (IronCalc).
- Import `.csv` and `.tsv`; read-only view of `.xls`, `.ods` and `.xlsb`.
- Excel shortcuts, undo and redo, copy and paste with Excel and Google Sheets, fill, sort,
  find and replace, and cell formatting (Ctrl+1).
- Several workbooks open at once, grouped in a sidebar; they come back on the next start.

### Safety

- Saving keeps what Zenkai cannot edit (charts, images, pivot tables and more) and asks
  before a save would drop anything.
- Autosave to a recovery folder, and recovery of your work after a crash.
- Hostile files (zip bombs, huge ranges, over-deep formulas) are rejected before they reach
  the engine.

### AI

- Chat assistant on the open workbook through Claude, Gemini CLI, Codex, opencode or Grok;
  edits ask for approval by default. Zenkai ships no model.
- `zenkai-mcp` lets MCP clients such as Claude Code work on the open workbook. Off by default.

### Install and updates

- Windows installer for your user only, without admin rights, plus a portable zip.
- Signed updates: Zenkai checks GitHub, verifies the signature, installs on restart and goes
  back to the previous version if the new one does not start.
