use super::*;

const SMART_RULE_CHOICES: [&str; 10] = [
    "Title contains",
    "Artist contains",
    "Album contains",
    "Comment contains",
    "Genre contains",
    "Genre excludes",
    "Rating above",
    "Year range",
    "Favorites",
    "Unplayed",
];

impl Shell {
    pub(in crate::ui) fn new_smart_playlist_dialog(self: &Rc<Self>) {
        let missing_defaults = self
            .controller
            .missing_builtin_smart_playlists()
            .unwrap_or_default();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("New Smart Playlist"))
            .body(tr(
                "Restore a default smart playlist or create a simple rule.",
            ))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        if !missing_defaults.is_empty() {
            dialog.add_response("restore", &tr("Restore Default"));
        }
        dialog.add_response("create", &tr("Create"));
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        if !missing_defaults.is_empty() {
            let default_titles = missing_defaults
                .iter()
                .map(|builtin| tr(builtin.title()))
                .collect::<Vec<_>>();
            let default_refs = default_titles
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let default_options = gtk::StringList::new(&default_refs);
            let default_dropdown =
                gtk::DropDown::new(Some(default_options), None::<gtk::Expression>);
            default_dropdown.set_hexpand(true);
            content.append(&default_dropdown);
            dialog.set_extra_child(Some(&content));

            let name = gtk::Entry::new();
            name.set_placeholder_text(Some(&tr("Playlist name")));
            let rule_options = gtk::StringList::new(&SMART_RULE_CHOICES);
            let rule_dropdown = gtk::DropDown::new(Some(rule_options), None::<gtk::Expression>);
            let value = gtk::Entry::new();
            value.set_placeholder_text(Some(&tr("Rule value")));
            content.append(&name);
            content.append(&rule_dropdown);
            content.append(&value);

            let controller = self.controller.clone();
            dialog.connect_response(None, move |_, response| match response {
                "restore" => {
                    let selected = default_dropdown.selected() as usize;
                    if let Some(builtin) = missing_defaults.get(selected).copied() {
                        controller.restore_builtin_smart_playlist(builtin);
                    }
                }
                "create" => {
                    let name = name.text().trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    if let Some(definition) = smart_playlist_definition_from_choice(
                        rule_dropdown.selected(),
                        &value.text(),
                    ) {
                        controller.save_smart_playlist(name, definition);
                    }
                }
                _ => {}
            });
            dialog.present(Some(&self.window));
            return;
        }

        let name = gtk::Entry::new();
        name.set_placeholder_text(Some(&tr("Playlist name")));
        let rule_options = gtk::StringList::new(&SMART_RULE_CHOICES);
        let rule_dropdown = gtk::DropDown::new(Some(rule_options), None::<gtk::Expression>);
        let value = gtk::Entry::new();
        value.set_placeholder_text(Some(&tr("Rule value")));
        content.append(&name);
        content.append(&rule_dropdown);
        content.append(&value);
        dialog.set_extra_child(Some(&content));

        let controller = self.controller.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "create" {
                return;
            }
            let name = name.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Some(definition) =
                smart_playlist_definition_from_choice(rule_dropdown.selected(), &value.text())
            {
                controller.save_smart_playlist(name, definition);
            }
        });
        dialog.present(Some(&self.window));
    }

    pub(in crate::ui) fn edit_smart_playlist_dialog(self: &Rc<Self>, playlist: SmartPlaylist) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Edit Smart Playlist"))
            .body(tr("Choose a simple replacement rule."))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("save", &tr("Save"));
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let name = gtk::Entry::new();
        name.set_placeholder_text(Some(&tr("Playlist name")));
        name.set_text(&playlist.name);
        let rule_options = gtk::StringList::new(&SMART_RULE_CHOICES);
        let rule_dropdown = gtk::DropDown::new(Some(rule_options), None::<gtk::Expression>);
        let value = gtk::Entry::new();
        value.set_placeholder_text(Some(&tr("Rule value")));
        content.append(&name);
        content.append(&rule_dropdown);
        content.append(&value);
        dialog.set_extra_child(Some(&content));

        let controller = self.controller.clone();
        let playlist_id = playlist.id.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "save" {
                return;
            }
            let name = name.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Some(definition) =
                smart_playlist_definition_from_choice(rule_dropdown.selected(), &value.text())
            {
                controller.update_smart_playlist(playlist_id.clone(), name, definition);
            }
        });
        dialog.present(Some(&self.window));
    }
}

fn smart_playlist_definition_from_choice(
    selected: u32,
    value: &glib::GString,
) -> Option<SmartPlaylistDefinition> {
    let value = value.trim();
    let rule = match selected {
        0 => text_rule(
            SmartPlaylistRuleField::Title,
            SmartPlaylistRuleOperator::Contains,
            value,
        )?,
        1 => text_rule(
            SmartPlaylistRuleField::Artist,
            SmartPlaylistRuleOperator::Contains,
            value,
        )?,
        2 => text_rule(
            SmartPlaylistRuleField::Album,
            SmartPlaylistRuleOperator::Contains,
            value,
        )?,
        3 => text_rule(
            SmartPlaylistRuleField::Comment,
            SmartPlaylistRuleOperator::Contains,
            value,
        )?,
        4 => text_rule(
            SmartPlaylistRuleField::Genre,
            SmartPlaylistRuleOperator::Contains,
            value,
        )?,
        5 => text_rule(
            SmartPlaylistRuleField::Genre,
            SmartPlaylistRuleOperator::NotContains,
            value,
        )?,
        6 => number_rule(
            SmartPlaylistRuleField::Rating,
            SmartPlaylistRuleOperator::Above,
            value.parse::<i64>().ok()?,
        ),
        7 => year_range_rule(value)?,
        8 => bool_rule(SmartPlaylistRuleField::Favorite, true),
        9 => bool_rule(SmartPlaylistRuleField::Played, false),
        _ => return None,
    };
    Some(SmartPlaylistDefinition {
        root: SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![SmartPlaylistRuleNode::Rule(rule)],
        },
        sort_field: SmartPlaylistSortField::Title,
        descending: false,
        limit: None,
    })
}

fn text_rule(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
    value: &str,
) -> Option<SmartPlaylistRule> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(SmartPlaylistRule {
        field,
        operator,
        value: Some(SmartPlaylistRuleValue::Text(value.to_string())),
    })
}

fn number_rule(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
    value: i64,
) -> SmartPlaylistRule {
    SmartPlaylistRule {
        field,
        operator,
        value: Some(SmartPlaylistRuleValue::Number(value)),
    }
}

fn bool_rule(field: SmartPlaylistRuleField, value: bool) -> SmartPlaylistRule {
    SmartPlaylistRule {
        field,
        operator: SmartPlaylistRuleOperator::Is,
        value: Some(SmartPlaylistRuleValue::Bool(value)),
    }
}

fn year_range_rule(value: &str) -> Option<SmartPlaylistRule> {
    let (min, max) = value.split_once('-')?;
    Some(SmartPlaylistRule {
        field: SmartPlaylistRuleField::Year,
        operator: SmartPlaylistRuleOperator::Between,
        value: Some(SmartPlaylistRuleValue::NumberRange {
            min: min.trim().parse().ok()?,
            max: max.trim().parse().ok()?,
        }),
    })
}
