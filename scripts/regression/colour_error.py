"""Mean colour error (OKLab distance) between the wanted and the block colours of a
-target.png picture (wanted on the left, blocks on the right), and how hard the borders are."""
import math
import sys

from PIL import Image


def lin(c):
    c /= 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def oklab(rgb):
    r, g, b = (lin(v) for v in rgb)
    l = (0.41222147 * r + 0.53633255 * g + 0.051445995 * b) ** (1 / 3)
    m = (0.2119035 * r + 0.6806995 * g + 0.10739696 * b) ** (1 / 3)
    s = (0.08830246 * r + 0.28171885 * g + 0.6299787 * b) ** (1 / 3)
    return [0.21045426 * l + 0.7936178 * m - 0.004072047 * s, 1.9779985 * l - 2.4285922 * m + 0.4505937 * s, 0.025904037 * l + 0.78277177 * m - 0.80867577 * s]


GREY = (203, 201, 195)  # background between and around the two halves
for path in sys.argv[1:]:
    im = Image.open(path).convert("RGB")
    w, h = im.size
    half = (w - 8) // 2
    cache = {}

    def lab(c):
        if c not in cache:
            cache[c] = oklab(c)
        return cache[c]

    px = im.load()
    total = n = edge = edges = 0.0
    for y in range(h):
        for x in range(half):
            t, m = px[x, y], px[w - half + x, y]
            if t == GREY:
                continue
            total += math.dist(lab(t), lab(m))
            n += 1
            if x + 1 < half and px[x + 1, y] != GREY:
                m2 = px[w - half + x + 1, y]
                if m2 != m:
                    edge += math.dist(lab(m), lab(m2))
                    edges += 1
    name = path.replace("\\", "/").rsplit("/", 1)[-1]
    print("%-28s mean off %.4f   borders: %5.1f%% of neighbours, mean step %.4f" % (name, total / n, 100 * edges / n, edge / max(edges, 1)))
