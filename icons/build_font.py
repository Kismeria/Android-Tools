import glob, os, re
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.svgLib.path import SVGPath

UPM, ASC, DESC = 1000, 850, -150
glyphs, cmap, metrics = {".notdef": None}, {}, {".notdef": (UPM, 0)}
order = [".notdef"]
for svg in sorted(glob.glob("svg/*.svg")):
    cp = os.path.splitext(os.path.basename(svg))[0]
    size = int(open("svg/" + cp + ".size").read())
    k = UPM / size
    name = "uni" + cp
    pen = TTGlyphPen(None)
    # SVG y grows down: flip and fit the icon box into [DESC, ASC].
    tpen = TransformPen(Cu2QuPen(pen, max_err=1.0, reverse_direction=True), (k, 0, 0, -k, 0, ASC))
    SVGPath(svg).draw(tpen)
    glyphs[name] = pen.glyph()
    cmap[int(cp, 16)] = name
    metrics[name] = (UPM, 0)
    order.append(name)
glyphs[".notdef"] = TTGlyphPen(None).glyph()

fb = FontBuilder(UPM, isTTF=True)
fb.setupGlyphOrder(order)
fb.setupCharacterMap(cmap)
fb.setupGlyf(glyphs)
fb.setupHorizontalMetrics(metrics)
fb.setupHorizontalHeader(ascent=ASC, descent=DESC)
fb.setupNameTable({"familyName": "AT Icons", "styleName": "Regular"})
fb.setupOS2(sTypoAscender=ASC, sTypoDescender=DESC, usWinAscent=ASC, usWinDescent=-DESC)
fb.setupPost()
pass
fb.font.flavor = "woff2"
fb.save("../ui/fonts/at-icons.woff2")
print(len(cmap), "glyphs", os.path.getsize("at-icons.woff2"), "bytes")
