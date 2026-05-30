use super::layout::large_popup_content_width;
use super::*;

const SMART_PLAYLIST_DIALOG_WIDTH: i32 = 700;
const SMART_PLAYLIST_DIALOG_HEIGHT: i32 = 510;
const RULE_GROUP_INDENT: i32 = 18;

type RerenderSlot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

#[derive(Clone)]
struct SmartPlaylistEditor {
    name: gtk::Entry,
    root: Rc<RefCell<SmartPlaylistRuleGroup>>,
    sort: gtk::DropDown,
    descending: gtk::CheckButton,
    limit: gtk::Entry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuleInputKind {
    None,
    Text,
    Number,
    NumberRange,
    Date,
    DateRange,
    Bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuleFieldSpec {
    field: SmartPlaylistRuleField,
    title: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuleOperatorSpec {
    operator: SmartPlaylistRuleOperator,
    title: &'static str,
    input: RuleInputKind,
}

const RULE_FIELDS: [RuleFieldSpec; 13] = [
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Title,
        title: "Title",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Artist,
        title: "Artist",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Album,
        title: "Album",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Comment,
        title: "Comment",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Genre,
        title: "Genre",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Rating,
        title: "Rating",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Year,
        title: "Year",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Favorite,
        title: "Favorite",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::Played,
        title: "Played",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::PlayCount,
        title: "Play count",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::SkipCount,
        title: "Skip count",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::LastPlayed,
        title: "Last played",
    },
    RuleFieldSpec {
        field: SmartPlaylistRuleField::DateAdded,
        title: "Date added",
    },
];

const SORT_FIELDS: [(SmartPlaylistSortField, &str); 10] = [
    (SmartPlaylistSortField::Title, "Title"),
    (SmartPlaylistSortField::Artist, "Artist"),
    (SmartPlaylistSortField::Album, "Album"),
    (SmartPlaylistSortField::Year, "Year"),
    (SmartPlaylistSortField::DateAdded, "Date added"),
    (SmartPlaylistSortField::LastPlayed, "Last played"),
    (SmartPlaylistSortField::PlayCount, "Play count"),
    (SmartPlaylistSortField::SkipCount, "Skip count"),
    (SmartPlaylistSortField::Rating, "Rating"),
    (SmartPlaylistSortField::Duration, "Duration"),
];

impl Shell {
    pub(in crate::ui) fn new_smart_playlist_dialog(self: &Rc<Self>) {
        let missing_defaults = self
            .controller
            .missing_builtin_smart_playlists()
            .unwrap_or_default();
        let editor = smart_playlist_editor(None, None);
        let (content, default_dropdown) =
            smart_playlist_editor_content(&editor, missing_defaults.as_slice());
        let actions = dialog_action_row();
        let restore =
            (!missing_defaults.is_empty()).then(|| dialog_button("Restore Default", None));
        if let Some(restore) = &restore {
            actions.append(restore);
        }
        let cancel = dialog_button("Cancel", None);
        let create = dialog_button("Create", Some("suggested-action"));
        sync_editor_button_enabled(&create, &editor);
        actions.append(&cancel);
        actions.append(&create);

        let dialog = smart_playlist_dialog("New Smart Playlist", &content, &actions);
        connect_editor_name_validation(&create, &editor);

        {
            let dialog = dialog.clone();
            cancel.connect_clicked(move |_| {
                dialog.close();
            });
        }

        let controller = self.controller.clone();
        if let Some(restore) = restore {
            let controller = controller.clone();
            let dialog = dialog.clone();
            restore.connect_clicked(move |_| {
                if let Some(default_dropdown) = default_dropdown.as_ref() {
                    let selected = default_dropdown.selected() as usize;
                    if let Some(builtin) = missing_defaults.get(selected).copied() {
                        controller.restore_builtin_smart_playlist(builtin);
                    }
                }
                dialog.close();
            });
        }
        {
            let dialog = dialog.clone();
            create.connect_clicked(move |_| {
                let Some((name, definition)) = editor.definition() else {
                    return;
                };
                controller.save_smart_playlist(name, definition);
                dialog.close();
            });
        }
        dialog.present(Some(&self.window));
    }

    pub(in crate::ui) fn edit_smart_playlist_dialog(self: &Rc<Self>, playlist: SmartPlaylist) {
        let editor = smart_playlist_editor(Some(&playlist.name), Some(&playlist.definition));
        let (content, _default_dropdown) = smart_playlist_editor_content(&editor, &[]);
        let actions = dialog_action_row();
        let cancel = dialog_button("Cancel", None);
        let save = dialog_button("Save", Some("suggested-action"));
        sync_editor_button_enabled(&save, &editor);
        actions.append(&cancel);
        actions.append(&save);

        let dialog = smart_playlist_dialog("Edit Smart Playlist", &content, &actions);
        connect_editor_name_validation(&save, &editor);

        {
            let dialog = dialog.clone();
            cancel.connect_clicked(move |_| {
                dialog.close();
            });
        }

        let controller = self.controller.clone();
        let playlist_id = playlist.id.clone();
        {
            let dialog = dialog.clone();
            save.connect_clicked(move |_| {
                let Some((name, definition)) = editor.definition() else {
                    return;
                };
                controller.update_smart_playlist(playlist_id.clone(), name, definition);
                dialog.close();
            });
        }
        dialog.present(Some(&self.window));
    }
}

impl SmartPlaylistEditor {
    fn definition(&self) -> Option<(String, SmartPlaylistDefinition)> {
        let name = playlist_name(&self.name.text())?;
        let mut root = self.root.borrow().clone();
        normalize_root_group(&mut root);
        let limit = self
            .limit
            .text()
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0);
        let sort_field = SORT_FIELDS
            .get(self.sort.selected() as usize)
            .map(|(field, _)| *field)
            .unwrap_or(SmartPlaylistSortField::Title);
        Some((
            name,
            SmartPlaylistDefinition {
                root,
                sort_field,
                descending: self.descending.is_active(),
                limit,
            },
        ))
    }
}

fn smart_playlist_editor(
    name: Option<&str>,
    definition: Option<&SmartPlaylistDefinition>,
) -> SmartPlaylistEditor {
    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some(&tr("Playlist name")));
    if let Some(name) = name {
        name_entry.set_text(name);
    }

