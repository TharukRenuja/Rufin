use std::fs;
use std::io::BufReader;
use std::path::Path;

use lofty::config::{GlobalOptions, ParseOptions, apply_global_options};
use lofty::file::{FileType, TaggedFile};
use lofty::probe::Probe;

const LOFTY_ALLOCATION_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MetadataReader {
    Lofty(&'static [FileType]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArtworkReader {
    Lofty(&'static [FileType]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AudioFormat {
    extensions: &'static [&'static str],
    metadata_reader: Option<MetadataReader>,
    artwork_reader: Option<ArtworkReader>,
}

impl AudioFormat {
    const fn lofty(extensions: &'static [&'static str], file_types: &'static [FileType]) -> Self {
        Self {
            extensions,
            metadata_reader: Some(MetadataReader::Lofty(file_types)),
            artwork_reader: Some(ArtworkReader::Lofty(file_types)),
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
    AudioFormat::lofty(&["aac"], &[FileType::Aac]),
    AudioFormat::lofty(&["aif", "aifc", "aiff"], &[FileType::Aiff]),
    AudioFormat::lofty(&["ape"], &[FileType::Ape]),
    AudioFormat::lofty(&["flac"], &[FileType::Flac]),
    AudioFormat::lofty(&["m4a", "mp4"], &[FileType::Mp4]),
    AudioFormat::lofty(&["mp1", "mp2", "mp3"], &[FileType::Mpeg]),
    AudioFormat::lofty(&["mp+", "mpc", "mpp"], &[FileType::Mpc]),
    AudioFormat::lofty(
        &["oga", "ogg"],
        &[FileType::Vorbis, FileType::Opus, FileType::Speex],
    ),
    AudioFormat::lofty(&["opus"], &[FileType::Opus]),
    AudioFormat::lofty(&["spx"], &[FileType::Speex]),
    AudioFormat::lofty(&["wav", "wave"], &[FileType::Wav]),
    AudioFormat::lofty(&["wv"], &[FileType::WavPack]),
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

pub(super) fn read_lofty(
    path: &Path,
    file_types: &[FileType],
    read_cover_art: bool,
) -> lofty::error::Result<Option<TaggedFile>> {
    apply_global_options(
        GlobalOptions::new()
            .allocation_limit(LOFTY_ALLOCATION_MAX_BYTES)
            .preserve_format_specific_items(false),
    );
    let options = ParseOptions::new().read_cover_art(read_cover_art);
    let probe = Probe::new(BufReader::new(fs::File::open(path)?))
        .options(options)
        .guess_file_type()?;
    let Some(file_type) = probe.file_type() else {
        return Ok(None);
    };
    if !file_types.contains(&file_type) {
        return Ok(None);
    }
    probe.read().map(Some)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;

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

    #[test]
    fn registered_extensions_match_the_verified_lofty_set_and_mka_gap() {
        let extensions = AUDIO_FORMATS
            .iter()
            .flat_map(|format| format.extensions)
            .copied()
            .collect::<Vec<_>>();

        assert_eq!(
            extensions,
            [
                "aac", "aif", "aifc", "aiff", "ape", "flac", "m4a", "mp4", "mp1", "mp2", "mp3",
                "mp+", "mpc", "mpp", "oga", "ogg", "opus", "spx", "wav", "wave", "wv", "mka",
            ]
        );
    }

    #[test]
    fn lofty_reader_requires_the_registered_content_type() {
        let directory = tempfile::tempdir().expect("audio fixture directory");
        let path = directory.path().join("mislabeled.aac");
        fs::write(&path, silent_wav()).expect("write WAV fixture");

        assert!(
            read_lofty(&path, &[FileType::Wav], false)
                .expect("inspect WAV content")
                .is_some()
        );
        assert!(
            read_lofty(&path, &[FileType::Aac], false)
                .expect("inspect mislabeled AAC")
                .is_none()
        );
    }

    fn silent_wav() -> Vec<u8> {
        let data_len = 16_000_u32;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8_000_u32.to_le_bytes());
        bytes.extend_from_slice(&16_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + data_len as usize, 0);
        bytes
    }
}
