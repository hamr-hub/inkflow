# inkflow · 综合升级计划 (Comprehensive Upgrade Plan)

> 目标：在保持 **零依赖、std-only、Linux DRM/KMS dumb-buffer 直绘** 这三条硬约束的同时，
> 把代码组织、文档、可读性、动画语汇、回退词库、字模覆盖、自检测试都向前推一步。

## 范围

### 1. 代码结构 (architecture)

- [ ] `main.rs` 拆分：renderer、fallback、llm_loop、surface、scene_anim
- [ ] 移除 `main.rs` 中的内联 surface 枚举，抽到 `surface.rs`
- [ ] `POOLS` 常量 + `local_glyph` 抽到 `fallback.rs`，加上文档
- [ ] `llm_worker` 抽到 `worker.rs`（不是 mod net_ollama — 那是协议层）
- [ ] 抽出 mood 控制流（warmth/energy 时间常数、idle 抖动）到 `mood.rs`

### 2. 内容 (content)

- [ ] 字模子集扩展：从 ~140 字增至 220+ 字（增加诗意字、罕见字、画面字）
- [ ] 扩展 fallback 词库：增加「意象碎片」「晚」「晨」「空」「光」「色」「声」「形」四档
- [ ] 增加 LLM prompt 的 prompt 风格表（5 种文艺风格 + system prompt）
- [ ] 抽出 `glyph_atlas.rs`：把 fontdata / bitmap_for / static_key_for 的查找路径做成
      LRU 缓存或二分（目前 O(N) 线性搜索在每个被渲染字符上，N=235 个字符）

### 3. 性能 (perf)

- [ ] `for px in pixels.iter_mut() { *px = bg; }` 替换为 `pixels.fill(bg)`
- [ ] HSL→RGB 每帧预计算（hue drift 是 sin 慢变，可以缓存）
- [ ] nebula 双层 5 圈合并为单层 8 圈（少 2 次 fill_circle 调用）
- [ ] 每帧的 `font::from_hsl` 重复计算 → 用 `hue_to_rgb` 表

### 4. 可靠性 (reliability)

- [ ] 修复 extract_font.py 死代码 (`drw = None`)
- [ ] 修复 scan_bool_field 里 `let _truth = ...`
- [ ] 修复 evdev.rs 的 `slots.entry().or_insert()` 与 ABS_MT_TRACKING_ID=-1 时的 remove
      当前实现 slot 不更新，TRACKING_ID=-1 仍 remove 旧的 slot，可能误删
- [ ] `drm.rs` 里 `restore()` 路径在 modeset_ok=false 时 saved_crtc 仍 Some — 此时还原
      一个未经 modeset 的 CRTC 状态可能不必要；改成只在我们成功过 modeset 时才恢复

### 5. 测试 (test)

- [ ] LCG 决定性（同一个 seed 出同一个值）
- [ ] Rgba::from_hsl 已知的几组值
- [ ] telemetry JSON 编码无尾逗号、能被 serde_json 反序列化
- [ ] escape_json、scan_response 的几个手写 JSON 片段
- [ ] net_ollama 的 find_newline / scan_done 边界用例
- [ ] nebula / star 计算的边界

### 6. 文档 (docs)

- [ ] 每个 mod 增加顶部 doc comment
- [ ] 增加 ARCHITECTURE.md（数据流图、模块依赖图）
- [ ] README.md 增加目录结构图、性能表、故障树
- [ ] 在每个公开函数加 `///` 注释（参数 / 返回值 / 安全要求）

### 7. 静态检查

- [ ] `cargo fmt`
- [ ] `cargo clippy --release -- -D warnings`（Linux target 下；macOS dev 机只 fmt）
- [ ] 移除所有 `#[allow(dead_code, unused_mut)]` 中真正死掉的项

## 原则

1. **不引入新依赖** — 零依赖铁律
2. **每改一处，跑一次 `cargo fmt`** — autoloop 的格式红线
3. **测量先于优化** — 不做无证据的「优化」
4. **单一职责** — main.rs 只负责组合，不实现
5. **可测的逻辑下沉到函数** — main loop 里的内联表达式尽量提成 fn
