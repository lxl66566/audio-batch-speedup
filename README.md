# audio-batch-speedup

English | [简体中文](./README_zh-CN.md)

A Rust library and command-line tool for batch processing and speeding up audio files using ffmpeg.

This crate was initially designed for visual novel audio speedup, as visual novels typically use a large number of OGG files for their audio system. It has since been extended to support a wider range of audio formats.

## Features

- Parallel processing of multiple audio files recursively, maximizing speed by utilizing multiple CPU cores.
- Configurable speed adjustment and parallelism.
- Support for multiple audio formats.
- Format detection: prioritizes detecting the audio format from file content (magic bytes), falling back to the file extension if the content is inconclusive. See [Format detection](#format-detection).

## Usage

### Command-line (bin) Usage

First, install the package. You can install it from [Release](https://github.com/lxl66566/audio-batch-speedup/releases) manually or use [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```bash
cargo binstall audio-batch-speedup
```

Then, run the executable:

```bash
abs --input /path/to/your/audio/folder --speed 1.5 --formats ogg,mp3     # speed up all OGG and MP3 files in /path/to/your/audio/folder by 1.5x
abs --input /path/to/your/audio/folder --speed 2 --jobs 4               # speed up all supported files with 4 parallel jobs
```

**Arguments:**

- `-i, --input <INPUT>`: Path to the folder containing audio files.
- `-s, --speed <SPEED>`: Audio speed multiplier (e.g., `1.5` for 1.5x speed).
- `-f, --formats <FORMATS>`: Comma-separated list of audio formats to process (e.g., `ogg,mp3,wav`). Use `all` to process all supported formats.
  Supported formats: `ogg`, `mp3`, `wav`, `flac`, `aac`, `opus`, `alac`, `wma`.
  Default: `all`.
- `-j, --jobs <JOBS>`: Number of parallel ffmpeg jobs. Default: use all CPU core.
- `-v, --verbose`: Print one log line per file (processed/skipped/failed) instead of the progress bar.

**Logging:** the log level defaults to `info` and can be overridden with the `SPDLOG_RS_LEVEL` environment variable (e.g. `SPDLOG_RS_LEVEL=debug abs ...`).

**Failure reporting:** each failed file is reported on one terminal line with its path and the ffmpeg exit code. Every failure appends the exact ffmpeg command and the last lines of the stderr to a shared `abs-fail-*.log` file in the system temp directory, whose path is printed at the end of the run. The tool exits with a non-zero code if any file failed.

### Library (lib) Usage

Add `audio-batch-speedup` to your `Cargo.toml`:

```toml
[dependencies]
audio-batch-speedup = "0.1" # Use the latest version
```

Then, in your Rust code:

```rust
use std::path::Path;
use audio_batch_speedup::{process_audio_files, AudioFormat, ProcessOptions};

fn main() -> std::io::Result<()> {
    let folder = Path::new("path/to/your/audio/files");
    let speed = 1.5;

    // Process OGG and MP3 files with one worker per CPU core
    let options = ProcessOptions::new(speed).formats(AudioFormat::OGG | AudioFormat::MP3);
    let summary = process_audio_files(folder, options)?;

    // Process all supported formats, limited to 4 parallel jobs, with one
    // log line per file instead of the progress bar
    let jobs = std::num::NonZeroUsize::new(4).unwrap();
    let options = ProcessOptions::new(speed).jobs(jobs).verbose();
    let summary = process_audio_files(folder, options)?;

    // An `Err` only means the run could not be started (invalid speed, ffmpeg
    // unavailable); per-file failures are collected in `summary.failed`.
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

## Format detection

Formats are detected from file content (magic bytes) first, falling back to the file extension when the content is inconclusive:

- Ogg containers are classified by their first page: an `OpusHead` ID header means Opus, anything else is treated as Vorbis (OGG).
- The mp4 family (`.m4a`) can hold either AAC or ALAC, which magic bytes cannot distinguish; ffprobe tells them apart by codec name. When ffprobe is missing, `.m4a` files are always treated as AAC, so ALAC files may be re-encoded lossily. This is a known limitation.
- Bare ADTS `.aac` files and MP3 files share the MPEG audio frame sync word; the layer bits distinguish the two.

## Encoding

Speeding up an encoded file always requires decoding and re-encoding the audio stream, so every processed file goes through one re-encode generation. To avoid the quality loss of ffmpeg's default per-container encoder settings, each format is re-encoded explicitly:

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

For lossy formats, the output bitrate is capped at the source bitrate (detected via ffprobe) so that a speedup never inflates the file size; files with a source bitrate at or above the defaults use the settings above. If ffprobe is unavailable, the default settings are used for all files.

Embedded cover art (attached pictures) is dropped by `-vn`, because the video stream cannot be kept through the re-encode.

This requires an ffmpeg build with the `libmp3lame`, `libvorbis`, and `libopus` encoders enabled (most full builds include them).

## Requirements

- FFmpeg must be installed and available in the system PATH.
- ffprobe (optional, usually shipped with ffmpeg): used to distinguish ALAC from AAC in mp4-family files and to cap lossy output bitrates at the source bitrate.
