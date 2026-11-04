"""Regenerate the Couple wordmark from the bundled Nunito font.

Writes, from the C+O mark geometry below plus "UPLE" outlined from
fonts/Nunito[wght].ttf (SIL OFL 1.1, wght 875 — the weight whose stems match
the 28-unit ring stroke):

  couple-wordmark.svg                            dark-surface wordmark
  couple-wordmark-light.svg                      light-surface wordmark
  ../../mac/Sources/Couple/Views/CoupleWordmarkGlyphs.swift
                                                 the same outlines as a SwiftUI Path,
                                                 so the app draws the lockup natively

Glyphs are emitted as outlines, so nothing needs the font at render time.

Lockup frame (SVG units): baseline y=157, text starts x=223, letters scaled so
the cap height is 113 (the mark is 118 tall — rings sit ~1.04x cap height),
tracking 10 added after each glyph. The mark's bbox is x 21..205, y 41..159.

Run from this directory with fontTools available:
  python3 -m venv v && v/bin/pip install fonttools && v/bin/python outline-uple.py
"""
import os
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools.pens.basePen import BasePen
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.misc.transform import Transform

HERE = os.path.dirname(os.path.abspath(__file__))
FONT = os.path.join(HERE, "fonts", "Nunito[wght].ttf")
WGHT = 875
CAP_HEIGHT = 113.0
BASELINE = 157.0
START_X = 223.0
TRACKING = 10.0
TEXT = "UPLE"
SWIFT_OUT = os.path.join(HERE, "..", "..", "mac", "Sources", "Couple", "Views", "CoupleWordmarkGlyphs.swift")

MARK = """<defs>
<mask id="cutTeal" maskUnits="userSpaceOnUse" x="0" y="0" width="{w}" height="200">
<rect x="0" y="0" width="{w}" height="200" fill="#ffffff"/>
<path d="M138.2 55.7 A45 45 0 0 0 101.7 92.2" fill="none" stroke="#000000" stroke-width="44"/>
</mask>
<mask id="cutAmber" maskUnits="userSpaceOnUse" x="0" y="0" width="{w}" height="200">
<rect x="0" y="0" width="{w}" height="200" fill="#ffffff"/>
<path d="M117.7 124.5 A45 45 0 0 1 76.1 144.8" fill="none" stroke="#000000" stroke-width="44" stroke-linecap="round"/>
</mask>
</defs>
<path d="M117.7 75.5 A45 45 0 1 0 117.7 124.5" fill="none" stroke="#4fd9c4" stroke-width="28" stroke-linecap="round" mask="url(#cutTeal)"/>
<circle cx="146" cy="100" r="45" fill="none" stroke="#f0b35e" stroke-width="28" mask="url(#cutAmber)"/>
"""


def fmt(v):
    s = "%.2f" % v
    s = s.rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


class SwiftPathPen(BasePen):
    """Emits SwiftUI Path calls; receives already-transformed (SVG-space) points."""

    def __init__(self, glyphset):
        super().__init__(glyphset)
        self.lines = []

    def _pt(self, p):
        return "CGPoint(x: %s, y: %s)" % (fmt(p[0]), fmt(p[1]))

    def _moveTo(self, p):
        self.lines.append("p.move(to: %s)" % self._pt(p))

    def _lineTo(self, p):
        self.lines.append("p.addLine(to: %s)" % self._pt(p))

    def _curveToOne(self, c1, c2, p):
        self.lines.append("p.addCurve(to: %s, control1: %s, control2: %s)" % (self._pt(p), self._pt(c1), self._pt(c2)))

    def _qCurveToOne(self, c, p):
        self.lines.append("p.addQuadCurve(to: %s, control: %s)" % (self._pt(p), self._pt(c)))

    def _closePath(self):
        self.lines.append("p.closeSubpath()")

    def _endPath(self):
        pass


