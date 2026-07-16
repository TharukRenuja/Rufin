use std::{cell::RefCell, rc::Rc};

use ::library::{MusicFolderId, TrackId, play_context::PlayContextDescriptor};

use crate::shell::Shell;
use playback::{LibraryWindowPlayRequest, QueueHandle};

use super::track_model::TrackCollectionModel;

#[derive(Clone)]
pub(crate) struct LoadedTrackPlayContext {
    descriptor: Rc<RefCell<PlayContextDescriptor>>,
    model: TrackCollectionModel,
    favorites_only: bool,
    favorite_first: bool,
}

impl LoadedTrackPlayContext {
    pub(crate) fn play_window(
        &self,
        controller: &QueueHandle,
        anchor_index: usize,
        anchor_track_id: TrackId,
    ) -> bool {
        controller.play_library_window(self.request(anchor_index, anchor_track_id))
    }

    pub(crate) fn request(
        &self,
        anchor_index: usize,
        anchor_track_id: TrackId,
    ) -> LibraryWindowPlayRequest {
        let settings = self.model.settings();
        let total_items = self.model.visible_count();
        let lookup = self.model.clone();
        LibraryWindowPlayRequest {
            descriptor: self.descriptor.borrow().clone(),
            sort: settings.sort_key.track_sort(),
            descending: settings.descending,
            query: self.model.query(),
            favorites_only: self.favorites_only,
            favorite_first: self.favorite_first,
            total_items,
            anchor_index,
            track_at: Box::new(move |index| {
                let candidate = lookup.track_at(index as u32)?;
                (index != anchor_index || candidate.id == anchor_track_id).then_some(candidate)
            }),
        }
    }
}

pub(crate) fn selected_music_folder_id(shell: &Shell) -> Option<MusicFolderId> {
    shell
        .source
        .presentation
        .borrow()
        .selected_music_folder_id
        .clone()
}

pub(crate) fn track_collection_play_context(
    descriptor: Rc<RefCell<PlayContextDescriptor>>,
    model: TrackCollectionModel,
    favorites_only: bool,
    favorite_first: bool,
) -> LoadedTrackPlayContext {
    LoadedTrackPlayContext {
        descriptor,
        model,
        favorites_only,
        favorite_first,
    }
}
