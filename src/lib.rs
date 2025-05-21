#![warn(clippy::cargo)]

use bitflags::bitflags;
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use log::{debug, error};
use rayon::prelude::*;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use walkdir::WalkDir;

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
        AudioFormat::OGG
            | AudioFormat::MP3
            | AudioFormat::WAV
            | AudioFormat::FLAC
            | AudioFormat::AAC
            | AudioFormat::OPUS
            | AudioFormat::ALAC
            | AudioFormat::WMA
    }
}

/// Detects the audio format of a file based on its magic bytes or file extension.
///
/// # Arguments
///
/// * `path` - The path to the audio file.
///
/// # Returns
///
/// * `Option<AudioFormat>` - The detected audio format, or `None` if it cannot be determined.
fn detect_audio_format(path: &Path) -> Option<AudioFormat> {
    // Try to detect by magic bytes first
    if let Ok(mut file) = File::open(path) {
        let mut buffer = [0; 12]; // Read enough bytes for common headers

        if file.read_exact(&mut buffer).is_ok() {
            // OGG (OggS)
            if buffer[0..4] == [0x4F, 0x67, 0x67, 0x53] {
                return Some(AudioFormat::OGG);
            }
            // MP3 (ID3 tag or starts with 0xFF FB/FA)
            if buffer[0..3] == [0x49, 0x44, 0x33]
                || (buffer[0] == 0xFF && (buffer[1] & 0xF6) == 0xF2)
            {
                return Some(AudioFormat::MP3);
            }
            // WAV (RIFF header with WAVE)
            if buffer[0..4] == [0x52, 0x49, 0x46, 0x46] && buffer[8..12] == [0x57, 0x41, 0x56, 0x45]
            {
                return Some(AudioFormat::WAV);
            }
            // FLAC (fLaC)
            if buffer[0..4] == [0x66, 0x4C, 0x61, 0x43] {
                return Some(AudioFormat::FLAC);
            }
            // AAC (often in MP4/M4A containers, which start with 'ftyp' or 'moov')
            // This is harder to detect purely by magic bytes without parsing the container.
            // We'll rely more on extension for AAC/M4A.
            // OPUS (often in Ogg containers, so OggS will catch it, or WebM)
            // ALAC (often in MP4/M4A containers)
            // WMA (ASF header)
            if buffer[0..4] == [0x30, 0x26, 0xB2, 0x75] {
                // GUID for ASF header
                return Some(AudioFormat::WMA);
            }
        }
    }

    // Fallback to file extension
    if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
        match extension.to_lowercase().as_str() {
            "ogg" => return Some(AudioFormat::OGG),
            "mp3" => return Some(AudioFormat::MP3),
            "wav" => return Some(AudioFormat::WAV),
            "flac" => return Some(AudioFormat::FLAC),
            "m4a" | "aac" => return Some(AudioFormat::AAC),
            "opus" => return Some(AudioFormat::OPUS),
            "alac" => return Some(AudioFormat::ALAC),
            "wma" => return Some(AudioFormat::WMA),
            _ => {}
        }
    }

    None
}

/// Process all audio files in the specified folder recursively with the given speed multiplier.
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

    let error_count = AtomicUsize::new(0);
    let skipped_count = AtomicUsize::new(0);

    // Process all files in parallel
    files
        .into_par_iter()
        .progress_with(process_pb.clone())
        .for_each(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return;
            }

            let detected_format = detect_audio_format(path);

            if detected_format.is_none() || !formats.contains(detected_format.unwrap()) {
                debug!(
                    "Skipping file (unsupported format or not selected): {}",
                    path.display()
                );
                skipped_count.fetch_add(1, Ordering::Relaxed);
                return;
            }

            let output_file = path.with_file_name(format!(
                "temp_{}",
                path.file_name().unwrap().to_str().unwrap()
            ));

            let status = Command::new("ffmpeg")
                .args([
                    "-i",
                    path.to_str().unwrap(),
                    "-filter:a",
                    &format!("atempo={}", speed),
                    "-vn",
                    output_file.to_str().unwrap(),
                    "-y",
                    "-loglevel",
                    "error",
                ])
                .status();

            if let Err(e) = status {
                error!("Error processing {}: {}", path.display(), e);
                error_count.fetch_add(1, Ordering::Relaxed);
                return;
            }

            if status.unwrap().success() {
                if let Err(e) = std::fs::rename(&output_file, path) {
                    error!("Error renaming file {}: {}", output_file.display(), e);
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                if output_file.exists() {
                    if let Err(e) = std::fs::remove_file(&output_file) {
                        error!("Error removing temp file {}: {}", output_file.display(), e);
                    }
                }
                error!("Error processing {}", path.display());
                error_count.fetch_add(1, Ordering::Relaxed);
            }
        });

    process_pb.finish_with_message("Processing complete!");

    let errors = error_count.load(Ordering::Relaxed);
    let skipped = skipped_count.load(Ordering::Relaxed);

    if errors > 0 {
        log::warn!("Finished with {} errors.", errors);
    }
    if skipped > 0 {
        log::info!("Skipped {} files.", skipped);
    }

    Ok(())
}
