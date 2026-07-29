use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MetadataReader {
    Lofty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArtworkReader {
    Lofty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AudioFormat {
    extensions: &'static [&'static str],
    metadata_reader: Option<MetadataReader>,
    artwork_reader: Option<ArtworkReader>,
}

impl AudioFormat {
    const fn lofty(extensions: &'static [&'static str]) -> Self {
        Self {
            extensions,
            metadata_reader: Some(MetadataReader::Lofty),
            artwork_reader: Some(ArtworkReader::Lofty),
        }
    }

    pub(super) fn metadata_reader(self) -> Option<MetadataReader> {
        self.metadata_reader
    }

    pub(super) fn artwork_reader(self) -> Option<ArtworkReader> {
        self.artwork_reader
    }
}

const AUDIO_FORMATS: &[AudioFormat] = &[
    AudioFormat::lofty(&["mp3"]),
    AudioFormat::lofty(&["flac"]),
    AudioFormat::lofty(&["m4a", "mp4"]),
    AudioFormat::lofty(&["wav"]),
    AudioFormat::lofty(&["ogg"]),
    AudioFormat::lofty(&["opus"]),
    AudioFormat {
        extensions: &["mka"],
        metadata_reader: None,
        artwork_reader: None,
    },
];

pub(super) fn audio_format(path: &Path) -> Option<&'static AudioFormat> {
    let extension = path.extension()?.to_str()?;
    AUDIO_FORMATS.iter().find(|format| {
        format
            .extensions
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_extension_selects_exactly_one_format_case_insensitively() {
        let mut extensions = HashSet::new();
        for format in AUDIO_FORMATS {
            for extension in format.extensions {
                assert!(
                    extensions.insert(extension.to_ascii_lowercase()),
                    "duplicate Local audio extension {extension}"
                );
                let upper = format!("Track.{}", extension.to_ascii_uppercase());
                assert_eq!(audio_format(Path::new(&upper)), Some(format));
            }
        }
    }
}
