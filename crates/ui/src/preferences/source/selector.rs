use std::{rc::Rc, sync::Arc};

use ::library::{MusicFolder, MusicFolderId, SourceId};
use adw::prelude::*;
use gtk::{gio, glib};
use localization::tr;

use crate::preferences::{
    present_add_server_preferences_dialog, present_library_preferences_dialog,
};
use crate::runtime::SelectedLibrary;
use crate::runtime::source::{ConfiguredSources, LocalFolder, SourceSummary};
use crate::shell::Shell;

use super::{configured_source_display_name, folder_count_text};

const SELECT_SOURCE_ACTION: &str = "select-source";
const SELECT_MUSIC_FOLDER_ACTION: &str = "select-music-folder";
const MANAGE_LIBRARIES_ACTION: &str = "manage-music-libraries";
const ADD_LIBRARY_ACTION: &str = "add-music-library";
const MANAGE_LIBRARIES_DETAILED_ACTION: &str = "win.manage-music-libraries";
const ADD_LIBRARY_DETAILED_ACTION: &str = "win.add-music-library";

struct SourceMenuContent {
    name: String,
    selected_source_id: Option<SourceId>,
    sources: Arc<[SourceSummary]>,
    local_folders: Arc<[LocalFolder]>,
    music_folders: Arc<[Arc<MusicFolder>]>,
    selected_music_folder_id: Option<MusicFolderId>,
}

pub(crate) fn install_source_menu_actions(shell: &Rc<Shell>) {
    let select_source = gio::SimpleAction::new_stateful(
        SELECT_SOURCE_ACTION,
        Some(glib::VariantTy::STRING),
        &"".to_variant(),
    );
    let select_source_shell = Rc::clone(shell);
    select_source.connect_activate(move |_, target| {
        let Some(source_id) = target.and_then(glib::Variant::str) else {
            return;
        };
        if select_source_shell
            .source
            .configured
            .borrow()
            .selected_source_id
            .as_ref()
            .is_some_and(|selected| selected.as_str() == source_id)
        {
            return;
        }
        select_source_shell
            .products
            .source
            .select_source(SourceId::new(source_id));
    });
    shell.chrome.window.add_action(&select_source);

    let select_music_folder = gio::SimpleAction::new_stateful(
        SELECT_MUSIC_FOLDER_ACTION,
        Some(glib::VariantTy::STRING),
        &"".to_variant(),
    );
    let select_music_folder_shell = Rc::clone(shell);
    select_music_folder.connect_activate(move |_, target| {
        let Some(folder_id) = target.and_then(glib::Variant::str) else {
            return;
        };
        let Some(source_id) = select_music_folder_shell
            .source
            .configured
            .borrow()
            .selected_source_id
            .clone()
        else {
            return;
        };
        let selected_folder_id = select_music_folder_shell
            .library
            .selected
            .borrow()
            .as_ref()
            .and_then(|selected| selected.music_folder_id.as_ref())
            .map(MusicFolderId::as_str)
            .unwrap_or_default()
            .to_string();
        if selected_folder_id == folder_id {
            return;
        }
        select_music_folder_shell.products.source.set_music_folder(
            source_id,
            (!folder_id.is_empty()).then(|| MusicFolderId::new(folder_id)),
        );
    });
    shell.chrome.window.add_action(&select_music_folder);

    let manage_libraries = gio::SimpleAction::new(MANAGE_LIBRARIES_ACTION, None);
    let manage_libraries_shell = Rc::clone(shell);
    manage_libraries.connect_activate(move |_, _| {
        present_library_preferences_dialog(&manage_libraries_shell);
    });
    shell.chrome.window.add_action(&manage_libraries);

    let add_library = gio::SimpleAction::new(ADD_LIBRARY_ACTION, None);
    let add_library_shell = Rc::clone(shell);
    add_library.connect_activate(move |_, _| {
        present_add_server_preferences_dialog(&add_library_shell);
    });
    shell.chrome.window.add_action(&add_library);
}

