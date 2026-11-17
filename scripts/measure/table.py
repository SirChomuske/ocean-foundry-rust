"""Gather the stage 1 measurement runs into one Markdown table.

Reads test-logs/measure/cases.txt (case name, runner exit code, run folder)
and, for each case, the runner's run.txt (whole-process peak) and the CLI's
memory=report lines in stdout.txt.
"""
import os
import re
import sys

MB = re.compile(r"([\d.]+) MB")


def mb(text):
    m = MB.search(text)
    return float(m.group(1)) if m else None


rows, details = [], []
for line in open(sys.argv[1], encoding="utf-8"):
    parts = line.split(maxsplit=2)
    if len(parts) < 3:
        continue
    name, code, folder = parts[0], int(parts[1]), parts[2].strip()
    run = open(os.path.join(folder, "run.txt"), encoding="utf-8").read() if os.path.exists(os.path.join(folder, "run.txt")) else ""
    out = open(os.path.join(folder, "stdout.txt"), encoding="utf-8", errors="replace").read() if os.path.exists(os.path.join(folder, "stdout.txt")) else ""
    peak = re.search(r"peak:\s+(\d+) MB committed", run)
    secs = re.search(r"seconds:\s+([\d.]+)", run)
    outcome = re.search(r"outcome:\s+(.*)", run)
    steps = []
    for m in re.finditer(r"memory: (.+?)\s+peak\s+([\d.]+) MB\s+after\s+([\d.]+) MB", out):
        steps.append((m.group(1).strip(), float(m.group(2)), float(m.group(3))))
    sizes = {}
    for m in re.finditer(r"memory size: (.*)", out):
        t = m.group(1)
        if t.startswith("all six layers"):
            sizes["layers"] = mb(t)
        elif t.startswith("per-column maps"):
            sizes["columns"] = mb(t)
        elif t.startswith("water as columns"):
            sizes["plan"] = mb(t)
        elif t.startswith("loaded build"):
            sizes["build"] = mb(t)
        elif t.startswith("shape lists") or t.startswith("shape reading"):
            sizes["shape"] = mb(t)
        elif t.startswith("box around"):
            cells = re.search(r"about (\d+) cells", t)
            sizes["box"] = int(cells.group(1)) if cells else None
            sizes["grid"] = re.findall(r"([\d.]+) MB", t)[0] if MB.search(t) else None
        elif ": file " in t:
            sizes.setdefault("files", []).append((t.split(":")[0], mb(t.split(": file ")[1])))
    top = max(steps, key=lambda s: s[1]) if steps else ("-", 0, 0)
    biggest = max(sizes.get("files", [("-", 0)]), key=lambda f: f[1])

    def f(v):
        return "-" if v is None else (f"{v:,.1f}" if isinstance(v, float) else f"{v:,}")

    rows.append(
        f"| {name} | {outcome.group(1) if outcome else code} | {peak.group(1) if peak else '-'} | {secs.group(1) if secs else '-'} "
        f"| {top[0]} | {f(sizes.get('build'))} | {f(sizes.get('shape'))} | {f(sizes.get('layers'))} | {f(sizes.get('columns'))} "
        f"| {f(sizes.get('plan'))} | {f(sizes.get('box'))} | {sizes.get('grid') or '-'} | {biggest[0]} {f(biggest[1])} |"
    )
    if steps:
        details.append(f"\n### {name}\n\n| step | peak MB | after MB |\n|---|---|---|")
        details += [f"| {s} | {p:,.1f} | {a:,.1f} |" for s, p, a in steps]
    not_drawn = re.search(r"not drawn, (.*)", out)
    if not_drawn:
        details.append(f"\nPictures not drawn: {not_drawn.group(1)}")

print("# Stage 1 memory measurements\n")
print("Peak = whole process, committed, from the capped runner. Sizes in MB, worked out from each structure's size.\n")
print("| case | outcome | peak MB | s | step with the highest peak | loaded build | shape lists | six layers | per-column maps | water as columns (plan) | box cells | 3D grid MB | largest file MB |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
print("\n".join(rows))
print("\n## Steps per case")
print("\n".join(details))
