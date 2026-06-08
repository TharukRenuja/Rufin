use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use rufin_core::RepeatMode;

const TRANSPORT_ICON_SIZE: i32 = 23;
const QUEUE_ICON_SIZE: i32 = 16;

pub(super) fn set_repeat_button_icon(button: &gtk::Button, repeat_mode: RepeatMode) {
    button.set_child(Some(&repeat_icon_area(repeat_mode)));
}

fn set_icon_source(area: &gtk::DrawingArea, context: &gtk::cairo::Context) {
    let color = area.color();
    context.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
}

fn drawing_icon_button(label: &str, icon: gtk::DrawingArea) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&crate::i18n::tr(label)));
    button.set_child(Some(&icon));
    button
}

pub(super) fn skip_icon_button(forward: bool, label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        let width = f64::from(width);
        let height = f64::from(height);
        let center_y = height / 2.0;
        let top = center_y - height * 0.28;
        let bottom = center_y + height * 0.28;
        let bar_width = (width * 0.12).clamp(2.1, 2.8);
        if forward {
            context.move_to(width * 0.30, top);
            context.line_to(width * 0.30, bottom);
            context.line_to(width * 0.70, center_y);
            context.close_path();
            let _ = context.fill();
            context.rectangle(width * 0.76, top, bar_width, bottom - top);
            let _ = context.fill();
        } else {
            context.rectangle(width * 0.20, top, bar_width, bottom - top);
            let _ = context.fill();
            context.move_to(width * 0.70, top);
            context.line_to(width * 0.70, bottom);
            context.line_to(width * 0.30, center_y);
            context.close_path();
            let _ = context.fill();
        }
    });
    drawing_icon_button(label, icon)
}

pub(super) fn play_icon_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let playing = Rc::new(Cell::new(false));
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    let icon_playing = Rc::clone(&playing);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        let width = f64::from(width);
        let height = f64::from(height);
        if icon_playing.get() {
            let bar_width = width * 0.15;
            let bar_height = height * 0.48;
            let y = (height - bar_height) / 2.0;
            context.rectangle(width * 0.34, y, bar_width, bar_height);
            context.rectangle(width * 0.56, y, bar_width, bar_height);
            let _ = context.fill();
        } else {
            context.move_to(width * 0.38, height * 0.28);
            context.line_to(width * 0.38, height * 0.72);
            context.line_to(width * 0.72, height * 0.50);
            context.close_path();
            let _ = context.fill();
        }
    });
    let button = drawing_icon_button(label, icon.clone());
    (button, icon, playing)
}

pub(super) fn shuffle_icon_button(label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.8);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let left = width * 0.22;
        let right = width * 0.72;
        let arrow = width * 0.13;
        let top_y = height * 0.34;
        let bottom_y = height * 0.66;

        context.move_to(left, top_y);
        context.curve_to(width * 0.38, top_y, width * 0.43, bottom_y, right, bottom_y);
        context.line_to(right - arrow, bottom_y - arrow * 0.75);
        context.move_to(right, bottom_y);
        context.line_to(right - arrow, bottom_y + arrow * 0.75);

        context.move_to(left, bottom_y);
        context.curve_to(width * 0.38, bottom_y, width * 0.43, top_y, right, top_y);
        context.line_to(right - arrow, top_y - arrow * 0.75);
        context.move_to(right, top_y);
        context.line_to(right - arrow, top_y + arrow * 0.75);
        let _ = context.stroke();
    });
    drawing_icon_button(label, icon)
}

pub(super) fn repeat_icon_button(label: &str) -> gtk::Button {
    let button = drawing_icon_button(label, repeat_icon_area(RepeatMode::Off));
    button.add_css_class("player-repeat-button");
    button
}

fn repeat_icon_area(repeat_mode: RepeatMode) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.9);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let center_x = width / 2.0;
        let center_y = height / 2.0;
        let radius = width.min(height) * 0.29;
        context.arc(
            center_x,
            center_y,
            radius,
            -0.15,
            std::f64::consts::PI * 1.72,
        );
        let _ = context.stroke();

        let arrow_x = center_x + radius * (-0.15_f64).cos();
        let arrow_y = center_y + radius * (-0.15_f64).sin();
        let arrow = width.min(height) * 0.13;
        context.move_to(arrow_x, arrow_y);
        context.line_to(arrow_x - arrow * 0.98, arrow_y - arrow * 0.35);
        context.move_to(arrow_x, arrow_y);
        context.line_to(arrow_x - arrow * 0.38, arrow_y + arrow);
        let _ = context.stroke();

        if repeat_mode == RepeatMode::One {
            context.set_line_width(1.35);
            let one_x = width / 2.0;
            let one_top = height * 0.40;
            let one_bottom = height * 0.66;
            context.move_to(one_x, one_top);
            context.line_to(one_x, one_bottom);
            context.move_to(one_x - 1.5, one_top + 1.0);
            context.line_to(one_x, one_top);
            let _ = context.stroke();
        }
    });
    icon
}