    let definition = definition.cloned().unwrap_or_else(default_definition);
    let sort = dropdown_from_titles(
        &SORT_FIELDS
            .iter()
            .map(|(_, title)| *title)
            .collect::<Vec<_>>(),
        sort_index(definition.sort_field),
    );
    let descending = gtk::CheckButton::with_label(&tr("Descending"));
    descending.set_active(definition.descending);
    let limit = gtk::Entry::new();
    limit.set_placeholder_text(Some(&tr("No limit")));
    limit.set_width_chars(8);
    if let Some(value) = definition.limit {
        limit.set_text(&value.to_string());
    }

    SmartPlaylistEditor {
        name: name_entry,
        root: Rc::new(RefCell::new(definition.root)),
        sort,
        descending,
        limit,
    }
}

fn smart_playlist_editor_content(
    editor: &SmartPlaylistEditor,
    missing_defaults: &[SmartPlaylistBuiltin],
) -> (gtk::Widget, Option<gtk::DropDown>) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(4);
    content.set_margin_bottom(4);
    content.set_margin_start(4);
    content.set_margin_end(4);

    let default_dropdown = if missing_defaults.is_empty() {
        None
    } else {
        let default_titles = missing_defaults
            .iter()
            .map(|builtin| tr(builtin.title()))
            .collect::<Vec<_>>();
        let default_refs = default_titles
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let default_dropdown = dropdown_from_titles(&default_refs, 0);
        default_dropdown.set_hexpand(true);
        let row = labeled_row("Restore", &[default_dropdown.clone().upcast()]);
        content.append(&row);
        Some(default_dropdown)
    };

    content.append(&editor.name);

    let match_dropdown = match_mode_dropdown(editor.root.borrow().mode);
    {
        let root = Rc::clone(&editor.root);
        match_dropdown.connect_selected_notify(move |dropdown| {
            root.borrow_mut().mode = match_mode_from_index(dropdown.selected());
        });
    }
    let settings = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    settings.append(&labeled_control("Match", &match_dropdown));
    settings.append(&labeled_control("Sort", &editor.sort));
    settings.append(&editor.descending);
    settings.append(&labeled_control("Limit", &editor.limit));
    content.append(&settings);

    let rules = gtk::Box::new(gtk::Orientation::Vertical, 10);
    rules.set_hexpand(true);
    let rerender_slot: RerenderSlot = Rc::new(RefCell::new(None));
    let rerender = {
        let rules = rules.clone();
        let root = Rc::clone(&editor.root);
        let rerender_slot = Rc::clone(&rerender_slot);
        Rc::new(move || {
            clear_box(&rules);
            let Some(rerender) = rerender_slot.borrow().as_ref().cloned() else {
                return;
            };
            append_rule_group(&rules, Rc::clone(&root), Vec::new(), rerender, false);
        })
    };
    *rerender_slot.borrow_mut() = Some(rerender.clone());
    rerender();
    content.append(&rules);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_max_content_height(SMART_PLAYLIST_DIALOG_HEIGHT);
    scroller.set_child(Some(&content));
    (scroller.upcast(), default_dropdown)
}

