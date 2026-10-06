use std::path::PathBuf;

use anyhow::{Result, bail};
use audio_batch_speedup::AudioFormat;
use clap::Parser;
use log::{LevelFilter, info};

#[derive(Parser)]
#[command(author, version, about = "Batch speed up audio files")]
struct Cli {
    /// Path to the folder containing audio files
    input: PathBuf,

    /// Audio speed multiplier
    #[arg(short, long)]
    speed: f32,

    /// Audio formats to process (seperated by commas, e.g., ogg,mp3,wav). Use
    /// 'all' for all supported formats. Supported formats: ogg, mp3, wav,
    /// flac, aac, opus, alac, wma.
    #[arg(short, long, value_delimiter = ',', default_value = "all")]
    formats: Vec<AudioFormat>,
}

fn main() -> Result<()> {
    _ = pretty_env_logger::formatted_builder()
        .filter_level(LevelFilter::Info)
        .format_timestamp_secs()
        .parse_default_env()
        .try_init();

    let args = Cli::parse();

    if !args.input.exists() {
        bail!("The specified folder does not exist.");
    }

    if !args.input.is_dir() {
        bail!("Please specify a folder path.");
    }

    let selected_formats = args
        .formats
        .iter()
        .fold(AudioFormat::empty(), |acc, f| acc | *f);

    info!("Starting processing for folder: {}", args.input.display());
    audio_batch_speedup::process_audio_files(&args.input, args.speed, selected_formats)?;
    info!("Processing complete.");

    Ok(())
}
