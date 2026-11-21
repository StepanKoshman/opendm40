#!/usr/bin/env python3
"""Add an 'S' to the stock DMM unit fonts UNIT and LETTERS (the nS unit of
the conductance range, dmm::OHMNS; owner 2026-11-21) in
src/stockfont_w.rs, next to mkwglyph.py's glyphs.

The S is FreeSans Bold's, scaled by the font's H ink box / the face's H ink
box (x and y, as mkwglyph.py's W from V), standing on H's baseline, 4 bpp,
adv = w + 1. Only the stock H's box (its E() entry) is needed, so the
fonts' pixel storage (Huffman, mkstockfont.py) is not decoded. Re-running
replaces the two entries.

    /usr/bin/python3 tools/mksglyph.py      (from firmware/; needs PIL + the face)
"""
import os
import re

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, '..', 'src')
FACE = '/usr/share/fonts/truetype/freefont/FreeSansBold.ttf'
SS = 400
FONTS = [('UNIT', 0x0805ECB8), ('LETTERS', 0x0805DAB4)]


def nat(ch):
    f = ImageFont.truetype(FACE, SS)
    im = Image.new('L', (SS * 2, SS * 2), 0)
    ImageDraw.Draw(im).text((SS // 2, SS // 2), ch, font=f, fill=255)
    return im.crop(im.getbbox())


def h_box(data, name):
    m = re.search(r'pub static %s_E: \[Entry; \d+\] = \[(.*?)\];' % name, data, re.S)
    for c, _off, w, h, _adv, yoff, _t, _ih in re.findall(r'E\((0x[0-9A-F]+),(\d+),(\d+),(\d+),(\d+),(-?\d+),(\d+),(\d+)\)', m.group(1)):
        if int(c, 16) == ord('H'):
            return int(w), int(h), int(yoff)
    raise KeyError(name)


def s_glyph(w0, h0, yoff):
    r0, g = nat('H'), nat('S')
    sx, sy = w0 / r0.size[0], h0 / r0.size[1]
    w, h = max(1, round(g.size[0] * sx)), max(1, round(g.size[1] * sy))
    r = g.resize((w, h), Image.BOX)
    px = [[min(15, (r.getpixel((x, y)) + 8) // 17) for x in range(w)] for y in range(h)]
    # S overshoots the caps a little: its box centred on H's.
    return dict(w=w, h=h, adv=w + 1, yoff=yoff - (h - h0) // 2, px=px)


def pack4(g):
    bits = [g['px'][y][x] for x in range(g['w']) for y in range(g['h'])]
    bits += [0] * (len(bits) & 1)
    return bytes(bits[i] << 4 | bits[i + 1] for i in range(0, len(bits), 2))


def main():
    data = open(os.path.join(SRC, 'stockfont_data.rs')).read()
    path = os.path.join(SRC, 'stockfont_w.rs')
    w = open(path).read()
    # Drop earlier S entries (comment block + tuple).
    w = re.sub(r"    // (UNIT|LETTERS) S\n(    //   .*\n)*    \(0x[0-9A-F]+, 'S',.*\n", '', w)
    add = []
    for name, addr in FONTS:
        g = s_glyph(*h_box(data, name))
        add.append('    // %s S' % name)
        for row in g['px']:
            add.append('    //   ' + ''.join('.:-=+*#%@@'[min(9, v * 10 // 16)] for v in row))
        add.append("    (0x%08X, 'S', true, %d, %d, %d, %d, b\"%s\")," % (addr, g['w'], g['h'], g['adv'], g['yoff'], ''.join('\\x%02x' % c for c in pack4(g))))
    w = w.replace('\n];\n', '\n' + '\n'.join(add) + '\n];\n')
    n = len(re.findall(r"^    \(0x[0-9A-F]+, '", w, re.M))
    w = re.sub(r'OWN_GLYPHS: \[\(u32, char, bool, u8, u8, u8, i8, &\[u8\]\); \d+\]', 'OWN_GLYPHS: [(u32, char, bool, u8, u8, u8, i8, &[u8]); %d]' % n, w)
    open(path, 'w').write(w)
    print('src/stockfont_w.rs: S for UNIT, LETTERS (%d own glyphs)' % n)


if __name__ == '__main__':
    main()
