//! End-to-end tests for `process_audio_files`. Each test generates its
//! samples with ffmpeg; tests (or single assertions) that need ffprobe are
//! skipped when it is missing.

use std::{
    ffi::OsStr,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::Command,
};

use audio_batch_speedup::{AudioFormat, process_audio_files};

/// Whether `program` runs and reports a version.
fn available(program: &str) -> bool {
    Command::new(program)
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Generates a short sine-wave audio file encoded with `codec`.
fn generate(dest: &Path, codec: &str) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(["-f", "lavfi", "-i", "sine=duration=0.3"])
        .args(["-c:a", codec])
        .arg(dest)
        .arg("-y")
        .status()
        .expect("run ffmpeg");
    assert!(
        status.success(),
        "ffmpeg failed to generate {}",
        dest.display()
    );
}

/// Probes the codec name of the first audio stream.
fn probe_codec(path: &Path) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0"])
        .args(["-show_entries", "stream=codec_name"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(
        output.status.success(),
        "ffprobe failed on {}",
        path.display()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A single-worker pool keeps the runs deterministic.
fn one_job() -> Option<NonZeroUsize> {
    NonZeroUsize::new(1)
}

#[test]
fn opus_files_keep_their_codec() {
    if !available("ffmpeg") {
        eprintln!("skipping: ffmpeg not found");
        return;
    }
    let dir = tempfile::tempdir().expect("create temp dir");
    let opus = dir.path().join("a.opus");
    generate(&opus, "libopus");

    let summary = process_audio_files(dir.path(), 2.0, AudioFormat::ALL, one_job()).expect("run");
    assert_eq!(summary.processed, 1);
    assert_eq!(summary.skipped, 0);
    assert!(
        summary.failed.is_empty(),
        "unexpected failures: {:?}",
        summary.failed
    );

    if !available("ffprobe") {
        eprintln!("skipping codec assertion: ffprobe not found");
        return;
    }
    assert_eq!(probe_codec(&opus), "opus");
}

#[test]
fn alac_files_keep_their_codec() {
    if !available("ffmpeg") {
        eprintln!("skipping: ffmpeg not found");
        return;
    }
    let dir = tempfile::tempdir().expect("create temp dir");
    let alac = dir.path().join("a.m4a");
    generate(&alac, "alac");

    let summary = process_audio_files(dir.path(), 2.0, AudioFormat::ALL, one_job()).expect("run");
    assert_eq!(summary.processed, 1);
    assert_eq!(summary.skipped, 0);
    assert!(
        summary.failed.is_empty(),
        "unexpected failures: {:?}",
        summary.failed
    );

    if !available("ffprobe") {
        eprintln!("skipping codec assertion: ffprobe not found");
        return;
    }
    assert_eq!(probe_codec(&alac), "alac");
}

#[test]
fn leftover_temp_files_are_left_alone() {
    if !available("ffmpeg") {
        eprintln!("skipping: ffmpeg not found");
        return;
    }
    let dir = tempfile::tempdir().expect("create temp dir");
    // A real playable file, as a crashed run would leave behind.
    let leftover: PathBuf = dir.path().join(".abs_leftover.mp3");
    generate(&leftover, "libmp3lame");
    let before = std::fs::read(&leftover).expect("read leftover");

    let summary = process_audio_files(dir.path(), 2.0, AudioFormat::ALL, one_job()).expect("run");
    // Internal artifacts are excluded from every counter.
    assert_eq!(summary.processed, 0);
    assert_eq!(summary.skipped, 0);
    assert!(
        summary.failed.is_empty(),
        "unexpected failures: {:?}",
        summary.failed
    );

    let after = std::fs::read(&leftover).expect("read leftover");
    assert_eq!(before, after, "leftover temp file was modified");
}

#[test]
fn corrupted_files_are_reported_as_failures() {
    if !available("ffmpeg") {
        eprintln!("skipping: ffmpeg not found");
        return;
    }
    let dir = tempfile::tempdir().expect("create temp dir");
    // Garbage with an MP3 magic so detection selects it for processing.
    let mut corrupt = b"ID3".to_vec();
    corrupt.extend_from_slice(&[0x04, 0x00, 0xff, 0xff, 0xff, 0x7f]);
    corrupt.extend_from_slice(&[0xa5; 64]);
    let path = dir.path().join("corrupt.mp3");
    std::fs::write(&path, &corrupt).expect("write corrupt file");

    let summary = process_audio_files(dir.path(), 2.0, AudioFormat::ALL, one_job()).expect("run");
    assert_eq!(summary.processed, 0);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.failed.len(), 1, "expected exactly one failure");
    let (failed_path, reason) = &summary.failed[0];
    assert_eq!(failed_path.file_name(), Some(OsStr::new("corrupt.mp3")));
    assert!(!reason.is_empty(), "failure reason must not be empty");
}
