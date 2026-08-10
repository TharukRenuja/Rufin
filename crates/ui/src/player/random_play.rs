use std::rc::Rc;

use ::library::{GenreId, GenreSummary, PlayedFilter, RandomCriteria};
use adw::prelude::*;
use playback::{QueuePlacement, RandomPlayRequest};

use localization::tr;

use crate::shell::Shell;
use crate::shell::actions::text_button;
use crate::shell::actions::{PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON};

const DEFAULT_LIMIT: f64 = 100.0;
const MIN_LIMIT: f64 = 1.0;
const MAX_LIMIT: f64 = 500.0;
const MIN_YEAR: f64 = 1850.0;
const MAX_YEAR: f64 = 2050.0;
const DEFAULT_MIN_YEAR: f64 = 2000.0;
const DEFAULT_MAX_YEAR: f64 = 2020.0;

#[derive(Clone)]
struct RandomPlayControls {
    limit: gtk::SpinButton,
    min_year_enabled: gtk::CheckButton,
    min_year: gtk::SpinButton,
    max_year_enabled: gtk::CheckButton,
    max_year: gtk::SpinButton,
    genre: gtk::DropDown,
    played_filter: gtk::DropDown,
}

pub(super) fn present_random_play_dialog(shell: &Rc<Shell>) {
    let Some(selected) = shell.selected_library().as_deref().cloned() else {
        return;
    };
    let genres = selected
        .library
        .genres(selected.music_folder_id.as_ref())
        .unwrap_or_default();
    let played_filters = [
        PlayedFilter::All,
        PlayedFilter::Unplayed,
        PlayedFilter::Played,
    ];

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title = adw::WindowTitle::new(&tr("Play random"), "");
    header.set_title_widget(Some(&title));
    toolbar.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let controls = RandomPlayControls {
        limit: count_spinner(DEFAULT_LIMIT, MIN_LIMIT, MAX_LIMIT),
        min_year_enabled: gtk::CheckButton::new(),
        min_year: count_spinner(DEFAULT_MIN_YEAR, MIN_YEAR, MAX_YEAR),
        max_year_enabled: gtk::CheckButton::new(),
        max_year: count_spinner(DEFAULT_MAX_YEAR, MIN_YEAR, MAX_YEAR),
        genre: genre_dropdown(&genres),
        played_filter: played_filter_dropdown(&played_filters),
    };
    controls.min_year.set_sensitive(false);
    controls.max_year.set_sensitive(false);
    connect_year_toggle(&controls.min_year_enabled, &controls.min_year);
    connect_year_toggle(&controls.max_year_enabled, &controls.max_year);

    content.append(&control_row(&tr("Number of songs"), &controls.limit));
    content.append(&optional_control_row(
        &tr("Minimum year"),
        &controls.min_year_enabled,
        &controls.min_year,
    ));
    content.append(&optional_control_row(
        &tr("Maximum year"),
        &controls.max_year_enabled,
        &controls.max_year,
    ));
    content.append(&control_row(&tr("Genre"), &controls.genre));
    content.append(&control_row(&tr("Play filter"), &controls.played_filter));

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let play_next = text_button(PLAY_NEXT_ICON, "Play Next");
    let play_now = text_button(PLAY_ICON, "Play");
    play_now.add_css_class("suggested-action");
    let play_later = text_button(PLAY_LATER_ICON, "Play Later");
    actions.append(&play_next);
    actions.append(&play_now);
    actions.append(&play_later);
    content.append(&actions);

    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .content_width(460)
        .child(&toolbar)
        .build();

    connect_action(
        &play_next,
        shell,
        &dialog,
        &controls,
        &genres,
        QueuePlacement::Next,
    );
    connect_action(
        &play_now,
        shell,
        &dialog,
        &controls,
        &genres,
        QueuePlacement::Now,
    );
    connect_action(
        &play_later,
        shell,
        &dialog,
        &controls,
        &genres,
        QueuePlacement::Last,
    );

    shell.present_selected_dialog(&dialog);
}

