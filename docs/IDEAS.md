# Ideas

Functionality outside the current spec, proposed here instead of implemented.

## Built overnight beyond the v0.1 spec (2026-10-08)

- Live chart panel from the selection (column, line, pie) with SVG export and Mermaid copy.
  Charts are out of scope for v0.1; the panel never writes charts into the xlsx.

## Next candidates

- Chart series with several columns, chart titles and axis labels, PNG export.
- Preserve page margins, page setup (without printer-settings parts), header/footer, tab
  colour and zoom on save by copying those elements from the original sheet XML, as
  `empty_rows.rs` does for rows. Needs care with sheet reordering and renamed sheets.
- Pick a font or fill colour from the keyboard: an action that opens the toolbar picker
  with focus on its swatches (today the palette has only "No fill" and "Automatic font
  colour"; Excel itself has no default colour shortcut).

## Agents inside Zenkai (beyond Phase 6)

Phase 6 in `docs/SPEC.md` now covers the settings file, the tool layer over the open
document and the MCP bridge, and later the ACP chat panel, context mentions and the
review layer. Still candidates on top of it:

- **`insert_rows` tool** for agents, once structural edits can be reviewed and undone as
  one named step.
- **Ctrl+K inline assist.** On a cell or range: natural language to formula ("sum
  North's 2024 sales"), explain a formula or an error (`#N/A`), clean a column (split,
  normalise dates, dedupe, unify spellings).
- **Workbook audit.** Inconsistent formulas in a column, hard-coded numbers where a
  formula is expected, ranges that stop short of the data, broken references.
- **Folder workspace.** Open a folder instead of a file (agent cwd = folder). Cross xlsx,
  csv, md and pdf sources; write results back: new summary sheets, HTML or image
  dashboards, md or pdf reports. Transformations saved as reproducible recipes (Power
  Query style) that refresh when the source files change.
- **Local models** through Ollama when detected.

Engine work these depend on: real charts written to the xlsx, and grouping/summarising
(pivot-like) for dashboards. Privacy is part of the design: the UI always shows which
data leaves the machine and to which provider.

## Beyond Excel (ideas, not scheduled)

Features no mainstream spreadsheet has, or that make Zenkai feel modern. Some were inspired
by Quadratic (quadratichq.com), a browser spreadsheet with a similar philosophy (Rust,
local computation); Zenkai's edge is being native: faster start, less memory, local files.

1. **Drop a file onto a cell.** Dropping a CSV or xlsx on the grid inserts its data at that
   cell (through the existing CSV preview); dropping it on the sidebar opens it as a file
   in the space.
2. **One-click AI actions.** Context menu and palette entries that wrap ready-made agent
   prompts: create a chart from the selection, analyse a column, clean data, fill a series.
   The user approves the result like any agent change.
3. **Column profile.** Hovering a column header shows its data type, blanks, distinct
   values, min/max and a small histogram, to understand a new file in seconds.
4. **Local-first as a product message.** Nothing leaves the machine except what an agent
   reads, always shown to the user; with a local model (Ollama) not even that.
5. **Code cells** (Python, SQL, JavaScript in the grid). Powerful but heavy: embedding an
   interpreter grows the app a lot. Possibly through the agent instead of embedded.
6. **Database connections** (Postgres, MySQL) that pull data into a sheet.
7. **Real-time collaboration.** Needs CRDT sync and a server; changes the architecture.
   Only after the app is solid.

8. **Presentation video.** A short product video of Zenkai in action: speed on large files,
   spaces, data generation and the agent. Real footage for every speed claim.
