//! Audio format detection from file content and extension.

use std::{fs::File, io::Read, path::Path};

use crate::{ffmpeg::probe_audio_codec, format::AudioFormat};

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
pub(crate) fn detect_audio_format(path: &Path, can_probe: bool) -> Option<AudioFormat> {
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
