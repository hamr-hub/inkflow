# inkflow 零依赖重构规格（ZERO-DEP REWRITE）

目标：一个 **std-only 静态风格单二进制**，用户无需安装/维护任何东西。开机即用，断电自恢复。

## 硬性约束
- Cargo.toml **零第三方依赖**：只用 `std`（允许 `libc`？——不，系统调用/ioctl 用裸 extern "C" 或手写，尽量纯 std；若确实必须，最多 libc 一个 crate，需论证）
- 不依赖 X11 / Wayland / GNOME / 桌面会话：直接 **DRM/KMS**
  - 打开 `/dev/dri/card0`（遍历 card*）
  - ioctl：`DRM_IOCTL_VERSION`、`MODE_GETRESOURCES`、`MODE_GETCONNECTOR`、`MODE_GETCRTC`
  - dumb buffer：`MODE_CREATE_DUMB` → mmap → `MODE_MAP_DUMB`；atomic/legacy modeset：`MODE_SETCRTC`
  - 纯软件渲染到 mmap（32bpp BGRA），无 GL
- **evdev 触控**：裸 open `/dev/input/event*`，手写 input_event，EVIOCGBIT/EVIOCGABS ioctl，多点 slot 协议；热插拔 inotify（或轮询）
- **ollama**：手写 TCP（std::net）连 127.0.0.1:11434，手写极简 HTTP/1.1 POST + 手写 JSON 解析（只取 response 字段）；超时/断连静默回退本地词库
- **中文字体**：嵌入一个极简点阵/字形。方案：构建期用脚本从 Noto Sans CJK 提取 ~600 个摆件意象字 + ASCII 的字形位图，生成 `src/fontdata.rs`（只含子集，控制二进制体积）。软件光栅化
- 帧截图：直接把 dumb buffer 降采样写 PPM/PNG（PPM 零依赖），供 AI 视觉
- 单线程主循环 + 非阻塞输入；或少量线程但无第三方 channel（std::mpsc）
- 内存：glyph/particle 固定数组复用，无每帧分配；RSS ≤ 200MB（无桌面后大幅下降）

## 保持的产品语义
- 深黑底，中文意象字漂浮上升 + 粒子 + 水平风；触控位置→冷暖、速度/触点数→能量→prompt/色/速
- 无 ollama/无触控/无网络均独立可用，画面永不黑
- telemetry（JSONL）+ 帧截图保留，供自治循环读取

## 运行形态（重构后）
- systemd **system** service（不依赖用户会话/桌面）：开机进入，直接接管屏幕。
  因无 sudo，先交付可执行文件 + 准备好的 .service，安装脚本 `scripts/install-systemd.sh`，
  说明需一次性 `sudo` 安装；安装后 24h 自启。
- 自治 timer 保留（user 即可，负责改代码/提交），重构完成后重新启用

## 验收
- 在真实 DRM 上跑起来并截图证明有中文+粒子
- `cargo build --release` 零依赖通过；clippy 零警告
- 无 X（停桌面后/或指定 VT）仍能呈现；断电/重启后 systemd 拉起
- 二进制体积、RSS、帧率用实测数字写入 PRODUCTION.md
