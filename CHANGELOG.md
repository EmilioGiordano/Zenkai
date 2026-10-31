# Changelog

All notable changes to Zenkai are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

First release.

### Files

- Open, edit, recalculate and save `.xlsx` files with an Excel-compatible formula engine
  (IronCalc) and Excel error codes. Saves are atomic, and a save that fails (file locked,
  read-only folder) opens Save As.
- Import `.csv` and `.tsv` with a preview that shows the detected encoding and separator,
  decimal comma and day-first date switches; Save As `.csv` exports with a BOM.
- Read-only view of `.xls`, `.ods` and `.xlsb` files, and of `.xlsx` files the engine cannot
  open, with their values and a notice.
- Several workbooks open at once, switched with Ctrl+Tab, grouped in a sidebar of named
  spaces (Ctrl+Alt+B) with recent files. Closing keeps your spaces, open files, selection,
  scroll and unsaved work for the next start.

### Large files

- Large workbooks open quickly with low memory use. <!-- LEAD: release numbers pending, docs/BENCHMARK.md only has Phase 0 (2026-10-08) figures: 100k x 20 values file opens in 1.9 s. -->

### Editing

- Undo and redo, copy, cut and paste that exchange data with Excel and Google Sheets (pasted
  formulas shift their references), paste values, fill handle, fill down and right, AutoSum,
  sort A to Z and Z to A, insert and delete rows and columns, find and replace, zoom, Go To.
- Formula editing with point mode, colored references and function AutoComplete.
- Excel keyboard shortcuts and a command palette (Ctrl+Shift+P).

### Formatting

- Formatting shown faithfully: number formats, fonts, bold, italic, underline, strikethrough,
  fills, borders, alignment, column widths, row heights, merged cells and frozen panes.
- Formatting edits: bold, italic, underline, alignment, wrap text, font and fill colors,
  borders, font size, decimals and number formats (Format Cells, Ctrl+1).

### Safety

- Saving keeps what Zenkai cannot edit (charts, images, comments, Excel tables, pivot
  tables, protection, filters, data validation, hyperlinks) wherever the edit allows. When a
  save would drop any of it, Zenkai lists exactly what and asks first, offering a copy with a
  new name; `.xlsm` originals are never overwritten.
- Files that could harm the app (zip bombs, huge array areas, over-deep formulas) are
  rejected before the engine sees them.
- Autosave: every 60 seconds by default (10 to 600 seconds in Settings), after a pause in
  typing, to a recovery folder. After a crash Zenkai offers to recover the work.
- Close prompt: closing a tab asks "Save changes to ...?" (Enter saves, N does not, Esc
  cancels). Quitting with unsaved workbooks lists them with checkboxes, all selected, to
  save the selected ones, and an option to reopen the tabs next time. In Settings you can
  choose to keep the changes for next time instead of being asked. On Windows, closing the
  window hides Zenkai in the tray.

### AI

- AI chat assistant that works on the open workbook through the agent you already have
  installed: Claude, Gemini CLI, Codex, opencode or Grok. Zenkai ships no model. Edits go
  through the normal path (recalculation, undo) and writes ask for approval by default.
- MCP bridge (`zenkai-mcp`) so external clients such as Claude Code can read and edit the
  open workbook live. It is off until you turn it on in Settings.

### Interface

- Live charts of a selection (column, line, pie) exported as SVG.
- Light, dark and high-contrast themes, English and Spanish interface, screen reader labels.
- Viewing of Markdown and PDF files next to your workbooks.
