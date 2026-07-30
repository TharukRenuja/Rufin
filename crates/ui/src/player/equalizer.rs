use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use playback::{EQUALIZER_BAND_COUNT, EqualizerSettings};

use localization::tr;

const EQUALIZER_FALLBACK_COMMIT_DELAY_MS: u64 = 1_200;
const EQUALIZER_SURFACE_SCROLL_FACTOR: f64 = 2.5;
const CUSTOM_PRESET: &str = "Custom";

pub(crate) fn equalizer_band_title(index: usize) -> String {
    const BANDS: [&str; EQUALIZER_BAND_COUNT] = [
        "60 Hz", "170 Hz", "310 Hz", "600 Hz", "1 kHz", "3 kHz", "6 kHz", "12 kHz", "14 kHz",
        "16 kHz",
    ];
    BANDS.get(index).copied().unwrap_or("Band").to_string()
}

pub(crate) fn equalizer_band_label_parts(index: usize) -> (String, String) {
    let title = equalizer_band_title(index);
    title
        .split_once(' ')
        .map(|(value, unit)| (value.to_string(), unit.to_string()))
        .unwrap_or_else(|| (title, String::new()))
}

pub(crate) fn equalizer_presets() -> Vec<(&'static str, Vec<f64>)> {
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

fn equalizer_preset_names() -> Vec<&'static str> {
    std::iter::once(CUSTOM_PRESET)
        .chain(equalizer_presets().iter().map(|(name, _)| *name))
        .collect()
}

pub(crate) fn equalizer_selected_preset(equalizer: &EqualizerSettings) -> String {
    if equalizer_preset_names()
        .iter()
        .any(|name| *name == equalizer.selected_preset)
    {
        equalizer.selected_preset.clone()
    } else {
        CUSTOM_PRESET.to_string()
    }
}

pub(crate) fn equalizer_preset_position(name: &str) -> u32 {
    equalizer_preset_names()
        .iter()
        .position(|preset| *preset == name)
        .unwrap_or_default() as u32
}

pub(crate) fn equalizer_preset_name_at(position: u32) -> Option<String> {
    equalizer_preset_names()
        .get(position as usize)
        .map(|name| (*name).to_string())
}

fn equalizer_preset_title(name: &str) -> String {
    match name {
        "Custom" => tr("Custom"),
        "Flat" => tr("Flat"),
        "Classical" => tr("Classical"),
        "Club" => tr("Club"),
        "Dance" => tr("Dance"),
        "Full Bass" => tr("Full Bass"),
        "Full Treble" => tr("Full Treble"),
        "Laptop/Headphones" => tr("Laptop/Headphones"),
        "Rock" => tr("Rock"),
        "Pop" => tr("Pop"),
        "Techno" => tr("Techno"),
        _ => name.to_string(),
    }
}

pub(crate) fn equalizer_default_preset_bands(name: &str) -> Vec<f64> {
    if name == CUSTOM_PRESET {
        return vec![0.0; EQUALIZER_BAND_COUNT];
    }
    equalizer_presets()
        .into_iter()
        .find_map(|(preset, bands)| (preset == name).then_some(bands))
        .unwrap_or_else(|| vec![0.0; EQUALIZER_BAND_COUNT])
}

pub(crate) fn equalizer_preset_bands(name: &str) -> Vec<f64> {
    equalizer_default_preset_bands(name)
}

fn equalizer_preset_model() -> gtk::StringList {
    let titles = equalizer_preset_names()
        .into_iter()
        .map(equalizer_preset_title)
        .collect::<Vec<_>>();
    let title_refs = titles.iter().map(String::as_str).collect::<Vec<_>>();
    gtk::StringList::new(&title_refs)
}

pub(crate) fn build_equalizer_preset_row(title: &str, selected: u32) -> adw::ComboRow {
    let model = equalizer_preset_model();
    adw::ComboRow::builder()
        .title(tr(title))
        .model(&model)
        .selected(selected)
        .build()
}

pub(crate) fn build_equalizer_preset_dropdown(selected: u32) -> gtk::DropDown {
    let model = equalizer_preset_model();
    gtk::DropDown::builder()
        .model(&model)
        .selected(selected)
        .build()
}

pub(crate) fn relocalize_equalizer_preset_dropdown(dropdown: &gtk::DropDown, selected: u32) {
    let model = equalizer_preset_model();
    dropdown.set_model(Some(&model));
    dropdown.set_selected(selected);
}

pub(crate) fn connect_equalizer_scale_commit(
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

    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pending_for_press = Rc::clone(&pending_update);
    let pointer_for_press = Rc::clone(&pointer_active);
    let changed_for_press = Rc::clone(&changed);
    gesture.connect_pressed(move |_, _, _, _| {
        pointer_for_press.set(true);
        changed_for_press.set(false);
        if let Some(source_id) = pending_for_press.borrow_mut().take() {
            source_id.remove();
        }
    });

    let finish_pointer_commit = {
        let guard = Rc::clone(&guard);
        let pending_update = Rc::clone(&pending_update);
        let pointer_active = Rc::clone(&pointer_active);
        let changed = Rc::clone(&changed);
        let commit = Rc::clone(&commit);
        Rc::new(move || {
            pointer_active.set(false);
            if let Some(source_id) = pending_update.borrow_mut().take() {
                source_id.remove();
            }
            if !guard.get() && changed.replace(false) {
                let commit_for_idle = Rc::clone(&commit);
                glib::idle_add_local_once(move || commit_for_idle());
            }
        })
    };
    let finish_for_release = Rc::clone(&finish_pointer_commit);
    gesture.connect_released(move |_, _, _, _| finish_for_release());
    gesture.connect_cancel(move |_, _| finish_pointer_commit());
    scale.add_controller(gesture);
}

pub(crate) fn install_equalizer_scroll(scale: &gtk::Scale) {
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scale_weak = scale.downgrade();
    controller.connect_scroll(move |controller, _, dy| {
        if dy == 0.0 {
            return gtk::glib::Propagation::Proceed;
        }

        let Some(scale) = scale_weak.upgrade() else {
            return gtk::glib::Propagation::Stop;
        };
        let scale_widget = scale.upcast::<gtk::Widget>();
        scroll_parent_vertically(&scale_widget, dy, controller.unit());
        gtk::glib::Propagation::Stop
    });
    scale.add_controller(controller);
}

fn scroll_parent_vertically(widget: &gtk::Widget, dy: f64, unit: gtk::gdk::ScrollUnit) {
    let Some(scroller) = nearest_parent_scroller(widget) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let page_size = adjustment.page_size();
    let multiplier = match unit {
        gtk::gdk::ScrollUnit::Surface => EQUALIZER_SURFACE_SCROLL_FACTOR,
        _ => page_size.powf(2.0 / 3.0),
    };
    let max_value = (adjustment.upper() - page_size).max(adjustment.lower());
    let value = (adjustment.value() + dy * multiplier).clamp(adjustment.lower(), max_value);
    adjustment.set_value(value);
}

fn nearest_parent_scroller(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut parent = widget.parent();
    while let Some(widget) = parent {
        if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
            return Some(scroller);
        }
        parent = widget.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{EQUALIZER_BAND_COUNT, equalizer_presets};

    #[test]
    fn equalizer_presets_cover_all_bands() {
        for (_, bands) in equalizer_presets() {
            assert_eq!(bands.len(), EQUALIZER_BAND_COUNT);
            assert!(bands.iter().all(|gain| (-12.0..=12.0).contains(gain)));
        }
    }
}
