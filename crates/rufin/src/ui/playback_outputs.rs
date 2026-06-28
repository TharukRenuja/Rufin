use super::*;
use playback::{AudioOutput, available_audio_outputs};
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::Duration;

type AudioOutputSelected = Rc<dyn Fn(Option<String>, String)>;

pub(in crate::ui) fn selected_audio_output_title(selected: Option<&str>) -> String {
    selected
        .and_then(static_audio_output_title)
        .unwrap_or_else(|| {
            if selected.is_some() {
                tr("Selected device")
            } else {
                tr("System default")
            }
        })
}

pub(in crate::ui) fn present_audio_output_popover(
    anchor: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    position: gtk::PositionType,
    on_selected: Option<AudioOutputSelected>,
) {
    let selected = shell.state.settings.borrow().playback.audio_output.clone();
    let popover = gtk::Popover::new();
    popover.add_css_class("audio-output-popover");
    popover.set_autohide(true);
    popover.set_has_arrow(false);
    popover.set_position(position);
    popover.set_parent(anchor);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    list.set_margin_top(4);
    list.set_margin_bottom(4);
    list.set_margin_start(0);
    list.set_margin_end(0);
    list.set_width_request(236);
    list.append(&audio_output_status_row(&tr("Loading...")));
    popover.set_child(Some(&list));
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();

    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _sent = sender.send(available_audio_outputs());
    });

    let popover_for_result = popover.clone();
    let list_for_result = list.clone();
    let shell_for_result = Rc::clone(shell);
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        if !popover_for_result.is_visible() {
            return gtk::glib::ControlFlow::Break;
        }
        match receiver.try_recv() {
            Ok(outputs) => {
                populate_audio_output_rows(
                    &list_for_result,
                    &popover_for_result,
                    &shell_for_result,
                    selected.as_deref(),
                    playback_output_options(outputs),
                    on_selected.as_ref(),
                );
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                replace_audio_output_rows(
                    &list_for_result,
                    [audio_output_status_row(&tr("No audio outputs found"))],
                );
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn playback_output_options(discovered: Vec<AudioOutput>) -> Vec<(Option<String>, String)> {
    let mut outputs = vec![(None, tr("System default"))];
    outputs.extend(
        discovered
            .into_iter()
            .filter(|output| output.id != "autoaudiosink")
            .map(|output| (Some(output.id), output.name)),
    );
    outputs
}

fn audio_output_index(outputs: &[(Option<String>, String)], selected: Option<&str>) -> u32 {
    outputs
        .iter()
        .position(|(id, _)| id.as_deref() == selected)
        .unwrap_or_default() as u32
}

fn static_audio_output_title(id: &str) -> Option<String> {
    Some(match id {
        "autoaudiosink" => tr("System default"),
        "pipewiresink" => tr("PipeWire"),
        "pulsesink" => tr("PulseAudio"),
        "alsasink" => tr("ALSA"),
        "jackaudiosink" => tr("JACK"),
        "osxaudiosink" => tr("macOS"),
        "wasapisink" => tr("WASAPI"),
        "directsoundsink" => tr("DirectSound"),
        _ => return None,
    })
}

fn populate_audio_output_rows(
    list: &gtk::Box,
    popover: &gtk::Popover,
    shell: &Rc<Shell>,
    selected: Option<&str>,
    outputs: Vec<(Option<String>, String)>,
    on_selected: Option<&AudioOutputSelected>,
) {
    let selected_index = audio_output_index(&outputs, selected) as usize;
    let rows = outputs
        .into_iter()
        .enumerate()
        .map(|(index, (id, title))| {
            audio_output_row(
                popover,
                shell,
                index == selected_index,
                id,
                title,
                on_selected,
            )
        })
        .collect::<Vec<_>>();
    replace_audio_output_rows(list, rows);
}

fn replace_audio_output_rows(
    list: &gtk::Box,
    rows: impl IntoIterator<Item = impl IsA<gtk::Widget>>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for row in rows {
        list.append(row.as_ref());
    }
}

fn audio_output_status_row(label: &str) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("audio-output-row");
    row.set_sensitive(false);
    row.set_halign(gtk::Align::Fill);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_halign(gtk::Align::Fill);
    content.set_valign(gtk::Align::Center);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    content.append(&label);
    row.set_child(Some(&content));
    row
}

fn audio_output_row(
    popover: &gtk::Popover,
    shell: &Rc<Shell>,
    active: bool,
    id: Option<String>,
    title: String,
    on_selected: Option<&AudioOutputSelected>,
) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("audio-output-row");
    row.set_halign(gtk::Align::Fill);
    row.set_tooltip_text(Some(&title));

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_halign(gtk::Align::Fill);
    content.set_valign(gtk::Align::Center);
    let check = gtk::Image::from_icon_name("object-select-symbolic");
    check.set_pixel_size(16);
    check.set_size_request(16, 16);
    check.set_opacity(if active { 1.0 } else { 0.0 });
    content.append(&check);
    let label = gtk::Label::new(Some(&title));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_max_width_chars(30);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&label);
    row.set_child(Some(&content));

    let row_shell = Rc::clone(shell);
    let row_popover = popover.clone();
    let on_selected = on_selected.cloned();
    row.connect_clicked(move |_| {
        let selected = id.clone();
        row_shell.update_playback_settings(|settings| {
            settings.audio_output = selected.clone();
        });
        if let Some(on_selected) = on_selected.as_ref() {
            on_selected(selected, title.clone());
        }
        row_popover.popdown();
    });
    row
}