fn append_rule_group(
    parent: &gtk::Box,
    root: Rc<RefCell<SmartPlaylistRuleGroup>>,
    path: Vec<usize>,
    rerender: Rc<dyn Fn()>,
    removable: bool,
) {
    let Some(group) = group_at(&root.borrow(), &path).cloned() else {
        return;
    };
    let frame = gtk::Frame::new(None);
    frame.set_hexpand(true);
    if !path.is_empty() {
        frame.set_margin_start(RULE_GROUP_INDENT);
    }
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
    box_.set_margin_top(10);
    box_.set_margin_bottom(10);
    box_.set_margin_start(10);
    box_.set_margin_end(10);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(&gtk::Label::new(Some(&tr("Match"))));
    let mode = match_mode_dropdown(group.mode);
    {
        let root = Rc::clone(&root);
        let path = path.clone();
        mode.connect_selected_notify(move |dropdown| {
            if let Some(group) = group_at_mut(&mut root.borrow_mut(), &path) {
                group.mode = match_mode_from_index(dropdown.selected());
            }
        });
    }
    header.append(&mode);

    let add_rule = text_button("list-add-symbolic", "Add Rule");
    {
        let root = Rc::clone(&root);
        let path = path.clone();
        let rerender = Rc::clone(&rerender);
        add_rule.connect_clicked(move |_| {
            if let Some(group) = group_at_mut(&mut root.borrow_mut(), &path) {
                group.rules.push(SmartPlaylistRuleNode::Rule(default_rule(
                    SmartPlaylistRuleField::Title,
                )));
            }
            rerender();
        });
    }
    header.append(&add_rule);

    let add_group = text_button("list-add-symbolic", "Add Group");
    {
        let root = Rc::clone(&root);
        let path = path.clone();
        let rerender = Rc::clone(&rerender);
        add_group.connect_clicked(move |_| {
            if let Some(group) = group_at_mut(&mut root.borrow_mut(), &path) {
                group
                    .rules
                    .push(SmartPlaylistRuleNode::Group(SmartPlaylistRuleGroup {
                        mode: SmartPlaylistMatchMode::All,
                        rules: vec![SmartPlaylistRuleNode::Rule(default_rule(
                            SmartPlaylistRuleField::Title,
                        ))],
                    }));
            }
            rerender();
        });
    }
    header.append(&add_group);

    if removable {
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some(&tr("Remove group")));
        let root = Rc::clone(&root);
        let path = path.clone();
        let rerender = Rc::clone(&rerender);
        remove.connect_clicked(move |_| {
            remove_node_at(&mut root.borrow_mut(), &path);
            rerender();
        });
        header.append(&remove);
    }
    box_.append(&header);

    for (index, node) in group.rules.iter().enumerate() {
        let mut child_path = path.clone();
        child_path.push(index);
        match node {
            SmartPlaylistRuleNode::Rule(rule) => append_rule_row(
                &box_,
                Rc::clone(&root),
                child_path,
                rule.clone(),
                Rc::clone(&rerender),
            ),
            SmartPlaylistRuleNode::Group(_) => append_rule_group(
                &box_,
                Rc::clone(&root),
                child_path,
                Rc::clone(&rerender),
                true,
            ),
        }
    }

    frame.set_child(Some(&box_));
    parent.append(&frame);
}

