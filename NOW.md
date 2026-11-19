# NOW — 2026-11-19 07:10 (Pacific/Auckland)

> 交接看 [HANDOFF.md](HANDOFF.md)（当前状态、规则、代码地图、坑）；本文只是这一轮的施工日志。
> 2026-11-17 的详细旧交接在 [HANDOFF-HISTORY.md](HANDOFF-HISTORY.md)。

## 2026-11-19 07:2x 验收包刷新（本轮）
- 发现：`target/renders/user2048-complete/` 那套验收文件是 05:55 用 05:25 的 dev 构建出的，而 `circ.rs`/tuning 的最后改动在 06:22 之后 ⇒ 比代码旧一版。
- 用 06:58 的 dev CLI 重跑同一命令（原参数原样，`-CapMB 8192`）：215.8 s、峰值 4221 MB、**35,631,674** 方块（旧 35,628,610，+3064 ≈ 0.009%），白水 26.2% 不变。
- 新旧 target 图差异只集中在建筑轮廓外一圈细边（delta 热图 `target/tmp/export-delta-heat.png`），符合 `squeeze_lift`/`mix` 的局部作用范围，海面大形未变。
- 结构校验（自写 NBT 解析，不走 CLI）：`wake.litematic` region 2048×824×1024、调色板 369 态、BlockStates 长度 = ceil(volume×9/64) 完全一致；water-only 2048×54×1024、12 态、4 bit 一致。
- `of-cli roundtrip` 对该文件不可用：35.6M 方块需要 >8 GB（先 4096 失败于 1.94 GB 分配，再 8192 失败于 6.71 GB 分配），是工具上限，不是文件问题。
- NOW#4 白沫仍按用户「保持现状」不动；2048 验收仍等用户进游戏看。

## 2026-11-19 08:2x 白沫「不该纯白」调研（待用户拍板）
- 用户：「白沫就算是这个速度也不应该纯白吧，去找几篇论文看一下，或者找现实的游戏做水面效果案例」（附 2048 俯视图）。
- 文献：Dierssen 2019（Front. Earth Sci. 7:14, doi 10.3389/feart.2019.00014）白沫可见光反照率 ≈ **40%**（不是 1）；Monahan & O'Muircheartaigh 1980（JPO 10:2094）W=2.95e-6·U^3.52 → 10/15/20 m/s ≈ **1.0/3.9/10.5%**；Callaghan 2008（ACP 16:13725 引用）0.8% @10、5.1% @20。我们 26% 白水对应 wave_height=40 格×0.5 m=**20 m 浪**，覆盖率量级自洽。
- 实拍对照：`target/tmp/ref/adriatic.jpg` 泡沫区（裁剪 250,430,1600,1250）min 通道 p50=**170**/255、≥245 占 **0.0%**、整体蓝灰 (168,179,195)；当前渲染 `wake-map.png` p50=**229**、≥245 占 **4.0%**，且 p50=p90=229（一片死平）。
- 根因：`crates/of-ocean/src/build.rs` 泡沫只有一行 `lab = colour::mix(lab, foam_lab, white)`，**不参与光照/坡面/阴影**，是平涂层；而调色板里白只有离散几档（snow 249 / quartz 236 / powder 226 / calcite 225 / white_concrete 207），一降就整片变灰。
- 已试四方案（每种都 2048 全量渲染 + 拼图，见 `target/tmp/foam-montage2-map.png`、`foam-montage2-3d.png`）：
  - **B** 只换色 `#DBE6EC`（`target/renders/foam-b/`）：均值 203，观感变**淡蓝冰面**，更糟。
  - **F** 受光 shade .85/form .30（`foam-f/`）：3D 有起伏，俯视图出现**银灰绸缎**。
  - **E** 受光 .72/.90（`foam-e/`）：明暗更重，像**金属/冰面**，小浪花变灰疙瘩。
  - **G** 只降白 `foam_most .78`（`foam-g/`）：俯视图几乎无变化，3D 略淡。
- 结论：**色调微调救不了实心白板**；建议结构改法——保持 26% 覆盖不变，把白沫打散成「沿流向的白色网络 + 蓝孔」并略降峰值白度。
- 代码实验已 stash：`git stash@{0} "foam lighting experiment (unshipped)"`（TUNING_VERSION 6 + foam_shade/foam_form + lit 混色）；**工作树与 dist 都没动**，等用户选方向再落地。

