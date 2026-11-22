# Changelog

All notable changes to Zenkai are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
