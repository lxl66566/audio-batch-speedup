#![warn(clippy::cargo)]

use std::{
    fs::File,
    io::Read,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bitflags::bitflags;
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;
use youpipe::{Workload, pipe};

bitflags! {
    /// Represents the supported audio formats for processing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AudioFormat: u32 {
        /// Ogg Vorbis format.
        const OGG = 1 << 0;
        /// MPEG Audio Layer III (MP3) format.
        const MP3 = 1 << 1;
        /// Waveform Audio File Format (WAV).
        const WAV = 1 << 2;
        /// Free Lossless Audio Codec (FLAC) format.
        const FLAC = 1 << 3;
        /// Advanced Audio Coding (AAC) format (often in MP4 containers).
        const AAC = 1 << 4;
        /// Opus Interactive Audio Codec (often in Ogg or WebM containers).
        const OPUS = 1 << 5;
        /// Apple Lossless Audio Codec (ALAC) format.
        const ALAC = 1 << 6;
        /// Windows Media Audio (WMA) format.
        const WMA = 1 << 7;
        /// All supported formats.
        const ALL = Self::OGG.bits() | Self::MP3.bits() | Self::WAV.bits() | Self::FLAC.bits() | Self::AAC.bits() | Self::OPUS.bits() | Self::ALAC.bits() | Self::WMA.bits();
    }
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self::ALL
    }
}

impl std::str::FromStr for AudioFormat {
    type Err = String;

    /// Parses a format name (case-insensitive). Accepts codec names, common
    /// file extensions (`m4a`), and `all` for every supported format.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "ogg" => Ok(Self::OGG),
            "mp3" => Ok(Self::MP3),
            "wav" => Ok(Self::WAV),
            "flac" => Ok(Self::FLAC),
            "aac" | "m4a" => Ok(Self::AAC),
            "opus" => Ok(Self::OPUS),
            "alac" => Ok(Self::ALAC),
            "wma" => Ok(Self::WMA),
            "all" => Ok(Self::ALL),
            other => Err(format!(
                "Unsupported format: {other}. Supported formats are: \
                 ogg, mp3, wav, flac, aac, opus, alac, wma, all."
            )),
        }
    }
}

/// Detects the audio format of a file based on its magic bytes or file
/// extension.
///
/// # Arguments
///
/// * `path` - The path to the audio file.
///
/// # Returns
///
/// * `Option<AudioFormat>` - The detected audio format, or `None` if it cannot
///   be determined.
fn detect_audio_format(path: &Path) -> Option<AudioFormat> {
    // Try to detect by magic bytes first
    let mut file = File::open(path).ok()?;
    let mut buffer = [0; 12]; // Read enough bytes for common headers
    file.read_exact(&mut buffer).ok()?;

    // OGG (OggS)
    if &buffer[0..4] == b"OggS" {
        return Some(AudioFormat::OGG);
    }
    // MP3 (ID3 tag or MPEG frame sync: 11 sync bits set, i.e. 0xFF Ex/FA/FB)
    if &buffer[0..3] == b"ID3" || (buffer[0] == 0xFF && (buffer[1] & 0xE0) == 0xE0) {
        return Some(AudioFormat::MP3);
    }
    // WAV (RIFF header with WAVE)
    if &buffer[0..4] == b"RIFF" && &buffer[8..12] == b"WAVE" {
        return Some(AudioFormat::WAV);
    }
    // FLAC (fLaC)
    if &buffer[0..4] == b"fLaC" {
        return Some(AudioFormat::FLAC);
    }
    // AAC (often in MP4/M4A containers, which start with 'ftyp' or 'moov')
    // This is harder to detect purely by magic bytes without parsing the
    // container. We'll rely more on extension for AAC/M4A.
    // OPUS (often in Ogg containers, so OggS will catch it, or WebM)
    // ALAC (often in MP4/M4A containers)
    // WMA (ASF header)
    if buffer[0..4] == [0x30, 0x26, 0xB2, 0x75] {
        // GUID for ASF header
        return Some(AudioFormat::WMA);
    }

    // Fallback to file extension
    if let Some(extension) = path.extension().and_then(|s| s.to_str())
        && let Ok(format) = extension.parse::<AudioFormat>()
        && format != AudioFormat::ALL
    {
        return Some(format);
    }

    None
}

/// Probes the audio bitrate (bits/s) of a file with ffprobe.
///
/// Returns `None` if the bitrate cannot be determined (ffprobe failure,
/// missing stream, or an unparseable/`N/A` value).
fn probe_audio_bitrate(path: &Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=bit_rate:format=bit_rate")
        .arg("-of")
        .arg("csv=p=0")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().trim_matches(',').parse::<u64>().ok())
        .find(|&bitrate| bitrate > 0)
}

