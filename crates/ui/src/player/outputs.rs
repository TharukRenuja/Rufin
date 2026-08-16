use crate::shell::Shell;
use gtk::prelude::*;
use localization::tr;
use playback::AudioOutput;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::{Duration, Instant};

type AudioOutputOptions = Vec<(Option<String>, String)>;
const AUDIO_OUTPUT_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn default_audio_output_options() -> AudioOutputOptions {
    vec![(None, tr("System default"))]
}

pub(crate) fn warm_audio_output_cache(shell: &Rc<Shell>) {
    request_audio_output_refresh(shell);
}

pub(crate) fn selected_audio_output_title(shell: &Rc<Shell>, selected: Option<&str>) -> String {
    selected
        .and_then(|selected| {
            shell
                .playback
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

pub(crate) fn audio_output_dropdown(shell: &Rc<Shell>, width: i32) -> gtk::DropDown {
    let selected = shell
        .settings
        .current
        .borrow()
        .playback
        .audio_output
        .clone();
    let selected = Rc::new(RefCell::new(selected));
    let options = Rc::new(RefCell::new(Vec::new()));
    let syncing = Rc::new(Cell::new(false));
    let dropdown = gtk::DropDown::from_strings(&[]);
    dropdown.add_css_class("audio-output-dropdown");
    dropdown.set_valign(gtk::Align::Center);
    dropdown.set_width_request(width);
    refresh_audio_output_dropdown(&dropdown, shell, &options, &selected, &syncing);

    let output_shell = Rc::clone(shell);
    let output_options = Rc::clone(&options);
    let output_selected = Rc::clone(&selected);
    let output_syncing = Rc::clone(&syncing);
    dropdown.connect_selected_notify(move |dropdown| {
        if output_syncing.get() {
            return;
        }
        let Some((id, title)) = output_options
            .borrow()
            .get(dropdown.selected() as usize)
            .cloned()
        else {
            return;
        };
        *output_selected.borrow_mut() = id.clone();
        dropdown.set_tooltip_text(Some(&title));
        output_shell.update_playback_settings(|settings| settings.audio_output = id);
    });

    let seen_generation = shell.playback.audio_output_refresh_generation.get();
    request_audio_output_refresh(shell);
    watch_audio_output_dropdown_refresh(
        shell,
        &dropdown,
        seen_generation,
        options,
        selected,
        syncing,
    );
    dropdown
}

fn refresh_audio_output_dropdown(
    dropdown: &gtk::DropDown,
    shell: &Rc<Shell>,
    options: &Rc<RefCell<AudioOutputOptions>>,
    selected: &Rc<RefCell<Option<String>>>,
    syncing: &Rc<Cell<bool>>,
) {
    let selected_id = selected.borrow().clone();
    let selected_title = selected_audio_output_title(shell, selected_id.as_deref());
    let cached_options = shell.playback.audio_output_options.borrow().clone();
    let shown =
        include_selected_audio_output(cached_options, selected_id.as_deref(), selected_title);
    let selected_index = audio_output_index(&shown, selected_id.as_deref()).unwrap_or_default();
    let titles = shown
        .iter()
        .map(|(_, title)| title.as_str())
        .collect::<Vec<_>>();
    let model = gtk::StringList::new(&titles);

    syncing.set(true);
    dropdown.set_model(Some(&model));
    dropdown.set_selected(selected_index as u32);
    dropdown.set_tooltip_text(shown.get(selected_index).map(|(_, title)| title.as_str()));
    syncing.set(false);
    *options.borrow_mut() = shown;
}

fn request_audio_output_refresh(shell: &Rc<Shell>) {
    if shell.playback.audio_output_refresh_running.get() {
        return;
    }
    if shell
        .playback
        .audio_output_refreshed_at
        .get()
        .is_some_and(|refreshed_at| refreshed_at.elapsed() < AUDIO_OUTPUT_REFRESH_INTERVAL)
    {
        return;
    }
    shell.playback.audio_output_refresh_running.set(true);

    let (sender, receiver) = std::sync::mpsc::channel();
    let transport = shell.products.playback.transport.clone();
    thread::spawn(move || {
        let _sent = sender.send(transport.available_audio_outputs());
    });

    let shell = Rc::clone(shell);
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(outputs) => {
                let options = playback_output_options(outputs);
                let mut cached = shell.playback.audio_output_options.borrow_mut();
                if *cached != options {
                    *cached = options;
                    shell
                        .playback
                        .audio_output_refresh_generation
                        .set(shell.playback.audio_output_refresh_generation.get() + 1);
                }
                shell.playback.audio_output_refresh_running.set(false);
                shell
                    .playback
                    .audio_output_refreshed_at
                    .set(Some(Instant::now()));
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                shell.playback.audio_output_refresh_running.set(false);
                shell
                    .playback
                    .audio_output_refreshed_at
                    .set(Some(Instant::now()));
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn watch_audio_output_dropdown_refresh(
    shell: &Rc<Shell>,
    dropdown: &gtk::DropDown,
    seen_generation: u64,
    options: Rc<RefCell<AudioOutputOptions>>,
    selected: Rc<RefCell<Option<String>>>,
    syncing: Rc<Cell<bool>>,
) {
    let dropdown = dropdown.downgrade();
    let shell = Rc::clone(shell);
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        let Some(dropdown) = dropdown.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        let current_generation = shell.playback.audio_output_refresh_generation.get();
        if current_generation != seen_generation {
            refresh_audio_output_dropdown(&dropdown, &shell, &options, &selected, &syncing);
            return gtk::glib::ControlFlow::Break;
        }
        if shell.playback.audio_output_refresh_running.get() {
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

fn include_selected_audio_output(
    mut options: AudioOutputOptions,
    selected: Option<&str>,
    selected_title: String,
) -> AudioOutputOptions {
    let selected = selected.filter(|id| *id != "autoaudiosink");
    if let Some(selected) = selected
        && audio_output_index(&options, Some(selected)).is_none()
    {
        options.push((Some(selected.to_string()), selected_title));
    }
    options
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

#[cfg(test)]
mod tests {
    use super::{audio_output_index, default_audio_output_options, include_selected_audio_output};

    #[test]
    fn unavailable_selected_output_does_not_mark_system_default_active() {
        let outputs = default_audio_output_options();

        assert_eq!(audio_output_index(&outputs, None), Some(0));
        assert_eq!(audio_output_index(&outputs, Some("gst-device:gone")), None);
    }

    #[test]
    fn unavailable_selected_output_remains_selectable() {
        let outputs = include_selected_audio_output(
            default_audio_output_options(),
            Some("gst-device:gone"),
            "Selected device".to_string(),
        );

        assert_eq!(audio_output_index(&outputs, None), Some(0));
        assert_eq!(
            audio_output_index(&outputs, Some("gst-device:gone")),
            Some(1)
        );
    }
}
