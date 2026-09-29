# 重构计划 (Refactoring plan)

> 手工维护。上一版是自动生成的，还在描述零依赖重写之前的架构——列了
> `mood.rs` / `llm_loop.rs` / `net_ollama.rs` / `drm.rs` 等十来个早已归档到
> `legacy/src/` 的模块，LOC 也差了好几倍。`scripts/check-docs.sh` 现在会在
> 文档引用不存在的源文件时让门失败，所以这份不会再悄悄烂掉。

## 模块规模

| 模块 | 行数 | 测试 |
|---|---:|---:|
| `background.rs` | 329 | 2 **over 300** |
| `color.rs` | 372 | 10 **over 300** |
| `compose.rs` | 564 | 7 **over 300** |
| `glyph.rs` | 321 | 5 **over 300** |
| `lib.rs` | 17 | — |
| `main.rs` | 483 | — **over 300** |
| `phrase.rs` | 224 | 5 |
| `png.rs` | 301 | 6 **over 300** |
| `rhythm.rs` | 236 | 2 |
| `scene.rs` | 568 | 4 **over 300** |
| `surface.rs` | 491 | — **over 300** |

`src/glyph_table.rs` 是 `scripts/build_font.py` 的生成产物（85,456 行），不计入
「每个 mod < 300 行」的约束，也不该手工编辑。

合计手写代码 3,906 行，41 个单元测试。

## 超预算的模块

以下模块超过 300 行的约定，需要拆：

- `src/background.rs` — 329 行
- `src/color.rs` — 372 行
- `src/compose.rs` — 564 行
- `src/glyph.rs` — 321 行
- `src/main.rs` — 483 行
- `src/png.rs` — 301 行
- `src/scene.rs` — 568 行
- `src/surface.rs` — 491 行

拆分的判据不是行数本身，而是「能不能单独测」。拆开之后才有意义：
单独 lint、单独替换、单独写测试。

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
