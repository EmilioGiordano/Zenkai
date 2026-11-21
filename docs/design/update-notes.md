# What's new tab (updater branch 2)

Design note for `feat/update-notes`, approved with the decisions below. Based on the approved
mockup `docs/design/auto-update/WhatsNew.dc.html` and the "What's new" section of
`docs/SPEC.md`.

## Opening it

- Action `ShowWhatsNew`, palette entry "What's New" (File group, next to "Check for Updates"),
  shortcut `Ctrl+Alt+Shift+U` (Cmd+Option+Shift+U on macOS), the pair of `Ctrl+Alt+U`.
- A "What's new" button on the update notice card, before "Later" and "Update now", as in
  `Main.dc.html`. It dispatches the same action, so its tooltip shows the shortcut.
- One tab at most: the action focuses it when it is already open. It joins the current space
  like any new tab and is named "What's new" in the sidebar, with "Read-only" as its line of
  detail instead of a folder.
- It is not written to the session and is never offered for recovery: it holds no user data,
  and after an update its content would be stale. A session file that names one is ignored.
  Close it with the normal Close tab action, which never asks to save; Save and Save As do
  nothing there, as with a PDF tab.

## Layout

As in the mockup, inside the normal workbook area (sidebar and status bar unchanged):

1. A banner, while an update is known (also during a re-check or after a failed one, from the
   notes kept before): "Zenkai 0.1.3 is available. You have 0.1.0.",
   a "View on GitHub" link button (the Releases page, through the same prompt as any link) and the primary "Update now" button (the existing `UpdateNow` action; in this
   branch it opens Releases, the install comes in a later branch). Both are Tab stops.
2. A 640 px column, centred, scrolling inside the tab. A "What's new" title, then one section
   per version, newest first, up to 20. Each section is:
   - a heading the tab draws itself: the version at 15 px semibold and the date beside it,
     lighter and in the muted colour;
   - the release text, drawn by the tab itself from the parsed note (below);
   - a row of link buttons (below).
3. The status bar says "What's new" and "Read-only". There is no Source or Split toggle, no
   caret position and no encoding: the tab shows the preview only and takes no typing, Find or
   Replace.

Zoom with `Ctrl+Plus`, `Ctrl+Minus` and `Ctrl+0` scales the notes as in a Markdown tab and is
not remembered. Ctrl+wheel zoom is not wired in this tab.
Up, Down, Page Up, Page Down, Home and End scroll the notes wherever the focus is inside it.

## Link buttons (keyboard access to links)

Under each version, a row of link buttons lists that version's vetted links, in the order they
appear in its text, without repeats and at most 12. The release page of the version is always
first, so a version with no other links shows only that button. Each button is a Tab stop with
a focus ring and Enter activates it, and goes through the same confirmation as a click. Labels
come from the target: "Release 0.1.3 on GitHub", "Issue #301", "Pull request #13", and for
any other link the link text (cut at 60 characters), or its address when it has no text.
There are no clickable links inside the text: a link is drawn as its text, and these buttons
are the only way to open one.

## How a note is drawn

The tab does not use the Markdown text view, or any gpui Markdown parsing, for the notes. This
deviates from the SPEC's "pestaña Markdown de solo lectura", for security and at the user's
decision: the text of a release is not signed, and the text view's inline HTML pairing, table
padding and re-parsing cost more than any check on the tree can bound. The Markdown pane's
preview uses the same renderer, at the user's decision, for the same reason.

- Text goes through `markdown_restricted::check`: control and bidirectional characters
  removed, line endings normalised, then parsed once with the `markdown` crate (GitHub
  flavour; a dollar sign is only a dollar sign) and refused if the tree is deeper than 32
  levels (64 in the Markdown pane) or past the node budget. A cheap pass before the parse turns
  away the shapes the parser is quadratic on: deep nesting, closers of emphasis without
  openers, paragraphs of thousands of delimiter runs, nested footnote definitions. The tree is
  measured without recursion, in the background.
