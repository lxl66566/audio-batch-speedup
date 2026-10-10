#![warn(clippy::cargo)]

use std::{
    ffi::OsStr,
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
                "Unsupported format: {other}. Supported formats are: ogg, mp3, wav, flac, aac, \
                 opus, alac, wma, all."
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
/// * `can_probe` - Whether ffprobe may be used to disambiguate formats that neither magic bytes nor
///   the extension can distinguish (ALAC vs AAC in the mp4 family).
///
/// # Returns
///
/// * `Option<AudioFormat>` - The detected audio format, or `None` if it cannot be determined.
fn detect_audio_format(path: &Path, can_probe: bool) -> Option<AudioFormat> {
    // Try to detect by magic bytes first. Files shorter than the read window
    // are judged on the bytes they actually contain.
    let mut file = File::open(path).ok()?;
    // Read enough bytes for common headers and the first Ogg packet header.
    let mut buffer = [0; 64];
    let n = file.read(&mut buffer).ok()?;
    let head = &buffer[..n];

    // OGG (OggS). Opus carries an "OpusHead" ID header as the first packet
    // of the first page, starting at offset 28 (27-byte Ogg page header +
    // 1 segment-table entry); anything else in an Ogg container is treated
    // as Vorbis.
    if head.starts_with(b"OggS") {
        return Some(match head.get(28..36) {
            Some(b"OpusHead") => AudioFormat::OPUS,
            _ => AudioFormat::OGG,
        });
    }
    // MP3 (ID3 tag)
    if head.starts_with(b"ID3") {
        return Some(AudioFormat::MP3);
    }
    // MPEG audio frame sync (11 set bits). The layer bits separate the two
    // families sharing this sync word: ADTS AAC uses layer 00 (a reserved
    // value for MPEG audio) while MP3 uses layer I/II/III.
    if let [0xff, b1, ..] = head {
        if b1 & 0xf6 == 0xf0 {
            return Some(AudioFormat::AAC);
        }
        if b1 & 0xe0 == 0xe0 && b1 & 0x06 != 0 {
            return Some(AudioFormat::MP3);
        }
    }
    // WAV (RIFF header with WAVE)
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return Some(AudioFormat::WAV);
    }
    // FLAC (fLaC)
    if head.starts_with(b"fLaC") {
        return Some(AudioFormat::FLAC);
    }
    // AAC (often in MP4/M4A containers, which start with 'ftyp' or 'moov')
    // This is harder to detect purely by magic bytes without parsing the
    // container. We'll rely more on extension for AAC/M4A.
    // WMA (ASF header)
    if head.starts_with(&[0x30, 0x26, 0xb2, 0x75]) {
        // GUID for ASF header
        return Some(AudioFormat::WMA);
    }

    // Fallback to file extension. The mp4 family (`.m4a`/`.aac`) can hold
    // either AAC or ALAC, which magic bytes cannot distinguish, so consult
    // ffprobe when it is available; otherwise assume AAC.
    if let Some(extension) = path.extension().and_then(|s| s.to_str())
        && let Ok(format) = extension.parse::<AudioFormat>()
        && format != AudioFormat::ALL
    {
        if format == AudioFormat::AAC
            && can_probe
            && probe_audio_codec(path).as_deref() == Some("alac")
        {
            return Some(AudioFormat::ALAC);
        }
        return Some(format);
    }

    None
}

/// Probes the codec name of the first audio stream with ffprobe.
///
/// Returns `None` if the codec cannot be determined (ffprobe failure or a
/// missing stream).
fn probe_audio_codec(path: &Path) -> Option<String> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=codec_name")
        .arg("-of")
        .arg("csv=p=0")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let codec = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!codec.is_empty()).then_some(codec)
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
        },
        _ => args.extend(default_args.iter().map(|&s| s.to_string())),
    }
    args
}

