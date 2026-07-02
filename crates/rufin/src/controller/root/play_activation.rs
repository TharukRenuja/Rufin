use domain::{
    PlaySourceKey, QueueAnchor, QueueItemInput, QueueReplacement, QueueReplacementSource,
    QueueSourceInput, Track, TrackId,
};

pub const FULL_LOADED_LIMIT: usize = 100;
pub const MATERIALIZED_WINDOW_LIMIT: usize = 100;
pub const MATERIALIZED_WINDOW_BEFORE_ANCHOR: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayActivation {
    pub target: PlayTarget,
}

impl PlayActivation {
    pub fn shuffled_start(mut self) -> Self {
        self.target = PlayTarget::ShuffleStart(Box::new(self.target));
        self
    }

    pub fn into_parts(self) -> (bool, PlayTarget) {
        match self.target {
            PlayTarget::ShuffleStart(target) => (true, *target),
            target => (false, target),
        }
    }

    pub fn shuffle_start(&self) -> bool {
        matches!(self.target, PlayTarget::ShuffleStart(_))
    }
}

// Track-only activations are accepted by the seam before route conversion emits them.
#[allow(dead_code)]
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayTarget {
    ShuffleStart(Box<PlayTarget>),
    TrackOnly(Track),
    LoadedSource {
        source_key: PlaySourceKey,
        completeness: LoadedCompleteness,
        items: Vec<PlaySourceItem>,
        anchor: PlayAnchor,
    },
    StoreBackedSource {
        source_key: PlaySourceKey,
        anchor: PlayAnchor,
    },
}

