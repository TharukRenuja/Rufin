use super::layout::large_popup_content_width;
use super::*;
use crate::i18n::msgid;
use domain::smart_playlists::{self as smart_policy, SmartPlaylistRuleValueKind};
use std::cell::Cell;

const SMART_PLAYLIST_DIALOG_WIDTH: i32 = 700;
const SMART_PLAYLIST_DIALOG_HEIGHT: i32 = 510;

type RerenderSlot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

#[derive(Clone)]
struct SmartPlaylistEditor {
    name: gtk::Entry,
    mode: Rc<Cell<SmartPlaylistMatchMode>>,
    rules: Rc<RefCell<Vec<SmartPlaylistRule>>>,
    nested_root: Option<Rc<RefCell<SmartPlaylistRuleGroup>>>,
    sort: gtk::DropDown,
    descending: gtk::CheckButton,
    limit: gtk::Entry,
}

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
        present_light_dismiss_dialog(&dialog, &self.window);
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
        present_light_dismiss_dialog(&dialog, &self.window);
    }
}

impl SmartPlaylistEditor {
    fn definition(&self) -> Option<(String, SmartPlaylistDefinition)> {
        let name = playlist_name(&self.name.text())?;
        let mut root = if let Some(nested_root) = &self.nested_root {
            let mut root = nested_root.borrow().clone();
            root.mode = self.mode.get();
            root
        } else {
            SmartPlaylistRuleGroup {
                mode: self.mode.get(),
                rules: self
                    .rules
                    .borrow()
                    .iter()
                    .cloned()
                    .map(SmartPlaylistRuleNode::Rule)
                    .collect(),
            }
        };
        smart_policy::normalize_root(&mut root);
        let limit = self
            .limit
            .text()
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0);
        let sort_field = smart_policy::sort_fields()
            .get(self.sort.selected() as usize)
            .copied()
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

    let definition = definition
        .cloned()
        .unwrap_or_else(smart_policy::default_definition);
    let sort_labels = sort_labels();
    let sort = dropdown_from_labels(&sort_labels, sort_index(definition.sort_field));
    let descending = gtk::CheckButton::with_label(&tr("Descending"));
    descending.set_active(definition.descending);
    let limit = gtk::Entry::new();
    limit.set_placeholder_text(Some(&tr("No limit")));
    limit.set_width_chars(8);
    if let Some(value) = definition.limit {
        limit.set_text(&value.to_string());
    }
    let (rules, nested_root) = match smart_policy::flat_rules(&definition.root) {
        Some(rules) => (rules, None),
        None => (
            Vec::new(),
            Some(Rc::new(RefCell::new(definition.root.clone()))),
        ),
    };

