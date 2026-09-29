# 测试覆盖 (Test coverage)

> 手工维护。上一版是自动生成的，报告「0 个测试文件、test/sLOC 0.00%」，
> 还建议用 `pytest --cov` / `vitest`——这是零第三方依赖的 Rust 项目，
> `cargo tarpaulin` 同样违反 ZERO_DEP.md。下面这张表直接数 `#[test]`。

## 现状

| 模块 | 测试数 |
|---|---:|
| `background.rs` | 2 |
| `color.rs` | 10 |
| `compose.rs` | 7 |
| `glyph.rs` | 5 |
| `phrase.rs` | 5 |
| `png.rs` | 6 |
| `rhythm.rs` | 2 |
| `scene.rs` | 4 |

合计 **41** 个单元测试（`cargo test --release`）。

## 没有测试的模块

`lib.rs`、`main.rs`、`surface.rs`

这两个是有理由的：

- `surface.rs` 走 `extern "C"` 调 DRM / fb0 的 `ioctl`、`mmap`，只能在真机
  Linux 上跑，macOS dev 机器上测不了。
- `main.rs` 只做编排（帧循环 + 命令行分派），逻辑都在上面的模块里。

## 门

`cargo test --release` 属于全门的一部分，CI 和 `scripts/pre-push` 都会跑。
渲染类断言（辉光不能有硬边、字形要铺满自身范围）刻意不只看包围盒——错误
实现同样填满包围盒，见 AGENTS.md L3。
