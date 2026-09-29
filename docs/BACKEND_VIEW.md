# 运行结构 (Runtime view)

> 手工维护。上一版是自动生成的，整篇都在描述零依赖重写之前的架构——`mood.rs`、
> `llm_loop.rs`、`net_ollama.rs`、`drm.rs`、`evdev.rs`、`telemetry.rs` 等十来个
> 早已移入 `legacy/src/` 的模块。零依赖重写之后，本机语言模型、触摸输入与遥测
> 都退役了：作品不再解释自己，也不暴露参数。

## 现在是什么

一件跑在 Jetson Orin Nano 上的生成式工艺品。无桌面、无 X、无 Wayland、无
systemd 之外的依赖。帧循环在 `main.rs`，只做编排：

```
advance → step → paint_background → paint_composition → present
```

各阶段下沉到 `rhythm.rs`（节拍调度）、`scene.rs`（槽位几何与生命周期）、
`background.rs`（天空/光晕/地平雾/暗角/月）、`compose.rs`（落笔顺序）、
`color.rs`（调色板与混合算子）、`glyph.rs`（字形面积采样）。

## 输出

三个后端统一在 `surface.rs`：内存 framebuffer、`/dev/fb0`、DRM/KMS dumb-buffer。
PNG 编码器在 `png.rs`，用的是**未压缩的 stored DEFLATE**——零依赖换体积。

## 曲库

`phrase.rs` 按主题钉死一整首五言绝句，五首作品，不拆成词条池。槽位到龄后按
读序回填，所以永远不会黑屏，也永远不会只剩半首诗。

详见 ARCHITECTURE.md。