    SmartPlaylistEditor {
        name: name_entry,
        mode: Rc::new(Cell::new(definition.root.mode)),
        rules: Rc::new(RefCell::new(rules)),
        nested_root,
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

    let match_dropdown = match_mode_dropdown(editor.mode.get());
    {
        let mode = Rc::clone(&editor.mode);
        match_dropdown.connect_selected_notify(move |dropdown| {
            mode.set(match_mode_from_index(dropdown.selected()));
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
    let rerender: Rc<dyn Fn()> = {
        let rules = rules.clone();
        let editor_rules = Rc::clone(&editor.rules);
        let rerender_slot = Rc::clone(&rerender_slot);
        let has_nested_rules = editor.nested_root.is_some();
        Rc::new(move || {
            clear_box(&rules);
            if has_nested_rules {
                append_nested_rules_fallback(&rules);
            } else {
                let Some(rerender) = rerender_slot.borrow().as_ref().cloned() else {
                    return;
                };
                append_rule_list(&rules, Rc::clone(&editor_rules), rerender);
            }
        })
    };
    *rerender_slot.borrow_mut() = Some(Rc::clone(&rerender));
    rerender();
    content.append(&rules);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_max_content_height(SMART_PLAYLIST_DIALOG_HEIGHT);
    scroller.set_child(Some(&content));
    (scroller.upcast(), default_dropdown)
}

fn append_rule_list(
    parent: &gtk::Box,
    rules: Rc<RefCell<Vec<SmartPlaylistRule>>>,
    rerender: Rc<dyn Fn()>,
) {
    let frame = gtk::Frame::new(None);
    frame.set_hexpand(true);
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
    box_.set_margin_top(10);
    box_.set_margin_bottom(10);
    box_.set_margin_start(10);
    box_.set_margin_end(10);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_halign(gtk::Align::End);

    let add_rule = text_button(ADD_ICON, "Add Rule");
    {
        let rules = Rc::clone(&rules);
        let rerender = Rc::clone(&rerender);
        add_rule.connect_clicked(move |_| {
            rules
                .borrow_mut()
                .push(smart_policy::default_rule(SmartPlaylistRuleField::Title));
            rerender();
        });
    }
    header.append(&add_rule);
    box_.append(&header);

    let current_rules = rules.borrow().clone();
    for (index, rule) in current_rules.into_iter().enumerate() {
        append_rule_row(&box_, Rc::clone(&rules), index, rule, Rc::clone(&rerender));
    }

    frame.set_child(Some(&box_));
    parent.append(&frame);
}

fn append_nested_rules_fallback(parent: &gtk::Box) {
    let frame = gtk::Frame::new(None);
    frame.set_hexpand(true);
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
    box_.set_margin_top(10);
    box_.set_margin_bottom(10);
    box_.set_margin_start(10);
    box_.set_margin_end(10);
    let label = gtk::Label::new(Some(&tr(
        "This playlist uses nested rule groups. Rule editing is unavailable in this editor.",
    )));
    label.set_wrap(true);
    label.set_xalign(0.0);
    box_.append(&label);
    frame.set_child(Some(&box_));
    parent.append(&frame);
}

fn append_rule_row(
    parent: &gtk::Box,
    rules: Rc<RefCell<Vec<SmartPlaylistRule>>>,
    index: usize,
    rule: SmartPlaylistRule,
    rerender: Rc<dyn Fn()>,
) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_hexpand(true);

    let field_labels = field_labels();
    let field = dropdown_from_labels(&field_labels, field_index(rule.field));
    field.set_hexpand(false);
    field.set_size_request(150, -1);
    {
        let rules = Rc::clone(&rules);
        let rerender = Rc::clone(&rerender);
        field.connect_selected_notify(move |dropdown| {
            let selected = smart_policy::rule_fields()
                .get(dropdown.selected() as usize)
                .copied()
                .unwrap_or(SmartPlaylistRuleField::Title);
            if let Some(rule) = rules.borrow_mut().get_mut(index) {
                *rule = smart_policy::default_rule(selected);
            }
            rerender();
        });
    }
    row.append(&field);

    let operators = smart_policy::rule_ops(rule.field);
    let operator_titles = op_labels(rule.field, operators);
    let operator = dropdown_from_labels(&operator_titles, operator_index(operators, rule.operator));
    operator.set_size_request(150, -1);
    {
        let rules = Rc::clone(&rules);
        let rerender = Rc::clone(&rerender);
        operator.connect_selected_notify(move |dropdown| {
            change_rule_operator(&rules, index, dropdown.selected(), || {
                rerender();
            });
        });
    }
    row.append(&operator);

    let value_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    value_box.set_hexpand(true);
    append_value_editor(&value_box, Rc::clone(&rules), index, &rule);
    row.append(&value_box);

    let remove = gtk::Button::from_icon_name(REMOVE_ICON);
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some(&tr("Remove rule")));
    {
        let rules = Rc::clone(&rules);
        let rerender = Rc::clone(&rerender);
        remove.connect_clicked(move |_| {
            remove_rule(&rules, index, || rerender());
        });
    }
    row.append(&remove);
    parent.append(&row);
}

fn append_value_editor(
    container: &gtk::Box,
    rules: Rc<RefCell<Vec<SmartPlaylistRule>>>,
    index: usize,
    rule: &SmartPlaylistRule,
) {
    match smart_policy::value_kind(rule.field, rule.operator)
        .unwrap_or(SmartPlaylistRuleValueKind::None)
    {
        SmartPlaylistRuleValueKind::None => {
            let label = gtk::Label::new(None);
            label.set_hexpand(true);
            container.append(&label);
        }
        SmartPlaylistRuleValueKind::Text => {
            let entry = gtk::Entry::new();
            entry.set_hexpand(true);
            entry.set_placeholder_text(Some(&text_placeholder(rule.field)));
            if let Some(SmartPlaylistRuleValue::Text(value)) = rule.value.as_ref() {
                entry.set_text(value);
            }
            entry.connect_changed(move |entry| {
                if let Some(rule) = rules.borrow_mut().get_mut(index) {
                    rule.value = Some(SmartPlaylistRuleValue::Text(entry.text().to_string()));
                }
            });
            container.append(&entry);
        }
        SmartPlaylistRuleValueKind::Number => {
            let (min, max, default) = smart_policy::number_bounds(rule.field);
            let value = match rule.value.as_ref() {
                Some(SmartPlaylistRuleValue::Number(value)) => *value,
                _ => default,
            };
            let spin = number_spin(value, min, max);
            spin.connect_value_changed(move |spin| {
                if let Some(rule) = rules.borrow_mut().get_mut(index) {
                    rule.value = Some(SmartPlaylistRuleValue::Number(i64::from(
                        spin.value_as_int(),
                    )));
                }
            });
            container.append(&spin);
        }
        SmartPlaylistRuleValueKind::NumberRange => {
            let (min_bound, max_bound, default) = smart_policy::number_bounds(rule.field);
            let (min_value, max_value) = match rule.value.as_ref() {
                Some(SmartPlaylistRuleValue::NumberRange { min, max }) => (*min, *max),
                _ => (default, default),
            };
            let min_spin = number_spin(min_value, min_bound, max_bound);
            let max_spin = number_spin(max_value, min_bound, max_bound);
            connect_number_range(rules, index, min_spin.clone(), max_spin.clone());
            container.append(&min_spin);
            container.append(&gtk::Label::new(Some(&tr("to"))));
            container.append(&max_spin);
        }
        SmartPlaylistRuleValueKind::Date => {
            let entry = gtk::Entry::new();
            entry.set_hexpand(true);
            entry.set_placeholder_text(Some("YYYY-MM-DD"));
            if let Some(SmartPlaylistRuleValue::Date(value)) = rule.value.as_ref() {
                entry.set_text(value);
            }
            entry.connect_changed(move |entry| {
                if let Some(rule) = rules.borrow_mut().get_mut(index) {
                    rule.value = Some(SmartPlaylistRuleValue::Date(entry.text().to_string()));
                }
            });
            container.append(&entry);
        }
        SmartPlaylistRuleValueKind::DateRange => {
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
            connect_date_range(rules, index, start.clone(), end.clone());
            container.append(&start);
            container.append(&gtk::Label::new(Some(&tr("to"))));
            container.append(&end);
        }
        SmartPlaylistRuleValueKind::Bool => {
            let active = matches!(rule.value, Some(SmartPlaylistRuleValue::Bool(true)));
            let dropdown = dropdown_from_titles(&[msgid("Yes"), msgid("No")], usize::from(!active));
            dropdown.connect_selected_notify(move |dropdown| {
                if let Some(rule) = rules.borrow_mut().get_mut(index) {
                    rule.value = Some(SmartPlaylistRuleValue::Bool(dropdown.selected() == 0));
                }
            });
            container.append(&dropdown);
        }
    }
}

fn connect_number_range(
    rules: Rc<RefCell<Vec<SmartPlaylistRule>>>,
    index: usize,
    min_spin: gtk::SpinButton,
    max_spin: gtk::SpinButton,
) {
    let min_for_update = min_spin.clone();
    let max_for_update = max_spin.clone();
    let update = move || {
        if let Some(rule) = rules.borrow_mut().get_mut(index) {
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
    rules: Rc<RefCell<Vec<SmartPlaylistRule>>>,
    index: usize,
    start: gtk::Entry,
    end: gtk::Entry,
) {
    let start_for_update = start.clone();
    let end_for_update = end.clone();
    let update = move || {
        if let Some(rule) = rules.borrow_mut().get_mut(index) {
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

fn change_rule_operator(
    rules: &Rc<RefCell<Vec<SmartPlaylistRule>>>,
    index: usize,
    selected: u32,
    after_change: impl FnOnce(),
) {
    {
        let mut rules = rules.borrow_mut();
        let Some(rule) = rules.get_mut(index) else {
            return;
        };
        let operators = smart_policy::rule_ops(rule.field);
        let Some(spec) = operators
            .get(selected as usize)
            .copied()
            .or_else(|| operators.first().copied())
        else {
            return;
        };
        rule.operator = spec.operator;
        rule.value = smart_policy::default_value(rule.field, spec.operator);
    }
    after_change();
}

fn remove_rule(
    rules: &Rc<RefCell<Vec<SmartPlaylistRule>>>,
    index: usize,
    after_change: impl FnOnce(),
) {
    {
        let mut rules = rules.borrow_mut();
        if index < rules.len() {
            rules.remove(index);
        }
    }
    after_change();
}

fn match_mode_dropdown(mode: SmartPlaylistMatchMode) -> gtk::DropDown {
    dropdown_from_titles(
        &[msgid("All"), msgid("Any")],
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

fn field_labels() -> Vec<String> {
    smart_policy::rule_fields()
        .iter()
        .map(|field| tr(field_title(*field)))
        .collect()
}

fn sort_labels() -> Vec<String> {
    smart_policy::sort_fields()
        .iter()
        .map(|field| tr(sort_title(*field)))
        .collect()
}

fn op_labels(
    field: SmartPlaylistRuleField,
    operators: &[smart_policy::SmartPlaylistRuleOp],
) -> Vec<String> {
    operators
        .iter()
        .map(|spec| tr(op_title(field, spec.operator)))
        .collect()
}

fn field_title(field: SmartPlaylistRuleField) -> &'static str {
    match field {
        SmartPlaylistRuleField::Title => msgid("Title"),
        SmartPlaylistRuleField::Artist => msgid("Artist"),
        SmartPlaylistRuleField::Album => msgid("Album"),
        SmartPlaylistRuleField::Comment => msgid("Comment"),
        SmartPlaylistRuleField::Genre => msgid("Genre"),
        SmartPlaylistRuleField::Rating => msgid("Rating"),
        SmartPlaylistRuleField::Year => msgid("Year"),
        SmartPlaylistRuleField::Favorite => msgid("Favorite"),
        SmartPlaylistRuleField::Played => msgid("Played"),
        SmartPlaylistRuleField::PlayCount => msgid("Play count"),
        SmartPlaylistRuleField::SkipCount => msgid("Skip count"),
        SmartPlaylistRuleField::LastPlayed => msgid("Last played"),
        SmartPlaylistRuleField::DateAdded => msgid("Date added"),
    }
}

fn sort_title(field: SmartPlaylistSortField) -> &'static str {
    match field {
        SmartPlaylistSortField::Title => msgid("Title"),
        SmartPlaylistSortField::Artist => msgid("Artist"),
        SmartPlaylistSortField::Album => msgid("Album"),
        SmartPlaylistSortField::Year => msgid("Year"),
        SmartPlaylistSortField::DateAdded => msgid("Date added"),
        SmartPlaylistSortField::LastPlayed => msgid("Last played"),
        SmartPlaylistSortField::PlayCount => msgid("Play count"),
        SmartPlaylistSortField::SkipCount => msgid("Skip count"),
        SmartPlaylistSortField::Rating => msgid("Rating"),
        SmartPlaylistSortField::Duration => msgid("Duration"),
    }
}

fn op_title(field: SmartPlaylistRuleField, operator: SmartPlaylistRuleOperator) -> &'static str {
    match (field, operator) {
        (SmartPlaylistRuleField::Genre, SmartPlaylistRuleOperator::NotContains) => {
            msgid("excludes")
        }
        (SmartPlaylistRuleField::Genre, SmartPlaylistRuleOperator::NotEquals) => msgid("is not"),
        (_, SmartPlaylistRuleOperator::Contains) => msgid("contains"),
        (_, SmartPlaylistRuleOperator::NotContains) => msgid("does not contain"),
        (_, SmartPlaylistRuleOperator::Equals) => msgid("equals"),
        (_, SmartPlaylistRuleOperator::NotEquals) => msgid("does not equal"),
        (_, SmartPlaylistRuleOperator::Above) => msgid("above"),
        (_, SmartPlaylistRuleOperator::Below) => msgid("below"),
        (_, SmartPlaylistRuleOperator::Between) => msgid("range"),
        (_, SmartPlaylistRuleOperator::Is) => msgid("is"),
        (_, SmartPlaylistRuleOperator::IsNot) => msgid("is not"),
        (_, SmartPlaylistRuleOperator::Before) => msgid("before"),
        (_, SmartPlaylistRuleOperator::After) => msgid("after"),
        (_, SmartPlaylistRuleOperator::IsEmpty) => msgid("is empty"),
        (_, SmartPlaylistRuleOperator::IsNotEmpty) => msgid("is not empty"),
    }
}

fn dropdown_from_titles(titles: &[&str], selected: usize) -> gtk::DropDown {
    let labels = titles.iter().map(|title| tr(title)).collect::<Vec<_>>();
    dropdown_from_labels(&labels, selected)
}

fn dropdown_from_labels(labels: &[String], selected: usize) -> gtk::DropDown {
    let refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let model = gtk::StringList::new(&refs);
    let dropdown = gtk::DropDown::new(Some(model), None::<gtk::Expression>);
    dropdown.set_selected(selected as u32);
    dropdown
}

fn field_index(field: SmartPlaylistRuleField) -> usize {
    smart_policy::rule_fields()
        .iter()
        .position(|candidate| *candidate == field)
        .unwrap_or(0)
}

fn operator_index(
    operators: &[smart_policy::SmartPlaylistRuleOp],
    operator: SmartPlaylistRuleOperator,
) -> usize {
    operators
        .iter()
        .position(|spec| spec.operator == operator)
        .unwrap_or(0)
}

fn sort_index(sort: SmartPlaylistSortField) -> usize {
    smart_policy::sort_fields()
        .iter()
        .position(|field| *field == sort)
        .unwrap_or(0)
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
    fn new_read_state() {
        let rules = Rc::new(RefCell::new(vec![SmartPlaylistRule {
            field: SmartPlaylistRuleField::Title,
            operator: SmartPlaylistRuleOperator::Contains,
            value: Some(SmartPlaylistRuleValue::Text("needle".to_string())),
        }]));
        let rerendered = std::cell::Cell::new(false);

        change_rule_operator(&rules, 0, 4, || {
            rerendered.set(true);
            assert!(
                rules.try_borrow().is_ok(),
                "rerender needs to read the editor state after the operator changes"
            );
        });

        assert!(rerendered.get());
        let rules = rules.borrow();
        assert_eq!(rules[0].operator, SmartPlaylistRuleOperator::IsEmpty);
        assert_eq!(rules[0].value, None);
    }

    #[test]
    fn new_remove_rule_releases_state_before_rerender() {
        let rules = Rc::new(RefCell::new(vec![
            SmartPlaylistRule {
                field: SmartPlaylistRuleField::Title,
                operator: SmartPlaylistRuleOperator::Contains,
                value: Some(SmartPlaylistRuleValue::Text("needle".to_string())),
            },
            SmartPlaylistRule {
                field: SmartPlaylistRuleField::Genre,
                operator: SmartPlaylistRuleOperator::Equals,
                value: Some(SmartPlaylistRuleValue::Text("Rock".to_string())),
            },
        ]));
        let rerendered = std::cell::Cell::new(false);

        remove_rule(&rules, 0, || {
            rerendered.set(true);
            assert!(
                rules.try_borrow().is_ok(),
                "rerender needs to read the editor state after a rule is removed"
            );
        });

        assert!(rerendered.get());
        let rules = rules.borrow();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].field, SmartPlaylistRuleField::Genre);
    }

    #[test]
    fn new_return_rules() {
        let group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                field: SmartPlaylistRuleField::Title,
                operator: SmartPlaylistRuleOperator::Contains,
                value: Some(SmartPlaylistRuleValue::Text("needle".to_string())),
            })],
        };

        let rules = smart_policy::flat_rules(&group).expect("top-level rules are flat");

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].field, SmartPlaylistRuleField::Title);
    }

    #[test]
    fn new_reject_fallback() {
        let group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![SmartPlaylistRuleNode::Group(SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::Any,
                rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Genre,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text("rock".to_string())),
                })],
            })],
        };

        assert!(smart_policy::flat_rules(&group).is_none());
    }
}