fn append_rule_row(
    parent: &gtk::Box,
    root: Rc<RefCell<SmartPlaylistRuleGroup>>,
    path: Vec<usize>,
    rule: SmartPlaylistRule,
    rerender: Rc<dyn Fn()>,
) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_hexpand(true);
    row.set_margin_start(RULE_GROUP_INDENT);

    let field_titles = RULE_FIELDS
        .iter()
        .map(|spec| spec.title)
        .collect::<Vec<_>>();
    let field = dropdown_from_titles(&field_titles, field_index(rule.field));
    field.set_hexpand(false);
    field.set_size_request(150, -1);
    {
        let root = Rc::clone(&root);
        let path = path.clone();
        let rerender = Rc::clone(&rerender);
        field.connect_selected_notify(move |dropdown| {
            let selected = RULE_FIELDS
                .get(dropdown.selected() as usize)
                .map(|spec| spec.field)
                .unwrap_or(SmartPlaylistRuleField::Title);
            if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
                *rule = default_rule(selected);
            }
            rerender();
        });
    }
    row.append(&field);

    let operators = operator_specs(rule.field);
    let operator_titles = operators.iter().map(|spec| spec.title).collect::<Vec<_>>();
    let operator =
        dropdown_from_titles(&operator_titles, operator_index(&operators, rule.operator));
    operator.set_size_request(150, -1);
    {
        let root = Rc::clone(&root);
        let path = path.clone();
        let rerender = Rc::clone(&rerender);
        operator.connect_selected_notify(move |dropdown| {
            let mut group = root.borrow_mut();
            let Some(rule) = rule_at_mut(&mut group, &path) else {
                return;
            };
            let operators = operator_specs(rule.field);
            let spec = operators
                .get(dropdown.selected() as usize)
                .copied()
                .unwrap_or_else(|| operators[0]);
            rule.operator = spec.operator;
            rule.value = default_value(rule.field, spec.operator);
            rerender();
        });
    }
    row.append(&operator);

    let value_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    value_box.set_hexpand(true);
    append_value_editor(&value_box, Rc::clone(&root), path.clone(), &rule);
    row.append(&value_box);

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some(&tr("Remove rule")));
    {
        let root = Rc::clone(&root);
        let path = path.clone();
        let rerender = Rc::clone(&rerender);
        remove.connect_clicked(move |_| {
            remove_node_at(&mut root.borrow_mut(), &path);
            rerender();
        });
    }
    row.append(&remove);
    parent.append(&row);
}

fn append_value_editor(
    container: &gtk::Box,
    root: Rc<RefCell<SmartPlaylistRuleGroup>>,
    path: Vec<usize>,
    rule: &SmartPlaylistRule,
) {
    match input_kind(rule.field, rule.operator) {
        RuleInputKind::None => {
            let label = gtk::Label::new(None);
            label.set_hexpand(true);
            container.append(&label);
        }
        RuleInputKind::Text => {
            let entry = gtk::Entry::new();
            entry.set_hexpand(true);
            entry.set_placeholder_text(Some(&text_placeholder(rule.field)));
            if let Some(SmartPlaylistRuleValue::Text(value)) = rule.value.as_ref() {
                entry.set_text(value);
            }
            entry.connect_changed(move |entry| {
                if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
                    rule.value = Some(SmartPlaylistRuleValue::Text(entry.text().to_string()));
                }
            });
            container.append(&entry);
        }
        RuleInputKind::Number => {
            let (min, max, default) = number_bounds(rule.field);
            let value = match rule.value.as_ref() {
                Some(SmartPlaylistRuleValue::Number(value)) => *value,
                _ => default,
            };
            let spin = number_spin(value, min, max);
            spin.connect_value_changed(move |spin| {
                if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
                    rule.value = Some(SmartPlaylistRuleValue::Number(i64::from(
                        spin.value_as_int(),
                    )));
                }
            });
            container.append(&spin);
        }
        RuleInputKind::NumberRange => {
            let (min_bound, max_bound, default) = number_bounds(rule.field);
            let (min_value, max_value) = match rule.value.as_ref() {
                Some(SmartPlaylistRuleValue::NumberRange { min, max }) => (*min, *max),
                _ => (default, default),
            };
            let min_spin = number_spin(min_value, min_bound, max_bound);
            let max_spin = number_spin(max_value, min_bound, max_bound);
            connect_number_range(root, path, min_spin.clone(), max_spin.clone());
            container.append(&min_spin);
            container.append(&gtk::Label::new(Some(&tr("to"))));
            container.append(&max_spin);
        }
        RuleInputKind::Date => {
            let entry = gtk::Entry::new();
            entry.set_hexpand(true);
            entry.set_placeholder_text(Some("YYYY-MM-DD"));
            if let Some(SmartPlaylistRuleValue::Date(value)) = rule.value.as_ref() {
                entry.set_text(value);
            }
            entry.connect_changed(move |entry| {
                if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
                    rule.value = Some(SmartPlaylistRuleValue::Date(entry.text().to_string()));
                }
            });
            container.append(&entry);
        }
        RuleInputKind::DateRange => {
            let start = gtk::Entry::new();
            let end = gtk::Entry::new();
            start.set_placeholder_text(Some("YYYY-MM-DD"));
            end.set_placeholder_text(Some("YYYY-MM-DD"));
            start.set_hexpand(true);
            end.set_hexpand(true);
            if let Some(SmartPlaylistRuleValue::DateRange { start: s, end: e }) =
                rule.value.as_ref()
            {
                start.set_text(s);
                end.set_text(e);
            }
            connect_date_range(root, path, start.clone(), end.clone());
            container.append(&start);
            container.append(&gtk::Label::new(Some(&tr("to"))));
            container.append(&end);
        }
        RuleInputKind::Bool => {
            let active = matches!(rule.value, Some(SmartPlaylistRuleValue::Bool(true)));
            let dropdown = dropdown_from_titles(&["Yes", "No"], usize::from(!active));
            dropdown.connect_selected_notify(move |dropdown| {
                if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
                    rule.value = Some(SmartPlaylistRuleValue::Bool(dropdown.selected() == 0));
                }
            });
            container.append(&dropdown);
        }
    }
}

