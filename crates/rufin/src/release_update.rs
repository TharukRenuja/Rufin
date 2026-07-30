//! One launch-time check for a newer Rufin release.
//!
//! UI chooses when to start the check and presents the result. This owner
//! performs the request, builds complete release notes, applies private-mode
//! policy, and records the version whose toast was shown.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_channel::Sender;
use tracing::debug;
use ui::runtime::{ReleaseNote, ReleaseUpdate, ReleaseUpdatePort};

use crate::settings::SettingsFile;

const FLATHUB_APPSTREAM_URL: &str = "https://flathub.org/api/v2/appstream/io.github.screwys.Rufin";
const RELEASE_CHECK_TIMEOUT: Duration = Duration::from_secs(4);
const RELEASE_NOTES_XML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/io.github.screwys.Rufin.metainfo.xml"
));

#[derive(Debug, Eq, PartialEq)]
struct FetchedReleaseUpdate {
    latest: String,
    notes: Vec<ReleaseNote>,
}

pub(crate) struct ReleaseUpdateOwner {
    settings: SettingsFile,
    runtime: tokio::runtime::Handle,
    events: Sender<ReleaseUpdate>,
    bundled_notes: Arc<[ReleaseNote]>,
}

impl ReleaseUpdateOwner {
    pub(crate) fn new(
        settings: SettingsFile,
        runtime: tokio::runtime::Handle,
        events: Sender<ReleaseUpdate>,
    ) -> Arc<Self> {
        Arc::new(Self {
            settings,
            runtime,
            events,
            bundled_notes: release_notes_from_appstream().into(),
        })
    }

    pub(crate) fn bundled_notes(&self) -> Arc<[ReleaseNote]> {
        Arc::clone(&self.bundled_notes)
    }
}

impl ReleaseUpdatePort for ReleaseUpdateOwner {
    fn check(&self) {
        if !release_check_allowed(&self.settings.load().ui) {
            return;
        }
        let settings = self.settings.clone();
        let events = self.events.clone();
        let bundled_notes = Arc::clone(&self.bundled_notes);
        self.runtime.spawn(async move {
            let fetched = match tokio::task::spawn_blocking(fetch_flathub_release_update).await {
                Ok(Ok(update)) => update,
                Ok(Err(error)) => {
                    debug!(%error, "failed to check Flathub release");
                    None
                }
                Err(error) => {
                    debug!(%error, "Flathub release check task failed");
                    None
                }
            };
            let (latest, notes) = complete_release_notes(fetched, &bundled_notes);
            let Some(latest) = latest else {
                return;
            };
            let notification_version =
                release_notification_due(&settings.load().ui, &latest, env!("CARGO_PKG_VERSION"))
                    .then_some(latest);
            let _ = events
                .send(ReleaseUpdate {
                    notes,
                    notification_version,
                })
                .await;
        });
    }

    fn mark_seen(&self, version: String) -> Result<(), String> {
        mark_release_notification_seen(&self.settings, &version)
    }
}

fn release_notes_from_appstream() -> Vec<ReleaseNote> {
    parse_appstream_release_notes(RELEASE_NOTES_XML, 5)
}

fn complete_release_notes(
    fetched: Option<FetchedReleaseUpdate>,
    bundled: &[ReleaseNote],
) -> (Option<String>, Arc<[ReleaseNote]>) {
    let latest = fetched
        .as_ref()
        .map(|update| update.latest.clone())
        .or_else(|| bundled.first().map(|note| note.version.clone()));
    let mut notes = fetched.map_or_else(Vec::new, |update| update.notes);
    for note in bundled {
        if !notes
            .iter()
            .any(|existing| existing.version == note.version)
        {
            notes.push(note.clone());
        }
    }
    (latest, notes.into())
}

fn fetch_flathub_release_update() -> Result<Option<FetchedReleaseUpdate>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(RELEASE_CHECK_TIMEOUT)
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    debug!(
        service = "flathub",
        method = "GET",
        public_url = FLATHUB_APPSTREAM_URL,
        "sending remote request"
    );
    let started = Instant::now();
    let response = client
        .get(FLATHUB_APPSTREAM_URL)
        .send()
        .map_err(|error| error.to_string())?;
    debug!(
        service = "flathub",
        method = "GET",
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "received remote response"
    );
    let value = response
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<serde_json::Value>()
        .map_err(|error| error.to_string())?;
    Ok(release_update_from_flathub_json(&value))
}

