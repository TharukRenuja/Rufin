use super::*;
use std::rc::Rc;
use std::time::Duration;

const RELEASE_NOTES_POPUP_WIDTH: i32 = 700;
const RELEASE_NOTES_POPUP_HEIGHT: i32 = 640;
const RELEASE_TOAST_TITLE: &str = "✨ New release is available!";
const FLATHUB_APPSTREAM_URL: &str = "https://flathub.org/api/v2/appstream/io.github.screwys.Rufin";
const RELEASE_CHECK_TIMEOUT_SECONDS: u64 = 4;
const RELEASE_NOTES_XML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/io.github.screwys.Rufin.metainfo.xml"
));

#[derive(Debug, Eq, PartialEq)]
struct ReleaseNote {
    version: String,
    date: String,
    summary: Option<String>,
    items: Vec<String>,
}

pub(in crate::ui) fn schedule_release_toast(shell: &Rc<Shell>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(Duration::from_millis(250), move || {
        shell.check_release_toast();
    });
}

pub(in crate::ui) fn about_release_notes() -> String {
    let Some(release) = release_notes_from_appstream().into_iter().next() else {
        return String::new();
    };
    let mut markup = String::new();
    if let Some(summary) = release.summary {
        markup.push_str("<p>");
        markup.push_str(&xml_escape(&summary));
        markup.push_str("</p>");
    }
    if !release.items.is_empty() {
        markup.push_str("<ul>");
        for item in release.items {
            markup.push_str("<li>");
            markup.push_str(&xml_escape(&item));
            markup.push_str("</li>");
        }
        markup.push_str("</ul>");
    }
    markup
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn release_notes_from_appstream() -> Vec<ReleaseNote> {
    parse_appstream_release_notes(RELEASE_NOTES_XML, 5)
}

fn latest_release_version() -> Option<String> {
    release_notes_from_appstream()
        .into_iter()
        .next()
        .map(|release| release.version)
}

fn fetch_latest_flathub_release_version() -> Result<Option<String>, String> {
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
    Ok(latest_release_version_from_flathub_json(&value))
}

fn latest_release_version_from_flathub_json(value: &serde_json::Value) -> Option<String> {
    value
        .get("releases")?
        .as_array()?
        .iter()
        .filter_map(|release| release.get("version")?.as_str())
        .map(str::trim)
        .find(|version| !version.is_empty())
        .map(str::to_string)
}

fn release_notification_due(settings: &AppSettings, latest: &str, current: &str) -> bool {
    settings.release_notifications_enabled
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
        rest = &rest[start..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..tag_end + 1];
        let body_start = tag_end + 1;
        let Some(body_end) = rest[body_start..].find("</release>") else {
            break;
        };
        let body = &rest[body_start..body_start + body_end];
        let description = tag_body(body, "description").unwrap_or(body);
        notes.push(ReleaseNote {
            version: attr_value(tag, "version").unwrap_or_default(),
            date: attr_value(tag, "date").unwrap_or_default(),
            summary: tag_texts(description, "p").into_iter().next(),
            items: tag_texts(description, "li"),
        });
        rest = &rest[body_start + body_end + "</release>".len()..];
    }
    notes
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')?;
    Some(xml_text(&tag[start..start + end]))
}

fn tag_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)?;
    Some(&text[start..start + end])
}

fn tag_texts(text: &str, tag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = text;
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else {
            break;
        };
        let value = xml_text(&strip_xml_tags(&rest[..end]));
        if !value.is_empty() {
            values.push(value);
        }
        rest = &rest[end + close.len()..];
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
    let url = format!("https://github.com/screwys/Rufin/releases/tag/{version_label}");
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
    let date = gtk::Label::new(Some(&note.date));
    date.add_css_class("release-note-date");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&version);
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

