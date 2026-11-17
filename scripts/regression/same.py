"""Compare every picture of a new run with the baseline, pixel for pixel.

Prints one line per picture and a total. Exit code 1 when any picture differs or is missing.
"""
import os
import sys

from PIL import Image, ImageChops

new, base = sys.argv[1], sys.argv[2]
same = total = 0
for root, _, files in os.walk(base):
    for n in sorted(files):
        if not n.endswith(".png"):
            continue
        total += 1
        pb = os.path.join(root, n)
        pa = os.path.join(new, os.path.relpath(pb, base))
        if not os.path.exists(pa):
            print(n, "missing")
            continue
        a, b = Image.open(pa).convert("RGB"), Image.open(pb).convert("RGB")
        ok = a.size == b.size and ImageChops.difference(a, b).getbbox() is None
        same += ok
        print(n, "same" if ok else "DIFFERENT")
print(f"{same}/{total} same")
sys.exit(0 if same == total and total > 0 else 1)
