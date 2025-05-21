# audio-batch-speedup

English | [简体中文](./README_zh-CN.md)

A Rust library and command-line tool for batch processing and speeding up audio files using ffmpeg.

This crate was initially designed for visual novel audio speedup, as visual novels typically use a large number of OGG files for their audio system. It has since been extended to support a wider range of audio formats.

## Features

- Parallel processing of multiple audio files recursively, maximizing speed by utilizing multiple CPU cores.
- Configurable speed adjustment.
- Support for multiple audio formats.
- Format detection: Prioritizes detecting audio format from file content (magic bytes) and falls back to file extension if content detection is not possible.

## Usage

### Command-line (bin) Usage

First, install the package. You can install it from [Release](https://github.com/lxl66566/audio-batch-speedup/releases) manually or use [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```bash
cargo binstall audio-batch-speedup
```

Then, run the executable:

```bash
abs --input /path/to/your/audio/folder --speed 1.5 --formats ogg,mp3     # speed up all OGG and MP3 files in /path/to/your/audio/folder by 1.5x
```

**Arguments:**

- `-i, --input <INPUT>`: Path to the folder containing audio files.
- `-s, --speed <SPEED>`: Audio speed multiplier (e.g., `1.5` for 1.5x speed).
- `-f, --formats <FORMATS>`: Comma-separated list of audio formats to process (e.g., `ogg,mp3,wav`). Use `all` to process all supported formats.
  Supported formats: `ogg`, `mp3`, `wav`, `flac`, `aac`, `opus`, `alac`, `wma`.
  Default: `all`.

### Library (lib) Usage

Add `audio-batch-speedup` to your `Cargo.toml`:

```toml
[dependencies]
audio-batch-speedup = "0.1" # Use the latest version
```

Then, in your Rust code:

```rust
use std::path::Path;
use audio_batch_speedup::{process_audio_files, AudioFormat};

fn main() -> std::io::Result<()> {
    let folder = Path::new("path/to/your/audio/files");
    let speed = 1.5;
    // Process OGG and MP3 files
    process_audio_files(folder, speed, AudioFormat::OGG | AudioFormat::MP3)?;

    // Process all supported audio files
    process_audio_files(folder, speed, AudioFormat::ALL)?;

    Ok(())
}
```

## Requirements

- FFmpeg must be installed and available in the system PATH.