pub(super) fn lyrics_icon_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let open = Rc::new(Cell::new(true));
    let icon = lyrics_icon_area(Rc::clone(&open));
    let button = drawing_icon_button(label, icon.clone());
    (button, icon, open)
}

pub(super) fn lyrics_icon_area(open: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.7);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let left = width * 0.25;
        let right = width * 0.75;
        let top = height * 0.25;
        let bottom = height * 0.66;
        let radius = width * 0.09;

        context.move_to(left + radius, top);
        context.line_to(right - radius, top);
        context.curve_to(right, top, right, top, right, top + radius);
        context.line_to(right, bottom - radius);
        context.curve_to(right, bottom, right, bottom, right - radius, bottom);
        context.line_to(width * 0.45, bottom);
        context.line_to(width * 0.32, height * 0.79);
        context.line_to(width * 0.34, bottom);
        context.line_to(left + radius, bottom);
        context.curve_to(left, bottom, left, bottom, left, bottom - radius);
        context.line_to(left, top + radius);
        context.curve_to(left, top, left, top, left + radius, top);
        let _ = context.stroke();

        if open.get() {
            context.move_to(width * 0.36, height * 0.42);
            context.line_to(width * 0.64, height * 0.42);
            context.move_to(width * 0.36, height * 0.54);
            context.line_to(width * 0.58, height * 0.54);
            let _ = context.stroke();
        }
    });
    icon
}

pub(super) fn auto_dj_icon_button(label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.8);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let center_x = width / 2.0;
        let center_y = height * 0.53;
        let radius = width.min(height) * 0.29;

        context.arc(
            center_x,
            center_y,
            radius,
            std::f64::consts::PI,
            std::f64::consts::TAU,
        );
        let _ = context.stroke();

        context.rectangle(width * 0.22, height * 0.50, width * 0.13, height * 0.24);
        context.rectangle(width * 0.65, height * 0.50, width * 0.13, height * 0.24);
        let _ = context.stroke();

        context.set_line_width(1.45);
        context.move_to(width * 0.75, height * 0.21);
        context.line_to(width * 0.75, height * 0.36);
        context.move_to(width * 0.68, height * 0.285);
        context.line_to(width * 0.82, height * 0.285);
        let _ = context.stroke();
    });
    drawing_icon_button(label, icon)
}

pub(super) fn random_clover_icon_button(label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(TRANSPORT_ICON_SIZE);
    icon.set_content_height(TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        let width = f64::from(width);
        let height = f64::from(height);
        let center_x = width / 2.0;
        let center_y = height * 0.48;
        let leaf_radius = width.min(height) * 0.14;

        for (x, y) in [
            (center_x - leaf_radius, center_y - leaf_radius),
            (center_x + leaf_radius, center_y - leaf_radius),
            (center_x - leaf_radius, center_y + leaf_radius),
            (center_x + leaf_radius, center_y + leaf_radius),
        ] {
            context.arc(x, y, leaf_radius, 0.0, std::f64::consts::TAU);
            let _ = context.fill();
        }

        context.set_line_width(1.6);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.move_to(center_x + width * 0.02, center_y + leaf_radius * 1.25);
        context.curve_to(
            center_x + width * 0.12,
            height * 0.70,
            center_x + width * 0.02,
            height * 0.77,
            center_x - width * 0.10,
            height * 0.83,
        );
        let _ = context.stroke();
    });
    drawing_icon_button(label, icon)
}

pub(super) fn queue_sidebar_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    let label = crate::i18n::tr(label);
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);

    let open = Rc::new(Cell::new(true));
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(QUEUE_ICON_SIZE);
    icon.set_content_height(QUEUE_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);

    let icon_open = Rc::clone(&open);
    icon.set_draw_func(move |area, context, width, height| {
        let color = area.color();
        let set_source = |alpha: f64| {
            context.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()) * alpha,
            );
        };

        let width = f64::from(width);
        let height = f64::from(height);
        let x = (width - 14.0) / 2.0;
        let y = (height - 12.0) / 2.0;
        let icon_width = 14.0;
        let icon_height = 12.0;
        let separator_x = x + icon_width - 4.5;
        let center_y = y + icon_height / 2.0;

        if icon_open.get() {
            set_source(0.32);
            context.rectangle(separator_x, y, icon_width - (separator_x - x), icon_height);
            let _ = context.fill();
        }

        set_source(1.0);
        context.set_line_width(1.4);
        context.rectangle(x + 0.7, y + 0.7, icon_width - 1.4, icon_height - 1.4);
        let _ = context.stroke();

        context.move_to(separator_x, y + 1.2);
        context.line_to(separator_x, y + icon_height - 1.2);
        let _ = context.stroke();

        if !icon_open.get() {
            context.set_line_width(1.5);
            context.move_to(separator_x + 2.6, center_y - 3.0);
            context.line_to(separator_x + 1.0, center_y);
            context.line_to(separator_x + 2.6, center_y + 3.0);
            let _ = context.stroke();
        }
    });
    button.set_child(Some(&icon));
    (button, icon, open)
}