fn connect_number_range(
    root: Rc<RefCell<SmartPlaylistRuleGroup>>,
    path: Vec<usize>,
    min_spin: gtk::SpinButton,
    max_spin: gtk::SpinButton,
) {
    let min_for_update = min_spin.clone();
    let max_for_update = max_spin.clone();
    let update = move || {
        if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
            rule.value = Some(SmartPlaylistRuleValue::NumberRange {
                min: i64::from(min_for_update.value_as_int()),
                max: i64::from(max_for_update.value_as_int()),
            });
        }
    };
    let update = Rc::new(update);
    let update_min = Rc::clone(&update);
    min_spin.connect_value_changed(move |_| update_min());
    max_spin.connect_value_changed(move |_| update());
}

fn connect_date_range(
    root: Rc<RefCell<SmartPlaylistRuleGroup>>,
    path: Vec<usize>,
    start: gtk::Entry,
    end: gtk::Entry,
) {
    let start_for_update = start.clone();
    let end_for_update = end.clone();
    let update = move || {
        if let Some(rule) = rule_at_mut(&mut root.borrow_mut(), &path) {
            rule.value = Some(SmartPlaylistRuleValue::DateRange {
                start: start_for_update.text().to_string(),
                end: end_for_update.text().to_string(),
            });
        }
    };
    let update = Rc::new(update);
    let update_start = Rc::clone(&update);
    start.connect_changed(move |_| update_start());
    end.connect_changed(move |_| update());
}

fn operator_specs(field: SmartPlaylistRuleField) -> Vec<RuleOperatorSpec> {
    use RuleInputKind::*;
    use SmartPlaylistRuleOperator::*;
    match field {
        SmartPlaylistRuleField::Title
        | SmartPlaylistRuleField::Artist
        | SmartPlaylistRuleField::Album
        | SmartPlaylistRuleField::Comment => vec![
            op(Contains, "contains", Text),
            op(Equals, "equals", Text),
            op(NotContains, "does not contain", Text),
            op(NotEquals, "does not equal", Text),
            op(IsEmpty, "is empty", None),
            op(IsNotEmpty, "is not empty", None),
        ],
        SmartPlaylistRuleField::Genre => vec![
            op(Contains, "contains", Text),
            op(Equals, "equals", Text),
            op(NotContains, "excludes", Text),
            op(NotEquals, "is not", Text),
        ],
        SmartPlaylistRuleField::Rating => vec![
            op(Above, "above", Number),
            op(Below, "below", Number),
            op(Equals, "equals", Number),
            op(Between, "range", NumberRange),
            op(IsEmpty, "is empty", None),
            op(IsNotEmpty, "is not empty", None),
        ],
        SmartPlaylistRuleField::Year
        | SmartPlaylistRuleField::PlayCount
        | SmartPlaylistRuleField::SkipCount => vec![
            op(Between, "range", NumberRange),
            op(Above, "above", Number),
            op(Below, "below", Number),
            op(Equals, "equals", Number),
            op(NotEquals, "does not equal", Number),
        ],
        SmartPlaylistRuleField::Favorite | SmartPlaylistRuleField::Played => {
            vec![op(Is, "is", Bool), op(IsNot, "is not", Bool)]
        }
        SmartPlaylistRuleField::LastPlayed | SmartPlaylistRuleField::DateAdded => vec![
            op(Between, "range", DateRange),
            op(After, "after", Date),
            op(Before, "before", Date),
            op(Equals, "equals", Date),
            op(IsEmpty, "is empty", None),
            op(IsNotEmpty, "is not empty", None),
        ],
    }
}

