# 合图 Hetu

把多张图片按 **横排 / 竖排 / 网格** 拼成一张大图，**每张图保持原始分辨率** —— 导出时不放大、不缩小。

Rust + egui 实现，编译产物是单个 exe（约 6 MB），不依赖 Python、Node 或 WebView2 运行时。

**下载**：[最新版本](https://github.com/choumine/hetu/releases/latest)（Windows x64，约 6 MB，解压即用）

## 功能

- 添加图片（多选）、粘贴剪贴板截图
- 三种布局：横排 / 竖排 / 网格（每行张数可调）
- 图片间距、外边距、单元格内对齐（左中右 / 上中下）
- 背景：透明 / 白 / 黑 / 灰
- 实时预览，导出始终按原始像素
- 导出 PNG / JPG / WebP，JPG / WebP 可调质量
- 列表拖拽排序、单张移除
- 自带应用图标（exe 文件图标 + 窗口/任务栏图标）

## 构建

```bash
cargo build --release
```

Windows + MSVC 工具链可直接编译。没有 MSVC 时也可以用 MinGW + GNU 目标：

```bash
scoop install mingw
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal
cargo +stable-x86_64-pc-windows-gnu build --release
```

产物：`target/release/hetu.exe`。只依赖系统 DLL，可直接拷走运行。

## 使用

双击运行 → 点「＋ 添加图片」选图 → 调整布局和间距 → 点「导出图片」。

## 实现要点

- **布局算法**：网格统一处理，`cols = n` 即横排、`cols = 1` 即竖排；列宽取该列最宽图，行高取该行最高图，单元格内可对齐。
- **不缩放**：导出时 `scale = 1`，只用 `image::imageops::overlay` 叠加，没有任何重采样。
- **预览**：按最长边 1300 缩放，参数变化时重算（单张 1376×768 约 48 ms）。
- **对话框不阻塞 UI**：文件对话框跑在独立线程，结果通过 channel 回传，每帧 `try_recv`。
- **中文**：egui 默认字体不含 CJK，启动时依次尝试 `simhei.ttf` / `Deng.ttf` / `msyh.ttc`。

## 结构

```
src/main.rs        全部逻辑（UI、布局、合成、导出）
assets/icon.png    图标源图（1024×1024，同时作为窗口图标）
assets/icon.ico    exe 图标（16–256 多尺寸）
app.rc             图标 + 文件版本属性
build.rs           构建时把资源编进 exe（rc.exe / windres）
tools/gen_icon.py  图标生成脚本（pillow）
Cargo.toml
```

## License

MIT