fn release_update_from_flathub_json(value: &serde_json::Value) -> Option<FetchedReleaseUpdate> {
    let notes: Vec<_> = value
        .get("releases")?
        .as_array()?
        .iter()
        .filter_map(release_note_from_flathub_json)
        .take(5)
        .collect();
    let latest = notes.first()?.version.clone();
    Some(FetchedReleaseUpdate { latest, notes })
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

fn release_check_allowed(settings: &ui::Settings) -> bool {
    settings.release_notifications_enabled && !settings.private_mode
}

fn release_notification_due(settings: &ui::Settings, latest: &str, current: &str) -> bool {
    release_check_allowed(settings)
        && release_version_is_newer(latest, current)
        && settings.release_notification_seen_version.as_deref() != Some(latest)
}

fn mark_release_notification_seen(settings: &SettingsFile, version: &str) -> Result<(), String> {
    let version = version.trim();
    if version.is_empty()
        || settings
            .load()
            .ui
            .release_notification_seen_version
            .as_deref()
            == Some(version)
    {
        return Ok(());
    }
    settings.update(|stored| {
        stored.ui.release_notification_seen_version = Some(version.to_string());
        Ok(())
    })
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

#[cfg(test)]
mod tests {
    use super::{
        FetchedReleaseUpdate, complete_release_notes, mark_release_notification_seen,
        parse_appstream_release_notes, release_notification_due, release_update_from_flathub_json,
        release_version_is_newer,
    };
    use crate::settings::SettingsFile;
    use ui::runtime::ReleaseNote;

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

        assert_eq!(
            parse_appstream_release_notes(xml, 1),
            vec![ReleaseNote {
                version: "2.0.0".to_string(),
                date: "2026-01-02".to_string(),
                summary: Some("Summary & context.".to_string()),
                items: vec!["First item".to_string(), "Second item".to_string()],
            }]
        );
    }

    #[test]
    fn fetched_release_notes_precede_missing_bundled_history() {
        let fetched = FetchedReleaseUpdate {
            latest: "2.0.0".to_string(),
            notes: vec![ReleaseNote {
                version: "2.0.0".to_string(),
                date: "2026-01-02".to_string(),
                summary: None,
                items: Vec::new(),
            }],
        };
        let bundled = vec![
            ReleaseNote {
                version: "2.0.0".to_string(),
                date: "old".to_string(),
                summary: None,
                items: Vec::new(),
            },
            ReleaseNote {
                version: "1.0.0".to_string(),
                date: "2026-01-01".to_string(),
                summary: None,
                items: Vec::new(),
            },
        ];

        let (latest, notes) = complete_release_notes(Some(fetched), &bundled);

        assert_eq!(latest.as_deref(), Some("2.0.0"));
        assert_eq!(
            notes
                .iter()
                .map(|note| note.version.as_str())
                .collect::<Vec<_>>(),
            vec!["2.0.0", "1.0.0"]
        );
    }

    #[test]
    fn release_notification_uses_current_private_and_seen_settings() {
        let mut settings = ui::Settings::default();

        assert!(release_notification_due(&settings, "2.0.0", "1.9.0"));
        assert!(!release_notification_due(&settings, "2.0.0", "2.0.0"));
        settings.release_notification_seen_version = Some("2.0.0".to_string());
        assert!(!release_notification_due(&settings, "2.0.0", "1.9.0"));
        settings.release_notification_seen_version = None;
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
            Some(FetchedReleaseUpdate {
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

    #[test]
    fn seen_release_update_preserves_other_settings_and_reopens() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let path = directory.path().join("settings.json");
        let settings = SettingsFile::open(path.clone()).expect("open settings");
        settings
            .update(|stored| {
                stored.ui.private_mode = true;
                stored.ui.notifications_enabled = true;
                Ok(())
            })
            .expect("prepare unrelated settings");

        mark_release_notification_seen(&settings, " 2.0.0 ").expect("mark release seen");

        let reopened = SettingsFile::open(path).expect("reopen settings").load();
        assert_eq!(
            reopened.ui.release_notification_seen_version.as_deref(),
            Some("2.0.0")
        );
        assert!(reopened.ui.private_mode);
        assert!(reopened.ui.notifications_enabled);
    }
}
