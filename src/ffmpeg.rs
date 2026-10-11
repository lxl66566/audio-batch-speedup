//! ffmpeg/ffprobe invocation helpers: argument building, probing, and
//! failure diagnostics.

use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

use crate::format::AudioFormat;

/// Probes the codec name of the first audio stream with ffprobe.
///
/// Returns `None` if the codec cannot be determined (ffprobe failure or a
/// missing stream).
pub(crate) fn probe_audio_codec(path: &Path) -> Option<String> {
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
pub(crate) fn probe_audio_bitrate(path: &Path) -> Option<u64> {
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
pub(crate) fn encoder_args(format: AudioFormat, source_bitrate: Option<u64>) -> Vec<String> {
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

/// The minimum/maximum tempo a single `atempo` filter instance accepts.
const ATEMPO_MIN: f32 = 0.5;
const ATEMPO_MAX: f32 = 100.0;

/// Builds an ffmpeg audio filter chain for the given speed multiplier.
///
/// A single `atempo` instance only accepts values in `[0.5, 100]`, so
/// out-of-range speeds are decomposed into a chain of `atempo` filters.
///
/// Returns `None` if `speed` is not a positive finite number.
pub(crate) fn build_atempo_filter(speed: f32) -> Option<String> {
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

/// Renders an ffmpeg invocation (program name plus args) for the failure log.
///
/// Arguments containing whitespace or commas (or empty ones) are double-quoted
/// so the logged line can be pasted into cmd, bash, or PowerShell as-is;
/// PowerShell splits unquoted comma-separated args into several arguments.
pub(crate) fn command_line(args: &[OsString]) -> String {
    fn push_arg(line: &mut String, arg: &OsStr) {
        let text = arg.to_string_lossy();
        if text.is_empty() || text.chars().any(|c| c.is_whitespace() || c == ',') {
            line.push('"');
            line.push_str(&text);
            line.push('"');
        } else {
            line.push_str(&text);
        }
    }

    let mut line = String::from("ffmpeg");
    for arg in args {
        line.push(' ');
        push_arg(&mut line, arg);
    }
    line
}

/// Negated `MKTAG`, the encoding of libavutil's four-character `AVERROR`
/// tags; ASCII tag bytes keep the value inside `i32` range.
const fn averror_tag(a: u8, b: u8, c: u8, d: u8) -> i32 {
    -((a as i32) | ((b as i32) << 8) | ((c as i32) << 16) | ((d as i32) << 24))
}

/// AVERROR_INVALIDDATA — corrupt or unsupported input data.
const AVERROR_INVALIDDATA: i32 = averror_tag(b'I', b'N', b'D', b'A');
/// AVERROR(ENOENT).
const AVERROR_ENOENT: i32 = -2;
/// AVERROR(EIO).
const AVERROR_EIO: i32 = -5;
/// AVERROR(EINVAL).
const AVERROR_EINVAL: i32 = -22;

/// A short cause name for a known ffmpeg exit status, or `None` when the
/// status is unrecognized.
///
/// ffmpeg propagates libav errors as its exit status: a negative `AVERROR`
/// value, either a negated errno or a negated four-character tag. The exact
/// status always goes to the failure log; the name keeps the per-file
/// terminal line short.
pub(crate) fn exit_status_cause(code: Option<i32>) -> Option<&'static str> {
    match code? {
        AVERROR_INVALIDDATA => Some("invalid data"),
        AVERROR_ENOENT => Some("file not found"),
        AVERROR_EIO => Some("I/O error"),
        AVERROR_EINVAL => Some("invalid argument"),
        _ => None,
    }
}

/// How many trailing stderr lines are kept per failure-log entry.
const FAILED_STDERR_LINES: usize = 5;

/// Returns at most the last `max_lines` lines of `text`.
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

/// Formats one entry of the run's shared failure log: the exact ffmpeg
/// command and the last lines of its stderr.
pub(crate) fn failure_entry(
    source: &Path,
    command: &str,
    exit_code: Option<i32>,
    stderr: &str,
) -> String {
    let exit_code = exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string());
    format!(
        "=== {} (exit code {exit_code})\ncommand: {command}\n{}\n\n",
        source.display(),
        tail_lines(stderr, FAILED_STDERR_LINES)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn atempo_rejects_invalid_speeds() {
        assert!(build_atempo_filter(0.0).is_none());
        assert!(build_atempo_filter(-1.5).is_none());
        assert!(build_atempo_filter(f32::NAN).is_none());
        assert!(build_atempo_filter(f32::INFINITY).is_none());
    }

    #[test]
    fn known_exit_statuses_map_to_cause_names() {
        // -1094995529 is ffmpeg's AVERROR_INVALIDDATA as an exit status.
        const INVALID_DATA_EXIT_CODE: i32 = -1_094_995_529;
        assert_eq!(
            exit_status_cause(Some(INVALID_DATA_EXIT_CODE)),
            Some("invalid data")
        );
        assert_eq!(exit_status_cause(Some(-2)), Some("file not found"));
        assert_eq!(exit_status_cause(Some(1)), None);
        assert_eq!(exit_status_cause(None), None);
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
    fn command_line_quotes_args_with_whitespace_or_commas() {
        let args: Vec<_> = [
            "-nostdin",
            "-i",
            "a b/two.mp3",
            "-filter:a",
            "atempo=0.5,atempo=0.5",
            "-y",
        ]
        .map(OsString::from)
        .into();
        assert_eq!(
            command_line(&args),
            "ffmpeg -nostdin -i \"a b/two.mp3\" -filter:a \"atempo=0.5,atempo=0.5\" -y"
        );
    }

    #[test]
    fn failure_entry_contains_header_command_and_stderr_tail() {
        const INVALID_DATA_EXIT_CODE: i32 = -1_094_995_529;
        let entry = failure_entry(
            Path::new("Z:/a b/corrupt.mp3"),
            "ffmpeg -nostdin -i x",
            Some(INVALID_DATA_EXIT_CODE),
            "l1\nl2\nl3\nl4\nl5\nl6\nl7\n",
        );
        assert!(entry.starts_with(&format!(
            "=== Z:/a b/corrupt.mp3 (exit code {INVALID_DATA_EXIT_CODE})\n"
        )));
        assert!(entry.contains("command: ffmpeg -nostdin -i x\n"));
        assert!(
            entry.ends_with(&format!(
                "{}\n\n",
                tail_lines("l1\nl2\nl3\nl4\nl5\nl6\nl7\n", FAILED_STDERR_LINES)
            )),
            "entry must end with the stderr tail: {entry}"
        );
        let unknown = failure_entry(Path::new("a"), "cmd", None, "");
        assert!(unknown.starts_with("=== a (exit code unknown)\n"));
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
}
