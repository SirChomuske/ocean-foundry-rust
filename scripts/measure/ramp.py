"""Stage 7: what each step of a build run adds over what was held before it.

For each run folder: the box, the loaded build, the bake's addition over the
laid land, the pictures' addition over the kept bake, and the whole run.
"""
import re
import sys

print("| case | box (M cells) | build blocks | loaded build MB | bake adds MB | 3D view and pictures add MB | job peak MB | whole run MB | seconds |")
print("|---|---|---|---|---|---|---|---|---|")
for folder in sys.argv[1:]:
    out = open(f"{folder}/stdout.txt", encoding="utf-8", errors="replace").read()
    run = open(f"{folder}/run.txt", encoding="utf-8").read()
    name = re.search(r"measure-(.+)$", folder.rstrip("/\\")).group(1)
    steps = {m.group(1).strip(): (float(m.group(2)), float(m.group(3))) for m in re.finditer(r"memory: (.+?)\s+peak\s+([\d.]+) MB\s+after\s+([\d.]+) MB", out)}
    box = int(re.search(r"box around every block: (\d+) cells", out).group(1))
    blocks = re.search(r"; (\d+) filled cells", out)
    build = re.search(r"loaded build: .*?, ([\d.]+) MB;", out)
    laid = steps["lay the land / read the build's shape"][1]
    bake_peak, kept = steps["bake"]
    pictures = max(steps["3D view scene from the columns"][0], steps["pictures (3D view grid)"][0], steps["report and shore picture"][0])
    job = max(p for s, (p, _) in steps.items() if "read back" not in s)
    whole = re.search(r"peak:\s+(\d+) MB committed", run).group(1)
    secs = re.search(r"seconds:\s+([\d.]+)", run).group(1)
    print(f"| {name} | {box / 1e6:,.0f} | {int(blocks.group(1)):,} | {float(build.group(1)):.0f} | {bake_peak - laid:.0f} | {pictures - kept:.0f} | {job:.0f} | {whole} | {float(secs):.0f} |")
