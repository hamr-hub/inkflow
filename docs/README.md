# inkflow · 墨流

> 手工维护。上一版是自动生成的快照：写着 276,780 行源码，还把路径硬编码成
> `/mnt/ssd/...`（那是 Jetson 上的路径，别的 checkout 上必然是错的——把机器专属的
> 路径写进文档，就是给未来的自己埋雷）。

## Snapshot

- **language**: Rust, zero third-party dependencies
- **hand-written `src/`**: 4,055 lines
- **generated `src/glyph_table.rs`**: 85,457 lines (`scripts/build_font.py`, not hand-edited)
- **unit tests**: 43 (`cargo test --release`)
- **runs on**: Jetson Orin Nano, no desktop, no X, no Wayland

仓库路径不写在这里——每台机器都不一样，`pwd` 就有。

## Source-of-truth sub-docs

- [Architecture](./ARCHITECTURE.md) — 架构总览
- [运行结构](./BACKEND_VIEW.md) — 帧循环、后端、输出
- [Frontend 视角](./FRONTEND_VIEW.md) — 为什么没有
- [测试覆盖](./TEST_COVERAGE.md) — 每个模块几个测试，以及为什么有的没有
- [重构计划](./REFACTOR_PLAN.md) — 模块规模与真正待办的事
- [代码审查](./CODE_REVIEW.md) — 门挡住了什么、还要人看什么
