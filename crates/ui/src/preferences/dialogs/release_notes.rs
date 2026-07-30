use std::rc::Rc;
use std::time::Duration;

use crate::layout::{large_popup_content_height, large_popup_content_width};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::runtime::{ReleaseNote, ReleaseUpdate};
use crate::shell::Shell;
use adw::prelude::*;
use gtk::glib;
use localization::{tr, trn_with};
use tracing::warn;

const RELEASE_NOTES_POPUP_WIDTH: i32 = 700;
const RELEASE_NOTES_POPUP_HEIGHT: i32 = 640;
const RELEASE_TOAST_TITLE: &str = "✨ New release is available!";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CivilDate {
    year: i32,
    month: u32,
    day: u32,
}

pub(crate) fn schedule_release_check(shell: &Rc<Shell>) {
    let release_updates = shell.products.release_updates.clone();
    glib::timeout_add_local_once(Duration::from_millis(250), move || {
        release_updates.check();
    });
}

fn current_civil_date() -> Option<CivilDate> {
    let now = glib::DateTime::now_local().ok()?;
    Some(CivilDate {
        year: now.year(),
        month: now.month() as u32,
        day: now.day_of_month() as u32,
    })
}

fn parse_civil_date(text: &str) -> Option<CivilDate> {
    let mut parts = text.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(CivilDate { year, month, day })
}

fn civil_days(date: CivilDate) -> i32 {
    let year = date.year - i32::from(date.month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = date.month as i32;
    let day = date.day as i32;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn release_relative_date_for(date: &str, today: CivilDate) -> String {
    let Some(release_date) = parse_civil_date(date) else {
        return date.to_string();
    };
    let days = civil_days(today).saturating_sub(civil_days(release_date));
    if days < 0 {
        return date.to_string();
    }
    match days {
        0 => tr("today"),
        1 => tr("yesterday"),
        2..=13 => {
            let count = days as u64;
            trn_with(
                "{count} day ago",
                "{count} days ago",
                count,
                &[("count", &count.to_string())],
            )
        }
        14..=59 => {
            let count = (days / 7) as u64;
            trn_with(
                "{count} week ago",
                "{count} weeks ago",
                count,
                &[("count", &count.to_string())],
            )
        }
        60..=729 => {
            let count = (days / 30) as u64;
            trn_with(
                "{count} month ago",
                "{count} months ago",
                count,
                &[("count", &count.to_string())],
            )
        }
        _ => {
            let count = (days / 365) as u64;
            trn_with(
                "{count} year ago",
                "{count} years ago",
                count,
                &[("count", &count.to_string())],
            )
        }
    }
}

fn release_relative_date(date: &str) -> String {
    current_civil_date()
        .map(|today| release_relative_date_for(date, today))
        .unwrap_or_else(|| date.to_string())
}

fn release_note_row(window: &adw::ApplicationWindow, note: &ReleaseNote) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 8);
    row.add_css_class("release-note-row");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.set_hexpand(true);
    let version_label = format!("v{}", note.version);
    let version = gtk::Button::new();
    version.add_css_class("flat");
    version.add_css_class("release-note-version");
    version.set_cursor_from_name(Some("pointer"));
    version.set_tooltip_text(Some(&tr("Open release notes")));
    let version_content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let version_text = gtk::Label::new(Some(&version_label));
    let version_icon = gtk::Image::from_icon_name("adw-external-link-symbolic");
    version_icon.set_pixel_size(12);
    version_content.append(&version_text);
    version_content.append(&version_icon);
    version.set_child(Some(&version_content));
    let url = format!(
        "https://github.com/screwys/Rufin/releases/tag/v{}",
        note.version
    );
    let window = window.downgrade();
    version.connect_clicked(move |_| {
        let Some(window) = window.upgrade() else {
            return;
        };
        let launcher = gtk::UriLauncher::new(&url);
        gtk::glib::spawn_future_local(async move {
            if let Err(error) = launcher.launch_future(Some(&window)).await {
                warn!(%error, "failed to open release notes link");
            }
        });
    });
    let date = gtk::Label::new(Some(&release_relative_date(&note.date)));
    date.add_css_class("release-note-date");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&version);
    if note.version == env!("CARGO_PKG_VERSION") {
        let installed = gtk::Label::new(Some(&tr("Installed")));
        installed.add_css_class("release-note-installed");
        header.append(&installed);
    }
    header.append(&spacer);
    header.append(&date);
    row.append(&header);

    if let Some(summary) = note.summary.as_ref() {
        let body = gtk::Label::new(Some(summary));
        body.add_css_class("release-note-summary");
        body.set_wrap(true);
        body.set_xalign(0.0);
        row.append(&body);
    }

    let items = gtk::Box::new(gtk::Orientation::Vertical, 4);
    items.add_css_class("release-note-items");
    for item in &note.items {
        let bullet = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bullet.add_css_class("release-note-bullet");
        let marker = gtk::Label::new(Some("•"));
        marker.add_css_class("release-note-marker");
        marker.set_valign(gtk::Align::Start);
        let text = gtk::Label::new(Some(item));
        text.add_css_class("release-note-item");
        text.set_wrap(true);
        text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.set_xalign(0.0);
        text.set_hexpand(true);
        bullet.append(&marker);
        bullet.append(&text);
        items.append(&bullet);
    }
    row.append(&items);

    row.upcast()
}

