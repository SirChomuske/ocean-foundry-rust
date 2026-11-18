# NOW — 2026-11-19 05:30 (Pacific/Auckland)

## 用户当前要求（按优先级）
1. ~~岸影射线扇硬边 / 水面细直划痕~~ — **已修，用户确认**（「the last one from cmp3 looks very good」）。
2. ~~顶部按钮看不清~~ — **已改成不透明**（用户 2026-11-19 05:20 要求「直接让按钮背景不透明」）：`crates/of-app/src/app.rs` 的 `top_bar()` 进入时把 `visuals.widgets` 的 inactive/open/noninteractive/hovered/active 全换成 `theme::bar_control()`（Dark `#3d3d43` / hover `#4a4a51` / active `#56565e`；Light `#d9d9df` / `#cdcdd5` / `#c0c0ca`），不再依赖半透明 `control`；对照图 `target/tmp/bar-cmp.png`，已进 dev 包。
3. ~~尾迹尺寸可调 `ocean.wake_scale`~~ — **已做**。
4. 高流速白浪过多 — **已定位+已调，力度有限**，见下「NOW#4 结论」。待用户判断够不够。
5. ~~波场大片斜条带~~ — **已做域扭曲**（`waves.wander`），2048 真实场景直条纹明显打散、代价≈0；见下「NOW#5」。
6. 绕流六项 → 新模块 `crates/of-ocean/src/circ.rs`。**六项全做完**：① 通道挤压 + 泡沫拉长（`squeeze`/`squeeze_reach`/`foam_stretch`，squeeze 默认 1.0）；② 驻点/迎流面压缩（`stagnation`=0.5）；③ 各向异性扩散 + ④ 尾迹混合（`mix`=0.5 / `mix_spread`=3）；⑤ 窄口抬水（`squeeze_lift`=0.5，只作用在离岸 3×`squeeze_reach` 内）。
7. ~~参考隔壁 Ocean Foundry V2~~ — **用户 2026-11-19 02:40 拍板：不看 V2**（Plan §1.7 / HANDOFF 禁令保留）。按旧笔记结论自己做。
8. ~~`of-cli pair` 的 `Volume::set` bug~~ — **已修**（`Region` 用世界坐标读、写入却是零基坐标；改成 `cells.set([x, y, z], id)`，新增测试 `a_pair_keeps_every_block_it_was_given`，`pair 20 40` 4851 → 13430 块）。

## NOW#5 域扭曲（已做，2026-11-19）
- `crates/of-ocean/src/waves.rs`：新增 `bent(x,z)`，用两路 2 阶 fbm 扭求值坐标（`wander`=0.35 峰值波长，`wander_size`=1.6 波长）；`sum()` 与 `steepness()` 都先 `bent`。
- `crates/of-ocean/src/tuning.rs` `WaveTuning`：`wander: 0.35`、`wander_size: 1.6`；`assets/TUNING.txt` 已写两条说明。
- 依据：旧笔记 `PROJECT_CONTEXT.md:36`（multiscale domain warping）、`:624`（"global bands/light can yield streaks. Recommend local direction/group-length variation and independent multiscale detail, not white noise."）。
- 代价：1024² 14.3 s vs 14.0 s（噪声内），512² 3.3/3.5 s ⇒ 免费。（2048 那次 125 s 是同时在跑测试，别当基线。）
- 观感：`target/shots/wander-cmp.png`（512 左关右开）、`target/shots/wander-2048-cmp.png`（2048 左旧右新）。2048 target 平均差 24.03。
- 四条测试随之调整（都是「浪会弯」带来的抽样问题，不是功能回退）：crest 长度那条用 `WaveTuning{wander:0}` 隔离光谱；`calm_water` 把对照窗口放大到 44..84；`an_island_shelters` 阈值 0.45→0.5（实测 0.456）；`a_cliff_throws` 采样带 150..190 → 140..200。

