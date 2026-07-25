# audio-batch-speedup（音频批量加速工具）

[English](./README.md) | 简体中文

一个使用 ffmpeg 进行音频文件的批量处理和加速的 Rust 库及命令行工具。

本工具最初专为视觉小说音频加速设计（因视觉小说通常使用大量 OGG 文件作为音频系统），现已扩展支持更多音频格式。

## 功能特性

- 并行递归处理：充分利用多核 CPU 性能，支持多音频文件并行处理
- 灵活调速：可自定义音频加速倍率
- 多格式支持
  - 格式检测：优先通过文件内容（magic bytes）检测音频格式，若无法识别则回退到文件扩展名判断

## 使用方法

### 命令行工具使用

首先安装工具包，您可以选择：

- 从[Release 页面](https://github.com/lxl66566/audio-batch-speedup/releases)手动下载
- 使用[cargo-binstall](https://github.com/cargo-bins/cargo-binstall)安装：
  ```bash
  cargo binstall audio-batch-speedup
  ```

运行示例：

```bash
abs --input 音频文件夹路径 --speed 1.5 --formats ogg,mp3  # 将该文件夹下所有OGG和MP3文件加速1.5倍
```

**参数说明**：

- `-i, --input <输入路径>`：包含音频文件的文件夹路径（必填）
- `-s, --speed <加速倍率>`：音频加速倍数（如 1.5 表示 1.5 倍速）（必填）
- `-f, --formats <格式列表>`：要处理的音频格式逗号分隔列表（如`ogg,mp3,wav`），使用`all`处理所有支持格式
  - 支持格式：`ogg`, `mp3`, `wav`, `flac`, `aac`, `opus`, `alac`, `wma`
  - 默认值：`all`

### 作为库使用

在 Cargo.toml 中添加依赖：

```toml
[dependencies]
audio-batch-speedup = "0.1"  # 请使用最新版本
```

代码示例：

```rust
use std::path::Path;
use audio_batch_speedup::{process_audio_files, AudioFormat};

fn main() -> std::io::Result<()> {
    let folder = Path::new("音频文件路径");
    let speed = 1.5;

    // 处理OGG和MP3文件
    process_audio_files(folder, speed, AudioFormat::OGG | AudioFormat::MP3)?;

    // 处理所有支持的音频格式
    process_audio_files(folder, speed, AudioFormat::ALL)?;

    Ok(())
}
```

## 编码说明

音频加速需要对音频流进行解码并重新编码。为避免 ffmpeg 默认编码参数导致的音质损失，本工具对每种格式显式指定编码器：

| 格式 | 编码参数                        |
| ---- | ------------------------------- |
| FLAC | `flac`（无损）                  |
| WAV  | `pcm_s24le`（无损）             |
| ALAC | `alac`（无损）                  |
| MP3  | `libmp3lame -q:a 2`（VBR ~190k）|
| OGG  | `libvorbis -q:a 6`（~192k）     |
| OPUS | `libopus -b:a 160k`             |
| AAC  | `aac -b:a 192k`                 |
| WMA  | `wmav2 -b:a 192k`               |

对于有损格式，输出码率会被钳制在源码率（通过 ffprobe 检测）以内，确保加速后文件体积不会膨胀；源码率不低于上表默认值时使用默认高质量参数。若 ffprobe 不可用，则所有文件使用默认参数。

这需要 ffmpeg 构建包含 `libmp3lame`、`libvorbis` 和 `libopus` 编码器（大多数完整版构建均已包含）。

## 系统要求

- 必须安装 FFmpeg 并配置在系统 PATH 环境变量中
