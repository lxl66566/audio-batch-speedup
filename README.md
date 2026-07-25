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

## Encoding

Speeding up requires decoding and re-encoding the audio stream. To avoid the
quality loss of ffmpeg's default per-container encoder settings, each format
is re-encoded explicitly:

| Format | Encoder settings                |
| ------ | ------------------------------- |
| FLAC   | `flac` (lossless)               |
| WAV    | `pcm_s24le` (lossless)          |
| ALAC   | `alac` (lossless)               |
| MP3    | `libmp3lame -q:a 2` (VBR ~190k) |
| OGG    | `libvorbis -q:a 6` (~192k)      |
| OPUS   | `libopus -b:a 160k`             |
| AAC    | `aac -b:a 192k`                 |
| WMA    | `wmav2 -b:a 192k`               |

For lossy formats, the output bitrate is capped at the source bitrate
(detected via ffprobe) so that a speedup never inflates the file size;
files with a source bitrate at or above the defaults use the settings above.
If ffprobe is unavailable, the default settings are used for all files.

This requires an ffmpeg build with the `libmp3lame`, `libvorbis`, and
`libopus` encoders enabled (most full builds include them).

## Requirements

- FFmpeg must be installed and available in the system PATH.
