use std::rc::Rc;

use adw::prelude::*;
use domain::{Genre, GenreId};
use source::PlayedFilter;

use crate::controller::{RandomPlayAction, RandomPlayRequest};
use crate::i18n::tr;
use crate::sources::RandomTrackDomain;

use super::{
    PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, Shell, present_light_dismiss_dialog, text_button,
};

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
    domain: RandomTrackDomain,
}

pub(super) fn present_random_play_dialog(shell: &Rc<Shell>) {
    let library = shell.state.library.borrow();
    let genres = library.genres.clone();
    drop(library);
    let Some(domain) = shell.controller.random_track_domain() else {
        return;
    };

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
        played_filter: played_filter_dropdown(domain.played_filters()),
        domain,
    };
    controls.min_year.set_sensitive(false);
    controls.max_year.set_sensitive(false);
    controls
        .min_year_enabled
        .set_sensitive(domain.allows_year_range());
    controls
        .max_year_enabled
        .set_sensitive(domain.allows_year_range());
    controls.genre.set_sensitive(domain.allows_genre());
    if domain.allows_year_range() {
        connect_year_toggle(&controls.min_year_enabled, &controls.min_year);
        connect_year_toggle(&controls.max_year_enabled, &controls.max_year);
    }

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
    let add_last = text_button(PLAY_LATER_ICON, "Add Last");
    actions.append(&play_next);
    actions.append(&play_now);
    actions.append(&add_last);
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
        RandomPlayAction::PlayNext,
    );
    connect_action(
        &play_now,
        shell,
        &dialog,
        &controls,
        &genres,
        RandomPlayAction::PlayNow,
    );
    connect_action(
        &add_last,
        shell,
        &dialog,
        &controls,
        &genres,
        RandomPlayAction::AddLast,
    );

    present_light_dismiss_dialog(&dialog, &shell.window);
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

fn genre_dropdown(genres: &[Genre]) -> gtk::DropDown {
    let mut labels = Vec::with_capacity(genres.len() + 1);
    labels.push(tr("Any genre"));
    labels.extend(genres.iter().map(|genre| genre.name.clone()));
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
    genres: &[Genre],
    action: RandomPlayAction,
) {
    let controller = shell.controller.clone();
    let dialog = dialog.clone();
    let controls = controls.clone();
    let genres = genres.to_vec();
    button.connect_clicked(move |_| {
        if let Some(request) = request_from_controls(&controls, &genres, action) {
            controller.play_random_tracks(request);
            dialog.close();
        }
    });
}

fn request_from_controls(
    controls: &RandomPlayControls,
    genres: &[Genre],
    action: RandomPlayAction,
) -> Option<RandomPlayRequest> {
    let (genre_id, genre_name) = if controls.domain.allows_genre() {
        selected_genre(genres, controls.genre.selected())
    } else {
        (None, None)
    };
    let played_filter = controls
        .domain
        .played_filters()
        .get(controls.played_filter.selected() as usize)
        .copied()?;
    Some(RandomPlayRequest {
        action,
        limit: controls.limit.value_as_int().clamp(1, 500) as usize,
        min_year: controls
            .domain
            .allows_year_range()
            .then(|| {
                controls
                    .min_year_enabled
                    .is_active()
                    .then(|| controls.min_year.value_as_int().clamp(1850, 2050) as u16)
            })
            .flatten(),
        max_year: controls
            .domain
            .allows_year_range()
            .then(|| {
                controls
                    .max_year_enabled
                    .is_active()
                    .then(|| controls.max_year.value_as_int().clamp(1850, 2050) as u16)
            })
            .flatten(),
        genre_id,
        genre_name,
        played_filter,
    })
}

fn selected_genre(genres: &[Genre], selected: u32) -> (Option<GenreId>, Option<String>) {
    if selected == gtk::INVALID_LIST_POSITION || selected == 0 {
        return (None, None);
    }
    let Some(genre) = genres.get((selected - 1) as usize) else {
        return (None, None);
    };
    (Some(genre.id.clone()), Some(genre.name.clone()))
}
