# 重构计划 (Refactoring plan)

> 手工维护。上一版是自动生成的，还在描述零依赖重写之前的架构——列了
> `mood.rs` / `llm_loop.rs` / `net_ollama.rs` / `drm.rs` 等十来个早已归档到
> `legacy/src/` 的模块，LOC 也差了好几倍。`scripts/check-docs.sh` 现在会在
> 文档引用不存在的源文件时让门失败，所以这份不会再悄悄烂掉。

## 模块规模

| 模块 | 行数 | 测试 |
|---|---:|---:|
| `background.rs` | 245 | 2 |
| `color.rs` | 222 | 10 |
| `compose.rs` | 428 | 7 |
| `glyph.rs` | 322 | 5 |
| `lib.rs` | 16 | — |
| `main.rs` | 486 | — |
| `phrase.rs` | 223 | 5 |
| `png.rs` | 138 | 6 |
| `rhythm.rs` | 322 | 4 |
| `scene.rs` | 581 | 4 |
| `surface.rs` | 490 | — |

`src/glyph_table.rs` 是 `scripts/build_font.py` 的生成产物（85,456 行），不计入
「每个 mod < 300 行」的约束，也不该手工编辑。

合计手写代码 3,906 行，41 个单元测试。

## 超预算的模块

- `src/compose.rs` — 428 行
- `src/glyph.rs` — 322 行
- `src/main.rs` — 486 行
- `src/rhythm.rs` — 322 行
- `src/scene.rs` — 581 行
- `src/surface.rs` — 490 行

## 真正待办的事

这些是代码之外、但会影响作品本身的：

- **样帧 gate 曾经红过**：`refresh-samples.sh` 拿 `$CARGO_TARGET_DIR` 构建却用
  硬编码的 `./target/release/inkflow` 渲染，刷新静默产出旧帧。已修，并加了推
  main 前的 `verify-samples.sh`。
- **自迭代循环退化成噪声**：连续二十几轮只把一个浮点数乘 1.025，commit message
  越写越长。已加 `art-turn-check.py` 机械拒绝，外加 `autoloop-selftest.sh`
  验证循环自己的控制流。
- **`state/` 和 `graphify-out/` 明明在 `.gitignore` 里却仍被 git 跟踪**，
  每轮都产生无意义 diff（`state/screen.ppm` 单个就 3 MB）。需要
  `git rm -r --cached state/ graphify-out/`。
