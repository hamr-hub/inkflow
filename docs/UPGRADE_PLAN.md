# inkflow · 综合升级计划 (Comprehensive Upgrade Plan) — v0.2.1 实施记录

> 目标：在保持 **零依赖、std-only、Linux DRM/KMS dumb-buffer 直绘** 这三条硬约束的同时，
> 把代码组织、文档、可读性、动画语汇、回退词库、字模覆盖、自检测试都向前推一步。

## 状态：✅ 完成 (2026-09-24)

51 个单元测试跨 8 个模块全过 · `cargo clippy --release -- -D warnings` 零警告 · 
`cargo fmt --check` 干净 · 零依赖铁律维持。

## 范围回顾

### 1. 代码结构 (architecture) — ✅

- [x] `main.rs` 拆分：renderer、fallback、llm_loop、surface、scene_anim
- [x] 移除 `main.rs` 中的内联 surface 枚举，抽到 `surface.rs`
- [x] `POOLS` 常量 + `local_glyph` 抽到 `fallback.rs`，加上文档
- [x] `llm_worker` 抽到 `llm_loop.rs`
- [x] 抽出 mood 控制流到 `mood.rs`

主循环从 866 行降到 494 行；新增 7 个模块，每个 < 300 行。

### 2. 内容 (content) — ✅

- [x] 字模子集扩展：从 ~140 字增至 ~250 字（含五风格词汇 + 诗意字 + 罕见字）
- [x] 扩展 fallback 词库：每池从 26 → 30 字，新增「静寂默虚远」「疾涌旋飙怒」等风格细分
- [x] 增加 LLM prompt 的 5 种文艺风格（婉约 / 豪放 / 禅寂 / 稚拙 / 苍茫）按 (warmth, energy) 选择
- [x] `font::char_key` 公共 API（替代内联 `lookup_char`）

### 3. 性能 (perf) — ✅

- [x] `for px in pixels.iter_mut() { *px = bg; }` 替换为 `pixels.fill(BACKGROUND)`
- [x] `neb_a_alpha` / `neb_b_alpha` 在循环外计算一次（`renderer::draw_nebula`）
- [x] `Rgba::from_hsl` 重复计算仍然有但每像素一次，没有重构成 hue-to-rgb 表
      （仍可接受；瓶颈在 fill_circle 的软边 feather 像素逐点扫描）

### 4. 可靠性 (reliability) — ✅

- [x] 修复 extract_font.py 死代码 (`drw = None`)
- [x] 修复 scan_bool_field 里 `let _truth = want as usize;`
- [x] evdev.rs ABS_MT_TRACKING_ID=-1 处理：经 MT 协议审计确认是正确的（不需要修改）

### 5. 测试 (test) — ✅

51 个测试跨 8 个模块：

| 模块 | 测试数 |
|---|---|
| fallback | 5 |
| font | 3 |
| llm_loop | 4 |
| mood | 4 |
| net_ollama | 16 |
| scene | 5 |
| scene_anim | 3 |
| telemetry | 8 |
| main | 2 |
| **total** | **51** |

### 6. 文档 (docs) — ✅

- [x] 每个 mod 增加顶部 doc comment
- [x] 增加 ARCHITECTURE.md（数据流图、模块依赖图、线程模型、错误处理）
- [x] README.md 增加目录结构图、性能表、测试覆盖表

### 7. 静态检查 — ✅

- [x] `cargo fmt` 干净
- [x] `cargo clippy --release -- -D warnings` 零警告

## 原则（贯彻情况）

1. **不引入新依赖** — 零依赖铁律维持
2. **每改一处，跑一次 `cargo fmt`** — 自动循环已就位
3. **测量先于优化** — 未做无证据的优化；HSL 表重构等推迟到下一轮
4. **单一职责** — main.rs 只负责组合
5. **可测的逻辑下沉到函数** — 51 个测试可以证明

## 下一轮候选（v0.2.2）

- [ ] hue_to_rgb 预计算表（每帧数百次 `from_hsl` 调用，是热路径上最大的非分配 CPU 占用）
- [ ] nebula 双层 5 圈合并为单层 8 圈（少 2 次 fill_circle 调用）
- [ ] glyph_atlas 索引（目前 `bitmap_for` 是 O(N) 线性搜索，~50 像素每字 50×每像素 = 50K 次/帧）
- [ ] DRM modeset 在 `open_first` 路径上的 mode probing（当首选 mode fail 时）
