# AGENTS.md · inkflow

> 给 AI 协作者的项目级约定。本文件由 session-reflection 自动沉淀。
> 阅读顺序：[ARTIFACT.md](ARTIFACT.md) → [ZERO_DEP.md](ZERO_DEP.md) → [PRODUCTION.md](PRODUCTION.md) → 本文件 → [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 项目硬约束（写给所有维护者）

| 约束 | 说明 | 强制点 |
|---|---|---|
| **零第三方依赖** | `Cargo.toml` `[dependencies]` 必须空；系统调用走 `src/sys.rs` 的裸 `extern "C"` | `cargo build --release` 必须成功 |
| **std-only build** | 无 X / Wayland / 桌面；渲染走 DRM/KMS dumb-buffer | `drm.rs` + `evdev.rs` + `sys.rs` |
| **Linux-only 链接** | macOS dev 机器 `cargo check` 通过但 `cargo build` 会因 `__errno_location` / `inotify_*` 缺符号失败 | 真机 Jetson Orin Nano 才能 release build |
| **每帧 < 2 ms** | `pixels.fill(BACKGROUND)`、`VecDeque` 上限守恒、PPM/PNG 异步 | `frame_min_us` / `frame_max_us` telemetry |
| **永不黑屏** | ollama 4s 退避 → 本地词库接管（永不空） | `net_ollama.rs` + `fallback.rs` |
| **main.rs 只做编排** | 帧循环 6 阶段：mood → publish → spawn → draw → telemetry → screenshot | 各阶段已下沉到 `mood.rs` / `llm_loop.rs` / `scene_anim.rs` / `renderer.rs` / `telemetry.rs` / `screenshot.rs` |
| **每个 mod < 300 行** | 抽出 = 可测、可替换、可独立 lint | `wc -l src/*.rs` 长期监控 |

## 模块归属（v0.2.1 起）

```
main.rs           → 帧循环 + --diag / --drm-test
fallback.rs       → 静态词库 + warm/cool picker（zero-alloc）
mood.rs           → touch → (warmth, energy, idle) 信封
llm_loop.rs       → LLM worker + Shared queue + 5 风格
scene_anim.rs     → spawn（glyphs + particles）
renderer.rs       → draw（clear → nebula → stars → particles → glyphs → fog）
surface.rs        → DRM / fb0 / Headless 统一 API
screenshot.rs     → 60s PPM/PNG 异步抓帧
drm.rs            → DRM/KMS dumb-buffer ioctl 编码
net_ollama.rs     → 手写 TCP + HTTP/1.1 + JSON scanner
font.rs           → 软件光栅化 + Rgba + char_key
evdev.rs          → touch hotplug + mouse-as-touch
sys.rs            → extern "C" syscall 入口
telemetry.rs      → JSONL writer + 2 MiB rotate
scene.rs          → VecDeque 池 + LCG
fontdata.rs       → 编译期生成（NOT in git）
```

修改任何一个文件前先确认它的角色归属，**不要在 main.rs 里塞业务逻辑**。

## 公约

- **autoloop 接管 commit**：本仓文件系统不允许 `.git/index.lock`（"Operation not permitted"），手动 `git commit` 必失败。改完放 working tree 让 `scripts/autoloop.sh` 的周期 `git add -A && git commit` 顺路带走。
- **autoloop.sh 路径硬编码**：`cd "$HOME/codespace/inkflow"`。当前工作目录 `/Volumes/ssd/codespace/personal/inkflow` 不是默认路径，要让 autoloop 接管须先 `ln -s` 或修改脚本。
- **每次改动前读 ARTIFACT.md**：commit message 写「这一改如何让作品更像自己主张的样子」，不是「修了一个 bug」。
- **每次改动跑全门**：`cargo fmt && cargo clippy --release -- -D warnings && cargo build --release`。clippy lint 集合随 rust 版本变化，rust 升级后必重跑。
- **零依赖测试**：所有单元测试走 std test，不允许加测试用 deps（`#[test]` 在 std 内）。

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
<!-- LEARNING_END -->

---

## 历史教训（详细版，仅作上下文）

### L1: drm.rs `Display::drop` 的 `restored saved_crtc` 风险

```rust
// 当前实现（v0.2.1）
impl Drop for Display {
    fn drop(&mut self) {
        self.restore();  // 始终尝试 SETCRTC
        // ...
    }
}

pub fn restore(&mut self) {
    if let Some(saved) = self.saved_crtc.take() {
        // 还原一个从未 modeset 的 CRTC
        let _ = sys::ioctl_struct(self.card_fd, DRM_IOCTL_MODE_SETCRTC, &mut restore);
    }
}
```

**问题**：`modeset_ok=false` 时 `saved_crtc` 仍为 `Some`（probe 流程无条件读了 CRTC 状态），drop 时会 SETCRTC 还原一个从未 modeset 的 CRTC。

**正确做法**：仅在 `modeset_ok` 为 true 时还原 saved_crtc。

```rust
pub fn restore(&mut self) {
    if !self.modeset_ok {
        return;
    }
    if let Some(saved) = self.saved_crtc.take() {
        let mut restore = saved;
        // ... 原逻辑
    }
}
```

这是 v0.2.1 升级 plan §4 列了但没做的项，留给下一轮。

### L2: 模块大小写规约

- `scene_anim.rs` 在 v0.2.1 是 236 行，处于上限边缘；再增逻辑请先抽 `scene_anim/voice.rs` 或 `scene_anim/spawn_curve.rs`。
- `net_ollama.rs` 在 v0.2.1 是 557 行（含测试），因为手写 JSON scanner；不要进一步加非协议逻辑，协议层保持瘦。

### L3: 五风格 system prompt

`net_ollama::style_for(warmth, energy)`：

| energy | warmth | style |
|---|---|---|
| `> 0.6` | any | 豪放 |
| `< 0.2` | `> 0.6` | 婉约 |
| `< 0.2` | `< 0.4` | 禅寂 |
| `< 0.2` | `0.4..=0.6` | 稚拙 |
| `0.2..=0.6` | `|w-0.5| > 0.25` | 苍茫 |
| `0.2..=0.6` | `|w-0.5| ≤ 0.25` | 稚拙 |

新增「N 风格」或调整阈值时，记得同步 `build_prompt_includes_style` 测试矩阵。

### L4: build / test 分离

- `cargo check --release`：macOS dev 机器通过（只编不链）
- `cargo build --release`：macOS 误链（缺 Linux extern）；**真机 Linux/aarch64 才走**
- `cargo test --release --bin inkflow`：macOS 通过（51 个单元测试不依赖 Linux syscall）
- `cargo clippy --release -- -D warnings`：macOS 通过

代码 review 时不要被 macOS `cargo build` 报错吓到，那是预期的。
