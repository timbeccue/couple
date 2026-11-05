# Couple — compact mode prototypes (2026-11-04)

**Round 2 (same day):** owner picked **B** and proposed dropping the approve
step. B is reworked on that model below; A and C stay on the page for
comparison only. A fourth panel shows the existing full-window queue rail
under the same rule.

Three HTML prototypes of a "compact mode": a window about the width of one
sidebar that covers **import → auto-sync → review → export** and leaves the
loupes, waveform stage, nudge/drift tools and the full media pool to the
existing full window (reachable from the `⤢ Full` button). Exploratory —
nothing here is decided or wired to the shell.

## Run

```
cd <repo root>
python3 -m http.server 8742
open http://localhost:8742/design/mockups-compact/index.html
```

(or `preview_start mockups` from `.claude/launch.json`). http is needed for the
thumbnails, which are the real bach-session frames from `../mockups-v2/assets/`.
Rows are the same five couples as the v2 mockup: three good, one ambiguous
("pick one"), one with no audio. The state buttons under each window step
through Empty / Syncing / Synced / Needs a pick / Exporting / Exported; the
candidate picker is live ("use" turns bach 170 into a good couple).

## Feature set shared by all three

Kept (the things auto-sync cannot do for you):
- drop/choose files or a folder; "+ Add" to a session
- one line per couple: thumbnail, video, chosen audio, verdict dot, duration
- a listen check: play the synced result, A/B Camera · Synced · External
- the candidate picker for a "pick one" couple (strength bar, listen, use),
  and drag-a-file-onto-the-couple as the rescue for "no audio"
- export: destination, format (Auto/MOV/MP4/WAV), keep-camera-audio, Export N
- activity strip (syncing / exporting progress), exported → Reveal in Finder
- `⤢ Full` → the current three-column window, on the same session

Dropped from compact (present in full): loupes, overview waveform, offset and
drift nudges, drift mode, anchors, fit sparkline, numeric readouts (offset and
fit stay as one tertiary line under the player), the media-pool list and its
sticky player, per-file name editing (default `<stem>_synced` only).

## Round 5 — ⤢ Full animates into the three-column window

Owner direction: the Full button should grow the window and move the pieces
into place; the full window's left rail should be the card list from D and
its export controls should be D's panel.

As mocked (D only; `toggleFullD` in index.html):
- The compact window is four blocks: session line, preview, list, export.
  Pressing ⤢ Full re-renders in the 1180-wide three-column layout, then
  animates each block from its old box to its new one (left/top/width/height,
  0.5 s, same curve) while the window width animates alongside. List and
  session line slide into the left rail, preview into the center, export
  into the right rail. Contents reflow during the move (the video frame grows
  with its block). ⤡ Compact reverses it; no inline state is left behind.
- The surfaces compact mode omits fade in after the move: in the center, the
  overview waveform (same two-lane rendering, taller) and the bottom bar with
  sync tools / loupes (unchanged from today, shown as a placeholder); in the
  right rail, the Files list (one output name per checked couple, with ↺).
- Left rail = D's list verbatim: To fix / Ready, thumbnail, names, checkbox.
  Export rail = D's panel verbatim (into / as / keep camera audio / Export N).
- Implementation note for the real shell: this is a single window changing
  size with the same views re-parented, i.e. one SwiftUI/Electron layout with
  two arrangements, not two windows. NSWindow.setFrame(animate:) plus
  matched-geometry transitions on the four blocks gets the same effect.

## Round 4 — D refinements (owner feedback)

- Empty session: the drop target fills the preview area and the export box
  stays pinned to the bottom.
- "Pick one": ▶ on a candidate plays the video with that audio at its best-fit
  offset (readout turns blue: "▶ ZOOM0027.WAV at best fit · offset · fit";
  ■ stops). After "use", the preview keeps "Audio chosen by you · Undo ·
  Compare others" and a "Change…" link in the title; Compare reopens the
  candidate list with the current one marked, Undo returns the couple to
  To fix.
- One Export button, counted from the checkboxes ("Export 4 files").
- Selecting a row never moves the list (scroll position is preserved on
  redraw).