/// Returns ffmpeg encoder arguments for the given format.
///
/// Speeding up requires decoding + re-encoding (a filter cannot run on a
/// copied stream), so ffmpeg's default per-container encoder settings would
/// silently degrade quality. Lossless formats are re-encoded losslessly.
/// Lossy formats use high-quality settings to minimize generation loss, but
/// the bitrate is capped at `source_bitrate` (when known) so that the output
/// can never grow beyond what the speed change alone would yield.
fn encoder_args(format: AudioFormat, source_bitrate: Option<u64>) -> Vec<String> {
    if format == AudioFormat::FLAC {
        return vec!["-c:a".into(), "flac".into()];
    }
    if format == AudioFormat::WAV {
        return vec!["-c:a".into(), "pcm_s24le".into()];
    }
    if format == AudioFormat::ALAC {
        return vec!["-c:a".into(), "alac".into()];
    }

    // (encoder, default quality args, approximate default bitrate in kbps)
    let (codec, default_args, default_kbps): (&str, &[&str], u64) = if format == AudioFormat::MP3 {
        ("libmp3lame", &["-q:a", "2"], 190)
    } else if format == AudioFormat::OGG {
        ("libvorbis", &["-q:a", "6"], 192)
    } else if format == AudioFormat::OPUS {
        ("libopus", &["-b:a", "160k"], 160)
    } else if format == AudioFormat::AAC {
        ("aac", &["-b:a", "192k"], 192)
    } else if format == AudioFormat::WMA {
        ("wmav2", &["-b:a", "192k"], 192)
    } else {
        return Vec::new();
    };

    let mut args = vec!["-c:a".to_string(), codec.to_string()];
    match source_bitrate {
        Some(bitrate) if bitrate < default_kbps * 1000 => {
            args.push("-b:a".to_string());
            args.push(bitrate.to_string());
        }
        _ => args.extend(default_args.iter().map(|&s| s.to_string())),
    }
    args
}

/// The minimum/maximum tempo a single `atempo` filter instance accepts.
const ATEMPO_MIN: f32 = 0.5;
const ATEMPO_MAX: f32 = 100.0;

/// Builds an ffmpeg audio filter chain for the given speed multiplier.
///
/// A single `atempo` instance only accepts values in `[0.5, 100]`, so
/// out-of-range speeds are decomposed into a chain of `atempo` filters.
///
/// Returns `None` if `speed` is not a positive finite number.
fn build_atempo_filter(speed: f32) -> Option<String> {
    if !speed.is_finite() || speed <= 0.0 {
        return None;
    }
    let mut parts = Vec::new();
    let mut remaining = speed;
    while remaining > ATEMPO_MAX {
        parts.push(format!("atempo={ATEMPO_MAX}"));
        remaining /= ATEMPO_MAX;
    }
    while remaining < ATEMPO_MIN {
        parts.push(format!("atempo={ATEMPO_MIN}"));
        remaining /= ATEMPO_MIN;
    }
    parts.push(format!("atempo={remaining}"));
    Some(parts.join(","))
}

