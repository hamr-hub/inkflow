# AGENTS.md · inkflow

> 给 AI 协作者的项目级约定。本文件由 session-reflection 自动沉淀。
> 阅读顺序：[ARTIFACT.md](ARTIFACT.md) → [ZERO_DEP.md](ZERO_DEP.md) → [PRODUCTION.md](PRODUCTION.md) → 本文件 → [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 项目硬约束（写给所有维护者）

| 约束 | 说明 | 强制点 |
|---|---|---|
| **零第三方依赖** | `Cargo.toml` `[dependencies]` 必须空；系统调用走 `src/surface.rs` 的裸 `extern "C"` | `cargo build --release` 必须成功；CI 有一条 gate 直接读 Cargo.toml |
| **std-only build** | 无 X / Wayland / 桌面；渲染走 DRM/KMS dumb-buffer 或 fb0 | `surface.rs` 的 `ioctl` / `mmap` 声明 |
| **Linux-only 链接** | macOS dev 机器 `cargo check` 通过但 `cargo build` 会因 `__errno_location` / `inotify_*` 缺符号失败 | 真机 Jetson Orin Nano 才能 release build；CI 因此固定 ubuntu-latest |
| **每帧 < 2 ms** | 热路径零分配（背景/合成不再逐帧 `Vec`）、`present` 复用常驻 mmap | `frame_min_us` / `frame_max_us` telemetry |
| **永不黑屏** | 曲库按主题钉死整首作品，槽位到龄后按读序回填 | `phrase.rs` + `scene.rs` 的 `Composition` |
| **main.rs 只做编排** | 帧循环：advance → step → paint_background → paint_composition | 各阶段已下沉到 `rhythm.rs` / `scene.rs` / `background.rs` / `compose.rs` |
| **每个 mod < 300 行** | 抽出 = 可测、可替换、可独立 lint | `wc -l src/*.rs` 长期监控（`glyph_table.rs` 是生成产物，例外） |

## 模块归属（零依赖重写版起）

```
main.rs           → 帧循环 + --headless / --layout-test / --compose-test /
                    --works-gallery / --drm-test
scene.rs          → 槽位几何 + 生命周期 + 尘埃/火花 + LCG
background.rs     → 天空渐变 / 光晕 / 地平雾 / 暗角 / 月 / 尘 / 火花
compose.rs        → 支撑笔画 → 主句（带 bloom）→ 落款
color.rs          → 调色板 + sRGB/linear 混合算子
glyph.rs          → 桶字形面积采样（Q8 定点）+ 落笔定位
glyph_table.rs    → 编译期生成（build_font.py，NOT hand-edited）
phrase.rs         → 曲库 + 诗组（标题 / 读序行索引）
rhythm.rs         → 节拍调度：entrance → hold → exit → rest
surface.rs        → memory / /dev/fb0 / DRM dumb 三后端统一 API
png.rs            → 零依赖 PNG 编码器
lib.rs            → 模块声明
```

修改任何一个文件前先确认它的角色归属，**不要在 main.rs 里塞业务逻辑**。
历史模块（`mood.rs` / `llm_loop.rs` / `net_ollama.rs` / `renderer.rs` /
`drm.rs` / `evdev.rs` / `screenshot.rs` / `telemetry.rs` / `font.rs` /
`fontdata.rs` / `phrases_raw.rs` / `poetry.rs` / `scene_anim.rs` /
`fallback.rs` / `sys.rs`）已随零依赖重写移出 `src/`，归档在 `legacy/`。

## 公约

- **每次改动前读 ARTIFACT.md**：commit message 写「这一改如何让作品更像自己主张的样子」，不是「修了一个 bug」。
- **每次改动跑全门**：`cargo fmt && cargo clippy --release --all-targets -- -D warnings && cargo test --release && cargo build --release`。clippy lint 集合随 rust 版本变化，rust 升级后必重跑。`scripts/pre-push` 与 `.github/workflows/ci.yml` 是同一道门。
- **本地 rustc 曾比 CI 旧**（2026-09-29 起两者都是 1.98，此条仅作历史）：clippy 每个版本加新 lint，所以「本地全绿」不等于「CI 全绿」——2026-09-29 首次推 CI 就被 1.98 的 `unnecessary_cast` 打回 12 处。升级 rustc 后除了跑本地全门，最好也跑一次 `rustup run stable cargo clippy --release --all-targets -- -D warnings`。
- **改了墨迹就刷样帧**：`scripts/refresh-samples.sh`。README 里的样帧由 CI 逐字节比对，渲染器一动样帧就过期。
- **零依赖测试**：所有单元测试走 std test，不允许加测试用 deps（`#[test]` 在 std 内）。
- **改字体必须重新生成**：`python3 scripts/build_font.py` 重写 `src/glyph_table.rs`。字形度量（尤其 `bearing_y`，定义是「基线向上到墨顶」）由生成器单方面定义，改了生成器就要同步 `glyph.rs` 的消费约定。
- **autoloop**：`scripts/autoloop.sh` 周期 `git add -A && git commit`。它 `cd "$HOME/codespace/inkflow"`，工作目录不同就先改脚本或 `ln -s`。注意它会顺路 commit 任何在途改动。

---

### 🧠 Session Learning（自动沉淀，请勿手动删除）

<!-- LEARNING_START -->
| 日期 | 发现 | 来源 |
|------|------|------|
| 2026-09-24 | 本仓网络挂载 FS 不允许 `.git/index.lock`，手动 `git add` / `git commit` 必失败 "Operation not permitted"；改动必须让 autoloop 周期 commit 顺路带走 | v0.2.1 comprehensive upgrade |
| 2026-09-24 | Rust 1.97 clippy 新增三条 lint：`doc_lazy_continuation`（`///` 列表后需空行/缩进）、`wildcard_in_or_patterns`（`Foo \| _` 拆两条 arm）、`collapsible_match`（嵌套 if 用 match guard） | v0.2.1 clippy pass |
| 2026-09-24 | `f32::clamp` 自 Rust 1.50 起 std 已有，别写自己的 clamp；项目里的 `font::clamp` 是冗余的，直接 `.clamp()` 即可 | v0.2.1 font.rs cleanup |
| 2026-09-24 | 常量名 ≠ 大小关系：`LLM_BACKUP_SIZE=28` < `FALLBACK_SIZE_BUMP=32`（fallback 反向 bump 防「流缩」）；测试断言要从注释/意图反推，不要从字面推 | v0.2.1 scene_anim tests |
| 2026-09-24 | `style_for` 决策树：high energy (`>0.6`) 直接走「豪放」与 warmth 无关；只有 energy < 0.2 才按 warmth 选 婉约/禅寂；测试用例的 (warmth, energy) 配对要先 trace | v0.2.1 net_ollama tests |
| 2026-09-24 | autoloop 周期跑 `git add -A && git commit`，任何 working-tree in-flight 改动都会被顺路 commit 走；做大块改动不必担心未 commit 丢失 | v0.2.1 working tree observed |
| 2026-09-24 | 编辑 Rust 源码别走 Python heredoc：换行/转义差异导致 `s.replace(old, new)` 多次不匹配；推荐 sed 按行处理或 Read+Edit | v0.2.1 multi-retry pattern |
| 2026-09-29 | **PIL `font.getbbox()` 的 y 是从排版框顶往下的，不是从基线往上的**。旧代码按后者理解，写出 `bearing_y = -by0`（其实是笔位偏移的取负），符号和量级都错。应写 `ascent - by0`，其中 `ascent` 来自 `font.getmetrics()[0]` | bearing_y 符号 bug |
| 2026-09-29 | **面积采样的两套坐标系**：Q8 里 1 屏幕像素 = 256 单位的屏幕空间，但索引源位图时 256 单位 = 1 个*原生*源像素。降采样时必须先把边界除以 `scale_q8` 换到源空间，否则每个目标像素恒定只覆盖 1 行源像素，字形只画出自己的顶部若干行——降采样渲染出「几条横杠」而不是汉字 | 面积采样坐标系 bug |
| 2026-09-29 | 判断「渲染是否退化」不能只看输出 bbox——错误实现同样填满整个 bbox。真正暴露问题的是**纵向分布**：把墨迹高度四等分，每段都必须有像素 | 回归测试设计 |
| 2026-09-29 | 提交样帧前先确认渲染器是对的：`docs/samples/` 里的图可能是旧管线产出的，README 引用它们就会静默撒谎。CI 现在逐字节比对样帧 | samples gate |
| 2026-09-29 | rtk 的 hook 会吞掉 `ls` / `cargo test` 的部分输出（返回空）。要看原始输出用 `rtk proxy "<cmd>"`，或重定向到文件再读 | 工具输出过滤 |
| 2026-09-29 | rtk 还会**改写** `git diff` 之类的多行输出再交给管道——粘进 `git checkout -- $(...)` 的文件列表会掉首字符（`docs/...` → `ocs/...`）。凡是 git 多行输出要喂给别的命令，一律走 `rtk proxy` | 工具输出过滤 |
| 2026-09-29 | 本地 rustc 1.92 < CI 的 stable 1.98，clippy 新增的 `unnecessary_cast` 让本地全绿的代码在 CI 挂 12 处。CI 跟 `stable` 是有价值的（它抓的就是本地抓不到的），代价是升级期要重跑一次 `rustup run stable cargo clippy` | 首次 CI 跑挂 |
| 2026-09-29 | **样帧 gate 曾在 main 上红了好几轮而没人发现**：`refresh-samples.sh` 用 `cargo build` 往 `$CARGO_TARGET_DIR` 构建，却拿硬编码的 `./target/release/inkflow` 去渲染——autoloop 导出了 `CARGO_TARGET_DIR`，所以那是**另一个（陈旧的或根本不存在的）二进制**。刷新静默产出旧帧 → `git diff --cached` 为空 → 不提交样帧 → 照常 push。写脚本时凡是自己 build 再执行的，一律用同一个 `$CARGO_TARGET_DIR/release/inkflow` 解析结果，别硬编码 `./target` | samples gate 红 |
| 2026-09-29 | autoloop 推 main 前的样帧提交是 `git commit ... \|\| true` 的尽力而为，失败也照推。又是一个独立的洞：即使刷新成功，样帧 commit 挂了照样 push 出一个红 main。现在推之前跑 `scripts/verify-samples.sh`（就是 CI 那套逐字节比对），不过就不推 | push 前验证 |
| 2026-09-29 | **别用「射线取最陡曲率」的直方图当圆环检测**。我据此宣称月亮周围有 r=171 的相干硬边，其实是噪声分布的众数：逐角度半径实际跨 66–272，sd=38。改用干净单射线 + 开关对照实验（关掉 paint_moon 看还在不在）才定论 | 图像判读 |
| 2026-09-29 | 月亮那圈「盘状硬边」= **`continue` 阈值本身**。这片天空 1 个 8-bit 色阶只有 ~0.0006 线性光，而 `paint_moon` 的早退是 `a <= 0.003`、每层还有 `sky/halo/body > 0.003`——等于 3-5 个色阶的悬崖，最后一圈画完就断。**加平滑窗函数救不了**：C2 窗只是把台阶搬到 alpha 穿过阈值的那一圈（实测 2.67@r=172 → 3.00@r=155）。正解是把阈值降到半个色阶以下（0.0003）并让径向 profile 真的归零。顺带：辉光是纯径向的，每帧建一张 LUT 把每像素两次 `exp` 去掉，再按平方距离提前 reject 看不见的环——不但修好缝，还让 headless 120 帧从 7.26s 掉到 2.98s | 辉光硬边真因 |
| 2026-09-29 | 让 agent 自己 `git add src/ scripts/` 是**无法收敛的设计**：路径级 add 根本分不清「这轮我写的」和「别人早就在那的」。改成外层 loop 全权负责 staging——开跑前先 `git status --porcelain` 快照 preexisting，收尾只 stage 这轮新增变脏的文件，`state/` 一律排除；agent 改写自己的改动、把一句话理由写进 `state/art_message.txt` 供外层当 commit message。外层还要自己重跑 gate、自己判断、verify-samples 过了才 push。**谁发布谁验证** | loop 越权 |
| 2026-09-29 | `color.rs` / `png.rs` 之前是 0 测试，但每个像素都过 color、README 和样帧都由 png 生成。补测试时抓到一条假注释：`srgb_to_lin` 注释写 gamma 2.2，代码其实是 `x*x`（2.0）——两边是匹配的一对，往返精确，别真去改成 2.2（会重排整个画面）。png 用**未压缩的 stored DEFLATE**，所以测试里手写解 stored block 就能做完整往返，不用 inflate：注入「每个 block 都标 final」的 bug 后，所有单 block 用例照样全绿，只有跨 block 那个抓到——这就是之前完全没有的覆盖 | 补测试 |
| 2026-09-29 | `place_slot` 的收缩循环写死 `for _ in 0..6`，但 320px 屏上 8 个字要缩 **8** 次才塞得下，于是那行字直接跑出屏幕 23px。改成「缩到塞下或触到 14px 下限为止」。**改完必须验样帧没变**：6 张全部逐字节相同——真实构图本来第一次就塞得下，受影响的只有本来就坏的那些。写这类「保险」循环时，固定次数的保险往往就不够 | place_slot 溢出 |
| 2026-09-29 | **无人值守且会往真远端推的控制流，必须有 hermetic 自测进 gate**。loop 的 staging/判断/推送逻辑重写后，`bash -n` 和人眼看都没发现它把 commit 标题写成 `art: art: ...`——是 `scripts/autoloop-selftest.sh` 第一次真跑才暴露的。做法：临时目录 + 本地 bare origin + stub 掉 flock/timeout/cargo/claude，跑真脚本，断言五种结局（正常提交 / 纯数值微调被拒且回滚改动 / 人的在途改动不被 stage / 空转不提交 / 样帧校验不过不 push）。自测本身也要能自证没坏：先断言「被测脚本确实装进去了」且「post-turn 分支确实执行了」，否则空跑也是绿的 | loop 自测 |
| 2026-09-29 | 这个仓的 `scripts/autoloop.sh` 每 20 分钟一轮且**会跟人抢工作树**：它 `git add src/ scripts/ docs/samples/` 一把梭，所以任何放在 `scripts/` 的未提交改动都会被下一轮裹进一条 `art:` 提交里。改脚本前先想好怎么交付 | 并发写入 |
| 2026-09-30 | **重构过的 loop 第一次真跑就证明它是对的**：日志里出现 `left unstaged (pre-existing, not ours): AGENTS.md docs/...`——外层快照机制把人的在途改动挡在了提交之外，而同一轮它正常提交了 rhythm.rs 的一处真改动并刷了样帧。重构前这正是吃掉人类提交的那条路径 |
| 2026-09-30 | **`--model MiniMax-M3[1m]` 一直没生效**。每一轮日志都打 `[claude-code:unrecognized_model]`（含成功的几轮），CLI 不认这个名字，直接静默回落到默认模型。所以别把轮与轮之间的时间差、或 `Token Plan` 429 归因到脚本里写的那个模型——它压根没被选中。已抽成 `TURN_MODEL` 变量并把这件事写在脚本里，换名字后先确认警告消失 |
| 2026-09-30 | **turn 之间没有记忆，loop 会重踏**。01:41「lengthens its hold」和 02:17「lengthens its rest」隔 36 分钟各改一次 `src/rhythm.rs`，都在拉长句子之间的停顿，谁也不知道对方做过——7 次有效迭代里有 2 次花在同一个念头上。对一件写着「克制是它最贵的部分」的作品，这就是在一个维度上过饱和。现在 loop 每次成功后把 subject + 触过的文件追加到 `state/iterations.md`，prompt 要求先读它 |
| 2026-09-30 | **我给 loop 打的补丁差点让它再也不会提交**。为保护人的在途改动，我把 staging 改成「快照 preexisting，只 stage 本轮新增变脏的文件」——但这个卷把每个文件都报成 755，于是快照里 83 个路径全是 preexisting（本该 21 个），turn 改的那个文件自然也在里面，于是被判成「本轮没有改动」而丢弃。日志只有一行 `turn produced no changes of its own`，白白扔掉一次改了字幕 alpha 0.62→0.30 的好 turn。三处 `git status` 都加了 `-c core.fileMode=false`。教训：**按「脏」判断归属之前，先确认这个文件系统到底什么叫脏**；自测也要 churn turn 自己的目标文件，churn 邻居是测不出来的 |
| 2026-09-30 | **被超时杀掉的轮次会永久毒化那个文件**。turn 中途被 `timeout` 杀，工作树里留着写了一半的改动；下一轮开跑时它已经是 preexisting，于是外层按设计拒绝 stage 那个文件——一轮失败就能让某条路径再也提交不了。现在 RC != 0 时按本轮快照回滚（只回滚本轮弄脏的，别人原有的不动）。发现路径：给 self-test 加「超时」场景时，紧随其后的场景被污染而失败 |
| 2026-09-30 | **定时器的空闲时间就是浪费的产能**。调度约 21 分钟一轮，而 `timeout 780` 只给 13 分钟，每轮白扔 8 分钟；`claude_rc=124`（中途被杀）和 `rc=0` 一样常见。提到 1080（留 3 分钟余量），并抽成 `TURN_TIMEOUT`。`flock` 会在重叠时跳过而不是搞坏工作树，所以宁可贴着节奏上限设 |
| 2026-09-30 | **`.git` 写不了是沙箱策略，不是盘的问题**。曾怀疑 `/Volumes/ssd` 掉盘导致 `index.lock: Operation not permitted`，掉盘后复验：盘回来、`.git` 依旧只读。排查环境问题别把「权限」和「硬件」混为一谈，先各自单独复验 |
| 2026-09-29 | `scripts/autoloop.sh` 会在你没提交时把在途改动 commit 走**并 push**。本轮它把两个 commit 的内容一次性吞掉，其中一个描述完全不符实；只能在 push 之后用 `git diff A B > delta` + `git checkout B -- files` 在远端 commit 之上补一个 delta commit 来救（非 force 路线） | autoloop 抢跑 |
<!-- LEARNING_END -->

---

## 历史教训（详细版，仅作上下文）

### L1: 字形度量的契约横跨生成器与渲染器

`glyph_table.rs` 由 `scripts/build_font.py` 生成，`glyph.rs` 消费。两边对
`bearing_y` 的理解必须一致：**正值 = 墨顶在基线之上多少像素**。

```rust
// glyph.rs —— 墨顶 = 基线 - bearing * scale
let bbox_y_q8 = fy as i64 - g.bearing_y as i64 * mul;
```

```python
# build_font.py —— 墨顶相对基线的高度
bearing_y = ascent - by0   # ascent 来自 pil_font.getmetrics()[0]
```

PIL 的 `font.getbbox()` 返回的 y 是**从排版框顶往下**的量。把它当成「从基线
往上」就会写出 `bearing_y = -by0`——符号反、量级也错（拿到的是笔位偏移而
不是墨高）。后果是每个字都被画到基线*下面*，主句因为只偏移不裁切而"看起来
没事"，支撑笔画则整段错位。

**教训**：生成器里的坐标约定要在注释里写清是哪个库、哪个方向，别只写一句
"y grows UP"——那正是当初写错的原因。

### L2: 面积采样的两套坐标系

Q8 定点里 `256 = 1 像素`，但**哪个像素**是两回事：目标缓冲区里是屏幕像素，
`HERO_DATA` 里是原生 128px em 的源像素。降采样时一个屏幕像素覆盖
`256 * 256 / scale_q8` 个源 Q8 单位，所以边界必须先除以 `scale_q8` 换到源
空间，才能再除 256 去索引源行。

漏掉这一步的后果很隐蔽：目标 bbox 依然是满的（所以看尺寸看不出来），但每个
目标像素恒定只取 1 行源像素，于是只画出了字形顶部 `h * scale / 256` 行——
降采样路径上所有汉字都变成「几条横杠」。

现在的写法把边界保持成对 `mul` 的精确分子，每条边只除一次，误差小于 1/256
个源像素：

```rust
const DEST_STEP_NUM: i64 = 256 * 256;      // 每目标像素的分子增量
let num_x = -bbox_x_q8 * 256;               // 目标像素 0 对应的分子
let src_x0 = ((num_x + sx as i64 * DEST_STEP_NUM) / mul).max(0);
```

### L3: 退化渲染的判据

验证"画对了"不能只看输出的包围盒——错误的实现同样填满包围盒。可用的判据：

- **纵向分布**：把墨迹高度四等分，每段都得有像素（`scaled_glyphs_render_their_full_extent`）。
- **覆盖率**：`lit / (w * h)`。但注意小字号下复杂汉字覆盖率会很高（19px 的
  `言` 能到 0.7），覆盖率只适合抓量级异常，不适合当上下界断言。
- **基线关系**：CJK 会合法地探到基线以下（`言` 探出约 11px），别断言
  `ink_bottom < baseline`。

### L4: build / test 分离

- `cargo check --release`：macOS dev 机器通过（只编不链）
- `cargo build --release`：~~macOS 误链（缺 Linux extern）~~ ——**此条已过期**。
  零依赖重写之后 macOS 能正常链接并产出可执行文件（2026-09-29 复验），本地全门
  可以跑满四步。CI 仍固定 `ubuntu-latest`，因为那才是真机部署目标。
- `cargo test --release`：任何平台通过（单元测试不碰 Linux syscall）
- `cargo clippy --release --all-targets -- -D warnings`：通过

曾经代码 review 时要提防 macOS `cargo build` 误链报错；现在本地四步都能跑通，
CI 固定 `ubuntu-latest` 只是因为那才是部署目标。