// Complete/window activations are normalized here before route conversion starts constructing them.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadedCompleteness {
    Complete,
    Window { start: usize, total: Option<usize> },
    Snippet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaySourceItem {
    pub track: Track,
    pub source_index: usize,
    pub source_item_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayAnchor {
    pub track_id: TrackId,
    pub source_index: usize,
    pub source_item_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalizedPlayTarget {
    TrackOnly(Box<Track>),
    Replacement(QueueReplacement),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedPlayActivation {
    pub target: NormalizedPlayTarget,
    pub shuffle_start: bool,
}

pub fn normalize_loaded_source_activation(
    activation: PlayActivation,
) -> Result<NormalizedPlayActivation, String> {
    let (shuffle_start, target) = activation.into_parts();
    let target = match target {
        PlayTarget::TrackOnly(track) => NormalizedPlayTarget::TrackOnly(Box::new(track)),
        PlayTarget::LoadedSource {
            source_key,
            completeness,
            items,
            anchor,
        } => normalize_loaded_source_target(source_key, completeness, items, anchor)?,
        PlayTarget::StoreBackedSource { .. } => {
            return Err("The selected source could not be resolved.".to_string());
        }
        PlayTarget::ShuffleStart(_) => {
            return Err("The selected source could not be resolved.".to_string());
        }
    };
    Ok(NormalizedPlayActivation {
        target,
        shuffle_start,
    })
}

fn normalize_loaded_source_target(
    source_key: PlaySourceKey,
    completeness: LoadedCompleteness,
    items: Vec<PlaySourceItem>,
    anchor: PlayAnchor,
) -> Result<NormalizedPlayTarget, String> {
    validate_loaded_items(&completeness, &items)?;
    let anchor_position = matching_anchor_position(&items, &anchor)?;

    if completeness == LoadedCompleteness::Snippet {
        return Ok(NormalizedPlayTarget::TrackOnly(Box::new(
            items[anchor_position].track.clone(),
        )));
    }

    let (materialized_start, materialized_end) = materialized_range(items.len(), anchor_position);
    let materialized_items = items[materialized_start..materialized_end]
        .iter()
        .map(|item| QueueItemInput::Source {
            track: item.track.clone(),
            source_index: item.source_index,
        })
        .collect::<Vec<_>>();
    let source = QueueSourceInput { source_key };
    Ok(NormalizedPlayTarget::Replacement(QueueReplacement {
        source: QueueReplacementSource::Source(source),
        items: materialized_items,
        anchor: QueueAnchor::SourcePosition {
            position: anchor_position - materialized_start,
            track_id: anchor.track_id,
        },
    }))
}

fn validate_loaded_items(
    completeness: &LoadedCompleteness,
    items: &[PlaySourceItem],
) -> Result<(), String> {
    let Some(first) = items.first() else {
        return Err("No tracks are available to play.".to_string());
    };
    match completeness {
        LoadedCompleteness::Complete if first.source_index != 0 => {
            return Err("The selected track is no longer available.".to_string());
        }
        LoadedCompleteness::Window { start, total } => {
            if first.source_index != *start {
                return Err("The selected track is no longer available.".to_string());
            }
            if let Some(total) = total {
                let Some(last_source_index) = items.last().map(|item| item.source_index) else {
                    return Err("No tracks are available to play.".to_string());
                };
                if *start >= *total || last_source_index >= *total {
                    return Err("The selected track is no longer available.".to_string());
                }
            }
        }
        _ => {}
    }
    for pair in items.windows(2) {
        let expected = pair[0]
            .source_index
            .checked_add(1)
            .ok_or_else(|| "The selected track is no longer available.".to_string())?;
        if pair[1].source_index != expected {
            return Err("The selected track is no longer available.".to_string());
        }
    }
    Ok(())
}

fn matching_anchor_position(
    items: &[PlaySourceItem],
    anchor: &PlayAnchor,
) -> Result<usize, String> {
    items
        .iter()
        .position(|item| {
            item.track.id == anchor.track_id
                && item.source_index == anchor.source_index
                && item.source_item_id == anchor.source_item_id
        })
        .ok_or_else(|| "The selected track is no longer available.".to_string())
}

fn materialized_range(item_count: usize, anchor_position: usize) -> (usize, usize) {
    if item_count <= FULL_LOADED_LIMIT {
        return (0, item_count);
    }
    let last_window_start = item_count - MATERIALIZED_WINDOW_LIMIT;
    let preferred_start = anchor_position.saturating_sub(MATERIALIZED_WINDOW_BEFORE_ANCHOR);
    let start = preferred_start.min(last_window_start);
    (start, start + MATERIALIZED_WINDOW_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        AlbumId, PlaySourceDescriptor, PlaylistEntrySortDescriptor, PlaylistId, SourceOrder,
    };

    fn track(number: u32) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: AlbumId::fake(1),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }

    fn source_key() -> PlaySourceKey {
        PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: PlaylistId::fake(7),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: None,
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        }
    }

    fn source_item(number: u32, source_index: usize) -> PlaySourceItem {
        PlaySourceItem {
            track: track(number),
            source_index,
            source_item_id: Some(format!("item-{source_index}")),
        }
    }

    fn activation(
        completeness: LoadedCompleteness,
        items: Vec<PlaySourceItem>,
        anchor: PlayAnchor,
    ) -> PlayActivation {
        PlayActivation {
            target: PlayTarget::LoadedSource {
                source_key: source_key(),
                completeness,
                items,
                anchor,
            },
        }
    }

    #[test]
    fn play_use_window() {
        let items = (0..1_200)
            .map(|index| source_item(index as u32, index))
            .collect::<Vec<_>>();
        let normalized = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Complete,
            items,
            PlayAnchor {
                track_id: TrackId::fake(650),
                source_index: 650,
                source_item_id: Some("item-650".to_string()),
            },
        ))
        .expect("complete activation should normalize");

        let NormalizedPlayTarget::Replacement(replacement) = normalized.target else {
            panic!("expected queue replacement");
        };
        assert_eq!(replacement.items.len(), MATERIALIZED_WINDOW_LIMIT);
        let QueueReplacementSource::Source(_source) = replacement.source else {
            panic!("expected source replacement");
        };
        assert_eq!(
            replacement.anchor,
            QueueAnchor::SourcePosition {
                position: 20,
                track_id: TrackId::fake(650),
            }
        );
    }

    #[test]
    fn play_start_windowed() {
        let items = (0..500)
            .map(|index| source_item(index as u32, index))
            .collect::<Vec<_>>();
        let normalized = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Complete,
            items,
            PlayAnchor {
                track_id: TrackId::fake(250),
                source_index: 250,
                source_item_id: Some("item-250".to_string()),
            },
        ))
        .expect("complete activation should normalize");

        let NormalizedPlayTarget::Replacement(replacement) = normalized.target else {
            panic!("expected queue replacement");
        };
        assert_eq!(replacement.items.len(), MATERIALIZED_WINDOW_LIMIT);
        let QueueReplacementSource::Source(_source) = replacement.source else {
            panic!("expected source replacement");
        };
        assert_eq!(
            replacement.anchor,
            QueueAnchor::SourcePosition {
                position: 20,
                track_id: TrackId::fake(250),
            }
        );
    }

    #[test]
    fn play_track_degrades() {
        let selected = TrackId::fake(9);
        let normalized = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Snippet,
            vec![PlaySourceItem {
                track: track(9),
                source_index: 99,
                source_item_id: None,
            }],
            PlayAnchor {
                track_id: selected.clone(),
                source_index: 99,
                source_item_id: None,
            },
        ))
        .expect("snippet activation should normalize");

        let NormalizedPlayTarget::TrackOnly(track) = normalized.target else {
            panic!("expected track-only fallback");
        };
        assert_eq!(track.id, selected);
    }

    #[test]
    fn play_start_index() {
        let error = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Window {
                start: 25,
                total: Some(100),
            },
            vec![source_item(1, 0), source_item(2, 1)],
            PlayAnchor {
                track_id: TrackId::fake(1),
                source_index: 0,
                source_item_id: Some("item-0".to_string()),
            },
        ))
        .expect_err("invalid window activation should fail");

        assert_eq!(error, "The selected track is no longer available.");
    }

    #[test]
    fn play_reject_zero() {
        let error = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Complete,
            vec![source_item(1, 10), source_item(2, 11)],
            PlayAnchor {
                track_id: TrackId::fake(1),
                source_index: 10,
                source_item_id: Some("item-10".to_string()),
            },
        ))
        .expect_err("invalid complete activation should fail");

        assert_eq!(error, "The selected track is no longer available.");
    }

    #[test]
    fn play_fit_total() {
        let error = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Window {
                start: 50,
                total: Some(3),
            },
            vec![source_item(1, 50)],
            PlayAnchor {
                track_id: TrackId::fake(1),
                source_index: 50,
                source_item_id: Some("item-50".to_string()),
            },
        ))
        .expect_err("invalid window activation should fail");

        assert_eq!(error, "The selected track is no longer available.");
    }

    #[test]
    fn play_reject_total() {
        let error = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Window {
                start: 1,
                total: Some(2),
            },
            vec![source_item(1, 1), source_item(2, 2)],
            PlayAnchor {
                track_id: TrackId::fake(1),
                source_index: 1,
                source_item_id: Some("item-1".to_string()),
            },
        ))
        .expect_err("invalid window activation should fail");

        assert_eq!(error, "The selected track is no longer available.");
    }

    #[test]
    fn play_match_occurrence() {
        let duplicate_id = TrackId::fake(42);
        let first = PlaySourceItem {
            track: Track {
                id: duplicate_id.clone(),
                ..track(1)
            },
            source_index: 0,
            source_item_id: Some("first".to_string()),
        };
        let second = PlaySourceItem {
            track: Track {
                id: duplicate_id.clone(),
                ..track(2)
            },
            source_index: 1,
            source_item_id: Some("second".to_string()),
        };
        let normalized = normalize_loaded_source_activation(activation(
            LoadedCompleteness::Complete,
            vec![first, second],
            PlayAnchor {
                track_id: duplicate_id.clone(),
                source_index: 1,
                source_item_id: Some("second".to_string()),
            },
        ))
        .expect("duplicate source item activation should normalize");

        let NormalizedPlayTarget::Replacement(replacement) = normalized.target else {
            panic!("expected queue replacement");
        };
        assert_eq!(
            replacement.anchor,
            QueueAnchor::SourcePosition {
                position: 1,
                track_id: duplicate_id,
            }
        );
    }
}