fn present_release_notes_dialog(window: &adw::ApplicationWindow, notes: &[ReleaseNote]) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr("Version History"), "")));
    toolbar.add_top_bar(&header);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("release-notes-list");
    for note in notes {
        list.append(&release_note_row(window, note));
    }

    let popup_width = large_popup_content_width(RELEASE_NOTES_POPUP_WIDTH);
    let popup_height = large_popup_content_height(window.height(), RELEASE_NOTES_POPUP_HEIGHT);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_margin_top(12);
    scroller.set_margin_bottom(12);
    scroller.set_margin_start(18);
    scroller.set_margin_end(18);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    toolbar.set_content(Some(&scroller));

    let dialog = adw::Dialog::builder()
        .title(tr("Version History"))
        .content_width(popup_width)
        .content_height(popup_height)
        .child(&toolbar)
        .build();
    present_light_dismiss_dialog(&dialog, window);
}

pub(crate) fn apply_release_update(shell: &Rc<Shell>, update: ReleaseUpdate) {
    *shell.preferences.release_notes.borrow_mut() = update.notes;
    let Some(version) = update.notification_version else {
        return;
    };

    let toast = adw::Toast::new(&tr(RELEASE_TOAST_TITLE));
    toast.set_timeout(0);
    toast.set_button_label(Some(&tr("View")));
    toast.set_action_name(Some("win.show-release-notes"));
    shell.chrome.toast_overlay.add_toast(toast);
    if let Err(error) = shell.products.release_updates.mark_seen(version) {
        warn!(%error, "failed to record the shown release notification");
    }
}

impl Shell {
    pub(crate) fn present_release_notes(&self) {
        present_release_notes_dialog(
            &self.chrome.window,
            self.preferences.release_notes.borrow().as_ref(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{CivilDate, release_relative_date_for};

    #[test]
    fn release_dates_use_relative_labels_without_singular_units() {
        let today = CivilDate {
            year: 2026,
            month: 6,
            day: 19,
        };

        assert_eq!(release_relative_date_for("2026-06-19", today), "today");
        assert_eq!(release_relative_date_for("2026-06-18", today), "yesterday");
        assert_eq!(release_relative_date_for("2026-06-10", today), "9 days ago");
        assert_eq!(
            release_relative_date_for("2026-05-01", today),
            "7 weeks ago"
        );
        assert_eq!(
            release_relative_date_for("2025-09-19", today),
            "9 months ago"
        );
        assert_eq!(
            release_relative_date_for("2025-06-19", today),
            "12 months ago"
        );
        assert_eq!(
            release_relative_date_for("2023-06-19", today),
            "3 years ago"
        );
        assert_eq!(release_relative_date_for("not-a-date", today), "not-a-date");
    }
}
