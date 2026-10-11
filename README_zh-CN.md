# audio-batch-speedup（音频批量加速工具）

[English](./README.md) | 简体中文

一个使用 ffmpeg 进行音频文件的批量处理和加速的 Rust 库及命令行工具。

本工具最初专为视觉小说音频加速设计（因视觉小说通常使用大量 OGG 文件作为音频系统），现已扩展支持更多音频格式。

## 功能特性

- 并行递归处理：充分利用多核 CPU 性能，支持多音频文件并行处理。
- 灵活调速：可自定义音频加速倍率。
- 多格式支持
  - 格式检测：优先通过文件内容（magic bytes）检测音频格式，无法识别时回退到文件扩展名判断。详见[格式检测](#格式检测)。

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
abs --input 音频文件夹路径 --speed 2 --jobs 4              # 以 4 个并行任务加速所有支持格式的文件
```

**参数说明**：

- `-i, --input <输入路径>`：包含音频文件的文件夹路径（必填）
- `-s, --speed <加速倍率>`：音频加速倍数（如 1.5 表示 1.5 倍速）（必填）。
- `-f, --formats <格式列表>`：要处理的音频格式逗号分隔列表（如`ogg,mp3,wav`），使用`all`处理所有支持格式
  - 支持格式：`ogg`, `mp3`, `wav`, `flac`, `aac`, `opus`, `alac`, `wma`
  - 默认值：`all`
- `-j, --jobs <并行数>`：并行 ffmpeg 任务数（默认：每个 CPU 核心一个任务）

**日志**：日志级别默认为 `info`，可通过 `SPDLOG_RS_LEVEL` 环境变量覆盖（如 `SPDLOG_RS_LEVEL=debug abs ...`）。

**失败报告**：每个失败的文件在终端输出一行错误，包含路径与 ffmpeg 退出码。每次失败会把实际执行的 ffmpeg 命令与 stderr 的最后几行追加到系统临时目录下共享的 `abs-fail-*.log` 文件，其路径在运行结束时输出。存在失败文件时以非零退出码结束。

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

    // 处理OGG和MP3文件，每个 CPU 核心一个 worker
    let summary = process_audio_files(folder, speed, AudioFormat::OGG | AudioFormat::MP3, None)?;

    // 处理所有支持的音频格式，限制为 4 个并行任务
    let jobs = std::num::NonZeroUsize::new(4);
    let summary = process_audio_files(folder, speed, AudioFormat::ALL, jobs)?;

    // `Err` 仅表示任务无法启动（speed 非法、ffmpeg 缺失）；
    // 单个文件的失败收集在 `summary.failed` 中。
    for (path, reason) in &summary.failed {
        eprintln!("failed: {}: {reason}", path.display());
    }
    println!(
        "processed {}, skipped {}, failed {}",
        summary.processed, summary.skipped, summary.failed.len()
    );

    Ok(())
}
```

## 格式检测

格式检测优先依据文件内容（magic bytes），内容无法判断时回退到文件扩展名：

- Ogg 容器通过首页区分：包含 `OpusHead` ID 头为 Opus，其余一律按 Vorbis（OGG）处理。
- mp4 家族（`.m4a`）可能封装 AAC 或 ALAC，magic bytes 无法区分；通过 ffprobe 按编码器名称区分。当 ffprobe 缺失时，`.m4a` 文件一律按 AAC 处理，因此 ALAC 文件可能被有损转码——这是已知限制。
- 裸 ADTS `.aac` 文件与 MP3 共享 MPEG 音频帧同步字；通过 layer 位区分两者。

## 编码说明

对已编码文件加速必然需要解码并重新编码音频流，因此每个被处理的文件都会经历一次重编码。为避免 ffmpeg 默认编码参数导致的音质损失，本工具对每种格式显式指定编码器：

| 格式 | 编码参数                         |
| ---- | -------------------------------- |
| FLAC | `flac`（无损）                   |
| WAV  | `pcm_s24le`（无损）              |
| ALAC | `alac`（无损）                   |
| MP3  | `libmp3lame -q:a 2`（VBR ~190k） |
| OGG  | `libvorbis -q:a 6`（~192k）      |
| OPUS | `libopus -b:a 160k`              |
| AAC  | `aac -b:a 192k`                  |
| WMA  | `wmav2 -b:a 192k`                |

对于有损格式，输出码率会被钳制在源码率（通过 ffprobe 检测）以内，确保加速后文件体积不会膨胀；源码率不低于上表默认值时使用默认高质量参数。若 ffprobe 不可用，则所有文件使用默认参数。

内嵌封面图（attached pic）会因 `-vn` 被丢弃，因为视频流无法在重编码过程中保留。

这需要 ffmpeg 构建包含 `libmp3lame`、`libvorbis` 和 `libopus` 编码器（大多数完整版构建均已包含）。

## 系统要求

- 必须安装 FFmpeg 并配置在系统 PATH 环境变量中。
- ffprobe（可选，通常随 ffmpeg 一起发行）：用于区分 mp4 家族文件中的 ALAC 与 AAC，以及将有损输出码率钳制在源码率。