fn op(
    operator: SmartPlaylistRuleOperator,
    title: &'static str,
    input: RuleInputKind,
) -> RuleOperatorSpec {
    RuleOperatorSpec {
        operator,
        title,
        input,
    }
}

fn default_definition() -> SmartPlaylistDefinition {
    SmartPlaylistDefinition {
        root: SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: Vec::new(),
        },
        sort_field: SmartPlaylistSortField::Title,
        descending: false,
        limit: None,
    }
}

fn default_rule(field: SmartPlaylistRuleField) -> SmartPlaylistRule {
    let operator = operator_specs(field)[0].operator;
    SmartPlaylistRule {
        field,
        operator,
        value: default_value(field, operator),
    }
}

fn default_value(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
) -> Option<SmartPlaylistRuleValue> {
    match input_kind(field, operator) {
        RuleInputKind::None => None,
        RuleInputKind::Text => Some(SmartPlaylistRuleValue::Text(String::new())),
        RuleInputKind::Number => Some(SmartPlaylistRuleValue::Number(number_bounds(field).2)),
        RuleInputKind::NumberRange => {
            let default = number_bounds(field).2;
            Some(SmartPlaylistRuleValue::NumberRange {
                min: default,
                max: default,
            })
        }
        RuleInputKind::Date => Some(SmartPlaylistRuleValue::Date(String::new())),
        RuleInputKind::DateRange => Some(SmartPlaylistRuleValue::DateRange {
            start: String::new(),
            end: String::new(),
        }),
        RuleInputKind::Bool => Some(SmartPlaylistRuleValue::Bool(true)),
    }
}

fn input_kind(field: SmartPlaylistRuleField, operator: SmartPlaylistRuleOperator) -> RuleInputKind {
    operator_specs(field)
        .into_iter()
        .find(|spec| spec.operator == operator)
        .map(|spec| spec.input)
        .unwrap_or(RuleInputKind::None)
}

fn normalize_root_group(group: &mut SmartPlaylistRuleGroup) {
    normalize_group_children(group);
}

fn normalize_group(group: &mut SmartPlaylistRuleGroup) -> Option<()> {
    normalize_group_children(group);
    (!group.rules.is_empty()).then_some(())
}

fn normalize_group_children(group: &mut SmartPlaylistRuleGroup) {
    let mut normalized = Vec::with_capacity(group.rules.len());
    for mut node in group.rules.drain(..) {
        let keep = match &mut node {
            SmartPlaylistRuleNode::Group(group) => normalize_group(group).is_some(),
            SmartPlaylistRuleNode::Rule(rule) => normalize_rule(rule).is_some(),
        };
        if keep {
            normalized.push(node);
        }
    }
    group.rules = normalized;
}

fn normalize_rule(rule: &mut SmartPlaylistRule) -> Option<()> {
    match input_kind(rule.field, rule.operator) {
        RuleInputKind::None => {
            rule.value = None;
            Some(())
        }
        RuleInputKind::Text => match rule.value.as_mut()? {
            SmartPlaylistRuleValue::Text(value) if !value.trim().is_empty() => {
                *value = value.trim().to_string();
                Some(())
            }
            _ => None,
        },
        RuleInputKind::Number => {
            matches!(rule.value, Some(SmartPlaylistRuleValue::Number(_))).then_some(())
        }
        RuleInputKind::NumberRange => {
            let Some(SmartPlaylistRuleValue::NumberRange { min, max }) = rule.value.as_mut() else {
                return None;
            };
            if *min > *max {
                std::mem::swap(min, max);
            }
            Some(())
        }
        RuleInputKind::Date => match rule.value.as_mut()? {
            SmartPlaylistRuleValue::Date(value) if !value.trim().is_empty() => {
                *value = value.trim().to_string();
                Some(())
            }
            _ => None,
        },
        RuleInputKind::DateRange => {
            let Some(SmartPlaylistRuleValue::DateRange { start, end }) = rule.value.as_mut() else {
                return None;
            };
            *start = start.trim().to_string();
            *end = end.trim().to_string();
            if start.is_empty() || end.is_empty() {
                return None;
            }
            if *start > *end {
                std::mem::swap(start, end);
            }
            Some(())
        }
        RuleInputKind::Bool => {
            matches!(rule.value, Some(SmartPlaylistRuleValue::Bool(_))).then_some(())
        }
    }
}