## 2026-11-19 08:4x-10:0x 白沫结构改法 / V 型打磨 / 浪花（本轮，已提交）
- **白沫长条孔**（用户 08:3x 选「结构改法」：白网络+蓝孔，保覆盖）：`ColourTuning.foam_net`=0.85、`foam_net_size`=[13,7]；`build.rs` 在 spray 提白后用 `flow.fresh()` 门控（越靠障碍物孔越少，符合用户「靠近障碍物没多少破碎孔」），沿流向把格子拉长 3.2×，孔里只削 72% 白（留漂白浅蓝）。渲染 H 版 `target/renders/foam-h/foamH-*`（196.3 s、4220 MB、51 种方块）。
- **V 型尾迹**（用户 08:5x「V 型打磨可以开始了」）：根因是 `flow.rs` 里 `smoothstep(0, 0.08, inside)` 只过渡 ~1.2°，以及 `wave_reach=4` 让发散波衰减 512 格。新增 `FlowTuning.wedge_soft`=0.35、`fan_reach`=1.6（另有试验档 0.6/1.2、0.7/0.9 走 `tuning=` JSON 免编译）。**定档 d：wedge_soft 0.7 / fan_reach 0.9 / wave_reach 2.2**（`target/tmp/tune-d.json`），球体图 `target/renders/vwake-new-b1/`，对照 `target/tmp/vwake-actual4.png`、`vw-diffstrip.png`；2048 三连差异（>8）：旧↔新默认 7.91%、旧↔软档 8.95%、新默认↔软档 2.13%。
- **浪花**（用户 09:2x「浪花也算进去」）：`FlowTuning.splash_height`=6.0；`build.rs` 用 `f.strike()`（上游堆积场）× 快流 × fbm 颗粒决定抛水，`climb` 抬水面，另有 28% 概率的 `spout` 水柱（`leap` ≤5 格）。**TUNING_VERSION 6→7**。渲染 `target/renders/splash-2048/`（249.1 s、4219 MB，water-only +4,330 方块）、球体 `splash-ball2/`。
- **位置争议（未改代码）**：用户 09:4x 斥「水花放到了尾迹一侧」。实测（`target/tmp/fielddump`，`tuning={"flow":{"splash_height":0}}` 对比 top 场）：球体 726 格抬高里**迎流 670 / 侧面 56 / 背流 0**，2048 场景 39,945 格里 18,893 迎流 vs 4,918 背流 ⇒ 几何位置本身对。真正原因是 `of-cli/src/ocean.rs:785` 的 map 是 north-up、3D 视图「seen from the south」，相机在背流侧；加上 build 多孔（水穿拱门），内面迎流从相机看像背面。
- **GTA V 调研**（用户 09:4x「have a look at how GTA5 render their ocean waves and collision physics」）：水面=解析式逐帧波面，`GET_WATER_HEIGHT` 含波动逐帧变、`GET_WATER_HEIGHT_NO_WAVES` 稳定、`SET_DEEP_OCEAN_SCALER` 风暴只在玩家 ~200 m 内、`ADD_EXTRA_CALMING_QUAD` 最多 8 块平静水域；浪不与几何碰撞，浮力靠探针采样，飞溅/尾迹由实体粒子+路径贴花。已给用户 4 步「浪面驱动 splash」改法，**用户回「never mind, seems like it doesnt apply that much」→ 不改，按现状收尾**。
- **验收包**：10:07 用最终 dev 构建重跑同一命令（`-CapMB 8192`）：267.2 s、峰值 4218 MB、**35,634,909** 方块（07:14 那版 35,631,674，+3235），water-only 10,920,113，51 种方块；日志 `20261119-100729-357-export-complete3`，产物仍在 `target/renders/user2048-complete/`。
- **收尾**：`cargo test --release --workspace --features of-app/dev,of-cli/dev` 0 failed；回归先测到 **11/41 same**（30 张随白沫/V/浪花变化），按用户 08:5x「可以重录」重录基线（`run.sh update`，41 张），再跑 **41/41 same**（日志 `20261119-100051-413-regression-verify`）；`scripts/dev-build.ps1` 刷新 `dist/OceanFoundry-dev` 10:05；提交 **1b35443**「Cut thick foam into plates, soften the wake's far edges and throw water up a struck face」。

## 2026-11-19 10:4x Build 背后的直缝（Height 视图红圈）
- 现象：福尔图恩庄园场景 Height 视图里，build 左下方一条淡淡的直线一直拉到画布边缘；V 型的臂不随距离化开。
- 复现：`dist/OceanFoundry-dev/OceanFoundry-data/` 复制到 `target/tmp/line-scene/`，dev 包 headless 截图（`--show height --bake --screenshot ... --set ocean.width=768 --set ocean.length=1024`）。模板见 HANDOFF §6。
- 根因：`crates/of-ocean/src/shore.rs` 的 arrival lag（绕行陆地的到达延迟）沿下游原样携带。波浪振幅会衰减，**相位位移不会**；在遮蔽边界处 lag 归零就成了阶跃，于是一条直缝贯穿到图边。排除项：`shelter_rays=90`、`shelter_reach=2.0` 都不影响长线；把 lag 临时置 0 重建后长线全消失 → 确认是 lag 相位缝。
- 修法：lag 按离陆地距离在 3 个峰值波长内指数松开（`exp(-from_land/relax)`），再按 `peak_length/10`（1–8 格）做一次半径模糊（`blurred`，shore.rs:511）。注释写明了理由：相位位移不会随振幅衰减，不松开就永远是直缝。
- 结果：`target/tmp/line-relax.png`；对比 `target/tmp/line-relax-compare.png`（baseline / relaxed / lag off 三联）、`target/tmp/line-near-compare.png`（近场：build 背后的阴影与扰动保留）。
- 回归 **41/41 same**（回归场景没有 build 阴影，lag 恒 0，所以基线不用重录）；日志 `test-logs/runs/20261119-104619-543-lag-regression`。

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

