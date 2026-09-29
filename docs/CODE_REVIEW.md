# 代码审查 (Code review)

> 手工维护。上一版是自动生成的，把 `src/glyph_table.rs` 报成 474024 行并建议
> 拆分——那是 `scripts/build_font.py` 的生成产物，本来就不该手改。

## 自动检查覆盖不到的东西

门已经挡住：fmt、clippy（-D warnings）、单元测试、零依赖契约、样帧与渲染器一致、
自迭代循环自测、文档引用完整性。`scripts/pre-push` 与 `.github/workflows/ci.yml`
是同一道门。

这些要靠人（或 agent）来看：

- [ ] 这次改动让作品更像它主张的样子吗，还是只是「修了一个 bug」
- [ ] 渲染类改动：样帧刷了吗？变更是可见的，还是又一个 2.5% 的微调
- [ ] 有没有把业务逻辑塞进 `main.rs`
- [ ] 新模块有测试吗，尤其是带几何或颜色空间计算的
- [ ] commit message 是一行，还是又写成了长篇

## 当前规模

手写模块最大 568 行（`src/scene.rs`），约定是 300 行以内。超预算的模块与拆分判据
见 REFACTOR_PLAN.md。