fn count_spinner(default: f64, min: f64, max: f64) -> gtk::SpinButton {
    let spinner = gtk::SpinButton::with_range(min, max, 1.0);
    spinner.set_value(default);
    spinner.set_numeric(true);
    spinner.set_width_chars(5);
    spinner
}

fn connect_year_toggle(check: &gtk::CheckButton, spinner: &gtk::SpinButton) {
    let spinner = spinner.clone();
    check.connect_toggled(move |check| {
        spinner.set_sensitive(check.is_active());
    });
}

fn genre_dropdown(genres: &[GenreSummary]) -> gtk::DropDown {
    let mut labels = Vec::with_capacity(genres.len() + 1);
    labels.push(tr("Any genre"));
    labels.extend(genres.iter().map(|genre| genre.genre.name.clone()));
    let refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let model = gtk::StringList::new(&refs);
    let dropdown = gtk::DropDown::new(Some(model), None::<gtk::Expression>);
    dropdown.set_enable_search(true);
    dropdown
}

fn played_filter_dropdown(filters: &[PlayedFilter]) -> gtk::DropDown {
    let labels = filters
        .iter()
        .map(|filter| match filter {
            PlayedFilter::All => tr("All tracks"),
            PlayedFilter::Unplayed => tr("Only unplayed tracks"),
            PlayedFilter::Played => tr("Only played tracks"),
        })
        .collect::<Vec<_>>();
    let refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let model = gtk::StringList::new(&refs);
    let dropdown = gtk::DropDown::new(Some(model), None::<gtk::Expression>);
    dropdown.set_sensitive(filters.len() > 1);
    dropdown
}

fn control_row<W: IsA<gtk::Widget>>(label: &str, control: &W) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_valign(gtk::Align::Center);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    row.append(&label);
    row.append(control);
    row
}

fn optional_control_row<W: IsA<gtk::Widget>>(
    label: &str,
    check: &gtk::CheckButton,
    control: &W,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_valign(gtk::Align::Center);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    row.append(&label);
    row.append(check);
    row.append(control);
    row
}

fn connect_action(
    button: &gtk::Button,
    shell: &Rc<Shell>,
    dialog: &adw::Dialog,
    controls: &RandomPlayControls,
    genres: &[GenreSummary],
    placement: QueuePlacement,
) {
    let radio = shell.products.playback.radio.clone();
    let dialog = dialog.downgrade();
    let controls = controls.clone();
    let genres = genres.to_vec();
    button.connect_clicked(move |_| {
        if let Some(request) = request_from_controls(&controls, &genres, placement) {
            radio.play_random(request);
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
}

fn request_from_controls(
    controls: &RandomPlayControls,
    genres: &[GenreSummary],
    placement: QueuePlacement,
) -> Option<RandomPlayRequest> {
    let (genre_id, genre_name) = selected_genre(genres, controls.genre.selected());
    let played_filter = [
        PlayedFilter::All,
        PlayedFilter::Unplayed,
        PlayedFilter::Played,
    ]
    .get(controls.played_filter.selected() as usize)
    .copied()?;
    Some(RandomPlayRequest {
        placement,
        criteria: RandomCriteria {
            limit: controls.limit.value_as_int().clamp(1, 500) as usize,
            min_year: controls
                .min_year_enabled
                .is_active()
                .then(|| controls.min_year.value_as_int().clamp(1850, 2050) as u16),
            max_year: controls
                .max_year_enabled
                .is_active()
                .then(|| controls.max_year.value_as_int().clamp(1850, 2050) as u16),
            genre_id,
            genre_name,
            played_filter,
        },
    })
}

fn selected_genre(genres: &[GenreSummary], selected: u32) -> (Option<GenreId>, Option<String>) {
    if selected == gtk::INVALID_LIST_POSITION || selected == 0 {
        return (None, None);
    }
    let Some(genre) = genres.get((selected - 1) as usize) else {
        return (None, None);
    };
    (Some(genre.genre.id.clone()), Some(genre.genre.name.clone()))
}