## NOW#1 结论（已修，用户确认）
细直划痕 = **反射（echo）按百分比位移采样回来的远处波场**，被镜面层以全空间频率画出来；`reflection.cliff/rocky/beach=0` 时全部消失。
- 最终修法：`reflection.reach 0.8 → 0.25`（反射只在离岸约 1/4 波长内），`reflection.soften = 3.0`（把 echo 高度增量按 N 格盒式平摊，`build.rs` fill 后 blur，`heights` 4 元组携带 echo 增量），`survey.rs` 的 `mirror`/`firm` 3 格平滑。
- 试过但无效（已撤）：只平滑镜面坡度（`mirror_slope`，2048² target 平均差 0.41）、加大平摊半径（spread8 vs spread3 差 0.313）。
- 度量教训：hairscore 会被白浪 speckle 污染，2048² 上对 sheen/soften 不敏感，**只能靠 1:1 裁图目视**。
- 三处 TEMP-DUMP（`OF_DUMP`/`OF_ECHO_DUMP`）已全部删除，`grep TEMP-DUMP|OF_DUMP` 为空。

## NOW#6 绕流（进行中）
- `crates/of-ocean/src/circ.rs`：`Circ::new(row, rows, land, clearance, toward, tuning)` 复用 `survey::distances` 的 `away` 当 clearance。
  - 挤压：`sides = 2*reach + 两侧 clearance`，`open` = 沿流向 2R/8R/18R 取最宽的同样式子，`faster = (open/sides).clamp(1, 1+most)`（most=1.5），`speed = 1 + squeeze*(faster-1)`。
  - 驻点：`soon` = 沿流向逐格取 `min(clearance)`（初值 +inf），**只有 `soon <= reach`**（陆地够近）才算 `blocked = ((here - soon)/here.max(1)).clamp(0,1)`；`speed_up *= (1-blocked)`，`runs *= 1 - stagnation*blocked`。
  - 收尾 `soften(&mut speed, row, rows, 2)` 去格边。
  - `Circ::draw_out` 反向 5 次取样把泡沫拉长；`Flow::squeezed()` 目前仍只返回 `room.speed()`。
- 顺手修掉一个潜在 bug：`Circ::speed()` 之前 `(x+1, z+1)` 索引而数组是 `row*rows`，读数整体偏一格；现在直接按下标。
- 测试 38 passed（新增 `the_current_is_stopped_where_it_runs_into_a_face`，另加 `wall()` helper）。
- ③④ `Circ::mix` 沿垂直流向按 1/(1+|d|) 权重混两遍；⑤ `Flow::new` 在 lift 夹紧后加 `head*squeeze_lift*(room.at(i)-1)*gate`，gate 由到岸距离决定（开阔海不抬）。
- 实测（512 用户场景，squeeze_lift 0 vs 0.5）：顶高平均 +0.26 格、51448 格变化、集中在障碍物周围一圈；目标色平均差 0.96（细节级）。`cargo test -p of-ocean` 40 passed，全量 0 failed，回归 41/41。

## NOW#3 `ocean.wake_scale`（已做）
- `params.rs`: `float(0.5, 3.0, 1.0, 1)`，排在 `ocean.wake_length` 之后；`build.rs` `Look.wake_scale`；`flow.rs` `Flow::new(..., wake_length, wake_scale, ...)`，`size=clamp(0.25,4)`，`life *= size`，`slack_spread*size`。
- 双语 label「Wake size」/「尾迹尺寸」+ help；`tests/golden/registry.txt` 已登记。
- 测试 `the_wake_grows_with_its_size`：big/normal 实测 0.599 vs 0.517；big.wake(170,100) 0.0411 vs 0.0093（横向摊开），lift/strike 不变。
- 512 真实场景：foam 均值 21.27→22.38（wake_scale 3），烘焙时间不变（3.5→3.7 s）。

