"""Extract U, P, L, E outlines from SF Rounded Bold as SVG paths.

Mirrors the canvas wordmark: font-size 158, baseline y=157, start x=223,
letter-spacing 10 (added after each glyph). Emits one <path> per letter,
y-flipped into SVG screen coordinates.
"""
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.misc.transform import Transform

FONT = "/System/Library/Fonts/SFNSRounded.ttf"
SIZE = 158.0
BASELINE = 157.0
START_X = 223.0
TRACKING = 10.0

font = TTFont(FONT)
axes = {a.axisTag: (a.minValue, a.defaultValue, a.maxValue) for a in font["fvar"].axes}
print("axes:", axes)
wght = 700 if "wght" in axes else None
if wght:
    lo, _, hi = axes["wght"]
    wght = max(lo, min(hi, 700))
    instantiateVariableFont(font, {"wght": wght}, inplace=True)

upm = font["head"].unitsPerEm
scale = SIZE / upm
cmap = font.getBestCmap()
glyphset = font.getGlyphSet()
hmtx = font["hmtx"]

pen_x = START_X
paths = []
for ch in "UPLE":
    gname = cmap[ord(ch)]
    glyph = glyphset[gname]
    spen = SVGPathPen(glyphset)
    tpen = TransformPen(spen, Transform(scale, 0, 0, -scale, pen_x, BASELINE))
    glyph.draw(tpen)
    d = spen.getCommands()
    paths.append((ch, d))
    adv = hmtx[gname][0] * scale
    pen_x += adv + TRACKING
print("end_x:", round(pen_x - TRACKING, 1))
with open("uple_paths.txt", "w") as f:
    for ch, d in paths:
        f.write(f'<path d="{d}"/><!-- {ch} -->\n')
print("wrote uple_paths.txt")
