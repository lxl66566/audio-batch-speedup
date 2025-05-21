use anyhow::Result;
use audio_batch_speedup::AudioFormat;
use clap::Parser;
use log::{LevelFilter, error, info};
use std::path::PathBuf; // Import AudioFormat

#[derive(Parser)]
#[command(author, version, about = "Batch speed up audio files")]
struct Cli {
    /// Path to the folder containing audio files
    input: PathBuf,

    /// Audio speed multiplier
    #[arg(short, long)]
    speed: f32,

    /// Audio formats to process (seperated by commas, e.g., ogg,mp3,wav). Use 'all' for all supported formats.
    /// Supported formats: ogg, mp3, wav, flac, aac, opus, alac, wma.
    #[arg(short, long, value_delimiter = ',', default_value = "all")]
    formats: String,
}

fn main() -> Result<()> {
    _ = pretty_env_logger::formatted_builder()
        .filter_level(LevelFilter::Info)
        .format_timestamp_secs()
        .parse_default_env()
        .try_init();

    let args = Cli::parse();

    if !args.input.exists() {
        error!("The specified folder does not exist.");
        std::process::exit(1);
    }

    if !args.input.is_dir() {
        error!("Please specify a folder path.");
        std::process::exit(1);
    }

    let mut selected_formats = AudioFormat::empty();
    if args.formats.to_lowercase() == "all" {
        selected_formats = AudioFormat::ALL;
    } else {
        for format_str in args.formats.split(',') {
            match format_str.trim().to_lowercase().as_str() {
                "ogg" => selected_formats |= AudioFormat::OGG,
                "mp3" => selected_formats |= AudioFormat::MP3,
                "wav" => selected_formats |= AudioFormat::WAV,
                "flac" => selected_formats |= AudioFormat::FLAC,
                "aac" => selected_formats |= AudioFormat::AAC,
                "opus" => selected_formats |= AudioFormat::OPUS,
                "alac" => selected_formats |= AudioFormat::ALAC,
                "wma" => selected_formats |= AudioFormat::WMA,
                _ => {
                    error!(
                        "Unsupported format specified: {}. Supported formats are: ogg, mp3, wav, flac, aac, opus, alac, wma, all.",
                        format_str
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    if selected_formats.is_empty() {
        error!("No valid audio formats selected for processing.");
        std::process::exit(1);
    }

    info!("Starting processing for folder: {}", args.input.display());
    audio_batch_speedup::process_audio_files(&args.input, args.speed, selected_formats)?;
    info!("Processing complete.");

    Ok(())
}
