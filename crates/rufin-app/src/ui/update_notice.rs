use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{TryRecvError, channel};
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use reqwest::blocking::Client;
use serde::Deserialize;
use tracing::debug;

use crate::i18n::tr;

use super::Shell;

const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/screwys/Rufin/releases/latest";
const RELEASE_NOTICE_DELAY_MS: u64 = 1_500;
const RELEASE_NOTICE_POLL_MS: u64 = 250;
const RELEASE_NOTICE_WIDTH: i32 = 440;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleaseNotice {
    tag_name: String,
    summary: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    body: Option<String>,
    html_url: String,
    draft: bool,
    prerelease: bool,
}

pub(super) fn schedule_release_notice(shell: &Rc<Shell>) {
    let settings = shell.state.settings.borrow();
    if settings.private_mode || settings.suppress_release_notices {
        return;
    }
    drop(settings);

    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(Duration::from_millis(RELEASE_NOTICE_DELAY_MS), move || {
        request_release_notice(shell);
    });
}

fn request_release_notice(shell: Rc<Shell>) {
    let settings = shell.state.settings.borrow();
    if settings.private_mode || settings.suppress_release_notices {
        return;
    }
    drop(settings);

    let (sender, receiver) = channel();
    thread::spawn(move || {
        let result = latest_release_notice(env!("CARGO_PKG_VERSION"));
        let _sent = sender.send(result);
    });

    glib::timeout_add_local(
        Duration::from_millis(RELEASE_NOTICE_POLL_MS),
        move || match receiver.try_recv() {
            Ok(Ok(Some(notice))) => {
                let settings = shell.state.settings.borrow();
                if !settings.private_mode && !settings.suppress_release_notices {
                    drop(settings);
                    present_release_notice(&shell, notice);
                }
                glib::ControlFlow::Break
            }
            Ok(Ok(None)) => glib::ControlFlow::Break,
            Ok(Err(error)) => {
                debug!(%error, "failed to check latest Rufin release");
                glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        },
    );
}

fn latest_release_notice(current_version: &str) -> Result<Option<ReleaseNotice>, String> {
    let release = fetch_latest_release()?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    if !release_tag_is_newer(current_version, &release.tag_name) {
        return Ok(None);
    }

    Ok(Some(ReleaseNotice {
        tag_name: release.tag_name,
        summary: release_summary(release.body.as_deref()).unwrap_or_default(),
        url: release.html_url,
    }))
}

fn fetch_latest_release() -> Result<GitHubRelease, String> {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(format!("Rufin/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?
        .get(GITHUB_LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<GitHubRelease>()
        .map_err(|error| error.to_string())
}

fn present_release_notice(shell: &Rc<Shell>, notice: ReleaseNotice) {
    if shell.state.settings.borrow().suppress_release_notices {
        return;
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let heading = gtk::Label::new(Some(&format!("{} is available!", notice.tag_name)));
    heading.add_css_class("title-2");
    heading.set_xalign(0.0);
    heading.set_wrap(true);
    content.append(&heading);

    if !notice.summary.is_empty() {
        let summary = gtk::Label::new(Some(&notice.summary));
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        summary.add_css_class("body");
        content.append(&summary);
    }

    let release_link =
        gtk::LinkButton::with_label(&notice.url, &format!("Release {}", notice.tag_name));
    release_link.set_halign(gtk::Align::Start);
    content.append(&release_link);

    let suppress = gtk::CheckButton::with_label(&tr("Don't show again"));
    suppress.set_halign(gtk::Align::Start);
    content.append(&suppress);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label(&tr("Close"));
    actions.append(&close);
    content.append(&actions);

    let dialog = adw::Dialog::builder()
        .content_width(RELEASE_NOTICE_WIDTH)
        .child(&content)
        .build();

    let persisted = Rc::new(Cell::new(false));
    let persist_dismissal = {
        let shell = Rc::clone(shell);
        let suppress = suppress.clone();
        let persisted = Rc::clone(&persisted);
        move || {
            if !suppress.is_active() || persisted.replace(true) {
                return;
            }
            shell.update_app_settings("release notice setting", |settings| {
                if settings.suppress_release_notices {
                    return false;
                }
                settings.suppress_release_notices = true;
                true
            });
        }
    };
    let persist_dismissal = Rc::new(persist_dismissal);

    let link_persist = Rc::clone(&persist_dismissal);
    release_link.connect_clicked(move |_| {
        link_persist();
    });

    let close_dialog = dialog.clone();
    close.connect_clicked(move |_| {
        close_dialog.close();
    });

    let close_persist = Rc::clone(&persist_dismissal);
    dialog.connect_closed(move |_| {
        close_persist();
    });

    dialog.present(Some(&shell.window));
}

fn release_summary(body: Option<&str>) -> Option<String> {
    let body = body?;
    let mut lines = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.eq_ignore_ascii_case("changelog") {
            break;
        }
        if line.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        lines.push(line);
    }
    let summary = lines.join("\n");
    (!summary.is_empty()).then_some(summary)
}

fn release_tag_is_newer(current_version: &str, release_tag: &str) -> bool {
    version_numbers(release_tag) > version_numbers(current_version)
}

fn version_numbers(value: &str) -> Vec<u64> {
    value
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|part| part.parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{release_summary, release_tag_is_newer, version_numbers};

    #[test]
    fn release_summary_uses_first_notes_block() {
        let summary = release_summary(Some(
            "Major performance overhaul & visual polishment\r\n\r\nChangelog\r\nfix(ui): one",
        ));

        assert_eq!(
            summary,
            Some("Major performance overhaul & visual polishment".to_string())
        );
    }

    #[test]
    fn release_version_comparison_uses_numeric_components() {
        assert!(release_tag_is_newer("0.3.1", "v0.3.2"));
        assert!(release_tag_is_newer("0.9.9", "v0.10.0"));
        assert!(!release_tag_is_newer("0.3.1", "v0.3.1"));
        assert!(!release_tag_is_newer("0.3.2", "v0.3.1"));
        assert_eq!(version_numbers("v0.3.1"), vec![0, 3, 1]);
    }
}
