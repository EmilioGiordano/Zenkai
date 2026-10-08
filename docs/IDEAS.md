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
- General-format numbers that do not fit should switch to scientific notation
  (1.23457E+11) like Excel instead of showing ####; only formatted numbers and dates
  should show ####.