fn group_at<'a>(
    group: &'a SmartPlaylistRuleGroup,
    path: &[usize],
) -> Option<&'a SmartPlaylistRuleGroup> {
    let mut current = group;
    for index in path {
        current = match current.rules.get(*index)? {
            SmartPlaylistRuleNode::Group(group) => group,
            SmartPlaylistRuleNode::Rule(_) => return None,
        };
    }
    Some(current)
}

fn group_at_mut<'a>(
    group: &'a mut SmartPlaylistRuleGroup,
    path: &[usize],
) -> Option<&'a mut SmartPlaylistRuleGroup> {
    let mut current = group;
    for index in path {
        current = match current.rules.get_mut(*index)? {
            SmartPlaylistRuleNode::Group(group) => group,
            SmartPlaylistRuleNode::Rule(_) => return None,
        };
    }
    Some(current)
}

fn rule_at_mut<'a>(
    group: &'a mut SmartPlaylistRuleGroup,
    path: &[usize],
) -> Option<&'a mut SmartPlaylistRule> {
    let (last, parent_path) = path.split_last()?;
    let parent = group_at_mut(group, parent_path)?;
    match parent.rules.get_mut(*last)? {
        SmartPlaylistRuleNode::Rule(rule) => Some(rule),
        SmartPlaylistRuleNode::Group(_) => None,
    }
}

fn remove_node_at(group: &mut SmartPlaylistRuleGroup, path: &[usize]) -> Option<()> {
    let (last, parent_path) = path.split_last()?;
    let parent = group_at_mut(group, parent_path)?;
    if *last < parent.rules.len() {
        parent.rules.remove(*last);
        Some(())
    } else {
        None
    }
}

fn match_mode_dropdown(mode: SmartPlaylistMatchMode) -> gtk::DropDown {
    dropdown_from_titles(
        &["All", "Any"],
        match mode {
            SmartPlaylistMatchMode::All => 0,
            SmartPlaylistMatchMode::Any => 1,
        },
    )
}

fn match_mode_from_index(index: u32) -> SmartPlaylistMatchMode {
    if index == 1 {
        SmartPlaylistMatchMode::Any
    } else {
        SmartPlaylistMatchMode::All
    }
}

fn dropdown_from_titles(titles: &[&str], selected: usize) -> gtk::DropDown {
    let model = gtk::StringList::new(titles);
    let dropdown = gtk::DropDown::new(Some(model), None::<gtk::Expression>);
    dropdown.set_selected(selected as u32);
    dropdown
}

fn field_index(field: SmartPlaylistRuleField) -> usize {
    RULE_FIELDS
        .iter()
        .position(|spec| spec.field == field)
        .unwrap_or(0)
}

fn operator_index(operators: &[RuleOperatorSpec], operator: SmartPlaylistRuleOperator) -> usize {
    operators
        .iter()
        .position(|spec| spec.operator == operator)
        .unwrap_or(0)
}

fn sort_index(sort: SmartPlaylistSortField) -> usize {
    SORT_FIELDS
        .iter()
        .position(|(field, _)| *field == sort)
        .unwrap_or(0)
}

fn number_bounds(field: SmartPlaylistRuleField) -> (i64, i64, i64) {
    match field {
        SmartPlaylistRuleField::Rating => (0, 5, 4),
        SmartPlaylistRuleField::Year => (0, 3000, 2000),
        SmartPlaylistRuleField::PlayCount | SmartPlaylistRuleField::SkipCount => (0, 999_999, 1),
        _ => (0, 999_999, 0),
    }
}

fn number_spin(value: i64, min: i64, max: i64) -> gtk::SpinButton {
    let adjustment = gtk::Adjustment::new(value as f64, min as f64, max as f64, 1.0, 10.0, 0.0);
    let spin = gtk::SpinButton::new(Some(&adjustment), 1.0, 0);
    spin.set_numeric(true);
    spin.set_width_chars(7);
    spin
}

fn text_placeholder(field: SmartPlaylistRuleField) -> String {
    match field {
        SmartPlaylistRuleField::Genre => tr("Genre"),
        SmartPlaylistRuleField::Comment => tr("Comment text"),
        SmartPlaylistRuleField::Artist => tr("Artist name"),
        SmartPlaylistRuleField::Album => tr("Album title"),
        _ => tr("Text"),
    }
}