- The amber coverage bar is gone from thumbnails. The seek bar is now a
  two-lane mini waveform: camera (teal) above, external (amber) below, the
  amber lane drawn only where that audio covers the video, with a faint
  baseline where it doesn't. Auditioning a candidate redraws the amber lane
  with that candidate's coverage, so a recorder that started late (ZOOM0027)
  shows as a gap. Procedural waveforms, seeded per row.

## Round 3 — D: fixed preview + list (owner direction, same day)

Owner feedback on B: the inline player made the list jump and never sat where
the eye was; Listen is redundant once selecting a row previews it; the
"not included / no audio" line in the footer is noise; the colored status dots
and the orange audio dot should go; A's export panel (into / as / keep camera
audio, checkbox count → "Export N files") is the one to keep, with export
selected and export all side by side.

D as mocked:
- **Preview block at the top, fixed.** Shows the selected couple: player with
  Camera / Synced / External and the offset-and-fit line, or, for a "pick one"
  couple, the player plus a compact candidate picker, or, for "no audio", a
  one-line "No matching audio in this session · Choose file…". Empty session:
  the drop target lives here.
- **List below only scrolls rows.** Row = thumbnail, video name + duration,
  audio name, and a checkbox for good couples. No dots anywhere; the words
  "pick one" (red) and "no audio" (grey) are the only state marks. Sections
  To fix / Ready.
- **Export = A's panel.** Checkboxes drive the count; "This one" (selected
  couple) sits beside "Export N files". No exclusion line. After export a row
  shows ✓ time and unchecks.

Open: whether "This one" earns its place next to the checkbox model, or the
checkbox alone is enough; whether the preview block should shrink when a
picker is showing (it currently stays 16:9).

## Round 2 — implicit approval (B, and the full window)

Owner direction: trust the auto-synced couples, deal with what is flagged,
export one or all. Approval is implicit in a couple *not* being flagged.
This applies to the main window too, not only compact mode.

Rule as mocked:
- **Export all** = every couple with a good verdict that is neither skipped
  nor already exported. Flagged couples (pick one / no audio) are never in it;
  the footer line names what is excluded and why.
- **Export** on a card exports that couple alone.
- **Skip** (hover action on a card; context menu on a full-window row) is the
  opt-out that replaces the opt-in approve. Skipped couples dim and count in
  the footer; **Include** reverses it.
- **Exported** replaces "approved" as the bookkeeping state: ✓ + time on the
  card, ↻ to export again. Export all skips already-exported couples.
- Queue sections become **To fix / Ready** (no QUEUED FOR EXPORT). The approve
  checkbox, the Approve button in the editor bar, the `v` key and the approve
  case of undo go away.

Caveats raised before adopting this (see chat 2026-11-04): the green verdict
becomes the only gate, so confirm the verdict thresholds on real sessions
first; keep the skip so export-all is not all-or-nothing; the eligibility rule
("exportable = good ∧ ¬skipped ∧ ¬exported") belongs in `core/src/policy/`
so both shells agree. Adopting it is a SPEC-CHANGE against SPEC-V2
"verify = queue for export" and the matching UX-DECISIONS V2 entries, and
removes rows from SHELL-UI-PARITY.md.

## What differs (round 1 — historical)

| | A · List | B · Cards | C · Drop & go |
|---|---|---|---|
| size | 320×600 | 360×600 | 300×470 |
| unit | queue row | card with thumbnail | one-line summary |
| approve | good couples arrive checked; untick to skip | ~~explicit Approve per card~~ → implicit (round 2) | none; good couples export, "pick one" is the only interruption |
| listen | click row → inline player | Listen button on every card | ▶ in the disclosed list |
| closest to | the existing queue rail | the v2 "hero" column | a utility (ImageOptim-style) |

Open questions these are meant to settle:
1. Is approve still a step in compact mode, or is "green = export" enough now
   that the classifier is trusted? (A and C say yes; B keeps the v2 rule.
   Changing it is a SPEC-V2 / UX-DECISIONS change.)
2. Is compact a second window layout of the same session (one app, two sizes,
   `⤢` toggles), or a separate mode with its own defaults?
3. Does the picker need "listen" per candidate in compact, or is the strength
   bar plus the full window enough?
