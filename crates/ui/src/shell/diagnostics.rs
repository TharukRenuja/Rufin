use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio::prelude::FileExtManual;
use gtk::{gio, glib};
use localization::tr;

use crate::layout::{large_popup_content_height, large_popup_content_width};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;

use super::Shell;

const DIAGNOSTICS_DIALOG_WIDTH: i32 = 680;
const DIAGNOSTICS_DIALOG_HEIGHT: i32 = 560;
const LOG_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn present_diagnostics(shell: &Rc<Shell>) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr("Troubleshooting"), "")));
    toolbar.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let debug_group = adw::PreferencesGroup::new();
    let debug = adw::SwitchRow::builder()
        .title(tr("Debug logging"))
        .subtitle(tr("Include detailed Rufin diagnostics in this session"))
        .active(shell.diagnostics.debug_enabled())
        .build();
    debug_group.add(&debug);
    content.append(&debug_group);

    let sharing_note = gtk::Label::builder()
        .label(tr(
            "Logs have secrets and absolute folder paths redacted, but you may still review the logs before sharing it",
        ))
        .wrap(true)
        .xalign(0.0)
        .margin_start(12)
        .build();
    sharing_note.add_css_class("dim-label");
    sharing_note.add_css_class("caption");
    content.append(&sharing_note);

    let log = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    log.update_property(&[gtk::accessible::Property::Label(&tr("Diagnostic log"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.add_css_class("card");
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&log));
    content.append(&scroller);

    let save = gtk::Button::builder()
        .label(tr("Save"))
        .halign(gtk::Align::End)
        .build();
    save.add_css_class("suggested-action");
    content.append(&save);
    toolbar.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title(tr("Troubleshooting"))
        .content_width(large_popup_content_width(DIAGNOSTICS_DIALOG_WIDTH))
        .content_height(large_popup_content_height(
            shell.chrome.window.height(),
            DIAGNOSTICS_DIALOG_HEIGHT,
        ))
        .child(&toolbar)
        .build();

    let changing_debug = Rc::new(Cell::new(false));
    let diagnostics = shell.diagnostics.clone();
    let toast_overlay = shell.chrome.toast_overlay.clone();
    let changing_debug_for_notify = Rc::clone(&changing_debug);
    debug.connect_active_notify(move |row| {
        if changing_debug_for_notify.get() {
            return;
        }
        let enabled = row.is_active();
        if let Err(error) = diagnostics.set_debug_enabled(enabled) {
            changing_debug_for_notify.set(true);
            row.set_active(!enabled);
            changing_debug_for_notify.set(false);
            toast_overlay.add_toast(adw::Toast::new(&error));
        }
    });

    let initial = shell.diagnostics.snapshot();
    log.buffer().set_text(&initial);
    scroll_to_log_end(&scroller);
    let last_revision = Rc::new(Cell::new(shell.diagnostics.revision()));
    let weak_dialog = dialog.downgrade();
    let diagnostics = shell.diagnostics.clone();
    let refresh_log = log.clone();
    let refresh_scroller = scroller.clone();
    let refresh_revision = Rc::clone(&last_revision);
    glib::timeout_add_local(LOG_REFRESH_INTERVAL, move || {
        if weak_dialog.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        let revision = diagnostics.revision();
        if refresh_revision.replace(revision) != revision {
            let adjustment = refresh_scroller.vadjustment();
            let follows_tail =
                adjustment.value() + adjustment.page_size() >= adjustment.upper() - 24.0;
            refresh_log.buffer().set_text(&diagnostics.snapshot());
            if follows_tail {
                scroll_to_log_end(&refresh_scroller);
            }
        }
        glib::ControlFlow::Continue
    });

    let save_window = shell.chrome.window.clone();
    let save_buffer = log.buffer();
    let save_toasts = shell.chrome.toast_overlay.clone();
    save.connect_clicked(move |_| {
        let window = save_window.clone();
        let buffer = save_buffer.clone();
        let toasts = save_toasts.clone();
        glib::spawn_future_local(async move {
            let chooser = gtk::FileDialog::builder()
                .title(tr("Save Diagnostic Log"))
                .initial_name("rufin-debug.log")
                .build();
            let Ok(file) = chooser.save_future(Some(&window)).await else {
                return;
            };
            let contents = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .as_bytes()
                .to_vec();
            match file
                .replace_contents_future(
                    contents,
                    None,
                    false,
                    gio::FileCreateFlags::REPLACE_DESTINATION,
                )
                .await
            {
                Ok(_) => toasts.add_toast(adw::Toast::new(&tr("Diagnostic log saved"))),
                Err((_, error)) => {
                    toasts.add_toast(adw::Toast::new(&format!(
                        "{}: {error}",
                        tr("Could not save diagnostic log")
                    )));
                }
            }
        });
    });

    present_light_dismiss_dialog(&dialog, &shell.chrome.window);
}

fn scroll_to_log_end(scroller: &gtk::ScrolledWindow) {
    let adjustment = scroller.vadjustment();
    adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
}
