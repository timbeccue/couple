# Couple logo assets

The mark is the wordmark's interlocked C+O: teal (camera) C, amber (external) O — the app's
two track colors from `mac/Sources/Couple/Theme.swift`. Over/under gaps are SVG mask cutouts,
so every file renders correctly on any background and stays crisp at any zoom.

## Final assets

- `couple-wordmark.svg` — full wordmark for dark surfaces. "UPLE" is outlined paths
  (SF Rounded, wght 700, size 158, baseline 157, tracking 10; extracted with fontTools),
  so no font is needed at render time. Note: SF Rounded is Apple's font — fine for the app
  and site, reconfirm licensing before use far outside Apple-platform contexts.
- `couple-appicon.svg` — app icon on the Apple icon grid (1024 canvas, 824 squircle,
  baked drop shadow). Source of the .icns.
- `Couple.icns` — generated from `couple-appicon.svg`; a copy ships from
  `mac/Resources/AppIcon.icns` (wired in `mac/Scripts/make-app.sh`).
- `e-co-wordmark-light.svg` — light-background variant (UPLE in dark ink). Parked, kept in
  sync with the mark geometry.

## Prototype leftovers

`a-sync-mark.svg`, `b-pair-dots.svg`, `c-chain-link.svg` (icon prototype board version),
`d-offset-snap.svg`, `e-co-wordmark.svg` (pre-outline wordmark, text renders in the viewer's
system font) — the round-2 exploration set; the design canvas "Couple Logo Prototypes" has
the full history.

## Regenerating Couple.icns

Requires `rsvg-convert` (librsvg) and macOS `iconutil`:

```sh
cd design/logo
mkdir AppIcon.iconset
for s in 16 32 128 256 512; do
  rsvg-convert -w $s -h $s couple-appicon.svg -o AppIcon.iconset/icon_${s}x${s}.png
  rsvg-convert -w $((s*2)) -h $((s*2)) couple-appicon.svg -o AppIcon.iconset/icon_${s}x${s}@2x.png
done
iconutil -c icns AppIcon.iconset -o Couple.icns
cp Couple.icns ../../mac/Resources/AppIcon.icns
rm -rf AppIcon.iconset
```

## Regenerating the outlined UPLE

`outline-uple.py` (needs fontTools in a venv: `python3 -m venv v && v/bin/pip install fonttools`,
then `v/bin/python outline-uple.py`). It prints the glyph `<path>` elements to paste into
`couple-wordmark.svg`; parameters are at the top of the script.