/// Processes one audio file in place: re-encodes it sped up by `atempo_filter`
/// into a sibling temp file, then renames over the original.
///
/// Failures are logged and counted instead of propagated, so one bad file
/// never aborts the batch.
fn process_file(
    path: &Path,
    atempo_filter: &str,
    formats: AudioFormat,
    can_probe: bool,
    pb: &ProgressBar,
    error_count: &AtomicUsize,
    skipped_count: &AtomicUsize,
) {
    // Suspend the progress bar while logging so log lines don't interleave
    // with the bar's redraw.
    macro_rules! log_suspended {
        ($level:ident, $($arg:tt)*) => {
            pb.suspend(|| log::$level!($($arg)*))
        };
    }

    let Some(detected_format) = detect_audio_format(path) else {
        log_suspended!(
            debug,
            "Skipping file (format not detected): {}",
            path.display()
        );
        skipped_count.fetch_add(1, Ordering::Relaxed);
        return;
    };

    if !formats.contains(detected_format) {
        log_suspended!(
            debug,
            "Skipping file (format not selected): {}",
            path.display()
        );
        skipped_count.fetch_add(1, Ordering::Relaxed);
        return;
    }

    // Create a uniquely-named temp file in the same directory (so the
    // final rename stays on one filesystem). The original extension is
    // kept as the suffix so ffmpeg can infer the output container.
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let temp_file = match tempfile::Builder::new()
        .prefix(".abs_")
        .suffix(&suffix)
        .tempfile_in(path.parent().unwrap_or(Path::new(".")))
    {
        Ok(f) => f,
        Err(e) => {
            log_suspended!(
                error,
                "Failed to create temp file for {}: {}",
                path.display(),
                e
            );
            error_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let output_file = temp_file.path().to_path_buf();

    let source_bitrate = can_probe.then(|| probe_audio_bitrate(path)).flatten();

    let status = Command::new("ffmpeg")
        // Batch mode keeps one ffmpeg per CPU core busy; intra-process
        // threading would only oversubscribe the machine, so decode,
        // filtergraph and encode are all pinned to a single thread.
        .arg("-threads")
        .arg("1")
        .arg("-filter_threads")
        .arg("1")
        .arg("-i")
        .arg(path)
        .arg("-filter:a")
        .arg(atempo_filter)
        .arg("-vn")
        .args(encoder_args(detected_format, source_bitrate))
        .arg("-threads")
        .arg("1")
        .arg("-map_metadata")
        .arg("0")
        .arg(&output_file)
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .status();

    match status {
        Ok(exit_status) => {
            if exit_status.success() {
                if let Err(e) = std::fs::rename(&output_file, path) {
                    log_suspended!(
                        error,
                        "Error renaming file from {} to {}: {}",
                        output_file.display(),
                        path.display(),
                        e
                    );
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                log_suspended!(
                    error,
                    "ffmpeg failed for {}. Exit code: {:?}",
                    path.display(),
                    exit_status.code()
                );
                error_count.fetch_add(1, Ordering::Relaxed);
                // The temp file is removed automatically on drop.
            }
        }
        Err(e) => {
            log_suspended!(
                error,
                "Error executing ffmpeg for {}: {}",
                path.display(),
                e
            );
            error_count.fetch_add(1, Ordering::Relaxed);
            // The temp file is removed automatically on drop.
        }
    }
}

/// Process all audio files in the specified folder recursively with the given
/// speed multiplier.
///
/// # Arguments
///
/// * `folder` - Path to the folder containing audio files
/// * `speed` - Speed multiplier (e.g., 1.5 for 1.5x speed)
/// * `formats` - A bitflags object indicating which audio formats to process.
///
/// # Returns
///
/// * `Result<()>` - Ok(()) if successful, or an error if processing fails
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// use audio_batch_speedup::{process_audio_files, AudioFormat};
///
/// let folder = Path::new("path/to/audio/files");
/// let speed = 1.5;
/// let formats = AudioFormat::OGG | AudioFormat::MP3;
/// process_audio_files(folder, speed, formats).unwrap();
/// ```
pub fn process_audio_files(
    folder: impl AsRef<Path>,
    speed: f32,
    formats: AudioFormat,
) -> std::io::Result<()> {
    let folder = folder.as_ref();

    let Some(atempo_filter) = build_atempo_filter(speed) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Invalid speed multiplier: {speed}. Must be a positive finite number."),
        ));
    };

    // Fail fast if ffmpeg is unavailable instead of erroring once per file.
    match Command::new("ffmpeg").arg("-version").output() {
        Ok(output) if output.status.success() => {}
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "ffmpeg is not installed or not available in PATH",
            ));
        }
    }

    // ffprobe is used to cap lossy output bitrate at the source bitrate. It
    // is optional: without it we fall back to the default encoder settings.
    let can_probe = Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !can_probe {
        log::warn!("ffprobe not found in PATH; falling back to default encoder settings");
    }

    // Collect all files that need to be processed
    let files: Vec<_> = WalkDir::new(folder)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file()) // Only count files for the progress bar
        .collect();

    let process_pb = ProgressBar::new(files.len() as u64);
    process_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("Internal Error: Failed to set progress bar style")
            .progress_chars("#>-"),
    );

    let error_count = Arc::new(AtomicUsize::new(0));
    let skipped_count = Arc::new(AtomicUsize::new(0));

    // One single-threaded ffmpeg per core saturates the CPU: the pool defaults
    // to one worker per core and each worker blocks on one ffmpeg at a time.
    // File durations are uneven, so allow fine-grained stealing to shrink the
    // tail.
    let item_error_count = Arc::clone(&error_count);
    let item_skipped_count = Arc::clone(&skipped_count);
    let item_pb = process_pb.clone();
    pipe(files)
        .with_workload(Workload::Unbalanced)
        .for_each(move |entry| {
            process_file(
                entry.path(),
                &atempo_filter,
                formats,
                can_probe,
                &item_pb,
                &item_error_count,
                &item_skipped_count,
            );
            item_pb.inc(1);
        });

    process_pb.finish_with_message("Processing complete!");

    let errors = error_count.load(Ordering::Relaxed);
    let skipped = skipped_count.load(Ordering::Relaxed);

    if errors > 0 {
        log::error!("Finished with {} errors.", errors);
        return Err(std::io::Error::other(format!(
            "{errors} file(s) failed to process"
        )));
    }
    if skipped > 0 {
        log::info!("Skipped {} files.", skipped);
    }

    Ok(())
}
