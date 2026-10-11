//! The set of supported audio formats and its parsing.

use bitflags::bitflags;

bitflags! {
    /// Represents the supported audio formats for processing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AudioFormat: u32 {
        /// Ogg Vorbis format.
        const OGG = 1 << 0;
        /// MPEG Audio Layer III (MP3) format.
        const MP3 = 1 << 1;
        /// Waveform Audio File Format (WAV) format.
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

// bitflags has no `#[default]` flag support, so a derive would default to
// the empty set; the manual impl keeps ALL as the default.
impl Default for AudioFormat {
    fn default() -> Self {
        Self::ALL
    }
}

impl AudioFormat {
    /// Parse table: (name, format) pairs; `m4a` is an `aac` alias.
    const NAME_TABLE: [(&str, Self); 10] = [
        ("ogg", Self::OGG),
        ("mp3", Self::MP3),
        ("wav", Self::WAV),
        ("flac", Self::FLAC),
        ("aac", Self::AAC),
        ("m4a", Self::AAC),
        ("opus", Self::OPUS),
        ("alac", Self::ALAC),
        ("wma", Self::WMA),
        ("all", Self::ALL),
    ];

    /// The first name listed for each format (the canonical one); aliases
    /// are excluded. Used for user-facing name lists. Comparison is by
    /// exact value because `ALL` contains every other format.
    fn canonical_names() -> Vec<&'static str> {
        let mut seen: Vec<Self> = Vec::new();
        let mut names = Vec::new();
        for &(name, format) in &Self::NAME_TABLE {
            if !seen.contains(&format) {
                seen.push(format);
                names.push(name);
            }
        }
        names
    }
}

impl std::str::FromStr for AudioFormat {
    type Err = String;

    /// Parses a format name (case-insensitive). Accepts codec names, common
    /// file extensions (`m4a`), and `all` for every supported format.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let name = s.trim().to_lowercase();
        if let Some(&(_, format)) = Self::NAME_TABLE.iter().find(|&&(n, _)| n == name) {
            return Ok(format);
        }
        Err(format!(
            "Unsupported format: {s}. Supported formats are: {}.",
            Self::canonical_names().join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
