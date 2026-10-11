use std::{num::NonZeroUsize, path::PathBuf};

use anyhow::{Result, bail};
use audio_batch_speedup::{AudioFormat, ProcessOptions};
use clap::Parser;
use log::info;
use spdlog::sink::GetSinkProp;

#[derive(Parser)]
#[command(author, version, about = "Batch speed up audio files")]
struct Cli {
    /// Path to the folder containing audio files
    input: PathBuf,

    /// Audio speed multiplier
    #[arg(short, long)]
    speed: f32,

    /// Audio formats to process (separated by commas, e.g., ogg,mp3,wav). Use
    /// 'all' for all supported formats. Supported formats: ogg, mp3, wav,
    /// flac, aac, opus, alac, wma.
    #[arg(short, long, value_delimiter = ',', default_value = "all")]
    formats: Vec<AudioFormat>,

    /// Number of parallel ffmpeg jobs (default: one per CPU core)
    #[arg(short, long)]
    jobs: Option<NonZeroUsize>,

    /// Print one log line per file (processed/skipped/failed) instead of the progress bar
    #[arg(short, long)]
    verbose: bool,
}

/// Sets up spdlog as the backend of the `log` facade.
///
/// Records are timestamped and written to stderr, the same stream the progress
/// bar draws on. The default level is `info`; the `SPDLOG_RS_LEVEL`
/// environment variable overrides it (e.g. `debug`).
fn init_logger() -> Result<()> {
    // Must run before the default logger is created so the environment
    // variable configures its level filter.
    if let Err(err) = spdlog::init_env_level() {
        eprintln!("Failed to parse SPDLOG_RS_LEVEL, using the default level: {err}");
    }
    // Replace the stock sink pair (info+ to stdout, warn+ to stderr) with a
    // single stderr sink, so every record stays on the progress bar's stream.
    let sink = spdlog::sink::StdStreamSink::builder()
        .stderr()
        .build_arc()?;
    // Terminal records carry only a timestamp, a level, and the message; the
    // logger name and the source location would be noise for a CLI.
    sink.prop().set_formatter(
        spdlog::formatter::FullFormatter::builder()
            .logger_name(false)
            .source_location(false)
            .build(),
    );
    let logger = spdlog::default_logger().fork_with(|new| {
        new.sinks_mut().clear();
        new.sinks_mut().push(sink);
        Ok(())
    })?;
    spdlog::set_default_logger(logger);
    spdlog::init_log_crate_proxy()?;
    // The facade drops records above its own max level before spdlog's
    // per-logger filter sees them, so forward everything.
    log::set_max_level(log::LevelFilter::Trace);
    Ok(())
}

fn main() -> Result<()> {
    init_logger()?;

    let args = Cli::parse();

    if !args.input.is_dir() {
        bail!(
            "The specified path is not a folder or does not exist: {}",
            args.input.display()
        );
    }

    if !args.speed.is_finite() || args.speed <= 0.0 {
        bail!(
            "Invalid speed multiplier {}: must be a positive finite number.",
            args.speed
        );
    }

    // Exact comparison is intended: the value comes from decimal CLI input,
    // where "1" and "1.0" parse to exactly 1.0f32.
    #[allow(clippy::float_cmp)]
    if args.speed == 1.0 {
        bail!(
            "Speed multiplier 1.0 is rejected: it adds one generation of re-encoding loss without \
             changing the speed."
        );
    }

    let selected_formats = args
        .formats
        .iter()
        .fold(AudioFormat::empty(), |acc, f| acc | *f);

    let mut options = ProcessOptions::new(args.speed).formats(selected_formats);
    if let Some(jobs) = args.jobs {
        options = options.jobs(jobs);
    }
    if args.verbose {
        options = options.verbose();
    }

    info!("Starting processing for folder: {}", args.input.display());
    let summary = audio_batch_speedup::process_audio_files(&args.input, options)?;

    info!(
        "Processing complete: {} processed, {} skipped, {} failed.",
        summary.processed,
        summary.skipped,
        summary.failed.len()
    );
    if let Some(log_path) = &summary.failure_log {
        info!("Failure log: {}", log_path.display());
    }
    // Each failure was already logged by the lib with its path and reason, so
    // only the summary is printed here.
    if !summary.failed.is_empty() {
        bail!("{} file(s) failed to process.", summary.failed.len());
    }

    Ok(())
}
