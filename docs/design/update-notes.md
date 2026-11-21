# What's new tab (updater branch 2)

Design note for `feat/update-notes`. Based on the approved mockup
`docs/design/auto-update/WhatsNew.dc.html` and the "What's new" section of `docs/SPEC.md`.
Nothing of the UI described here is implemented yet; the data side (release fetch, cache,
restricted filter, link check) is.

## Opening it

- Action `ShowWhatsNew`, palette entry "What's New" (File group, next to "Check for Updates"),
  shortcut `Ctrl+Alt+Shift+U` (Cmd+Option+Shift+U on macOS), the pair of `Ctrl+Alt+U`.
- A "What's new" button on the update notice card, before "Later" and "Update now", as in
  `Main.dc.html`. It dispatches the same action, so it shows the shortcut in its tooltip.
- One tab at most: the action focuses it when it is already open. It joins the current
  space like any new tab and is named "What's new" in the sidebar, with "Read only" as its
  line of detail instead of a folder.
- It is not written to the session and is never offered for recovery: it holds no user data,
  and after an update its content would be stale. Close it with the normal Close tab action,
  which never asks to save.

## Layout

As in the mockup, inside the normal workbook area (sidebar and status bar unchanged):

1. A banner, only while an update is available: "Zenkai 0.1.3 is available. You have 0.1.0.",
   a "View on GitHub" link button (opens the Releases page, a build-time constant, no
   confirmation) and the primary "Update now" button (the existing `UpdateNow` action; in this
   branch it opens Releases, the install comes in a later branch). Both are focusable.
2. The notes, in the existing Markdown preview, centred, 640 px column (the mockup's width),
   scrolling inside the pane. Heading "What's new", then one `##` section per version,
   newest first, up to 20, each followed by that release's text.
3. The status bar says "Read only" (existing `status.read_only` string). There is no Source or
   Split toggle, no caret position and no encoding: the pane renders the preview only and
   ignores typing, Find, Replace and mode cycling.

Zoom (`Ctrl+wheel`, `Ctrl+Plus/Minus`) works as in any Markdown tab and is not remembered.

Two small differences from the mockup, to be approved: the date sits in the heading in
parentheses, `0.1.3 (2026-12-04)`, because a Markdown heading cannot style part of itself in a
muted colour; the column is 640 px where other Markdown tabs use 860.

## Restricted mode

The existing Markdown pane gets a restricted mode instead of a second renderer. It is on
whenever the tab is a What's new tab and cannot be turned off.

- Text goes through `markdown_restricted::clean` before it reaches the parser: control and
  bidirectional characters removed, line endings normalised, nesting limit 16.
- Raw HTML is already shown as text by the pane's existing plugin. Images are replaced by their
  alt text and no image is ever loaded (a plugin, plus the pane's image source always blocked).
- Every link is judged by `markdown_restricted::vetted_link` (https, exactly `github.com`, no
  user, no port, no `..`, path `/EmilioGiordano/Zenkai` or below). A link that passes is
  rendered as a link; any other link, including reference links and bare URLs, is rendered as
  its plain text. The click handler checks again, so a link can never open on the strength of
  how it was drawn.

## Opening a link

A click on a valid link asks first, with the existing web-link prompt (warning style, the same
focus and keyboard handling as the prompt for links in other documents):

> Open this link in your browser?
> Site: github.com  Link: https://github.com/EmilioGiordano/Zenkai/releases/tag/v0.1.3
> [Open in browser]  [Cancel]

The text shown and the address opened are both the normalised address returned by
`vetted_link`, never the text of the note, so what is confirmed is what the browser gets. The
prompt cuts a link past 80 characters with an ellipsis, as it does elsewhere. Enter confirms,
Esc cancels.

## States

Each state says what is going on in words and an icon, never by colour alone.

| State | Shows |
|---|---|
| Notes available | Banner (if an update is available) and the notes. |
| Checking | A line "Checking for updates..." above the notes or the empty message. |
| Offline or failed check, notes cached | The cached notes with a warning line, icon included: "Could not check for updates. Showing the notes saved from the last check." |
| Up to date, nothing newer | Centred: "Zenkai is up to date." and a "Check for Updates" button (`CheckForUpdates`). |
| Never checked, or failed with nothing cached | Centred: "No release notes yet. Zenkai fetches them when it checks for updates, which needs a connection." and the same button. |
| A version whose note is too large (over 64 KiB), empty, or too deeply nested | Its heading stays, followed by "The notes for this version are not shown here." and a link "Read them on GitHub" to that version's release page. The update stays available. |
| More than 20 versions behind | After the last note: "Older versions are on the Releases page on GitHub." with a link to it. |
| Cache file corrupt or over 4 MiB | Ignored, as if there were no cache; the next check writes a new one. |

The tab does not make its own request: it shows what the update check already fetched, and
refreshes when a check finishes while it is open.

## Hosting

A new tab variant next to the text and PDF ones, `Entry::Notes` / `Slot::Notes` with
`DocumentKind::WhatsNew`, following how `PdfTab` is wired: the sidebar item, the window title,
agent routing (not a workbook), close and restore (skipped, see above) and the status bar each
get an arm. Save and Save As do nothing there, with the same treatment as a PDF tab
(to be confirmed against that code when implementing).

## Keyboard path

`Ctrl+Alt+Shift+U` opens or focuses the tab and puts focus in its pane (focus ring as in the
preview); Tab reaches "View on GitHub" and "Update now" in the banner; Esc in the link prompt
cancels. One gap, shared with the existing Markdown preview: links inside the notes are
reachable by mouse only, because the text view does not expose keyboard activation of links.
The banner's "View on GitHub" and `UpdateNow` cover the Releases page from the keyboard; a
specific release link has no keyboard path until the text view gains one.

## Decisions taken in the parts already built

- The cache is `whats-new.json` in the same folder as `updates.json`; it holds at most 20
  versions and the oldest are left out if escaping would take it past 4 MiB; a file over 4 MiB,
  with more than 20 versions, a body over 64 KiB, versions out of order or a malformed
  version or date is refused whole.
- Versions are kept only above the installed one when fetched, and cut again when read, since
  the installed version changes after an update.
- A link with an explicit port, including `:443`, is refused as written, even though the URL
  parser would drop a default port. The nesting limit in restricted mode is 16 (64 elsewhere).
- The one new dependency is the `url` crate for the link check, already in the build through
  GPUI; no network or licence change.

## Open points for approval

1. The two mockup differences above (date in the heading, 640 px column).
2. `Ctrl+Alt+Shift+U` as the shortcut.
3. The tab not surviving a restart.
4. The keyboard gap for links inside the notes.