## NOW#4 高流速白浪（已调，力度有限）
- 新增 `FlowTuning::impact_bonus = 0.2`（原硬编码 0.35 的岸线冲击加成）、`FlowTuning::fast_cap = 0.35`（快流下冲击面与拖尾白沫的封顶下调，`fast = smoothstep(foam_from, 3*foam_from, speed)`）。
- `build.rs`：flow 段外先算 `fast`/`fast_cap`；`face = impact_foam*2*fast_cap`；拖尾 `cap = spent.most*fast_cap`；岸线 `shore_foam *= (1 + impact_bonus*fast*push) * lerp(...)`。
- 实测（512 真实场景，flow 2.6）：foam 均值 21.09→19.20（−9%），white>200 19330→17266（−11%）；`target/tmp/foam-fastcap.png` 左旧右新，船体白边变薄。
- **归因**：`flow_speed=0` 对比 white>200 15840 vs 19330 ⇒ 流只贡献 18% 的白；`impact_foam=0` 砍掉 41%，`wake_foam=0` 无影响 ⇒ 白沫主量是浪打碎（steepness 触发 breaking），流的部分集中在船体/岸线一圈细白边。想再明显减少，需要动 `whitecaps`/`foam_amount` 的全局关系，或改「快流时降 breaking 灵敏度」——要用户拍板。验收判据引 Plan M6：「maximum wind/current/shallow settings do not whiten the scene」。
- `assets/TUNING.txt` flow 段已补 `impact_bonus`/`fast_cap`。

## 旧交接清理（已完成 2026-11-19 07:0x）
- 拆分：`crates/of-ocean/src/build.rs` 1938 → **1521 行**。
  - `crates/of-ocean/src/look.rs`（137 行）：`Look`、`Dither`、`impl Default for Look`。
  - `crates/of-ocean/src/settings.rs`（168 行）：`OceanSettings`、`Seabed`、`impl OceanSettings`（含 `from_params`）。
  - `crates/of-ocean/src/baked.rs`（127 行）：`Stats`、`Baked`、`Maps`、`impl Baked`、`DRY`、`MOST_HEIGHT`、`height_reached`、`tall_enough`。
- `lib.rs` 加 `pub mod look; pub mod settings; pub mod baked;`，`pub use look::{Dither, Look}; pub use settings::{OceanSettings, Seabed}; pub use baked::{Baked, Stats, DRY};`；`build.rs` 顶部 `pub use crate::baked::{...}` 保住旧的 `crate::build::Baked` 路径。
- 无用 pub fn：全仓库扫过，没有「完全无引用」的；`blocks::runs_at`、`flow::churn`、`circ::speed` 只被测试用 → 降为私有 + `#[cfg(test)]`。
- 结果：`cargo build --release -p of-ocean` **0 warning**；`cargo test --release --workspace --features of-app/dev,of-cli/dev` 0 failed；回归 41/41；`scripts/dev-build.ps1` 已刷新 dist（06:59）。

## 数字
- 2048² 真实 build 烘焙：旧版 98 s → STEP3+jitter 131 s → rays10 131 s（出图 114 s）。域扭曲后 1024² 14.3 s（无扭曲 14.0 s）；挤压/驻点几乎零代价（2048 关 115.7 s / 开 114.8 s）。
- `cargo test --release --workspace --features of-app/dev,of-cli/dev`：**0 failed**（of-ocean 40 passed）。
- 回归：**41/41 same**（基线已于 2026-11-19 03:0x 全部重录，用户同意）。
- 2048×1024 完整导出（用户场景）：用户 2026-11-19 05:20 同意 `-UserApproved`，正在 `target/renders/user2048` 跑（cap 10000 MB）。
- `dist/OceanFoundry-dev` 已刷新到 2026-11-19 05:25。

## 命令
- 构建：`RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p of-cli --features of-cli/dev`（app 去掉 `--features`）
- 烘焙：`target/release/of-cli.exe ocean target/renders/<dir> wake build=target/renders/build.litematic waterline=0.09 $(cat target/tmp/real-args.txt)`
- 回归：先构建 dev CLI，再 `bash scripts/regression/run.sh`
- 一律经 `scripts/capped-run.ps1`；后台烘焙时不要重建（exe 被占用）。
- 诊断工具 `target/tmp/fielddump/`（独立 crate，dump top/depth/foam/sheen/target，512² 约 3.5 s）。
- **参数顺序**：`ocean.*=value` 必须写在 `$(cat target/tmp/real-args.txt)` **之后**，否则被覆盖（已踩两次）。
