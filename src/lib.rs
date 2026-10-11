//! Batch speed up audio files by re-encoding them with ffmpeg.

mod detect;
mod ffmpeg;
mod format;

use std::{
    ffi::{OsStr, OsString},
    io::Write,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::Command,
};

use detect::detect_audio_format;
use ffmpeg::{
    build_atempo_filter, command_line, encoder_args, exit_status_cause, failure_entry,
    probe_audio_bitrate,
};
use indicatif::{ProgressBar, ProgressStyle};
use walkdir::WalkDir;
use youpipe::{Workload, pipe};

pub use self::format::AudioFormat;

/// Prefix of the temp files created next to each source file.
const TEMP_PREFIX: &str = ".abs_";

/// Whether `name` belongs to this crate's temp-file namespace. Leftovers
/// from an interrupted run must be excluded from processing because their
/// content is already sped up.
fn is_temp_leftover(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with(TEMP_PREFIX)
}

/// Result of processing one file, returned by the workers and folded into a
/// [`ProcessSummary`] after the pool joins.
enum Outcome {
    Processed,
    Skipped,
    /// The file failed before ffmpeg ran (or at the final rename); `reason`
    /// is a single-line, user-facing cause.
    Failed {
        path: PathBuf,
        reason: String,
    },
    /// ffmpeg itself failed; the command and stderr go to the run's shared
    /// failure log.
    FfmpegFailed {
        path: PathBuf,
        reason: String,
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },
}

impl Outcome {
    /// Logs the failure with the progress bar suspended, and wraps it for
    /// the run summary.
    fn fail(path: &Path, pb: &ProgressBar, reason: String) -> Outcome {
        pb.suspend(|| log::error!("Failed to process {}: {}", path.display(), reason));
        Outcome::Failed {
            path: path.to_path_buf(),
            reason,
        }
    }
}

/// Processes one audio file in place: re-encodes it sped up by `atempo_filter`
/// into a sibling temp file, then renames over the original.
///
/// Failures are returned as an [`Outcome`] instead of propagated, so one bad
/// file never aborts the batch.
fn process_file(
    path: &Path,
    atempo_filter: &str,
    formats: AudioFormat,
    can_probe: bool,
    pb: &ProgressBar,
) -> Outcome {
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
        return Outcome::Skipped;
    };

    if !formats.contains(detected_format) {
        log_suspended!(
            debug,
            "Skipping file (format not selected): {}",
            path.display()
        );
        return Outcome::Skipped;
    }

    // Create a uniquely-named temp file in the same directory (so the
    // final rename stays on one filesystem). The original extension is
    // kept as the suffix so ffmpeg can infer the output container. The
    // temp file is removed automatically on drop if processing fails.
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
            return Outcome::fail(path, pb, format!("failed to create temp file: {e}"));
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

    // The exact command line is recorded in the failure log, so the args are
    // built as one list instead of chained builder calls.
    let mut args: Vec<OsString> = vec![
        // Parallel workers run many ffmpeg processes at once; none of them
        // may read the terminal's stdin.
        "-nostdin".into(),
        // Batch mode keeps one ffmpeg per CPU core busy; intra-process
        // threading would only oversubscribe the machine, so decode,
        // filtergraph and encode are all pinned to a single thread.
        "-threads".into(),
        "1".into(),
        "-filter_threads".into(),
        "1".into(),
        "-i".into(),
        path.as_os_str().to_os_string(),
        "-filter:a".into(),
        atempo_filter.into(),
        "-vn".into(),
    ];
    for arg in encoder_args(detected_format, source_bitrate) {
        args.push(arg.into());
    }
    args.extend([
        "-threads".into(),
        "1".into(),
        "-map_metadata".into(),
        "0".into(),
    ]);
    args.push(output_file.as_os_str().to_os_string());
    args.extend(["-y".into(), "-loglevel".into(), "error".into()]);

    // stdout/stderr are captured instead of inherited so ffmpeg diagnostics
    // never interleave with the progress bar; they only reach the failure
    // log.
    match Command::new("ffmpeg").args(&args).output() {
        Ok(output) if output.status.success() => {
            if let Err(e) = std::fs::rename(&output_file, path) {
                Outcome::fail(path, pb, format!("failed to rename temp file: {e}"))
            } else {
                Outcome::Processed
            }
        },
        // The terminal line stays short; the command and the stderr tail
        // go to the run's shared failure log.
        Ok(output) => {
            let exit_code = output.status.code();
            let reason = match exit_status_cause(exit_code) {
                Some(cause) => format!("ffmpeg failed ({cause})"),
                None => "ffmpeg failed".to_string(),
            };
            log_suspended!(error, "Failed to process {}: {}", path.display(), reason);
            Outcome::FfmpegFailed {
                path: path.to_path_buf(),
                reason,
                command: command_line(&args),
                exit_code,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
        },
        Err(e) => Outcome::fail(path, pb, format!("failed to execute ffmpeg: {e}")),
    }
}

