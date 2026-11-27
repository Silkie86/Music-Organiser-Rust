"""Draws the DAWmate logo's SVG files in assets/logo (Solomon, 2026-11-15: "Dm, in the well"; finished 2026-11-27).

The SVGs are the logo's source. `cargo run --manifest-path tools/logo/Cargo.toml` then makes every picture from them
(the .png, the Windows .ico, the Mac .icns and the installer's pictures). Run this file only to change the drawing:

    pip install fonttools uharfbuzz
    python tools/logo/draw.py

What it draws:
- logo.svg: the icon, 64 by 64: a dark unit with a sunk "well", a white D and a lit green m. Used from 40 pixels up.
- logo-16.svg, logo-20.svg, logo-24.svg, logo-32.svg: the same letters drawn again on each size's own pixel grid
  (no well, no glow), so Windows' small icons (title bar, Explorer lists, the taskbar at 100 to 200%) stay sharp.
- logo-mac.svg: the icon on Apple's grid (1024 canvas, 824 body, soft shadow), for the Mac's .icns.
- wordmark-dark.svg, wordmark-light.svg: the icon with "DAWmate" in Space Grotesk 600 (the app's title font), the
  letters turned into outlines so no font is needed. "DAW" white (or black) and "mate" in the accent, like D and m.
- setup-side.svg, setup-small.svg: the installer's pictures (Inno Setup: the tall one on its first and last pages,
  the small square one top right on the others).
"""

import pathlib

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

TOP = pathlib.Path(__file__).resolve().parents[2]
OUT = TOP / "assets" / "logo"
FONT = TOP / "crates" / "mo-ui" / "fonts" / "SpaceGrotesk-600.ttf"

GREEN = "#3DDC97"  # the app's accent: light means "on" (docs/look.md)
GREEN_ON_LIGHT = "#0E9F63"  # the accent darkened enough to read on a light background
WHITE = "#F2F2F2"
INK = "#141414"


def n(v):
    """A number for SVG: no trailing zeros."""
    return ("%.3f" % v).rstrip("0").rstrip(".")


def d_path(x0, top, base, stem, width):
    """The D: an outline and its hole (fill-rule evenodd). The bowl is round, the stem `stem` wide."""
    h = base - top
    r_out = min(h / 2, width - stem)
    r_in = r_out - stem
    outer = (
        f"M{n(x0)} {n(top)} H{n(x0 + width - r_out)} A{n(r_out)} {n(r_out)} 0 0 1 {n(x0 + width)} {n(top + r_out)} "
        f"V{n(base - r_out)} A{n(r_out)} {n(r_out)} 0 0 1 {n(x0 + width - r_out)} {n(base)} H{n(x0)} Z"
    )
    xi, ti, bi, xr = x0 + stem, top + stem, base - stem, x0 + width - stem
    if r_in > 0:
        inner = (
            f"M{n(xi)} {n(ti)} V{n(bi)} H{n(xr - r_in)} A{n(r_in)} {n(r_in)} 0 0 0 {n(xr)} {n(bi - r_in)} "
            f"V{n(ti + r_in)} A{n(r_in)} {n(r_in)} 0 0 0 {n(xr - r_in)} {n(ti)} Z"
        )
    else:
        inner = f"M{n(xi)} {n(ti)} V{n(bi)} H{n(xr)} V{n(ti)} Z"
    return outer + " " + inner


def m_path(x0, base, x_height, stem, gap):
    """The m as one stroked line, `stem` wide: three stems `gap` apart, joined by two round arches."""
    c1 = x0 + stem / 2
    c2 = c1 + stem + gap
    c3 = c2 + stem + gap
    r = (stem + gap) / 2
    arch = x_height + stem / 2 + r
    return (
        f"M{n(c1)} {n(base)} V{n(arch)} A{n(r)} {n(r)} 0 0 1 {n(c2)} {n(arch)} V{n(base)} "
        f"M{n(c2)} {n(arch)} A{n(r)} {n(r)} 0 0 1 {n(c3)} {n(arch)} V{n(base)}"
    ), stem


