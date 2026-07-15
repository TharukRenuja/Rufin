use std::{cell::RefCell, rc::Rc};

use ::library::{MusicFolderId, Track, play_context::PlayContextDescriptor};

use crate::shell::Shell;
use crate::{LibraryListKey, LibraryListSettings};
use playback::{LibraryWindowPlayRequest, QueueHandle};

#[derive(Clone)]
pub(crate) struct LoadedTrackPlayContext {
    descriptor: Rc<RefCell<PlayContextDescriptor>>,
    settings: Rc<dyn Fn() -> LibraryListSettings>,
    query: Rc<RefCell<String>>,
    favorites_only: bool,
    favorite_first: bool,
}

impl LoadedTrackPlayContext {
    pub(crate) fn play_window(
        &self,
        controller: &QueueHandle,
        total_items: usize,
        anchor_index: usize,
        track_at: impl FnMut(usize) -> Option<Track> + 'static,
    ) -> bool {
        let settings = (self.settings)();
        controller.play_library_window(LibraryWindowPlayRequest {
            descriptor: self.descriptor.borrow().clone(),
            sort: settings.sort_key.track_sort(),
            descending: settings.descending,
            query: self.query.borrow().to_string(),
            favorites_only: self.favorites_only,
            favorite_first: self.favorite_first,
            total_items,
            anchor_index,
            track_at: Box::new(track_at),
        })
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
    shell: &Rc<Shell>,
    descriptor: Rc<RefCell<PlayContextDescriptor>>,
    key: LibraryListKey,
    query: Rc<RefCell<String>>,
    favorites_only: bool,
    favorite_first: bool,
) -> LoadedTrackPlayContext {
    let shell = Rc::clone(shell);
    LoadedTrackPlayContext {
        descriptor,
        settings: Rc::new(move || shell.settings.current.borrow().library_list(key)),
        query,
        favorites_only,
        favorite_first,
    }
}