/// Per-run outcome: how many files were processed and skipped, plus the
/// files that failed with their reasons.
#[derive(Clone, Debug)]
pub struct ProcessSummary {
    pub processed: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub failure_log: Option<PathBuf>,
}

/// Creates the run's shared failure log in the temp dir. `disable_cleanup`
/// keeps the file on drop and after the process exits.
fn new_failure_log() -> Option<tempfile::NamedTempFile> {
    tempfile::Builder::new()
        .prefix("abs-fail-")
        .suffix(".log")
        .disable_cleanup(true)
        .tempfile()
        .ok()
}

/// Folds the workers' per-file outcomes into the run summary.
///
/// ffmpeg failures are appended to one shared log file in the temp dir,
/// created lazily so clean runs leave nothing behind.
fn summarize(outcomes: Vec<Outcome>) -> ProcessSummary {
    let mut processed = 0;
    let mut skipped = 0;
    let mut failed: Vec<(PathBuf, String)> = Vec::new();
    let mut failure_log: Option<tempfile::NamedTempFile> = None;
    let mut log_write_failed = false;

    for outcome in outcomes {
        match outcome {
            Outcome::Processed => processed += 1,
            Outcome::Skipped => skipped += 1,
            Outcome::Failed { path, reason } => failed.push((path, reason)),
            Outcome::FfmpegFailed {
                path,
                mut reason,
                command,
                exit_code,
                stderr,
            } => {
                let entry = failure_entry(&path, &command, exit_code, &stderr);
                if failure_log.is_none() {
                    failure_log = new_failure_log();
                }
                let write_ok = failure_log
                    .as_mut()
                    .is_some_and(|file| file.write_all(entry.as_bytes()).is_ok());
                if !write_ok {
                    log_write_failed = true;
                    reason.push_str("; failed to write the failure log");
                }
                failed.push((path, reason));
            },
        }
    }
    if log_write_failed {
        log::warn!("Failed to write the ffmpeg failure log to the temp dir");
    }

    ProcessSummary {
        processed,
        skipped,
        failed,
        failure_log: failure_log.map(|file| file.path().to_path_buf()),
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
/// * `jobs` - Number of parallel workers; `None` uses one worker per CPU core.
///
/// # Returns
///
/// * `Result<ProcessSummary>` - The run summary, or an error if the run could not be started
///   (invalid speed, ffmpeg unavailable). Individual file failures are reported through
///   `ProcessSummary::failed`; their ffmpeg diagnostics are appended to the shared log exposed as
///   `ProcessSummary::failure_log`.
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
/// let summary = process_audio_files(folder, speed, formats, None).unwrap();
/// println!(
///     "processed {}, skipped {}, failed {}",
///     summary.processed,
///     summary.skipped,
///     summary.failed.len()
/// );
/// ```
pub fn process_audio_files(
    folder: impl AsRef<Path>,
    speed: f32,
    formats: AudioFormat,
    jobs: Option<NonZeroUsize>,
) -> std::io::Result<ProcessSummary> {
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
        // Traversal errors (unreadable directory, name too long, ...) skip
        // the entry instead of aborting the run.
        .filter_map(|entry| {
            match entry {
                Ok(entry) => Some(entry),
                Err(err) => {
                    log::debug!("Skipping unreadable directory entry: {err}");
                    None
                },
            }
        })
        // Only count files for the progress bar. The entry's cached file
        // type avoids one stat syscall per path.
        .filter(|e| e.file_type().is_file())
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

    // One single-threaded ffmpeg per core saturates the CPU: the pool defaults
    // to one worker per core and each worker blocks on one ffmpeg at a time.
    // File durations are uneven, so allow fine-grained stealing to shrink the
    // tail.
    //
    // Workers share nothing: each returns its per-file outcome, and the
    // outcomes are folded into the summary after the pool joins.
    let item_pb = process_pb.clone();
    let mut pool = pipe(files).with_workload(Workload::Unbalanced);
    if let Some(jobs) = jobs {
        pool = pool.with_compute_workers(jobs.get());
    }
    let outcomes = pool
        .map(move |entry| {
            let outcome = process_file(entry.path(), &atempo_filter, formats, can_probe, &item_pb);
            item_pb.inc(1);
            outcome
        })
        .collect();

    process_pb.finish_with_message("Processing complete!");

    Ok(summarize(outcomes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_leftover_names_are_excluded_otherwise_hidden_files_are_not() {
        assert!(is_temp_leftover(OsStr::new(".abs_leftover.mp3")));
        assert!(is_temp_leftover(OsStr::new(".abs_")));
        // Only the exact prefix matters, not hidden files in general.
        assert!(!is_temp_leftover(OsStr::new(".hidden.mp3")));
        assert!(!is_temp_leftover(OsStr::new("abs_x.mp3")));
        assert!(!is_temp_leftover(OsStr::new(".mp3")));
    }
}
