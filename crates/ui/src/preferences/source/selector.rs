use std::{rc::Rc, sync::Arc};

use ::library::{MusicFolder, MusicFolderId, SourceId};
use adw::prelude::*;
use gtk::gio;
use localization::tr;

use crate::preferences::{
    present_add_server_preferences_dialog, present_library_preferences_dialog,
};
use crate::runtime::SelectedLibrary;
use crate::runtime::source::{ConfiguredSources, LocalFolder, SourceSummary};
use crate::shell::Shell;

use super::{configured_source_display_name, folder_count_text};

const SOURCE_CHOICE_ACTION_PREFIX: &str = "source-choice-";
const MUSIC_FOLDER_CHOICE_ACTION_PREFIX: &str = "music-folder-choice-";
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
    replace_selection_actions(shell, &content);

    let menu = gio::Menu::new();
    let sources = gio::Menu::new();
    if content.sources.is_empty() && content.local_folders.is_empty() {
        sources.append(Some(&tr("No sources configured")), None);
    } else {
        for index in source_order(&content.sources, content.selected_source_id.as_ref()) {
            let source = &content.sources[index];
            let label = source_menu_label(source, &content.local_folders);
            sources.append(
                Some(&label),
                Some(&format!("win.{SOURCE_CHOICE_ACTION_PREFIX}{index}")),
            );
        }
    }
    menu.append_section(Some(&tr("Select Source")), &sources);

    if !content.music_folders.is_empty() {
        let folders = gio::Menu::new();
        folders.append(
            Some(&tr("All Music")),
            Some(&format!("win.{MUSIC_FOLDER_CHOICE_ACTION_PREFIX}0")),
        );
        for (index, folder) in content.music_folders.iter().enumerate() {
            folders.append(
                Some(&folder.name),
                Some(&format!(
                    "win.{MUSIC_FOLDER_CHOICE_ACTION_PREFIX}{}",
                    index + 1
                )),
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

fn replace_selection_actions(shell: &Rc<Shell>, content: &SourceMenuContent) {
    for name in shell.chrome.window.list_actions() {
        if name.starts_with(SOURCE_CHOICE_ACTION_PREFIX)
            || name.starts_with(MUSIC_FOLDER_CHOICE_ACTION_PREFIX)
        {
            shell.chrome.window.remove_action(&name);
        }
    }

    for (index, source) in content.sources.iter().enumerate() {
        let action = choice_action(
            &format!("{SOURCE_CHOICE_ACTION_PREFIX}{index}"),
            content.selected_source_id.as_ref() == Some(&source.id),
            {
                let shell = Rc::clone(shell);
                let source_id = source.id.clone();
                move || {
                    if shell.source.configured.borrow().selected_source_id.as_ref()
                        != Some(&source_id)
                    {
                        shell.products.source.select_source(source_id.clone());
                    }
                }
            },
        );
        shell.chrome.window.add_action(&action);
    }

    if content.music_folders.is_empty() {
        return;
    }
    let all_music = choice_action(
        &format!("{MUSIC_FOLDER_CHOICE_ACTION_PREFIX}0"),
        content.selected_music_folder_id.is_none(),
        {
            let shell = Rc::clone(shell);
            move || select_music_folder(&shell, None)
        },
    );
    shell.chrome.window.add_action(&all_music);
    for (index, folder) in content.music_folders.iter().enumerate() {
        let folder_id = folder.id.clone();
        let action = choice_action(
            &format!("{MUSIC_FOLDER_CHOICE_ACTION_PREFIX}{}", index + 1),
            content.selected_music_folder_id.as_ref() == Some(&folder_id),
            {
                let shell = Rc::clone(shell);
                move || select_music_folder(&shell, Some(folder_id.clone()))
            },
        );
        shell.chrome.window.add_action(&action);
    }
}

fn choice_action(name: &str, active: bool, select: impl Fn() + 'static) -> gio::SimpleAction {
    let action = gio::SimpleAction::new_stateful(name, None, &active.to_variant());
    action.connect_activate(move |action, _| {
        action.set_state(&true.to_variant());
        select();
    });
    action
}

fn select_music_folder(shell: &Shell, folder_id: Option<MusicFolderId>) {
    let Some(source_id) = shell.source.configured.borrow().selected_source_id.clone() else {
        return;
    };
    let selected_folder_id = shell
        .library
        .selected
        .borrow()
        .as_ref()
        .and_then(|selected| selected.music_folder_id.clone());
    if selected_folder_id.as_ref() == folder_id.as_ref() {
        return;
    }
    shell.products.source.set_music_folder(source_id, folder_id);
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
    use std::cell::Cell;

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

    #[test]
    fn selection_choices_stay_checked_when_activated() {
        let selected = Rc::new(Cell::new(false));
        let selected_for_action = Rc::clone(&selected);
        let action = choice_action("choice", false, move || selected_for_action.set(true));

        assert!(action.parameter_type().is_none());
        assert_eq!(
            action.state().and_then(|state| state.get::<bool>()),
            Some(false)
        );

        action.activate(None);

        assert!(selected.get());
        assert_eq!(
            action.state().and_then(|state| state.get::<bool>()),
            Some(true)
        );
    }
}
