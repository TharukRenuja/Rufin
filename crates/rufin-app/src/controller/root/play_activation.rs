use rufin_core::{
    PlaySourceKey, QueueAnchor, QueueItemInput, QueueReplacement, QueueReplacementSource,
    QueueSourceInput, Track, TrackId,
};

pub const FULL_LOADED_LIMIT: usize = 100;
pub const MATERIALIZED_WINDOW_LIMIT: usize = 100;
pub const MATERIALIZED_WINDOW_BEFORE_ANCHOR: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayActivation {
    pub action: PlayAction,
    pub target: PlayTarget,
}

// Route conversion will construct every action after this normalization seam is wired.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayAction {
    ReplaceNow,
    InsertNext,
    AppendLast,
    ActivateQueueEntry,
    MoveQueueEntryAfterCurrent,
}

// Track-only activations are accepted by the seam before route conversion emits them.
#[allow(dead_code)]
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayTarget {
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
    TrackOnly(Track),
    Replacement(QueueReplacement),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedPlayActivation {
    pub action: PlayAction,
    pub target: NormalizedPlayTarget,
}

pub fn normalize_loaded_source_activation(
    activation: PlayActivation,
) -> Result<NormalizedPlayActivation, String> {
    let PlayActivation { action, target } = activation;
    let target = match target {
        PlayTarget::TrackOnly(track) => NormalizedPlayTarget::TrackOnly(track),
        PlayTarget::LoadedSource {
            source_key,
            completeness,
            items,
            anchor,
        } => normalize_loaded_source_target(&action, source_key, completeness, items, anchor)?,
        PlayTarget::StoreBackedSource { .. } => {
            return Err("The selected source could not be resolved.".to_string());
        }
    };
    Ok(NormalizedPlayActivation { action, target })
}

fn normalize_loaded_source_target(
    action: &PlayAction,
    source_key: PlaySourceKey,
    completeness: LoadedCompleteness,
    items: Vec<PlaySourceItem>,
    anchor: PlayAnchor,
) -> Result<NormalizedPlayTarget, String> {
    validate_loaded_items(&completeness, &items)?;
    let anchor_position = matching_anchor_position(&items, &anchor)?;

    if *action == PlayAction::ReplaceNow && completeness == LoadedCompleteness::Snippet {
        return Ok(NormalizedPlayTarget::TrackOnly(
            items[anchor_position].track.clone(),
        ));
    }

    let (materialized_start, materialized_end) = materialized_range(items.len(), anchor_position);
    let materialized_items = items[materialized_start..materialized_end]
        .iter()
        .map(|item| QueueItemInput::Source {
            track: item.track.clone(),
            source_index: item.source_index,
            source_item_id: item.source_item_id.clone(),
        })
        .collect::<Vec<_>>();
    let materialized_len = materialized_items.len();
    let source = QueueSourceInput {
        source_key,
        total_source_items: total_source_items(&completeness, items.len()),
        materialized_start: items[materialized_start].source_index,
        materialized_len,
        capped: materialized_len < items.len(),
    };
    Ok(NormalizedPlayTarget::Replacement(QueueReplacement {
        source: QueueReplacementSource::Source(source),
        items: materialized_items,
        anchor: QueueAnchor::SourceOccurrence {
            track_id: anchor.track_id,
            source_index: anchor.source_index,
            source_item_id: anchor.source_item_id,
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
                let last_source_index = items
                    .last()
                    .expect("non-empty source has a last item")
                    .source_index;
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

fn total_source_items(completeness: &LoadedCompleteness, loaded_len: usize) -> Option<usize> {
    match completeness {
        LoadedCompleteness::Complete => Some(loaded_len),
        LoadedCompleteness::Window { total, .. } => *total,
        LoadedCompleteness::Snippet => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_core::{
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
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
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
            action: PlayAction::ReplaceNow,
            target: PlayTarget::LoadedSource {
                source_key: source_key(),
                completeness,
                items,
                anchor,
            },
        }
    }

    #[test]
    fn complete_loaded_source_over_limit_uses_anchor_biased_window() {
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
        let QueueReplacementSource::Source(source) = replacement.source else {
            panic!("expected source replacement");
        };
        assert_eq!(source.materialized_start, 630);
        assert_eq!(source.materialized_len, MATERIALIZED_WINDOW_LIMIT);
        assert_eq!(source.total_source_items, Some(1_200));
        assert!(source.capped);
        assert_eq!(
            replacement.anchor,
            QueueAnchor::SourceOccurrence {
                track_id: TrackId::fake(650),
                source_index: 650,
                source_item_id: Some("item-650".to_string()),
            }
        );
    }

    #[test]
    fn five_hundred_item_source_is_windowed_for_play_start() {
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
        let QueueReplacementSource::Source(source) = replacement.source else {
            panic!("expected source replacement");
        };
        assert_eq!(source.materialized_start, 230);
        assert_eq!(source.total_source_items, Some(500));
        assert!(source.capped);
    }

    #[test]
    fn snippet_replace_degrades_to_track_only() {
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
    fn window_rejects_items_when_first_source_index_does_not_match_start() {
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
    fn complete_rejects_items_when_first_source_index_is_not_zero() {
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
    fn window_rejects_items_that_do_not_fit_inside_total() {
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
    fn window_rejects_items_when_last_source_index_is_outside_total() {
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
    fn duplicate_track_ids_with_different_source_item_id_selects_matching_anchor_occurrence() {
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
            QueueAnchor::SourceOccurrence {
                track_id: duplicate_id,
                source_index: 1,
                source_item_id: Some("second".to_string()),
            }
        );
    }
}
