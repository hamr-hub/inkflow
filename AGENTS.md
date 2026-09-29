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
- `cargo build --release`：macOS 误链（缺 Linux extern）；**真机 Linux/aarch64 才走**
- `cargo test --release`：任何平台通过（单元测试不碰 Linux syscall）
- `cargo clippy --release --all-targets -- -D warnings`：通过

代码 review 时不要被 macOS `cargo build` 报错吓到，那是预期的。CI 因此固定
`ubuntu-latest`——只有 Linux 能真正链接。
