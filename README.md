# inkflow · 墨流

一件**自我迭代的生成式工艺品**：Jetson Orin Nano + 全屏中文文字氛围流 + 粒子，
触控的位置、力度、触点数实时改变生成内容、色调与流动；本地 LLM（ollama /
`gemma3:1b`）持续供字，LLM 慢或离线时内置词库永不断流。

> **先读 [ARTIFACT.md](ARTIFACT.md)** —— 它是这件作品的自陈，也是所有改动的美学基准。

## 文档

- [ARTIFACT.md](ARTIFACT.md) — 美学宣言 / 作品自陈：作品主张什么、反对什么、AI 时代属性如何被命名。
- [ZERO_DEP.md](ZERO_DEP.md) — 零依赖硬约束的契约来源；写给维护者的不可违反项。
- [PRODUCTION.md](PRODUCTION.md) — 当前已实测兑现的工程指标清单；任何回归都要先看这里。
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — 模块结构、数据流、线程模型、性能契约。
- [docs/UPGRADE_PLAN.md](docs/UPGRADE_PLAN.md) — 综合升级计划（v0.2.1 实施进度跟踪）。

## 模块结构

```
src/
├── main.rs           — thin orchestrator, frame loop, --diag / --drm-test
├── fallback.rs       — 静态词库 + warm/cool/slow/fast picker (zero-alloc)
├── mood.rs           — touch → (warmth, energy, idle) envelope
├── llm_loop.rs       — LLM worker thread + Shared queue
├── scene_anim.rs     — per-frame spawn logic (glyphs + particles)
├── renderer.rs       — per-frame drawing (clear → nebula → stars → particles → glyphs → fog)
├── surface.rs        — Surface enum: DRM / fb0 / Headless unified API
├── screenshot.rs     — 60s PPM/PNG frame capture (off-thread)
├── telemetry.rs      — JSONL writer + 2 MiB rotation
├── net_ollama.rs     — hand-rolled TCP + HTTP/1.1 + minimal JSON
├── evdev.rs          — touch hotplug + slot protocol
├── drm.rs            — DRM/KMS dumb-buffer, /dev/fb0 fallback, Headless
├── font.rs           — software rasterizer + Rgba + char_key lookup
├── scene.rs          — fixed-cap VecDeque pools + LCG
├── sys.rs            — extern "C" syscall surface (zero libc)
└── fontdata.rs       — generated, NOT in git
```

`scripts/`：
- `autoloop.sh` — Claude 无人值守单轮：查内存 → 读 telemetry → 看屏幕截图 →
  改一处 → fmt/clippy/build → 重启 → 自动提交；失败自动回退。
- `extract_font.py` / `extract-font.sh` — 从 Noto Sans CJK 提取字模子集。
- `install-systemd.sh` — 一键安装为 system 服务（开机即用）。
- `inkflow-zero.service` / `pre-push` / `run-drm-fix.sh` / `run-zero-rewrite.sh` — 部署与排障脚本。

systemd（user，Linger 已开）：
- `inkflow.service` — 摆件本体，`Restart=always`，`MemoryMax=1300M` 防 OOM。
- `inkflow-autoloop.timer` — 每 20 分钟一轮 Claude 自治。

## 常用

```bash
systemctl --user status inkflow inkflow-autoloop.timer
tail -f ~/codespace/inkflow/state/telemetry.jsonl
tail -f ~/codespace/inkflow/state/autoloop.log
git -C ~/codespace/inkflow log --oneline
DISPLAY=:0 /mnt/ssd/codespace/.cargo-target/inkflow/release/inkflow --diag

# v0.2.1 — 运行单元测试 (48 个测试，无 libc 依赖，跨平台可跑)
cargo test --release --bin inkflow

# 格式化 / 静态检查
cargo fmt
cargo clippy --release -- -D warnings
cargo build --release
```

## 约束（写给维护者 Claude）

- 画面永远不能黑、不能卡；本地词库是最后防线。
- 无菜单、无 HUD、无调试文字；中文优先；保持沉静氛围。
- 单轮只做一处小改动；fmt/clippy/build 全过才重启和提交；不 push。
- MemAvailable < 700MB 时 autoloop 自动跳过。
- 每次改动前先读 `ARTIFACT.md`；commit message 写「这一改如何让作品更像自己主张的样子」。

## 测试覆盖（v0.2.1）

48 个单元测试跨 8 个模块，跨平台（macOS dev / Linux prod）都跑：

| 模块 | 测试数 | 覆盖范围 |
|---|---|---|
| `fallback` | 5 | pool picker 决定性、空池 sentinel、不越界 |
| `font` | 3 | char_key 命中/缺位、clamp 边界 |
| `llm_loop` | 4 | Shared cap、pop 顺序、snapshot 原子性 |
| `mood` | 4 | warmth/idle/hue 全在 [0, 1]、effective_energy 取 max |
| `net_ollama` | 12 | JSON scanner、UTF-8 解码、prompt 模板 |
| `scene` | 5 | LCG 决定性、上限守恒、star 位置在帧内 |
| `scene_anim` | 3 | size 常数层级、accumulator 初值 |
| `telemetry` | 8 | 双逗号回归、sentinel=null、特殊字符转义 |
| `main` | 2 | frame_target = 16 667 µs |
| **total** | **48** | |