fn labeled_control(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let label = gtk::Label::new(Some(&tr(label)));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    box_.append(&label);
    box_.append(widget);
    box_.upcast()
}

fn labeled_row(label: &str, widgets: &[gtk::Widget]) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Label::new(Some(&tr(label))));
    for widget in widgets {
        row.append(widget);
    }
    row.upcast()
}

fn clear_box(box_: &gtk::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}

fn playlist_name(value: &str) -> Option<String> {
    let name = value.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn smart_playlist_dialog(
    title: &str,
    content: &impl IsA<gtk::Widget>,
    actions: &impl IsA<gtk::Widget>,
) -> adw::Dialog {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr(title), "")));
    toolbar.add_top_bar(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_vexpand(true);
    body.append(content);
    body.append(actions);
    toolbar.set_content(Some(&body));

    adw::Dialog::builder()
        .title(tr(title))
        .content_width(large_popup_content_width(SMART_PLAYLIST_DIALOG_WIDTH))
        .content_height(SMART_PLAYLIST_DIALOG_HEIGHT)
        .child(&toolbar)
        .build()
}

fn dialog_action_row() -> gtk::Box {
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(12);
    actions.set_margin_bottom(14);
    actions.set_margin_start(18);
    actions.set_margin_end(18);
    actions
}

fn dialog_button(label: &str, css_class: Option<&str>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr(label));
    if let Some(css_class) = css_class {
        button.add_css_class(css_class);
    }
    button
}

fn sync_editor_button_enabled(button: &gtk::Button, editor: &SmartPlaylistEditor) {
    button.set_sensitive(playlist_name(&editor.name.text()).is_some());
}

fn connect_editor_name_validation(button: &gtk::Button, editor: &SmartPlaylistEditor) {
    let button = button.clone();
    let editor = editor.clone();
    editor
        .name
        .clone()
        .connect_changed(move |_| sync_editor_button_enabled(&button, &editor));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_group_keeps_nested_year_and_genre_rules() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![
                SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Year,
                    operator: SmartPlaylistRuleOperator::Between,
                    value: Some(SmartPlaylistRuleValue::NumberRange {
                        min: 2001,
                        max: 1999,
                    }),
                }),
                SmartPlaylistRuleNode::Group(SmartPlaylistRuleGroup {
                    mode: SmartPlaylistMatchMode::Any,
                    rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Genre,
                        operator: SmartPlaylistRuleOperator::Contains,
                        value: Some(SmartPlaylistRuleValue::Text(" rock ".to_string())),
                    })],
                }),
            ],
        };

        normalize_group(&mut group).expect("valid rules");

        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("first node should be a rule");
        };
        assert_eq!(
            rule.value,
            Some(SmartPlaylistRuleValue::NumberRange {
                min: 1999,
                max: 2001,
            })
        );
        let SmartPlaylistRuleNode::Group(group) = &group.rules[1] else {
            panic!("second node should be a group");
        };
        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("nested node should be a rule");
        };
        assert_eq!(
            rule.value,
            Some(SmartPlaylistRuleValue::Text("rock".to_string()))
        );
    }

    #[test]
    fn normalize_group_supports_date_ranges() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                field: SmartPlaylistRuleField::DateAdded,
                operator: SmartPlaylistRuleOperator::Between,
                value: Some(SmartPlaylistRuleValue::DateRange {
                    start: "2024-12-31".to_string(),
                    end: "2024-01-01".to_string(),
                }),
            })],
        };

        normalize_group(&mut group).expect("valid date range");

        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("node should be a rule");
        };
        assert_eq!(
            rule.value,
            Some(SmartPlaylistRuleValue::DateRange {
                start: "2024-01-01".to_string(),
                end: "2024-12-31".to_string(),
            })
        );
    }

    #[test]
    fn normalize_root_group_allows_empty_rules() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: Vec::new(),
        };

        normalize_root_group(&mut group);

        assert!(group.rules.is_empty());
    }

    #[test]
    fn normalize_group_drops_invalid_children_without_rejecting_valid_rules() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![
                SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Title,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text(String::new())),
                }),
                SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Genre,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text("rock".to_string())),
                }),
            ],
        };

        normalize_group(&mut group).expect("valid remaining rule");

        assert_eq!(group.rules.len(), 1);
        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("remaining node should be a rule");
        };
        assert_eq!(rule.field, SmartPlaylistRuleField::Genre);
    }
}
