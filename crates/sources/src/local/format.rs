use std::fs;
use std::io::BufReader;
use std::path::Path;

use lofty::config::{GlobalOptions, ParseOptions, apply_global_options};
use lofty::file::{FileType, TaggedFile};
use lofty::probe::Probe;
use lofty::tag::ItemKey;

use super::discoverer;
use library::{MetadataEditing, MetadataField};

const LOFTY_ALLOCATION_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MetadataReader {
    Lofty(&'static [FileType]),
    Discoverer(discoverer::Format),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArtworkReader {
    Lofty(&'static [FileType]),
    Discoverer(discoverer::Format),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MetadataWriter {
    Lofty(&'static [FileType]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AudioFormat {
    extensions: &'static [&'static str],
    metadata_reader: MetadataReader,
    artwork_reader: ArtworkReader,
}

impl AudioFormat {
    const fn lofty(extensions: &'static [&'static str], file_types: &'static [FileType]) -> Self {
        Self {
            extensions,
            metadata_reader: MetadataReader::Lofty(file_types),
            artwork_reader: ArtworkReader::Lofty(file_types),
        }
    }

    const fn discoverer(extensions: &'static [&'static str], format: discoverer::Format) -> Self {
        Self {
            extensions,
            metadata_reader: MetadataReader::Discoverer(format),
            artwork_reader: ArtworkReader::Discoverer(format),
        }
    }

    pub(super) fn metadata_reader(self) -> MetadataReader {
        self.metadata_reader
    }

    pub(super) fn artwork_reader(self) -> ArtworkReader {
        self.artwork_reader
    }

    pub(super) fn metadata_writer(self) -> Option<MetadataWriter> {
        match self.metadata_reader {
            MetadataReader::Lofty(file_types)
                if file_types.iter().all(|file_type| {
                    file_type
                        .tag_support(file_type.primary_tag_type())
                        .is_writable()
                }) =>
            {
                Some(MetadataWriter::Lofty(file_types))
            }
            MetadataReader::Lofty(_) | MetadataReader::Discoverer(_) => None,
        }
    }

    pub(super) fn metadata_editing(self) -> Option<MetadataEditing> {
        let MetadataWriter::Lofty(file_types) = self.metadata_writer()?;
        let fields = [
            (MetadataField::Title, ItemKey::TrackTitle),
            (MetadataField::SortTitle, ItemKey::TrackTitleSortOrder),
            (MetadataField::Artist, ItemKey::TrackArtist),
            (MetadataField::Album, ItemKey::AlbumTitle),
            (MetadataField::AlbumArtist, ItemKey::AlbumArtist),
            (MetadataField::TrackNumber, ItemKey::TrackNumber),
            (MetadataField::DiscNumber, ItemKey::DiscNumber),
            (MetadataField::Year, ItemKey::RecordingDate),
            (MetadataField::Genre, ItemKey::Genre),
            (MetadataField::Comment, ItemKey::Comment),
            (MetadataField::Bpm, ItemKey::IntegerBpm),
            (
                MetadataField::MusicBrainzRecordingId,
                ItemKey::MusicBrainzRecordingId,
            ),
            (
                MetadataField::MusicBrainzReleaseTrackId,
                ItemKey::MusicBrainzTrackId,
            ),
            (
                MetadataField::MusicBrainzAlbumId,
                ItemKey::MusicBrainzReleaseId,
            ),
            (
                MetadataField::MusicBrainzReleaseGroupId,
                ItemKey::MusicBrainzReleaseGroupId,
            ),
        ]
        .into_iter()
        .filter_map(|(field, key)| {
            file_types
                .iter()
                .all(|file_type| {
                    metadata_field_is_writable(file_type.primary_tag_type(), field, key)
                })
                .then_some(field)
        })
        .collect::<Vec<_>>();
        (!fields.is_empty()).then(|| MetadataEditing::new(fields))
    }

    pub(super) fn metadata_key_is_writable(self, key: ItemKey) -> bool {
        let Some(MetadataWriter::Lofty(file_types)) = self.metadata_writer() else {
            return false;
        };
        file_types
            .iter()
            .all(|file_type| metadata_key_is_writable(file_type.primary_tag_type(), key))
    }
}

fn metadata_key_is_writable(tag_type: lofty::tag::TagType, key: ItemKey) -> bool {
    key.map_key(tag_type).is_some()
        || tag_type == lofty::tag::TagType::Id3v2 && key == ItemKey::MusicBrainzRecordingId
}

fn metadata_field_is_writable(
    tag_type: lofty::tag::TagType,
    field: MetadataField,
    key: ItemKey,
) -> bool {
    if field == MetadataField::Bpm {
        return bpm_key(tag_type).is_some();
    }
    metadata_key_is_writable(tag_type, key)
}

pub(super) fn bpm_key(tag_type: lofty::tag::TagType) -> Option<ItemKey> {
    // Lofty's generic MP4 conversion writes `tmpo` as text and can preserve an
    // existing integer atom beside it, so that path is not a safe BPM writer.
    if tag_type == lofty::tag::TagType::Mp4Ilst {
        return None;
    }
    [ItemKey::IntegerBpm, ItemKey::Bpm]
        .into_iter()
        .find(|key| metadata_key_is_writable(tag_type, *key))
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
    AudioFormat::discoverer(&["mka"], discoverer::Format::Mka),
    AudioFormat::discoverer(&["asf", "wma"], discoverer::Format::Asf),
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

pub(super) fn read_lofty_for_edit(
    path: &Path,
    file_types: &[FileType],
) -> lofty::error::Result<Option<TaggedFile>> {
    apply_global_options(
        GlobalOptions::new()
            .allocation_limit(LOFTY_ALLOCATION_MAX_BYTES)
            .preserve_format_specific_items(true),
    );
    let probe = Probe::new(BufReader::new(fs::File::open(path)?))
        .options(ParseOptions::new().read_cover_art(true))
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
    fn registered_extensions_match_the_verified_reader_set() {
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
                "asf", "wma",
            ]
        );
    }

    #[test]
    fn each_extension_selects_one_declared_reader_without_a_parser_chain() {
        assert!(matches!(
            audio_format(Path::new("track.m4a")).map(|format| format.metadata_reader()),
            Some(MetadataReader::Lofty(types)) if types == [FileType::Mp4]
        ));
        assert_eq!(
            audio_format(Path::new("track.mka")).map(|format| format.metadata_reader()),
            Some(MetadataReader::Discoverer(discoverer::Format::Mka))
        );
        assert_eq!(
            audio_format(Path::new("track.wma")).map(|format| format.metadata_reader()),
            Some(MetadataReader::Discoverer(discoverer::Format::Asf))
        );
    }

    #[test]
    fn metadata_editing_follows_the_exact_registered_writer() {
        let editable_fields = [
            (MetadataField::Title, ItemKey::TrackTitle),
            (MetadataField::SortTitle, ItemKey::TrackTitleSortOrder),
            (MetadataField::Artist, ItemKey::TrackArtist),
            (MetadataField::Album, ItemKey::AlbumTitle),
            (MetadataField::AlbumArtist, ItemKey::AlbumArtist),
            (MetadataField::TrackNumber, ItemKey::TrackNumber),
            (MetadataField::DiscNumber, ItemKey::DiscNumber),
            (MetadataField::Year, ItemKey::RecordingDate),
            (MetadataField::Genre, ItemKey::Genre),
            (MetadataField::Comment, ItemKey::Comment),
            (MetadataField::Bpm, ItemKey::IntegerBpm),
            (
                MetadataField::MusicBrainzRecordingId,
                ItemKey::MusicBrainzRecordingId,
            ),
            (
                MetadataField::MusicBrainzReleaseTrackId,
                ItemKey::MusicBrainzTrackId,
            ),
            (
                MetadataField::MusicBrainzAlbumId,
                ItemKey::MusicBrainzReleaseId,
            ),
            (
                MetadataField::MusicBrainzReleaseGroupId,
                ItemKey::MusicBrainzReleaseGroupId,
            ),
        ];
        for format in AUDIO_FORMATS {
            match format.metadata_reader() {
                MetadataReader::Lofty(file_types) => {
                    assert_eq!(
                        format.metadata_writer(),
                        Some(MetadataWriter::Lofty(file_types))
                    );
                    let editing = format.metadata_editing().expect("Lofty metadata editing");
                    for file_type in file_types {
                        let tag_type = file_type.primary_tag_type();
                        for (field, key) in editable_fields {
                            assert_eq!(
                                editing.includes(field),
                                file_types.iter().all(|file_type| {
                                    metadata_field_is_writable(
                                        file_type.primary_tag_type(),
                                        field,
                                        key,
                                    )
                                }),
                                "{file_type:?} field mismatch for {key:?} in {tag_type:?}"
                            );
                        }
                    }
                }
                MetadataReader::Discoverer(_) => {
                    assert_eq!(format.metadata_writer(), None);
                }
            }
        }

        assert_eq!(
            audio_format(Path::new("track.m4a")).and_then(|format| format.metadata_writer()),
            Some(MetadataWriter::Lofty(&[FileType::Mp4]))
        );
        assert_eq!(
            audio_format(Path::new("track.mka")).and_then(|format| format.metadata_writer()),
            None
        );
        assert_eq!(
            audio_format(Path::new("track.wma")).and_then(|format| format.metadata_writer()),
            None
        );
        for extension in ["flac", "mp3", "ogg"] {
            assert!(
                audio_format(Path::new(&format!("track.{extension}")))
                    .and_then(|format| format.metadata_editing())
                    .expect("Lofty metadata editing")
                    .includes(MetadataField::Bpm),
                "{extension} should expose its writable BPM key"
            );
        }
        assert!(
            !audio_format(Path::new("track.m4a"))
                .and_then(|format| format.metadata_editing())
                .expect("MP4 metadata editing")
                .includes(MetadataField::Bpm),
            "MP4 BPM stays read-only until Lofty can write an integer tmpo atom"
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