/// Returns at most the last `max_lines` lines of `text`, to keep failure
/// logs concise.
fn tail_lines(text: &str, max_lines: usize) -> &str {
    if max_lines == 0 {
        return "";
    }
    let start = text
        .char_indices()
        .rev()
        .filter(|&(_, ch)| ch == '\n')
        .nth(max_lines)
        .map_or(0, |(i, _)| i + 1);
    &text[start..]
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

/// Prefix of the temp files created next to each source file.
const TEMP_PREFIX: &str = ".abs_";

/// Whether `name` belongs to this crate's temp-file namespace. Leftovers
/// from an interrupted run must be excluded from processing because their
/// content is already sped up.
fn is_temp_leftover(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with(TEMP_PREFIX)
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

    let Some(detected_format) = detect_audio_format(path, can_probe) else {
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
        .prefix(TEMP_PREFIX)
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
        },
    };
    let output_file = temp_file.path().to_path_buf();

    // Only lossy re-encodes consume the source bitrate; lossless formats
    // ignore it, so their ffprobe round trip is skipped.
    let lossy_formats = AudioFormat::MP3
        | AudioFormat::OGG
        | AudioFormat::OPUS
        | AudioFormat::AAC
        | AudioFormat::WMA;
    let source_bitrate = (can_probe && lossy_formats.contains(detected_format))
        .then(|| probe_audio_bitrate(path))
        .flatten();

    // stdout/stderr are captured instead of inherited so ffmpeg diagnostics
    // never interleave with the progress bar; they are only logged on
    // failure.
    let output = Command::new("ffmpeg")
        // Parallel workers run many ffmpeg processes at once; none of them
        // may read the terminal's stdin.
        .arg("-nostdin")
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
        .output();

    match output {
        Ok(output) if output.status.success() => {
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
        },
        Ok(output) => {
            // `-loglevel error` keeps stderr short; cap it anyway so a
            // pathological file cannot flood the log.
            let stderr_text = String::from_utf8_lossy(&output.stderr);
            let stderr = tail_lines(&stderr_text, 5);
            log_suspended!(
                error,
                "ffmpeg failed for {} (exit code {:?}): {}",
                path.display(),
                output.status.code(),
                stderr
            );
            error_count.fetch_add(1, Ordering::Relaxed);
            // The temp file is removed automatically on drop.
        },
        Err(e) => {
            log_suspended!(
                error,
                "Error executing ffmpeg for {}: {}",
                path.display(),
                e
            );
            error_count.fetch_add(1, Ordering::Relaxed);
            // The temp file is removed automatically on drop.
        },
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
///
/// use audio_batch_speedup::{AudioFormat, process_audio_files};
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
        Ok(output) if output.status.success() => {},
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "ffmpeg is not installed or not available in PATH",
            ));
        },
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

    // Collect all files that need to be processed, excluding this crate's
    // own leftover temp files (already sped up, would compound the speed).
    let files: Vec<_> = WalkDir::new(folder)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file()) // Only count files for the progress bar
        .filter(|e| !is_temp_leftover(e.file_name()))
        .collect();

    let process_pb = ProgressBar::new(files.len() as u64);
    process_pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) \
                 {msg}",
            )
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
        log::error!("Finished with {errors} errors.");
        return Err(std::io::Error::other(format!(
            "{errors} file(s) failed to process"
        )));
    }
    if skipped > 0 {
        log::info!("Skipped {skipped} files.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `bytes` to `dir/name` and returns the path. The caller keeps
    /// the temp dir alive for as long as the path is used.
    fn write_in(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    #[test]
    fn atempo_in_range_is_a_single_filter() {
        assert_eq!(build_atempo_filter(1.5).as_deref(), Some("atempo=1.5"));
        assert_eq!(build_atempo_filter(0.5).as_deref(), Some("atempo=0.5"));
        assert_eq!(build_atempo_filter(100.0).as_deref(), Some("atempo=100"));
    }

    #[test]
    fn atempo_out_of_range_speeds_decompose_into_a_chain() {
        for speed in [0.4_f32, 0.1, 0.25, 120.0, 10_000.0] {
            let chain =
                build_atempo_filter(speed).unwrap_or_else(|| panic!("no filter chain for {speed}"));
            let values: Vec<f32> = chain
                .split(',')
                .map(|part| {
                    part.strip_prefix("atempo=")
                        .unwrap_or_else(|| panic!("bad segment {part}"))
                        .parse::<f32>()
                        .expect("numeric tempo")
                })
                .collect();
            assert!(values.len() > 1, "expected a chain for {speed}: {chain}");
            let product: f32 = values.iter().product();
            assert!(
                (product - speed).abs() < 1e-3,
                "chain product {product} != {speed}"
            );
            for &value in &values {
                assert!(
                    (ATEMPO_MIN..=ATEMPO_MAX).contains(&value),
                    "segment {value} out of range for {speed}"
                );
            }
        }
    }

    #[test]
    fn tail_lines_keeps_only_the_trailing_lines() {
        let text = "l1\nl2\nl3\n";
        assert_eq!(tail_lines(text, 5), text);
        assert_eq!(tail_lines(text, 2), "l2\nl3\n");
        assert_eq!(tail_lines(text, 1), "l3\n");
        assert_eq!(tail_lines("", 3), "");
        assert_eq!(tail_lines("l1\nl2\nl3\n", 0), "");
    }

    #[test]
    fn atempo_rejects_invalid_speeds() {
        assert!(build_atempo_filter(0.0).is_none());
        assert!(build_atempo_filter(-1.5).is_none());
        assert!(build_atempo_filter(f32::NAN).is_none());
        assert!(build_atempo_filter(f32::INFINITY).is_none());
    }

    #[test]
    fn format_parsing_accepts_names_alias_and_all() {
        assert_eq!("OGG".parse::<AudioFormat>().ok(), Some(AudioFormat::OGG));
        assert_eq!(" Mp3 ".parse::<AudioFormat>().ok(), Some(AudioFormat::MP3));
        assert_eq!("m4a".parse::<AudioFormat>().ok(), Some(AudioFormat::AAC));
        assert_eq!("all".parse::<AudioFormat>().ok(), Some(AudioFormat::ALL));
    }

    #[test]
    fn format_parse_error_lists_canonical_names() {
        let err = "flac2".parse::<AudioFormat>().unwrap_err();
        for name in [
            "ogg", "mp3", "wav", "flac", "aac", "opus", "alac", "wma", "all",
        ] {
            assert!(err.contains(name), "missing {name} in error: {err}");
        }
        assert!(!err.contains("m4a"), "alias leaked into error: {err}");
    }

    #[test]
    fn lossless_encoder_args_are_fixed() {
        let cases = [
            (AudioFormat::FLAC, "flac"),
            (AudioFormat::WAV, "pcm_s24le"),
            (AudioFormat::ALAC, "alac"),
        ];
        for (format, codec) in cases {
            for bitrate in [None, Some(64_000), Some(1_000_000)] {
                assert_eq!(
                    encoder_args(format, bitrate),
                    ["-c:a", codec].map(String::from),
                    "bitrate must not affect {codec}"
                );
            }
        }
    }

    #[test]
    fn lossy_encoder_args_cap_bitrate_below_default() {
        // MP3 defaults to -q:a 2 (~190 kbps); a lower source bitrate switches
        // to an explicit -b:a cap.
        assert_eq!(
            encoder_args(AudioFormat::MP3, Some(128_000)),
            ["-c:a", "libmp3lame", "-b:a", "128000"].map(String::from)
        );
        assert_eq!(
            encoder_args(AudioFormat::OPUS, Some(96_000)),
            ["-c:a", "libopus", "-b:a", "96000"].map(String::from)
        );
        // At or above the default (or unknown) the quality preset is kept.
        assert_eq!(
            encoder_args(AudioFormat::MP3, Some(192_000)),
            ["-c:a", "libmp3lame", "-q:a", "2"].map(String::from)
        );
        assert_eq!(
            encoder_args(AudioFormat::MP3, None),
            ["-c:a", "libmp3lame", "-q:a", "2"].map(String::from)
        );
    }

    #[test]
    fn detect_format_by_magic_bytes() {
        let dir = tempfile::tempdir().expect("create temp dir");

        // Ogg Opus: "OpusHead" ID header at offset 28.
        let mut opus = vec![0; 28];
        opus[..4].copy_from_slice(b"OggS");
        opus.extend_from_slice(b"OpusHead");
        opus.extend_from_slice(&[0; 16]);
        let path = write_in(dir.path(), "a.opus", &opus);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::OPUS));

        // Ogg Vorbis: first packet starts at offset 28 (27-byte page header
        // + 1 segment-table entry).
        let mut vorbis = vec![0; 28];
        vorbis[..4].copy_from_slice(b"OggS");
        vorbis.extend_from_slice(b"\x01vorbis");
        vorbis.extend_from_slice(&[0; 16]);
        let path = write_in(dir.path(), "a.ogg", &vorbis);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::OGG));

        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0; 4]);
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(&[0; 8]);
        let path = write_in(dir.path(), "a.wav", &wav);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::WAV));

        let mut flac = b"fLaC".to_vec();
        flac.extend_from_slice(&[0; 8]);
        let path = write_in(dir.path(), "a.flac", &flac);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::FLAC));

        // ASF header GUID.
        let mut wma = vec![
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62,
            0xce, 0x6c,
        ];
        wma.extend_from_slice(&[0; 16]);
        let path = write_in(dir.path(), "a.wma", &wma);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::WMA));

        let mut id3 = b"ID3".to_vec();
        id3.extend_from_slice(&[0; 20]);
        let path = write_in(dir.path(), "a.mp3", &id3);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::MP3));

        // MPEG frame sync with layer bits set (0xFF 0xFB).
        let mut mp3 = vec![0xff, 0xfb, 0x90, 0x00];
        mp3.extend_from_slice(&[0; 20]);
        let path = write_in(dir.path(), "a.mp3", &mp3);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::MP3));

        // ADTS AAC frame sync with layer bits 00 (0xFF 0xF1 / 0xFF 0xF9).
        for b1 in [0xf1, 0xf9] {
            let mut adts = vec![0xff, b1, 0x50, 0x80];
            adts.extend_from_slice(&[0; 20]);
            let path = write_in(dir.path(), "a.aac", &adts);
            assert_eq!(
                detect_audio_format(&path, false),
                Some(AudioFormat::AAC),
                "ADTS sync byte {b1:#x}"
            );
        }
    }

    #[test]
    fn detect_format_on_files_shorter_than_the_header_window() {
        let dir = tempfile::tempdir().expect("create temp dir");
        // A 3-byte ID3 fragment plus the .mp3 extension is still an MP3.
        let path = write_in(dir.path(), "a.mp3", b"ID3");
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::MP3));
        // Truncated magic falls back to the extension.
        let path = write_in(dir.path(), "b.wav", b"RIFF");
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::WAV));
    }

    #[test]
    fn temp_leftover_names_are_excluded_otherwise_hidden_files_are_not() {
        assert!(is_temp_leftover(OsStr::new(".abs_leftover.mp3")));
        assert!(is_temp_leftover(OsStr::new(".abs_")));
        // Only the exact prefix matters, not hidden files in general.
        assert!(!is_temp_leftover(OsStr::new(".hidden.mp3")));
        assert!(!is_temp_leftover(OsStr::new("abs_x.mp3")));
        assert!(!is_temp_leftover(OsStr::new(".mp3")));
    }

    #[test]
    fn detect_format_by_extension_fallback() {
        let dir = tempfile::tempdir().expect("create temp dir");

        // mp4-family containers have no magic check; the extension decides.
        let mut m4a = vec![0, 0, 0, 0x18];
        m4a.extend_from_slice(b"ftypM4A ");
        m4a.extend_from_slice(&[0; 8]);
        let path = write_in(dir.path(), "a.m4a", &m4a);
        assert_eq!(detect_audio_format(&path, false), Some(AudioFormat::AAC));

        // Garbage without a known extension is undetectable.
        let path = write_in(dir.path(), "noext", &[0x01; 20]);
        assert_eq!(detect_audio_format(&path, false), None);
    }
}
