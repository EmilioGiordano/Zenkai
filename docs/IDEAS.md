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

## Phase 6 candidate: agents inside Zenkai

Agents work on the open document, never on the file on disk: no reload, no lost unsaved
work, and every change goes through the same edit path as the user's (recalc, live grid,
undo). Ordered so each step is useful on its own.

1. **Zenkai as an MCP server.** Tools over the open workbooks: `list_workbooks`,
   `list_sheets`, `get_selection`, `read_range`, `find`, `write_cells`, `set_formula`,
   `format_range`, `insert_rows`. An external Claude Code (or any MCP client) can then
   drive the running app live. The in-app chat reuses the same tools.
2. **Chat panel over ACP** (Agent Client Protocol, as in Zed). Zenkai spawns the agent
   (Claude Code, Gemini CLI, Codex, ...) and passes it the MCP server. Setup must be
   trivial, unlike Zed: installed agents are detected on PATH and work with their own
   login; optional own API key stored in Windows Credential Manager; local models via
   Ollama when detected.
3. **Context mentions, as in Cursor.** Select a cell or range and add it to the chat
   (shortcut plus context menu); it appears as a chip such as `[Ventas!A1:N200001]` so the
   agent knows exactly what the request is about instead of guessing. Sheets and whole
   workbooks can be mentioned too.
4. **Review layer.** Each agent batch is highlighted in the grid with accept/reject per
   change or all at once, is one named undo step ("Agent: normalise dates"), and never
   saves the file. Per-session permission: read only, ask before writing, automatic.
5. **Ctrl+K inline assist.** On a cell or range: natural language to formula ("sum
   North's 2024 sales"), explain a formula or an error (`#N/A`), clean a column (split,
   normalise dates, dedupe, unify spellings).
6. **Workbook audit.** Inconsistent formulas in a column, hard-coded numbers where a
   formula is expected, ranges that stop short of the data, broken references.
7. **Folder workspace.** Open a folder instead of a file (agent cwd = folder). Cross xlsx,
   csv, md and pdf sources; write results back: new summary sheets, HTML or image
   dashboards, md or pdf reports. Transformations saved as reproducible recipes (Power
   Query style) that refresh when the source files change.

Engine work these depend on: real charts written to the xlsx, and grouping/summarising
(pivot-like) for dashboards. Privacy is part of the design: the UI always shows which
data leaves the machine and to which provider.
