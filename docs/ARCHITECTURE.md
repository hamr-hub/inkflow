# inkflow · 架构与数据流 (Architecture & Data Flow)

> v0.2.1 — comprehensive upgrade. 这份文档是代码组织的权威说明。

## 模块结构

```
src/
├── main.rs           — thin orchestrator, frame loop, --diag / --drm-test
├── fallback.rs       — 静态词库 + warm/cool/slow/fast picker (zero-alloc)
├── mood.rs           — touch → (warmth, energy, idle) envelope
├── llm_loop.rs       — LLM worker thread + Shared queue
├── scene_anim.rs     — per-frame spawn logic (glyphs + particles) + ink_current_x(t)
├── renderer.rs       — per-frame drawing (clear → nebula → moon → stars → particles → glyphs → fog) + self-portrait test
├── surface.rs        — Surface enum: DRM / fb0 / Headless unified API
├── screenshot.rs     — 60s PPM/PNG frame capture (off-thread)
├── telemetry.rs      — JSONL writer + 2 MiB rotation; carries aesthetic state (voice, ink_x, hue)
├── net_ollama.rs     — hand-rolled TCP + HTTP/1.1 + minimal JSON
├── evdev.rs          — touch hotplug + slot protocol
├── drm.rs            — DRM/KMS dumb-buffer, /dev/fb0 fallback, Headless
├── font.rs           — software rasterizer + Rgba + char_key lookup
├── scene.rs          — fixed-cap VecDeque pools + LCG
├── sys.rs            — extern "C" syscall surface (zero libc)
└── fontdata.rs       — generated, NOT in git
```

## 数据流（一帧）

```
┌─────────────────────────────────────────────────────────────────┐
│ main loop (render thread)                                       │
└─────────────────────────────────────────────────────────────────┘
        │
        ├─► mood::tick(&mut touch_state, dt, t)
        │       └► FrameMood { warmth, energy, idle, contacts, device }
        │
        ├─► llm_loop::publish_mood(mood_state, &frame)
        │       └► LLM worker sees the new (warmth, energy) on its next generate()
        │
        ├─► scene_anim::spawn_for_frame(scene, accum, frame, touch, shared, ..., t)
        │       ├─► ink_current_x(t) → cluster column x (留白 composition)
        │       ├─► llm_loop::pop_char → if Some(c) → font::char_key → Glyph
        │       │   else → fallback::local_glyph(warmth, energy, tick) → Glyph
        │       └─► Per-contact particles + ambient drift particles
        │
        ├─► mood::hue_at(t, frame.warmth) → hue
        │
        ├─► renderer::draw_frame(&mut surface, glyphs, particles, stars, dt, t, hue)
        │       ├─ clear         (pixels.fill(BACKGROUND))
        │       ├─ nebula        (2 × 5 concentric circles, slow drift)
        │       ├─ moon          (single silhouette, complementary hue, anchors composition)
        │       └─ (self-portrait: render_portrait() composes all phases into a PPM
        │           under #[cfg(test)] for offline verification on any host)
        │       ├─ stars         (90 twinkles, per-star phase)
        │       ├─ particles     (step + draw, retain alive)
        │       ├─ glyphs        (step + draw, retain alive)
        │       └─ top_fog       (3px black bar at y=0)
        │
        ├─► telemetry::append (every 10s)
        │       ├─► /var/.../state/telemetry.jsonl (rotate at 2 MiB)
        │       ├─► perf fields:  fps, frame_min_us, frame_max_us, glyphs, particles
        │       ├─► LLM fields:   llm_ok, llm_toks_per_s, llm_model, llm_last
        │       └─► aesthetic:    voice (5-voice picker), ink_x (留白 column), hue
        │
        └─► screenshot::spawn_grab (every 60s)
                └─► worker thread: PPM write + ffmpeg → PNG
                
        └─► sleep until next_frame (aligned cadence)
                └─► if late: skip-ahead, never catch-up
```

## 模块依赖图