fn present_release_notes_popup(window: &adw::ApplicationWindow, overlay: &gtk::Overlay) {
    let backdrop = gtk::Overlay::new();
    backdrop.add_css_class("release-notes-backdrop");
    backdrop.set_halign(gtk::Align::Fill);
    backdrop.set_valign(gtk::Align::Fill);
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    let hit_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    hit_area.set_hexpand(true);
    hit_area.set_vexpand(true);
    backdrop.set_child(Some(&hit_area));

    let card = gtk::Box::new(gtk::Orientation::Vertical, 16);
    card.add_css_class("release-notes-card");
    card.set_hexpand(true);
    card.set_width_request(1);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.set_hexpand(true);
    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    title_box.set_hexpand(true);
    let title = gtk::Label::new(Some(&tr("Release notes")));
    title.add_css_class("release-notes-title");
    title.set_xalign(0.0);
    title_box.append(&title);

    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("flat");
    close.add_css_class("circular");
    close.set_tooltip_text(Some(&tr("Close")));
    let current_version = gtk::Label::new(Some(&format!(
        "{} v{}",
        tr("Current version:"),
        env!("CARGO_PKG_VERSION")
    )));
    current_version.add_css_class("release-notes-current-version");
    header.append(&title_box);
    header.append(&current_version);
    header.append(&close);
    card.append(&header);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("release-notes-list");
    for note in release_notes_from_appstream() {
        list.append(&release_note_row(window, &note));
    }

    let popup_width = large_popup_content_width(RELEASE_NOTES_POPUP_WIDTH);
    let popup_height = large_popup_content_height(window.height(), RELEASE_NOTES_POPUP_HEIGHT);
    let scroller_height = popup_height.saturating_sub(104).max(280);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_min_content_height(scroller_height);
    scroller.set_max_content_height(scroller_height);
    scroller.set_propagate_natural_height(false);
    scroller.set_child(Some(&list));
    card.append(&scroller);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(popup_width);
    clamp.set_tightening_threshold(popup_width);
    clamp.set_halign(gtk::Align::Fill);
    clamp.set_valign(gtk::Align::Center);
    clamp.set_margin_start(24);
    clamp.set_margin_end(24);
    clamp.set_margin_top(24);
    clamp.set_margin_bottom(24);
    clamp.set_child(Some(&card));
    backdrop.add_overlay(&clamp);
    backdrop.set_measure_overlay(&clamp, false);
    overlay.add_overlay(&backdrop);
    overlay.set_measure_overlay(&backdrop, false);

    let overlay = overlay.clone();
    let backdrop_for_close = backdrop.clone();
    let close_overlay = overlay.clone();
    close.connect_clicked(move |_| close_overlay.remove_overlay(&backdrop_for_close));

    let backdrop_for_click = backdrop.clone();
    add_widget_click(hit_area.upcast_ref(), move || {
        overlay.remove_overlay(&backdrop_for_click)
    });
}

impl Shell {
    pub(in crate::ui) fn check_release_toast(self: &Rc<Self>) {
        if !self.state.settings.borrow().release_notifications_enabled {
            return;
        };
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let latest = match gio::spawn_blocking(fetch_latest_flathub_release_version).await {
                Ok(Ok(Some(version))) => Some(version),
                Ok(Ok(None)) => latest_release_version(),
                Ok(Err(error)) => {
                    debug!(%error, "failed to check Flathub release");
                    latest_release_version()
                }
                Err(_) => {
                    debug!("Flathub release check task failed");
                    latest_release_version()
                }
            };
            if let Some(latest) = latest {
                shell.show_release_toast_if_needed(&latest);
            }
        });
    }

    fn show_release_toast_if_needed(self: &Rc<Self>, latest: &str) {
        if !release_notification_due(
            &self.state.settings.borrow(),
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
        toast.set_timeout(5);
        toast.set_button_label(Some(&tr("View")));
        toast.set_action_name(Some("win.show-release-notes"));
        self.toast_overlay.add_toast(toast);
    }

    pub(in crate::ui) fn present_release_notes(&self) {
        present_release_notes_popup(&self.window, &self.app_root_overlay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn release_notification_uses_seen_version_and_current_version() {
        let mut settings = AppSettings::default();

        assert!(release_notification_due(&settings, "2.0.0", "1.9.0"));
        assert!(!release_notification_due(&settings, "2.0.0", "2.0.0"));
        assert!(!release_notification_due(&settings, "1.9.0", "2.0.0"));

        settings.release_notification_seen_version = Some("2.0.0".to_string());
        assert!(!release_notification_due(&settings, "2.0.0", "1.9.0"));

        settings.release_notification_seen_version = Some("1.9.0".to_string());
        assert!(release_notification_due(&settings, "2.0.0", "1.9.0"));

        settings.release_notifications_enabled = false;
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
    fn flathub_appstream_json_uses_first_release_version() {
        let value = serde_json::json!({
            "releases": [
                { "version": "2.0.0", "timestamp": "1780000000" },
                { "version": "1.0.0", "timestamp": "1770000000" }
            ]
        });

        assert_eq!(
            latest_release_version_from_flathub_json(&value),
            Some("2.0.0".to_string())
        );
    }
}