- `markdown_render/blocks.rs` turns that tree into a flat list of lines with an iterative walk
  and a hard budget on nodes visited, lines and text bytes; a note past it, or past 5,000
  lines in all the notes together, is shown as "not shown here". Headings keep three sizes
  (deeper ones use the third), paragraphs, bullet and ordered list items (marker beside the
  text, indent capped at four levels), block quotes (a rule and muted text), code blocks and
  inline code (monospace), and bold and italic runs.
- Everything else is its plain text: links (in a note), images (the alt text), raw HTML,
  tables (one line per row, cells joined by " | "), footnotes and thematic breaks (nothing).
- `markdown_render/draw.rs` draws each line as plain GPUI text with per-run fonts and colours.
- The Markdown pane's preview draws a document the same way, in a list that draws only the
  lines on screen, with the 256 KiB preview cap and a larger line budget. Its links are
  clickable (an underlined run) and go through the same prompt as before for links in a
  document, not limited to github.com; reference links are resolved; images show their alt
  text; the caret in Split mode scrolls the preview to its line. Text in the preview cannot be
  selected; the Source can.

## Opening a link

A click or Enter on a link button asks first with the existing web-link prompt
(warning style, the same focus and keyboard handling as the prompt for links in other
documents):

> Open this link in your browser?
> Site: github.com  Link: https://github.com/EmilioGiordano/Zenkai/releases/tag/v0.1.3
> [Open in browser]  [Cancel]

The text shown and the address opened are both the normalised address `vetted_link` returns,
never the text of the note, so what is confirmed is what the browser gets. The prompt cuts a
link past 80 characters with an ellipsis, as it does elsewhere. Enter confirms, Esc cancels.

## States

Each state says what is going on in words, never by colour alone.

| State | Shows |
|---|---|
| Notes available | Banner (if an update is available) and the notes. |
| Checking | A line "Checking for updates..." above the notes or the empty message. |
| Offline or failed check, notes cached | The cached notes with a warning line led by a warning mark: "Could not check for updates. Showing the notes saved from the last check." |
| Up to date, nothing newer | Centred: "Zenkai is up to date." and a "Check for Updates" button (`CheckForUpdates`). |
| Never checked, or failed with nothing cached | Centred: "No release notes yet. Zenkai fetches them when it checks for updates, which needs a connection." and the same button. |
| A version whose note is too large (over 64 KiB), empty, or too deeply nested | Its heading stays, followed by "The notes for this version are not shown here." and its link row, which always has the release page. The update stays available. |
| More than 20 versions behind (the list was cut, not merely full) | After the last note: "Older versions are on the Releases page on GitHub." with an "All releases on GitHub" link button. |
| Cache file corrupt or over 4 MiB | Ignored, as if there were no cache; the next check writes a new one. |

The tab does not make its own request: it shows what the update check already fetched and
refreshes when a check finishes while it is open. The restricted filter and the search for
links run in the background.

## Hosting

A new tab variant next to the text and PDF ones, `Entry::Notes` / `Slot::Notes` with
`DocumentKind::WhatsNew`, wired like `PdfTab`: the sidebar item, the window title, agent
routing (not a workbook), close, restore (skipped) and the status bar each get an arm.

## Keyboard path

`Ctrl+Alt+Shift+U` opens or focuses the tab and puts focus in it (focus ring on the tab, as in
a Markdown preview); Tab reaches "View on GitHub", "Update now", and then each link button of
each version; Enter activates; Esc in the link prompt cancels; the scroll keys and the zoom
keys work inside the tab.

## Decisions taken in the parts already built

- The cache is `whats-new.json` in the same folder as `updates.json`; it holds at most 20
  versions and the oldest are left out if escaping would take it past 4 MiB; a file over 4 MiB,
  with more than 20 versions, a body over 64 KiB, versions out of order or a malformed
  version or date is refused whole.
- Versions are kept only above the installed one when fetched, and cut again when read, since
  the installed version changes after an update.
- A link with an explicit port, including `:443`, is refused as written, even though the URL
  parser would drop a default port. The nesting limit is on the parsed tree: 32 for notes, 64 in the Markdown pane.
- New direct dependencies: `url` for the link check and `markdown` (the parser GPUI already
  uses, pinned to the same version) to parse the notes; neither changes the build.
