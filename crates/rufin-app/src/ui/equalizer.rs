use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use rufin_core::{EQUALIZER_BAND_COUNT, EqualizerSettings};

const EQUALIZER_FALLBACK_COMMIT_DELAY_MS: u64 = 1_200;
const CUSTOM_PRESET: &str = "Custom";

pub(in crate::ui) fn equalizer_band_title(index: usize) -> String {
    const BANDS: [&str; EQUALIZER_BAND_COUNT] = [
        "60 Hz", "170 Hz", "310 Hz", "600 Hz", "1 kHz", "3 kHz", "6 kHz", "12 kHz", "14 kHz",
        "16 kHz",
    ];
    BANDS.get(index).copied().unwrap_or("Band").to_string()
}

pub(in crate::ui) fn equalizer_band_label_parts(index: usize) -> (String, String) {
    let title = equalizer_band_title(index);
    title
        .split_once(' ')
        .map(|(value, unit)| (value.to_string(), unit.to_string()))
        .unwrap_or_else(|| (title, String::new()))
}

pub(in crate::ui) fn equalizer_presets() -> Vec<(&'static str, Vec<f64>)> {
    vec![
        ("Flat", vec![0.0; EQUALIZER_BAND_COUNT]),
        (
            "Classical",
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -7.2, -7.2, -7.2, -9.6],
        ),
        (
            "Club",
            vec![0.0, 0.0, 3.2, 5.6, 5.6, 5.6, 3.2, 0.0, 0.0, 0.0],
        ),
        (
            "Dance",
            vec![9.6, 7.2, 2.4, 0.0, 0.0, -5.6, -7.2, -7.2, 0.0, 0.0],
        ),
        (
            "Full Bass",
            vec![9.6, 9.6, 9.6, 5.6, 1.6, -4.0, -8.0, -10.4, -11.2, -11.2],
        ),
        (
            "Full Treble",
            vec![-9.6, -9.6, -9.6, -4.0, 2.4, 11.2, 12.0, 12.0, 12.0, 12.0],
        ),
        (
            "Laptop/Headphones",
            vec![4.8, 11.2, 5.6, -3.2, -2.4, 1.6, 4.8, 9.6, 12.0, 12.0],
        ),
        (
            "Rock",
            vec![8.0, 4.8, -5.6, -8.0, -3.2, 4.0, 8.8, 11.2, 11.2, 11.2],
        ),
        (
            "Pop",
            vec![-1.6, 4.8, 7.2, 8.0, 5.6, 0.0, -2.4, -2.4, -1.6, -1.6],
        ),
        (
            "Techno",
            vec![8.0, 5.6, 0.0, -5.6, -4.8, 0.0, 8.0, 9.6, 9.6, 8.8],
        ),
    ]
}

pub(in crate::ui) fn equalizer_preset_names() -> Vec<&'static str> {
    std::iter::once(CUSTOM_PRESET)
        .chain(equalizer_presets().iter().map(|(name, _)| *name))
        .collect()
}

pub(in crate::ui) fn equalizer_selected_preset(equalizer: &EqualizerSettings) -> String {
    if equalizer_preset_names()
        .iter()
        .any(|name| *name == equalizer.selected_preset)
    {
        equalizer.selected_preset.clone()
    } else {
        CUSTOM_PRESET.to_string()
    }
}

pub(in crate::ui) fn equalizer_preset_position(name: &str) -> u32 {
    equalizer_preset_names()
        .iter()
        .position(|preset| *preset == name)
        .unwrap_or_default() as u32
}

pub(in crate::ui) fn equalizer_preset_name_at(position: u32) -> Option<String> {
    equalizer_preset_names()
        .get(position as usize)
        .map(|name| (*name).to_string())
}

pub(in crate::ui) fn equalizer_default_preset_bands(name: &str) -> Vec<f64> {
    if name == CUSTOM_PRESET {
        return vec![0.0; EQUALIZER_BAND_COUNT];
    }
    equalizer_presets()
        .into_iter()
        .find_map(|(preset, bands)| (preset == name).then_some(bands))
        .unwrap_or_else(|| vec![0.0; EQUALIZER_BAND_COUNT])
}

pub(in crate::ui) fn equalizer_preset_bands(equalizer: &EqualizerSettings, name: &str) -> Vec<f64> {
    equalizer
        .preset_overrides
        .get(name)
        .cloned()
        .unwrap_or_else(|| equalizer_default_preset_bands(name))
}

pub(in crate::ui) fn connect_equalizer_scale_commit(
    scale: &gtk::Scale,
    guard: Rc<Cell<bool>>,
    pending_update: Rc<RefCell<Option<glib::SourceId>>>,
    pointer_active: Rc<Cell<bool>>,
    commit: Rc<dyn Fn()>,
) {
    let changed = Rc::new(Cell::new(false));

    let guard_for_change = Rc::clone(&guard);
    let pending_for_change = Rc::clone(&pending_update);
    let pointer_for_change = Rc::clone(&pointer_active);
    let changed_for_change = Rc::clone(&changed);
    let commit_for_change = Rc::clone(&commit);
    scale.connect_value_changed(move |_| {
        if guard_for_change.get() {
            return;
        }
        changed_for_change.set(true);
        if let Some(source_id) = pending_for_change.borrow_mut().take() {
            source_id.remove();
        }
        if pointer_for_change.get() {
            return;
        }
        let pending_for_timeout = Rc::clone(&pending_for_change);
        let changed_for_timeout = Rc::clone(&changed_for_change);
        let commit_for_timeout = Rc::clone(&commit_for_change);
        let source_id = glib::timeout_add_local_once(
            Duration::from_millis(EQUALIZER_FALLBACK_COMMIT_DELAY_MS),
            move || {
                *pending_for_timeout.borrow_mut() = None;
                if changed_for_timeout.replace(false) {
                    commit_for_timeout();
                }
            },
        );
        *pending_for_change.borrow_mut() = Some(source_id);
    });

    let controller = gtk::EventControllerLegacy::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let guard_for_event = Rc::clone(&guard);
    let pending_for_event = Rc::clone(&pending_update);
    let pointer_for_event = Rc::clone(&pointer_active);
    let changed_for_event = Rc::clone(&changed);
    let commit_for_event = Rc::clone(&commit);
    controller.connect_event(move |_, event| {
        match event.event_type() {
            gtk::gdk::EventType::ButtonPress => {
                pointer_for_event.set(true);
                changed_for_event.set(false);
                if let Some(source_id) = pending_for_event.borrow_mut().take() {
                    source_id.remove();
                }
            }
            gtk::gdk::EventType::ButtonRelease => {
                pointer_for_event.set(false);
                if let Some(source_id) = pending_for_event.borrow_mut().take() {
                    source_id.remove();
                }
                if !guard_for_event.get() && changed_for_event.replace(false) {
                    let commit_for_idle = Rc::clone(&commit_for_event);
                    glib::idle_add_local_once(move || commit_for_idle());
                }
            }
            _ => {}
        }
        glib::Propagation::Proceed
    });
    scale.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equalizer_presets_cover_all_bands() {
        for (_, bands) in equalizer_presets() {
            assert_eq!(bands.len(), EQUALIZER_BAND_COUNT);
            assert!(bands.iter().all(|gain| (-12.0..=12.0).contains(gain)));
        }
    }
}
