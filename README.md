# inkflow · 墨流

一件**自我迭代的生成式工艺品**：Jetson Orin Nano + 全屏中文文字氛围流 + 粒子，
触控的位置、力度、触点数实时改变生成内容、色调与流动；本地 LLM（ollama /
`gemma3:1b`）持续供字，LLM 慢或离线时内置词库永不断流。

## 结构

- `src/main.rs` — Rust 主体：macroquad 全屏渲染、evdev 触控热插拔、ollama 流式拉取、
  情绪向量（warmth/energy）→ prompt/hue/流速、`state/telemetry.jsonl` 每 10s 落盘。
- `scripts/autoloop.sh` — Claude 无人值守单轮：查内存 → 读 telemetry → 看屏幕截图 →
  改一处 → fmt/clippy/build → 重启 → 自动提交；失败自动回退。
- systemd（user，Linger 已开）：
  - `inkflow.service` — 摆件本体，`Restart=always`，`MemoryMax=1300M` 防 OOM。
  - `inkflow-autoloop.timer` — 每 20 分钟一轮 Claude 自治。

## 常用

```bash
systemctl --user status inkflow inkflow-autoloop.timer
tail -f ~/codespace/inkflow/state/telemetry.jsonl
tail -f ~/codespace/inkflow/state/autoloop.log
git -C ~/codespace/inkflow log --oneline
DISPLAY=:0 /mnt/ssd/codespace/.cargo-target/inkflow/release/inkflow --diag
```

## 约束（写给维护者 Claude）

- 画面永远不能黑、不能卡；本地词库是最后防线。
- 无菜单、无 HUD、无调试文字；中文优先；保持沉静氛围。
- 单轮只做一处小改动；fmt/clippy/build 全过才重启和提交；不 push。
- MemAvailable < 700MB 时 autoloop 自动跳过。
