use std::rc::Rc;
use std::time::Duration;

use crate::Settings as UiSettings;
use crate::layout::{large_popup_content_height, large_popup_content_width};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use adw::prelude::*;
use gtk::{gio, glib};
use localization::{tr, trn_with};
use tracing::{debug, warn};

const RELEASE_NOTES_POPUP_WIDTH: i32 = 700;
const RELEASE_NOTES_POPUP_HEIGHT: i32 = 640;
const RELEASE_TOAST_TITLE: &str = "✨ New release is available!";
const FLATHUB_APPSTREAM_URL: &str = "https://flathub.org/api/v2/appstream/io.github.screwys.Rufin";
const RELEASE_CHECK_TIMEOUT_SECONDS: u64 = 4;
const RELEASE_NOTES_XML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/io.github.screwys.Rufin.metainfo.xml"
));

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReleaseNote {
    version: String,
    date: String,
    summary: Option<String>,
    items: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct ReleaseUpdate {
    latest: String,
    notes: Vec<ReleaseNote>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CivilDate {
    year: i32,
    month: u32,
    day: u32,
}

pub(crate) fn schedule_release_toast(shell: &Rc<Shell>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(Duration::from_millis(250), move || {
        shell.check_release_toast();
    });
}

fn release_notes_from_appstream() -> Vec<ReleaseNote> {
    parse_appstream_release_notes(RELEASE_NOTES_XML, 5)
}

fn release_notes_with_fetched(fetched: &[ReleaseNote]) -> Vec<ReleaseNote> {
    let mut notes = fetched.to_vec();
    for note in release_notes_from_appstream() {
        if !notes
            .iter()
            .any(|existing| existing.version == note.version)
        {
            notes.push(note);
        }
    }
    notes
}

fn latest_release_version() -> Option<String> {
    release_notes_from_appstream()
        .into_iter()
        .next()
        .map(|release| release.version)
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

fn fetch_flathub_release_update() -> Result<Option<ReleaseUpdate>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(RELEASE_CHECK_TIMEOUT_SECONDS))
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let value = client
        .get(FLATHUB_APPSTREAM_URL)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json::<serde_json::Value>()
        .map_err(|error| error.to_string())?;
    Ok(release_update_from_flathub_json(&value))
}

fn fallback_release_update() -> Option<ReleaseUpdate> {
    latest_release_version().map(|latest| ReleaseUpdate {
        latest,
        notes: release_notes_from_appstream(),
    })
}

fn release_update_from_flathub_json(value: &serde_json::Value) -> Option<ReleaseUpdate> {
    let notes: Vec<_> = value
        .get("releases")?
        .as_array()?
        .iter()
        .filter_map(release_note_from_flathub_json)
        .take(5)
        .collect();
    let latest = notes.first()?.version.clone();
    Some(ReleaseUpdate { latest, notes })
}

fn release_note_from_flathub_json(value: &serde_json::Value) -> Option<ReleaseNote> {
    let version = value.get("version")?.as_str()?.trim();
    if version.is_empty() {
        return None;
    }
    let description = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Some(ReleaseNote {
        version: version.to_string(),
        date: flathub_release_date(value),
        summary: tag_texts(description, "p").into_iter().next(),
        items: tag_texts(description, "li"),
    })
}

fn flathub_release_date(value: &serde_json::Value) -> String {
    value
        .get("date")
        .and_then(serde_json::Value::as_str)
        .filter(|date| !date.trim().is_empty())
        .map(|date| date.trim().to_string())
        .or_else(|| {
            value
                .get("timestamp")
                .and_then(serde_json::Value::as_str)
                .and_then(|timestamp| timestamp.parse::<i64>().ok())
                .map(unix_timestamp_date)
        })
        .unwrap_or_default()
}

fn unix_timestamp_date(timestamp: i64) -> String {
    glib::DateTime::from_unix_utc(timestamp)
        .map(|date| {
            format!(
                "{:04}-{:02}-{:02}",
                date.year(),
                date.month(),
                date.day_of_month()
            )
        })
        .unwrap_or_default()
}

fn release_notification_due(settings: &UiSettings, latest: &str, current: &str) -> bool {
    settings.allows_release_update_check()
        && release_version_is_newer(latest, current)
        && settings.release_notification_seen_version.as_deref() != Some(latest)
}

fn release_version_is_newer(latest: &str, current: &str) -> bool {
    let Some(latest_parts) = release_version_parts(latest) else {
        return false;
    };
    let Some(current_parts) = release_version_parts(current) else {
        return false;
    };
    let len = latest_parts.len().max(current_parts.len());
    for index in 0..len {
        let latest_part = latest_parts.get(index).copied().unwrap_or(0);
        let current_part = current_parts.get(index).copied().unwrap_or(0);
        if latest_part != current_part {
            return latest_part > current_part;
        }
    }
    false
}

fn release_version_parts(version: &str) -> Option<Vec<u64>> {
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let version = version.split(['-', '+']).next().unwrap_or(version);
    if version.is_empty() {
        return None;
    }
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

fn parse_appstream_release_notes(xml: &str, limit: usize) -> Vec<ReleaseNote> {
    let mut notes = Vec::new();
    let mut rest = xml;
    while notes.len() < limit {
        let Some(start) = rest.find("<release ") else {
            break;
        };
        rest = rest.get(start..).unwrap_or_default();
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let tag = rest.get(..tag_end + 1).unwrap_or_default();
        let body_start = tag_end + 1;
        let Some(body_end) = rest
            .get(body_start..)
            .and_then(|rest| rest.find("</release>"))
        else {
            break;
        };
        let body = rest
            .get(body_start..body_start + body_end)
            .unwrap_or_default();
        let description = tag_body(body, "description").unwrap_or(body);
        notes.push(ReleaseNote {
            version: attr_value(tag, "version").unwrap_or_default(),
            date: attr_value(tag, "date").unwrap_or_default(),
            summary: tag_texts(description, "p").into_iter().next(),
            items: tag_texts(description, "li"),
        });
        rest = rest
            .get(body_start + body_end + "</release>".len()..)
            .unwrap_or_default();
    }
    notes
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag.get(start..)?.find('"')?;
    Some(xml_text(tag.get(start..start + end)?))
}

fn tag_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text.get(start..)?.find(&close)?;
    text.get(start..start + end)
}

fn tag_texts(text: &str, tag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = text;
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    while let Some(start) = rest.find(&open) {
        rest = rest.get(start + open.len()..).unwrap_or_default();
        let Some(end) = rest.find(&close) else {
            break;
        };
        let value = xml_text(&strip_xml_tags(rest.get(..end).unwrap_or_default()));
        if !value.is_empty() {
            values.push(value);
        }
        rest = rest.get(end + close.len()..).unwrap_or_default();
    }
    values
}

fn strip_xml_tags(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
}

fn xml_text(text: &str) -> String {
    text.trim()
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
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
    let version_icon = gtk::Image::from_icon_name("external-link-symbolic");
    version_icon.set_pixel_size(12);
    version_content.append(&version_text);
    version_content.append(&version_icon);
    version.set_child(Some(&version_content));
    let url = format!(
        "https://github.com/screwys/Rufin/releases/tag/v{}",
        note.version
    );
    let window = window.clone();
    version.connect_clicked(move |_| {
        let launcher = gtk::UriLauncher::new(&url);
        let window = window.clone();
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

impl Shell {
    pub(crate) fn check_release_toast(self: &Rc<Self>) {
        if !self.settings.current.borrow().allows_release_update_check() {
            return;
        };
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let update = match gio::spawn_blocking(fetch_flathub_release_update).await {
                Ok(Ok(Some(update))) => Some(update),
                Ok(Ok(None)) => fallback_release_update(),
                Ok(Err(error)) => {
                    debug!(%error, "failed to check Flathub release");
                    fallback_release_update()
                }
                Err(_) => {
                    debug!("Flathub release check task failed");
                    fallback_release_update()
                }
            };
            if let Some(update) = update {
                let ReleaseUpdate { latest, notes } = update;
                shell.store_fetched_release_notes(notes);
                shell.show_release_toast_if_needed(&latest);
            }
        });
    }

    fn store_fetched_release_notes(&self, notes: Vec<ReleaseNote>) {
        if !notes.is_empty() {
            *self.preferences.release_notes.borrow_mut() = notes;
        }
    }

    fn show_release_toast_if_needed(self: &Rc<Self>, latest: &str) {
        if !release_notification_due(
            &self.settings.current.borrow(),
            latest,
            env!("CARGO_PKG_VERSION"),
        ) {
            return;
        }
        self.show_release_toast();
        self.mark_release_notification_seen(latest);
    }

    fn show_release_toast(&self) {
        let toast = adw::Toast::new(&tr(RELEASE_TOAST_TITLE));
        toast.set_timeout(0);
        toast.set_button_label(Some(&tr("View")));
        toast.set_action_name(Some("win.show-release-notes"));
        self.chrome.toast_overlay.add_toast(toast);
    }

    pub(crate) fn present_release_notes(&self) {
        let notes = release_notes_with_fetched(&self.preferences.release_notes.borrow());
        present_release_notes_dialog(&self.chrome.window, &notes);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CivilDate, ReleaseNote, ReleaseUpdate, parse_appstream_release_notes,
        release_notes_with_fetched, release_notification_due, release_relative_date_for,
        release_update_from_flathub_json, release_version_is_newer,
    };
    use crate::Settings as UiSettings;

    #[test]
    fn appstream_release_notes_use_latest_entries() {
        let xml = r#"
            <component>
              <releases>
                <release version="2.0.0" date="2026-01-02">
                  <description>
                    <p>Summary &amp; context.</p>
                    <ul><li>First item</li><li>Second item</li></ul>
                  </description>
                </release>
                <release version="1.0.0" date="2026-01-01">
                  <description><ul><li>Older item</li></ul></description>
                </release>
              </releases>
            </component>
        "#;

        let notes = parse_appstream_release_notes(xml, 1);

        assert_eq!(
            notes,
            vec![ReleaseNote {
                version: "2.0.0".to_string(),
                date: "2026-01-02".to_string(),
                summary: Some("Summary & context.".to_string()),
                items: vec!["First item".to_string(), "Second item".to_string()],
            }]
        );
    }

    #[test]
    fn fetched_release_notes_are_prepended_to_bundled_history() {
        let notes = release_notes_with_fetched(&[ReleaseNote {
            version: "999.0.0".to_string(),
            date: "2026-01-03".to_string(),
            summary: Some("Fetched release.".to_string()),
            items: Vec::new(),
        }]);

        assert_eq!(
            notes.first().map(|note| note.version.as_str()),
            Some("999.0.0")
        );
        assert!(
            notes
                .iter()
                .any(|note| note.version == env!("CARGO_PKG_VERSION"))
        );
    }

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

    #[test]
    fn release_notification_uses_seen_version_and_current_version() {
        let mut settings = UiSettings::default();

        assert!(release_notification_due(&settings, "2.0.0", "1.9.0"));
        assert!(!release_notification_due(&settings, "2.0.0", "2.0.0"));
        assert!(!release_notification_due(&settings, "1.9.0", "2.0.0"));

        settings.release_notification_seen_version = Some("2.0.0".to_string());
        assert!(!release_notification_due(&settings, "2.0.0", "1.9.0"));

        settings.release_notification_seen_version = Some("1.9.0".to_string());
        assert!(release_notification_due(&settings, "2.0.0", "1.9.0"));

        settings.release_notifications_enabled = false;
        assert!(!release_notification_due(&settings, "2.0.0", "1.9.0"));

        settings.release_notifications_enabled = true;
        settings.private_mode = true;
        assert!(!release_notification_due(&settings, "2.0.0", "1.9.0"));
    }

    #[test]
    fn release_versions_compare_numeric_segments() {
        assert!(release_version_is_newer("v2.0.0", "1.9.9"));
        assert!(release_version_is_newer("1.10.0", "1.9.9"));
        assert!(release_version_is_newer("1.0.1", "1.0"));
        assert!(!release_version_is_newer("1.0.0", "1.0"));
        assert!(!release_version_is_newer("1.0.0", "1.0.1"));
    }

    #[test]
    fn flathub_appstream_json_uses_live_release_notes() {
        let value = serde_json::json!({
            "releases": [
                {
                    "version": "2.0.0",
                    "timestamp": "1782604800",
                    "description": "<p>Summary &amp; context.</p><ul><li>First item</li></ul><ul><li>Second item</li></ul>"
                },
                {
                    "version": "1.0.0",
                    "date": "2026-01-01",
                    "description": "<p>Older item</p>"
                }
            ]
        });

        assert_eq!(
            release_update_from_flathub_json(&value),
            Some(ReleaseUpdate {
                latest: "2.0.0".to_string(),
                notes: vec![
                    ReleaseNote {
                        version: "2.0.0".to_string(),
                        date: "2026-06-28".to_string(),
                        summary: Some("Summary & context.".to_string()),
                        items: vec!["First item".to_string(), "Second item".to_string()],
                    },
                    ReleaseNote {
                        version: "1.0.0".to_string(),
                        date: "2026-01-01".to_string(),
                        summary: Some("Older item".to_string()),
                        items: Vec::new(),
                    }
                ],
            })
        );
    }
}
