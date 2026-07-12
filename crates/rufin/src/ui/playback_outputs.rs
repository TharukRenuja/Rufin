use super::*;
use playback::AudioOutput;
use playback_gstreamer::available_audio_outputs;
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::{Duration, Instant};

type AudioOutputOptions = Vec<(Option<String>, String)>;
type AudioOutputSelected = Rc<dyn Fn(Option<String>, String)>;
const AUDIO_OUTPUT_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

pub(in crate::ui) fn default_audio_output_options() -> AudioOutputOptions {
    vec![(None, tr("System default"))]
}

pub(in crate::ui) fn warm_audio_output_cache(shell: &Rc<Shell>) {
    request_audio_output_refresh(shell);
}

pub(in crate::ui) fn selected_audio_output_title(
    shell: &Rc<Shell>,
    selected: Option<&str>,
) -> String {
    selected
        .and_then(|selected| {
            shell
                .state
                .audio_output_options
                .borrow()
                .iter()
                .find(|(id, _)| id.as_deref() == Some(selected))
                .map(|(_, title)| title.clone())
        })
        .or_else(|| selected.and_then(static_audio_output_title))
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
    let options = shell.state.audio_output_options.borrow().clone();
    let shown_options = Rc::new(RefCell::new(options.clone()));

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
    populate_audio_output_rows(
        &list,
        &popover,
        shell,
        selected.as_deref(),
        options,
        on_selected.as_ref(),
    );
    popover.set_child(Some(&list));
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();

    let seen_generation = shell.state.audio_output_refresh_generation.get();
    request_audio_output_refresh(shell);
    watch_audio_output_refresh(
        shell,
        &popover,
        &list,
        selected,
        seen_generation,
        shown_options,
        on_selected,
    );
}

fn request_audio_output_refresh(shell: &Rc<Shell>) {
    if shell.state.audio_output_refresh_running.get() {
        return;
    }
    if shell
        .state
        .audio_output_refreshed_at
        .get()
        .is_some_and(|refreshed_at| refreshed_at.elapsed() < AUDIO_OUTPUT_REFRESH_INTERVAL)
    {
        return;
    }
    shell.state.audio_output_refresh_running.set(true);

    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _sent = sender.send(available_audio_outputs());
    });

    let shell = Rc::clone(shell);
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(outputs) => {
                let options = playback_output_options(outputs);
                let mut cached = shell.state.audio_output_options.borrow_mut();
                if *cached != options {
                    *cached = options;
                    shell
                        .state
                        .audio_output_refresh_generation
                        .set(shell.state.audio_output_refresh_generation.get() + 1);
                }
                shell.state.audio_output_refresh_running.set(false);
                shell
                    .state
                    .audio_output_refreshed_at
                    .set(Some(Instant::now()));
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                shell.state.audio_output_refresh_running.set(false);
                shell
                    .state
                    .audio_output_refreshed_at
                    .set(Some(Instant::now()));
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn watch_audio_output_refresh(
    shell: &Rc<Shell>,
    popover: &gtk::Popover,
    list: &gtk::Box,
    selected: Option<String>,
    seen_generation: u64,
    shown_options: Rc<RefCell<AudioOutputOptions>>,
    on_selected: Option<AudioOutputSelected>,
) {
    let popover = popover.clone();
    let list = list.clone();
    let shell = Rc::clone(shell);
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        if !popover.is_visible() {
            return gtk::glib::ControlFlow::Break;
        }
        let current_generation = shell.state.audio_output_refresh_generation.get();
        if current_generation != seen_generation {
            let options = shell.state.audio_output_options.borrow().clone();
            if *shown_options.borrow() != options {
                populate_audio_output_rows(
                    &list,
                    &popover,
                    &shell,
                    selected.as_deref(),
                    options.clone(),
                    on_selected.as_ref(),
                );
                *shown_options.borrow_mut() = options;
            }
            return gtk::glib::ControlFlow::Break;
        }
        if shell.state.audio_output_refresh_running.get() {
            gtk::glib::ControlFlow::Continue
        } else {
            gtk::glib::ControlFlow::Break
        }
    });
}

fn playback_output_options(discovered: Vec<AudioOutput>) -> AudioOutputOptions {
    let mut outputs = default_audio_output_options();
    outputs.extend(
        discovered
            .into_iter()
            .filter(|output| output.id != "autoaudiosink")
            .map(|output| (Some(output.id), output.name)),
    );
    outputs
}

fn audio_output_index(
    outputs: &[(Option<String>, String)],
    selected: Option<&str>,
) -> Option<usize> {
    outputs.iter().position(|(id, _)| id.as_deref() == selected)
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
    options: AudioOutputOptions,
    on_selected: Option<&AudioOutputSelected>,
) {
    let selected_index = audio_output_index(&options, selected);
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for (index, (id, title)) in options.into_iter().enumerate() {
        list.append(&audio_output_row(
            popover,
            shell,
            Some(index) == selected_index,
            id,
            title,
            on_selected,
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_selected_output_does_not_mark_system_default_active() {
        let outputs = default_audio_output_options();

        assert_eq!(audio_output_index(&outputs, None), Some(0));
        assert_eq!(audio_output_index(&outputs, Some("gst-device:gone")), None);
    }
}
