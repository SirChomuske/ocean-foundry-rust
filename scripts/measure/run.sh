# Stage 1 of the column bake plan: measure what today's code holds in memory.
# Every case is its own capped run (scripts/capped-run.ps1), smallest first.
# A series stops at the first case that hits its cap, so nothing grows past it.
# Why: the later stages are judged against these numbers; and an unbounded
# run once filled the PC's memory.
#
# From the project folder:  bash scripts/measure/run.sh [quick|figures]
# quick: the 256 cases only; figures: only the tall figure cases.
# Results: target/measure/<case>/ and test-logs/measure/table.md
cd "$(dirname "$0")/../.."
cli=./target/release/of-cli.exe
out=target/measure
mkdir -p "$out" test-logs/measure
: > test-logs/measure/cases.txt
storm="ocean.wave_height=13 ocean.peak_distance=120 ocean.irregularity=0.85 ocean.swell_detail=0.6 ocean.crest_sharpness=0.8"
calm="ocean.wave_height=2.5 ocean.peak_distance=56 ocean.irregularity=0.2 ocean.swell_detail=0.2 ocean.crest_sharpness=0.15"

# case <name> <cap MB> <args...>; returns the runner's exit code
case_run() {
  name=$1; cap=$2; shift 2
  mkdir -p "$out/$name"
  pwsh -NoProfile -File scripts/capped-run.ps1 -CapMB "$cap" -TimeoutS 1200 -Name "measure-$name" -- "$@" > "$out/$name/runner.txt" 2>&1
  code=$?
  echo "$name $code $(ls -d test-logs/runs/*-measure-$name | tail -1)" >> test-logs/measure/cases.txt
  tail -1 "$out/$name/runner.txt"
  return $code
}

sizes="256 512 1024"
[ "$1" = "quick" ] && sizes="256"
[ "$1" = "figures" ] && sizes=""

# Generated land (test scenes), calm and storm, growing plots.
for land in open coast cliffs; do
  for sea in calm storm; do
    eval "sea_args=\$$sea"
    for n in $sizes; do
      [ "$land" = open ] && l="" || l=$land
      cap=$(( n <= 256 ? 1024 : n <= 512 ? 2048 : 4096 ))
      case_run "$land-$sea-$n" $cap $cli ocean "$out/$land-$sea-$n" "$land-$sea-$n" $l memory=report ocean.width=$n ocean.length=$n $sea_args || break
    done
  done
done

# A tall hollow figure as the land, waterline 20%, storm: the statue stand-in.
# Its height grows with the plot (it is about 1.15 x its height long).
for pair in "256:200" "512:256" "512:420" "1024:420" "1024:860"; do
  n=${pair%%:*}; h=${pair##*:}
  [ "$1" = "quick" ] && [ "$n" != 256 ] && continue
  fig="$out/figure-$h.litematic"
  [ -f "$fig" ] || pwsh -NoProfile -File scripts/capped-run.ps1 -CapMB 4096 -TimeoutS 1200 -Name "measure-figure-$h" -- $cli testbuild "$fig" $h | tail -1
  cap=$(( n <= 256 ? 2048 : 4096 ))
  case_run "figure$h-storm-$n" $cap $cli ocean "$out/figure$h-storm-$n" "figure$h-storm-$n" memory=report build="$fig" waterline=0.2 ocean.width=$n ocean.length=$n $storm || break
done

python -X utf8 scripts/measure/table.py test-logs/measure/cases.txt > test-logs/measure/table.md
echo "table: test-logs/measure/table.md"