pub(crate) fn source_submenu(shell: &Rc<Shell>) -> (String, gio::Menu) {
    let configured = shell.source.configured.borrow();
    let selected = shell.library.selected.borrow();
    let content = source_menu_content(&configured, selected.as_ref());
    update_selection_action_states(shell, &content);

    let menu = gio::Menu::new();
    let sources = gio::Menu::new();
    if content.sources.is_empty() && content.local_folders.is_empty() {
        sources.append(Some(&tr("No sources configured")), None);
    } else {
        for index in source_order(&content.sources, content.selected_source_id.as_ref()) {
            let source = &content.sources[index];
            let label = source_menu_label(source, &content.local_folders);
            append_targeted_item(&sources, &label, "win.select-source", source.id.as_str());
        }
    }
    menu.append_section(Some(&tr("Select Source")), &sources);

    if !content.music_folders.is_empty() {
        let folders = gio::Menu::new();
        append_targeted_item(&folders, &tr("All Music"), "win.select-music-folder", "");
        for folder in content.music_folders.iter() {
            append_targeted_item(
                &folders,
                &folder.name,
                "win.select-music-folder",
                folder.id.as_str(),
            );
        }
        menu.append_section(Some(&tr("Server Library")), &folders);
    }

    let commands = gio::Menu::new();
    commands.append(Some(&tr("Manage")), Some(MANAGE_LIBRARIES_DETAILED_ACTION));
    commands.append(
        Some(&tr("Add music library")),
        Some(ADD_LIBRARY_DETAILED_ACTION),
    );
    menu.append_section(None, &commands);

    (content.name, menu)
}

fn source_menu_content(
    configured: &ConfiguredSources,
    selected: Option<&SelectedLibrary>,
) -> SourceMenuContent {
    let selected_source_id = configured.selected_source_id.clone();
    let active_source = selected_source_id.as_ref().and_then(|selected| {
        configured
            .sources
            .iter()
            .find(|source| &source.id == selected)
            .cloned()
    });
    let Some(source) = active_source else {
        return SourceMenuContent {
            name: tr("No source"),
            selected_source_id,
            sources: Arc::clone(&configured.sources),
            local_folders: Arc::clone(&configured.local_folders),
            music_folders: Arc::from([]),
            selected_music_folder_id: None,
        };
    };

    let music_folders = selected
        .filter(|selected| selected.source_id == source.id)
        .and_then(|selected| selected.loaded.music_folders().ok())
        .unwrap_or_else(|| Arc::from([]));
    let selected_music_folder_id = if music_folders.is_empty() {
        None
    } else {
        selected.and_then(|selected| selected.music_folder_id.clone())
    };
    let name = configured_source_display_name(&source);
    SourceMenuContent {
        name,
        selected_source_id,
        sources: Arc::clone(&configured.sources),
        local_folders: Arc::clone(&configured.local_folders),
        music_folders,
        selected_music_folder_id,
    }
}

fn update_selection_action_states(shell: &Shell, content: &SourceMenuContent) {
    let source_id = content
        .selected_source_id
        .as_ref()
        .map(SourceId::as_str)
        .unwrap_or_default();
    set_action_state(shell, SELECT_SOURCE_ACTION, source_id);
    let folder_id = content
        .selected_music_folder_id
        .as_ref()
        .map(MusicFolderId::as_str)
        .unwrap_or_default();
    set_action_state(shell, SELECT_MUSIC_FOLDER_ACTION, folder_id);
}

fn set_action_state(shell: &Shell, name: &str, state: &str) {
    let Some(action) = shell.chrome.window.lookup_action(name) else {
        return;
    };
    let Ok(action) = action.downcast::<gio::SimpleAction>() else {
        return;
    };
    action.set_state(&state.to_variant());
}

fn append_targeted_item(menu: &gio::Menu, label: &str, action: &str, target: &str) {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    menu.append_item(&item);
}

fn source_menu_label(source: &SourceSummary, local_folders: &[LocalFolder]) -> String {
    let title = configured_source_display_name(source);
    if source.kind == "local" {
        format!(
            "{title} · {}",
            folder_count_text(local_folders.len() as u64)
        )
    } else {
        title
    }
}

fn source_order(sources: &[SourceSummary], selected: Option<&SourceId>) -> Vec<usize> {
    let mut order = Vec::with_capacity(sources.len());
    if let Some(selected) = selected
        && let Some(index) = sources.iter().position(|source| &source.id == selected)
    {
        order.push(index);
    }
    order.extend(
        sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| (Some(&source.id) != selected).then_some(index)),
    );
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str) -> SourceSummary {
        SourceSummary {
            id: SourceId::new(id),
            kind: "test".to_string(),
            name: id.to_string(),
        }
    }

    #[test]
    fn selected_source_is_first_without_reordering_the_others() {
        let sources = vec![source("first"), source("second"), source("selected")];
        let selected = sources[2].id.clone();
        assert_eq!(source_order(&sources, Some(&selected)), [2, 0, 1]);
        assert_eq!(source_order(&sources, None), [0, 1, 2]);
    }
}
