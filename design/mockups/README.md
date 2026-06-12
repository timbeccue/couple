# Mockup screenshots

Screenshots of the validated interactive mockup (working title "RESYNC" in the
shots; offsets shown are the mockup's stream-relative values, see SPEC §1.3).
They are linked from the `[SCREENSHOT]` marker sites in UX-DECISIONS.md §1.

## Inventory

- `01-full-window-post-sync.png` — full window, post-auto-sync, side-by-side
  loupes. The primary structural reference.
- `side-by-side-view.png` — side-by-side loupe layout.
- `stacked-view.png` — stacked loupe layout.
- `04-pre-sync-no-alignment.png` — badly misaligned mapping (offset ≈ −39 s,
  87% overlap), loupes visibly disagreeing.

Not captured (text in UX-DECISIONS.md is the only reference): drift mode None
(greyed drift group, "uncorrected" summary line), the residual guidance arrow,
and the true virgin pre-sync state (offset 0, "no alignment" chip).

**These are structural reference, not a visual contract.**

- **Binding** (unchanged by anything in this directory): UX-DECISIONS.md is the
  sole authority for behavior — every DECIDED item, the §2 interactions, and
  the §4 state/algorithm parameters. Where a screenshot and the document
  disagree, the document wins.
- **Indicative only**: the visual design shown here — layout proportions,
  spacing, typography, colors, control styling, exact placement. Treat the
  screenshots as "the structure that worked": the four-band arrangement, what
  is grouped into which panel, what the loupes and readouts contain.
- The implementer has **explicit creative license** to depart from these
  visuals wherever it makes the interface work better, provided behavior stays
  per UX-DECISIONS.md. A departure that touches a DECIDED item is still a
  SPEC-CHANGE entry in PROGRESS.md, as always.