def letters(d, m, glow=None):
    m_line, m_stem = m_path(*m)
    out = ""
    if glow:
        cx, cy, r, opacity = glow
        out += f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="url(#glow)" opacity="{opacity}"/>'
    out += f'<path d="{d_path(*d)}" fill="{WHITE}" fill-rule="evenodd"/>'
    out += f'<path d="{m_line}" fill="none" stroke="{GREEN}" stroke-width="{n(m_stem)}" stroke-linejoin="round"/>'
    return out


# The gradients every drawing uses: the m's glow, the unit's face (lit from above) and the well (dark, deeper at top).
GRADIENTS = (
    f'<radialGradient id="glow"><stop offset="0" stop-color="{GREEN}" stop-opacity="0.55"/>'
    f'<stop offset="1" stop-color="{GREEN}" stop-opacity="0"/></radialGradient>'
    '<linearGradient id="face" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#262626"/>'
    '<stop offset="1" stop-color="#161616"/></linearGradient>'
    '<linearGradient id="well" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#080808"/>'
    '<stop offset="1" stop-color="#121212"/></linearGradient>'
)


def icon_body():
    """The 64 by 64 icon without its <svg>: the unit, its top edge catching the light, the well, the letters."""
    return (
        '<rect x="2" y="2" width="60" height="60" rx="13" fill="url(#face)" stroke="#050505" stroke-width="1.5"/>'
        '<path d="M12 3.4 H52" stroke="#fff" stroke-opacity="0.16" stroke-width="1.1" stroke-linecap="round"/>'
        '<rect x="6.5" y="9" width="51" height="46" rx="6" fill="url(#well)"/>'
        '<path d="M9 54.4 H55" stroke="#fff" stroke-opacity="0.09" stroke-width="1"/>'
        + letters(d=(10, 18, 46, 6, 20), m=(33, 46, 27, 4.4, 4), glow=(44, 38, 14, 1))
    )


# The small sizes: every stem and gap a whole number of pixels, starting on a pixel's edge.
SMALL = {
    16: dict(d=(1, 3, 13, 2, 7), m=(10, 13, 6, 1, 1)),
    20: dict(d=(1, 4, 16, 2, 8), m=(11, 16, 8, 2, 1)),
    24: dict(d=(2, 5, 19, 3, 10), m=(13, 19, 9, 2, 2)),
    32: dict(d=(2, 6, 26, 4, 13), m=(17, 26, 12, 3, 2), glow=(24, 21, 9, 0.8)),
}
SMALL_CORNER = {16: 3, 20: 3.5, 24: 4, 32: 6}


def svg(width, height, body, extra_defs=""):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{n(width)}" height="{n(height)}" '
        f'viewBox="0 0 {n(width)} {n(height)}"><defs>{GRADIENTS}{extra_defs}</defs>{body}</svg>\n'
    )


def small_icon(size):
    rx = SMALL_CORNER[size]
    body = (
        f'<rect width="{size}" height="{size}" rx="{rx}" fill="{INK}"/>'
        f'<rect x="0.5" y="0.5" width="{size - 1}" height="{size - 1}" rx="{rx - 0.5}" fill="none" stroke="#fff" '
        'stroke-opacity="0.10" stroke-width="1"/>' + letters(**SMALL[size])
    )
    return svg(size, size, body)


def mac_icon():
    """Apple's grid: the unit fills 824 of 1024, from 100 in, with a soft shadow under it."""
    scale = 824 / 60
    shift = 100 - 2 * scale
    shadow = (
        '<filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">'
        '<feDropShadow dx="0" dy="12" stdDeviation="14" flood-color="#000" flood-opacity="0.38"/></filter>'
    )
    body = f'<g filter="url(#shadow)"><g transform="translate({n(shift)} {n(shift)}) scale({n(scale)})">{icon_body()}</g></g>'
    return svg(1024, 1024, body, shadow)


