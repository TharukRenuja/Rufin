use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use domain::{EQUALIZER_BAND_COUNT, EqualizerSettings};
use gtk::glib;
use gtk::prelude::*;

use crate::i18n::tr;

const EQUALIZER_FALLBACK_COMMIT_DELAY_MS: u64 = 1_200;
const EQUALIZER_PRESET_MENU_HEIGHT: i32 = 160;
const EQUALIZER_SURFACE_SCROLL_FACTOR: f64 = 2.5;
const CUSTOM_PRESET: &str = "Custom";

pub(in crate::ui) struct EqualizerPresetDropdown {
    pub(in crate::ui) button: gtk::MenuButton,
    pub(in crate::ui) popover: gtk::Popover,
    pub(in crate::ui) buttons: Vec<(gtk::Button, String)>,
}

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

pub(in crate::ui) fn equalizer_preset_bands(name: &str) -> Vec<f64> {
    equalizer_default_preset_bands(name)
}

pub(in crate::ui) fn equalizer_preset_button_label(button: &gtk::MenuButton, preset: &str) {
    button.set_label(&tr(preset));
}

pub(in crate::ui) fn build_equalizer_preset_dropdown(
    menu_css_class: Option<&str>,
) -> EqualizerPresetDropdown {
    let button = gtk::MenuButton::new();
    equalizer_preset_button_label(&button, CUSTOM_PRESET);
    button.set_valign(gtk::Align::Center);

    let popover = gtk::Popover::new();
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_width(true);
    scroller.set_propagate_natural_height(false);
    scroller.set_min_content_width(180);
    scroller.set_min_content_height(EQUALIZER_PRESET_MENU_HEIGHT);
    scroller.set_max_content_height(EQUALIZER_PRESET_MENU_HEIGHT);
    scroller.set_height_request(EQUALIZER_PRESET_MENU_HEIGHT);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu.add_css_class("equalizer-preset-menu");
    if let Some(css_class) = menu_css_class {
        menu.add_css_class(css_class);
    }

    let mut buttons = Vec::new();
    for name in equalizer_preset_names() {
        let item = gtk::Button::with_label(&tr(name));
        item.set_halign(gtk::Align::Fill);
        item.set_valign(gtk::Align::Center);
        item.add_css_class("flat");
        menu.append(&item);
        buttons.push((item, name.to_string()));
    }

    scroller.set_child(Some(&menu));
    popover.set_child(Some(&scroller));
    button.set_popover(Some(&popover));

    EqualizerPresetDropdown {
        button,
        popover,
        buttons,
    }
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

pub(in crate::ui) fn install_equalizer_scroll(scale: &gtk::Scale) {
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
    use super::*;

    #[test]
    fn equalizer_presets_cover_all_bands() {
        for (_, bands) in equalizer_presets() {
            assert_eq!(bands.len(), EQUALIZER_BAND_COUNT);
            assert!(bands.iter().all(|gain| (-12.0..=12.0).contains(gain)));
        }
    }
}
