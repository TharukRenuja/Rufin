use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CueSheet {
    pub album_title: Option<String>,
    pub album_performer: Option<String>,
    pub files: Vec<CueFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CueFile {
    pub path: PathBuf,
    pub tracks: Vec<CueTrack>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CueTrack {
    pub number: u16,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub index_start_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenTrack {
    number: u16,
    title: Option<String>,
    performer: Option<String>,
    index_start_ms: Option<u64>,
}

pub(super) fn parse_cue_sheet(cue_path: &Path, text: &str) -> Option<CueSheet> {
    let mut sheet = CueSheet {
        album_title: None,
        album_performer: None,
        files: Vec::new(),
    };
    let mut current_file: Option<CueFile> = None;
    let mut current_track: Option<OpenTrack> = None;

    for line in text.lines() {
        let mut fields = CueFields::new(line);
        let Some(command) = fields.next_word() else {
            continue;
        };
        match command.to_ascii_uppercase().as_str() {
            "REM" => {}
            "TITLE" => {
                let Some(value) = fields.next_value() else {
                    continue;
                };
                if let Some(track) = current_track.as_mut() {
                    track.title = Some(value);
                } else {
                    sheet.album_title = Some(value);
                }
            }
            "PERFORMER" => {
                let Some(value) = fields.next_value() else {
                    continue;
                };
                if let Some(track) = current_track.as_mut() {
                    track.performer = Some(value);
                } else {
                    sheet.album_performer = Some(value);
                }
            }
            "FILE" => {
                push_track(&mut current_file, current_track.take());
                push_file(&mut sheet, current_file.take());
                let Some(path) = fields.next_value() else {
                    continue;
                };
                current_file = Some(CueFile {
                    path: resolve_cue_file_path(cue_path, &path),
                    tracks: Vec::new(),
                });
            }
            "TRACK" => {
                push_track(&mut current_file, current_track.take());
                let Some(number) = fields
                    .next_word()
                    .and_then(|value| value.parse::<u16>().ok())
                else {
                    continue;
                };
                let track_type = fields.next_word().unwrap_or_default();
                if !track_type.eq_ignore_ascii_case("AUDIO") {
                    current_track = None;
                    continue;
                }
                current_track = Some(OpenTrack {
                    number,
                    title: None,
                    performer: None,
                    index_start_ms: None,
                });
            }
            "INDEX" => {
                let Some(index_number) = fields.next_word() else {
                    continue;
                };
                if index_number != "01" {
                    continue;
                }
                let Some(start_ms) = fields.next_word().and_then(cue_time_ms) else {
                    continue;
                };
                if let Some(track) = current_track.as_mut() {
                    track.index_start_ms = Some(start_ms);
                }
            }
            _ => {}
        }
    }
    push_track(&mut current_file, current_track);
    push_file(&mut sheet, current_file);
    sheet.files.retain(|file| !file.tracks.is_empty());
    (!sheet.files.is_empty()).then_some(sheet)
}

fn push_file(sheet: &mut CueSheet, file: Option<CueFile>) {
    if let Some(file) = file
        && !file.tracks.is_empty()
    {
        sheet.files.push(file);
    }
}

fn push_track(file: &mut Option<CueFile>, track: Option<OpenTrack>) {
    let Some(file) = file else {
        return;
    };
    let Some(track) = track else {
        return;
    };
    let Some(index_start_ms) = track.index_start_ms else {
        return;
    };
    file.tracks.push(CueTrack {
        number: track.number,
        title: track.title,
        performer: track.performer,
        index_start_ms,
    });
}

fn resolve_cue_file_path(cue_path: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        cue_path
            .parent()
            .map(|parent| parent.join(path.as_path()))
            .unwrap_or(path)
    }
}

fn cue_time_ms(value: &str) -> Option<u64> {
    let mut parts = value.split(':');
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parts.next()?.parse::<u64>().ok()?;
    let frames = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() || seconds >= 60 || frames >= 75 {
        return None;
    }
    Some((minutes * 60_000) + (seconds * 1_000) + (frames * 1_000 / 75))
}

struct CueFields<'a> {
    value: &'a str,
}

impl<'a> CueFields<'a> {
    fn new(value: &'a str) -> Self {
        Self {
            value: value.trim_start(),
        }
    }

    fn next_word(&mut self) -> Option<&'a str> {
        self.value = self.value.trim_start();
        if self.value.is_empty() {
            return None;
        }
        let end = self
            .value
            .find(char::is_whitespace)
            .unwrap_or(self.value.len());
        let word = &self.value[..end];
        self.value = &self.value[end..];
        Some(word)
    }

    fn next_value(&mut self) -> Option<String> {
        self.value = self.value.trim_start();
        if self.value.is_empty() {
            return None;
        }
        if let Some(rest) = self.value.strip_prefix('"') {
            let mut escaped = false;
            let mut value = String::new();
            for (index, ch) in rest.char_indices() {
                if escaped {
                    value.push(ch);
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '"' {
                    self.value = &rest[index + ch.len_utf8()..];
                    return Some(value);
                }
                value.push(ch);
            }
            Some(value)
        } else {
            let end = self
                .value
                .find(char::is_whitespace)
                .unwrap_or(self.value.len());
            let value = self.value[..end].to_string();
            self.value = &self.value[end..];
            Some(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_file_cue_tracks() {
        let sheet = parse_cue_sheet(
            Path::new("/music/album.cue"),
            r#"
PERFORMER "Album Artist"
TITLE "Album Title"
FILE "album.flac" WAVE
  TRACK 01 AUDIO
    TITLE "First"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Second"
    PERFORMER "Guest"
    INDEX 01 04:12:30
"#,
        )
        .expect("cue sheet");

        assert_eq!(sheet.album_title.as_deref(), Some("Album Title"));
        assert_eq!(sheet.album_performer.as_deref(), Some("Album Artist"));
        assert_eq!(sheet.files.len(), 1);
        assert_eq!(sheet.files[0].path, PathBuf::from("/music/album.flac"));
        assert_eq!(sheet.files[0].tracks.len(), 2);
        assert_eq!(sheet.files[0].tracks[1].index_start_ms, 252_400);
        assert_eq!(sheet.files[0].tracks[1].performer.as_deref(), Some("Guest"));
    }
}