class Words:
    """'DAWmate' in Space Grotesk 600, shaped by HarfBuzz (so the font's kerning is kept) and turned into outlines."""

    def __init__(self):
        self.font = TTFont(FONT)
        self.glyphs = self.font.getGlyphSet()
        self.order = self.font.getGlyphOrder()
        self.upm = self.font["head"].unitsPerEm
        self.cap = self.font["OS/2"].sCapHeight / self.upm
        self.hb = hb.Font(hb.Face(hb.Blob.from_file_path(str(FONT))))

    def outline(self, parts, size, x, base, track=0.0):
        """Each of `parts` as its own path, set as one line from (x, base). Returns the paths and where the line ends."""
        buf = hb.Buffer()
        buf.add_str("".join(parts))
        buf.guess_segment_properties()
        hb.shape(self.hb, buf, {"kern": True, "liga": True})
        scale = size / self.upm
        ends, total = [], 0
        for p in parts:
            total += len(p)
            ends.append(total)
        pens = [SVGPathPen(self.glyphs) for _ in parts]
        cx = x
        for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
            k = next(i for i, e in enumerate(ends) if info.cluster < e)
            at = (scale, 0, 0, -scale, cx + pos.x_offset * scale, base - pos.y_offset * scale)
            self.glyphs[self.order[info.codepoint]].draw(TransformPen(pens[k], at))
            cx += pos.x_advance * scale + track
        return [p.getCommands() for p in pens], cx - track


def wordmark(words, dark):
    """The icon, then 'DAWmate' with its capitals as tall as the icon's D and on the same line as it."""
    size = 0.4375 * 64 / words.cap  # the D is 28 of the icon's 64 high
    (daw, mate), end = words.outline(["DAW", "mate"], size, 64 + 14, 46, track=0.2)
    first, second = (WHITE, GREEN) if dark else (INK, GREEN_ON_LIGHT)
    body = f'<g>{icon_body()}</g><path d="{daw}" fill="{first}"/><path d="{mate}" fill="{second}"/>'
    return svg(end + 3, 64, body)


def setup_side(words):
    """Inno Setup's tall picture (164:314): the icon and the name on the dark unit's colours, a lit line at the foot."""
    w, h = 164, 314
    icon, top = 88, 84
    (daw, mate), end = words.outline(["DAW", "mate"], 27, 0, 0, track=0.2)
    pool = (
        f'<linearGradient id="back" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1C1C1C"/>'
        f'<stop offset="1" stop-color="#0C0C0C"/></linearGradient>'
        f'<radialGradient id="pool"><stop offset="0" stop-color="{GREEN}" stop-opacity="0.18"/>'
        f'<stop offset="1" stop-color="{GREEN}" stop-opacity="0"/></radialGradient>'
    )
    body = (
        f'<rect width="{w}" height="{h}" fill="url(#back)"/>'
        f'<ellipse cx="{w / 2}" cy="{n(top + icon * 0.62)}" rx="80" ry="70" fill="url(#pool)"/>'
        f'<g transform="translate({n((w - icon) / 2)} {top}) scale({n(icon / 64)})">{icon_body()}</g>'
        f'<g transform="translate({n((w - end) / 2)} {top + icon + 38})">'
        f'<path d="{daw}" fill="{WHITE}"/><path d="{mate}" fill="{GREEN}"/></g>'
        f'<rect y="{h - 3}" width="{w}" height="3" fill="{GREEN}" opacity="0.85"/>'
    )
    return svg(w, h, body, pool)


def setup_small():
    """Inno Setup's small square picture: the icon on the wizard's white top strip."""
    body = f'<rect width="58" height="58" fill="#FFFFFF"/><g transform="translate(3 3) scale({n(52 / 64)})">{icon_body()}</g>'
    return svg(58, 58, body)


def main():
    words = Words()
    files = {
        "logo.svg": svg(64, 64, icon_body()),
        "logo-mac.svg": mac_icon(),
        "wordmark-dark.svg": wordmark(words, dark=True),
        "wordmark-light.svg": wordmark(words, dark=False),
        "setup-side.svg": setup_side(words),
        "setup-small.svg": setup_small(),
    }
    for size in SMALL:
        files[f"logo-{size}.svg"] = small_icon(size)
    # Each file is written beside the old one, checked, then renamed over it (copy, check, rename), all of them
    # written before any is put in place, so a failed run never leaves a mix of old and new drawings.
    parts = []
    for name, text in files.items():
        part = OUT / (name + ".part")
        part.write_text(text, encoding="ascii", newline="\n")
        if part.read_text(encoding="ascii") != text:
            raise SystemExit(f"{part} didn't write in full")
        parts.append((part, OUT / name))
    for part, final in parts:
        part.replace(final)
        print("wrote", final)


if __name__ == "__main__":
    main()