## 2026-11-19 13:0x 白沫物理：覆盖率跟风速吗（本轮，未改代码）

用户问「白沫覆盖率跟的是风速 / 这个确认了嘛 / 涌浪也会造成白沫，但规模要够大速度够快 / 多找论文 / 我说的可能是错的，按流体力学来」。

- 文献结论：W 与 U10 的强经验关系确认（Monahan & O'Muircheartaigh 1980：W=2.95e-6 U10^3.52，10/15/20 m/s ≈ 1/3.9/10.5%；Callaghan 2008 加风历史；Albert 2016 仍用 U10，全球均值 ~3.4%）。但物理解释是**陡度阈值**：破碎发生在波陡接近极限（ak≈0.44，Banner & Peregrine 1993），主波陡度决定破碎概率（Banner, Babanin & Young 2000），风只是把海面抬陡；wave age 与平均波坡解释 W 变率的 80-85%（PMC6209970）。涌浪同理：只有被推到极限（极大极快、对撞流、浅滩、与风浪叠加）才碎，普通涌浪远低于阈值。
- 我们的场景（夏活群岛 1024²，wave_height 4 m，flow 0）：岛后 long 掉到 0.45-0.70、short 仍在 0.96-1.00、白沫 3.09%→0.6-0.8%。原因：阈值用**外海**的 spread 标定（steepness_deviation(ride)），涌浪被挡后陡度低于它，仍在的风浪永远不碎。
- 首个修法（局部 spread 折算阈值，按 local_deviation/open_deviation）实验：岛后 0.67%→2.77%（对），但外海 3.02%→3.62%、中段 1.7%→4.6%（过度漂白），阈值尾部太敏感。补丁 target/tmp/wind-sea-fit.patch，git stash stash@{0}，**未上线、未进 dev**。
- 设计三选一（详见 HANDOFF.md §8）：①coverage 落位 W_here=W_open×(0.25+0.75×short^1.7)（与 m02624 用户预期一致）；②局部 spread 重标定 cover_per_height/most_breaking 让外海仍 3.0%；③波影 through 随 distance 淡出（exp(-distance/(k·peak_length))，k≈3），顺带解决 V 跑到图边。
- 另：arrival_slope 只压了沿流向，横向 lag 梯度仍 p99 5.38/max 52（m02702 看到的另一种压缩），计划 arrival_side。
- HANDOFF.md 已重写 §1/§5/§6/§7，新增 §8 长文与 §9 history；本提交只动 NOW.md。dist dev 仍是 12:58 构建（2d3e2a3），不含白沫实验。

## 2026-11-20 03:0x 岛后白沫 + 浪脊密度（本轮，未提交）

- **岛后白沫**（用户 m00645「compression 修好了」之后剩下的问题）：阈值改成跟“当前海况的 spread”——`from = tail_point(most_breaking * (least + (1-least)*kept^1.76)) * local_σ`，`kept = local_σ/open_σ` 夹 [0,1]，`least` 0.1。1024² 岛场景 mask：影区 0.67%→2.02%，外海 2.68%→2.81%；场景 bake 白水 1.2%→1.7%。先试过按 `short` 幅度折算（影区 3.53% > 外海 2.87%，过白），弃。
- **压缩**：`near_land.arrival_side` 0.35 横向限坡，与 `arrival_slope` 0.12 交替 `held_back` 三次；两轴同时满足（dlag/dx p99 0.35、dlag/dz p99 0.12），长直白线车道消失。
- **密度斑块**（用户 m00950「density difference between areas」）：经他拍板选“收窄方向扩散”，新增 `waves.spread_curve` 1.35（0.62 档位 ≈ 老 0.42 的浪向集中度）。试过 1.8 → 用户「large areas of flat, no good」。
- **试过又撤掉**：`shadow_soft`（对 long/short 做掩膜 box blur，2.5λ 只有 5.9% 像素变化，肉眼无感）、`arrival_soft`（把延迟场预模糊到 1.5λ，会洗掉近岸浅滩延迟，`waves_shrink_and_turn_toward_a_beach` 在 ≥0.75 时失败；0.5 又暴露 shelter ray fan 的台阶）。两处代码均已回退，只留记录。
- **测试**：workspace 全绿（of-ocean 50）。`calm_water...` 里 open roughness 的绝对下限 1.0→0.5（方向变窄后外海起伏变小，测试真正守的是“calm < 0.2×rough”）。
- **回归 8/41 same**：spread 曲线动了所有外海画面，基线待用户同意后 `bash scripts/regression/run.sh update` 重录。
- dev build 已刷新（03:00，含 1.35）。**代码未提交。**
