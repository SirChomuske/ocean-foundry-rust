"""Stage 6: compare the up-front memory estimate with the measured peaks.

For each run folder given (test-logs/runs/...), reads the CLI's memory=report
lines: the estimate (what is held + what the job adds) against the highest
step of the job itself (bake, 3D view, pictures, report, saving files), and
against the whole run, which also has the CLI-only read-back checks.
"""
import re
import sys

print("| case | survey estimate MB | estimate MB | job peak MB | estimate vs job | whole run MB | estimate vs whole |")
print("|---|---|---|---|---|---|---|")
for folder in sys.argv[1:]:
    out = open(f"{folder}/stdout.txt", encoding="utf-8", errors="replace").read()
    run = open(f"{folder}/run.txt", encoding="utf-8").read()
    name = re.search(r"measure-(?:extra-)?(.+)$", folder.rstrip("/\\")).group(1)
    sv = re.search(r"estimate for the whole job ([\d.]+) MB", out)
    sv = f"{float(sv.group(1)):.1f}" if sv else "-"
    est = re.search(r"estimate for the bake, pictures and files: ([\d.]+) MB", out)
    steps = [(m.group(1).strip(), float(m.group(2))) for m in re.finditer(r"memory: (.+?)\s+peak\s+([\d.]+) MB", out)]
    job = max((p for s, p in steps if "read back" not in s), default=0)
    whole = re.search(r"peak:\s+(\d+) MB committed", run)
    if sv != "-":
        e = max(float(sv), float(est.group(1)) if est else 0)
        w = float(whole.group(1)) if whole else 0
        print(f"| {name} | {sv} | {est.group(1) if est else '-'} | {job:.1f} | {100 * (e - job) / job:+.1f}% | {w:.0f} | {100 * (e - w) / w:+.1f}% |")
        continue
    if not est:
        print(f"| {name} | {sv} | - | {job:.1f} | - | {whole.group(1) if whole else '-'} | - |")
        continue
    e = float(est.group(1))
    w = float(whole.group(1)) if whole else 0
    print(f"| {name} | {sv} | {e:.1f} | {job:.1f} | {100 * (e - job) / job:+.1f}% | {w:.0f} | {100 * (e - w) / w:+.1f}% |")
