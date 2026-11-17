# Regression check: bakes the nine test runs and two bright-pair seas with the
# release CLI and compares every picture with the accepted baseline.
# Why: every new feature must leave the default output exactly as it was.
#
# Run it through the capped runner, from the project folder:
#   pwsh -NoProfile -File scripts/capped-run.ps1 -CapMB 4096 -Name regression -- bash scripts/regression/run.sh
# Add "update" after run.sh to make the new pictures the baseline (only after the user accepts a change in look).
set -e
cd "$(dirname "$0")/../.."
here=scripts/regression
out=target/regression
cli=./target/release/of-cli.exe
# The test runs use the coast and cliffs lands, which only the development
# build has: build with --features of-app/dev,of-cli/dev (or run dev-build.ps1)
# first. A package build in target/release is refused here.
"$cli" params lava > /dev/null 2>&1 || { echo "regression needs the development build of of-cli (see run.sh)"; exit 1; }
mkdir -p "$out"
for d in "$out"/*/; do [ -d "$d" ] && rm -r -- "$d"; done
"$cli" testruns "$out" > /dev/null
C="ocean.width=320 ocean.length=224 ocean.direction=200 ocean.seed=7 ocean.dark_colour=#4fb0e8 ocean.light_colour=#bfeaf8"
mkdir -p "$out/bright-calm" "$out/bright-moderate"
"$cli" ocean "$out/bright-calm" bright-calm coast $C ocean.wave_height=2.5 ocean.peak_distance=56 ocean.irregularity=0.2 ocean.swell_detail=0.2 ocean.crest_sharpness=0.15 > /dev/null
"$cli" ocean "$out/bright-moderate" bright-moderate coast $C ocean.wave_height=7 ocean.peak_distance=44 ocean.irregularity=0.6 ocean.swell_detail=0.5 > /dev/null
if [ "$1" = "update" ]; then
  for d in "$here"/baseline/*/; do rm -r -- "$d"; done
  (cd "$out" && find . -name "*.png" -exec cp --parents {} "../../$here/baseline/" \;)
  echo "baseline updated: $(find "$here/baseline" -name '*.png' | wc -l) pictures"
  exit 0
fi
targets=""
for n in open-calm open-moderate open-storm coast-calm coast-moderate coast-storm cliffs-calm cliffs-moderate cliffs-storm bright-calm bright-moderate; do targets="$targets $out/$n/$n-target.png"; done
python -X utf8 "$here/colour_error.py" $targets
python -X utf8 "$here/same.py" "$out" "$here/baseline"