```
                    main
                     │
        ┌────────────┼─────────────────┐
        │            │                 │
        ▼            ▼                 ▼
    surface       mood            llm_loop
        │            │                 │
        │            ▼                 │
        │       scene_anim              │
        │            │                 │
        │            ▼                 │
        │      fallback (zero-alloc)    │
        │                               │
        ▼                               ▼
      drm                          net_ollama
        │                               │
        └──────────────┬────────────────┘
                       ▼
                     font ← fontdata (generated)
                       │
                       ▼
                     scene
                       │
                       ▼
                  renderer

    evdev ───► main (touch supervisor)
    sys   ───► drm, evdev, net_ollama
    telemetry, surface, screenshot, llm_loop — siblings, no upward deps
```

## 线程模型

| 线程 | 启动者 | 职责 | 终止时机 |
|---|---|---|---|
| **render** | `main` | 帧循环、spawn、draw、telemetry、screenshot | never (systemd Restart=always) |
| **llm_worker** | `main` | ollama 拉流、回退到本地词库节奏 | never |
| **touch supervisor** | `evdev::start_supervisor` | inotify 监视 /dev/input | never |
| **touch reader N** | supervisor spawn | 解析 input_event → TouchState | never (重连 on EIO) |
| **screenshot N** | render spawn (60s) | PPM 写盘 + ffmpeg 转码 | 一次性，写完即退 |

线程之间通过 `Arc<Mutex<…>>` 共享：

- `Arc<Mutex<TouchState>>`  — evdev reader → render
- `Arc<Mutex<llm_loop::Shared>>` — llm_worker → render
- `Arc<Mutex<(f32, f32)>>` — render → llm_worker (mood)

## 关键不变量

| 不变量 | 在哪里强制 | 怎么强制 |
|---|---|---|
| 单帧成本 < 2 ms | `main` 帧循环 | 永远在 render 线程只做 O(W·H) 的 fill + 260 个粒子的循环 + 260 个 glyph 的循环 |
| glyph / particle 数量硬上限 | `scene::push_glyph/push_particle` | `VecDeque::pop_front()` 在 push 之前 |
| 永不分配（稳态） | `scene.rs` (preallocated)、`font.rs` (lookup 是 `&'static str`) | 视觉运行时不分配（除每 60s 一次 `pixels.to_vec()`） |
| 永不黑屏 | `fallback::local_glyph` + `llm_loop` 的回退节奏 | ollama 断开/慢 → 4s 后回退本地词库；本地词库本身永不空 |
| 帧率稳定 60 Hz | `FRAME_TARGET` + 对齐 cadence | `sleep until next_frame`，work 超 budget 时 skip-ahead 不 catch-up |
| 二进制 ≤ 1 MB | 无依赖 + LTO + strip | `Cargo.toml [dependencies]` 空；release profile `lto=thin` + `strip=symbols` |

## 性能契约（PRODUCTION.md 同源）

| 指标 | 上限 | 验证方式 |
|---|---|---|
| RSS 稳态 | ≤ 500 MB | `/proc/$PID/status` |
| 帧率 | ≥ 60 FPS | `telemetry.jsonl` 中 `frame_min_us` 字段 |
| 帧抖动 | min ~ max ≤ 4 ms | `frame_min_us` vs `frame_max_us` |
| 每帧时间 | < 2 ms（主循环） | `telemetry.jsonl` 中 `frame_max_us` |
| 二进制大小 | ≤ 1 MB（release stripped） | `ls -la /usr/local/bin/inkflow` |
| glyph 数 | ≤ 260（硬上限） | `scene.glyphs.len()` |
| particle 数 | ≤ 260（硬上限） | `scene.particles.len()` |
| 触控响应 | < 100 ms（slot 协议） | `evdev.rs` 路径 |
| ollama 回退 | ≤ 1 s | `net_ollama.rs` 超时 800ms + 4s 退避 |
| 日志大小 | ≤ 4 MB（2 MiB × 2 文件） | `telemetry.rs::rotate_if_needed` |

## 错误处理

每条错误路径的默认动作：

- **DRM open 失败** → `/dev/fb0` → `Headless`
- **/dev/fb0 失败** → `Headless`
- **ollama TCP 连不上** → 4 s 退避，本地词库接管
- **ollama 流但是 0 token** → 3 s 退避（不要 spin）
- **触摸设备拔出** → reader sleep 1.5 s 重连
- **telemetry 写盘失败** → 静默，下一次 tick 重试
- **截图 ffmpeg 失败** → PPM 保留，PNG 不强求
- **inotify 不可用** → 退到 2 s 轮询

永远不会因为任何一个失败路径 panic 整个 render loop。