font = TTFont(FONT)
axes = {a.axisTag: (a.minValue, a.maxValue) for a in font["fvar"].axes}
lo, hi = axes["wght"]
instantiateVariableFont(font, {"wght": max(lo, min(hi, WGHT))}, inplace=True)

upm = font["head"].unitsPerEm
cmap = font.getBestCmap()
glyphset = font.getGlyphSet()
hmtx = font["hmtx"]

bp = BoundsPen(glyphset)
glyphset[cmap[ord("E")]].draw(bp)
size = CAP_HEIGHT * upm / bp.bounds[3]
scale = size / upm
print("font size %.2f for cap height %.0f (upm %d)" % (size, CAP_HEIGHT, upm))

svg_paths = []
swift_lines = []
pen_x = START_X
for ch in TEXT:
    gname = cmap[ord(ch)]
    xform = Transform(scale, 0, 0, -scale, pen_x, BASELINE)
    spen = SVGPathPen(glyphset)
    glyphset[gname].draw(TransformPen(spen, xform))
    svg_paths.append((ch, spen.getCommands()))
    wpen = SwiftPathPen(glyphset)
    glyphset[gname].draw(TransformPen(wpen, xform))
    swift_lines.append("        // %s" % ch)
    swift_lines.extend("        " + l for l in wpen.lines)
    pen_x += hmtx[gname][0] * scale + TRACKING
end_x = pen_x - TRACKING
width = int(round(end_x + 28))
print("text ends at x=%.1f; viewBox width %d" % (end_x, width))


def write_svg(name, fill, comment):
    body = MARK.format(w=width)
    body += '<g fill="%s">\n' % fill
    body += "".join('<path d="%s"/><!-- %s -->\n' % (d, ch) for ch, d in svg_paths)
    body += "</g>\n"
    with open(os.path.join(HERE, name), "w") as f:
        f.write('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d 200">\n' % width)
        f.write("<!-- %s -->\n" % comment)
        f.write(body)
        f.write("</svg>\n")
    print("wrote", name)


GEN = "Generated by design/logo/outline-uple.py — do not edit by hand."
write_svg(
    "couple-wordmark.svg",
    "#ebeced",
    "Couple wordmark for dark surfaces. Interlocked C+O (mask-built weave) + UPLE outlined\n"
    "     from Nunito wght 875 (SIL OFL 1.1; font in design/logo/fonts), so no font is needed\n"
    "     at render time. Light variant: couple-wordmark-light.svg. " + GEN,
)
write_svg(
    "couple-wordmark-light.svg",
    "#15171b",
    "Couple wordmark for light surfaces: same geometry as couple-wordmark.svg, UPLE in the\n"
    "     app's dark ink. The repo README picks this file on GitHub's light theme. " + GEN,
)

with open(SWIFT_OUT, "w") as f:
    f.write(
        "// %s\n"
        "// \"UPLE\" of the Couple wordmark as outlines (Nunito wght %d, SIL OFL 1.1), in the\n"
        "// coordinate frame of design/logo/couple-wordmark.svg: baseline y=%s, cap height %s,\n"
        "// text from x=%s to x=%s. CoupleWordmark draws this next to the C+O mark.\n\n"
        "import SwiftUI\n\n"
        "enum CoupleWordmarkGlyphs {\n"
        "    static let capHeight: CGFloat = %s\n"
        "    static let baseline: CGFloat = %s\n"
        "    static let startX: CGFloat = %s\n"
        "    static let endX: CGFloat = %s\n\n"
        "    static func path() -> Path {\n"
        "        var p = Path()\n"
        "%s\n"
        "        return p\n"
        "    }\n"
        "}\n"
        % (GEN, WGHT, fmt(BASELINE), fmt(CAP_HEIGHT), fmt(START_X), fmt(end_x),
           fmt(CAP_HEIGHT), fmt(BASELINE), fmt(START_X), fmt(end_x), "\n".join(swift_lines))
    )
print("wrote", os.path.relpath(SWIFT_OUT, HERE))
