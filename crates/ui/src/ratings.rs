use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

use adw::prelude::*;
use gtk::cairo;
use library::{FavoriteItemId, MetadataItemId};
use localization::{msgid, tr};

const STAR_WIDTH: i32 = 95;
const STAR_HEIGHT: i32 = 20;

#[derive(Clone)]
pub(crate) struct RatingControl {
    area: gtk::DrawingArea,
    value: Rc<Cell<u8>>,
    preview: Rc<Cell<Option<u8>>>,
}

impl RatingControl {
    pub(crate) fn new(rating: Option<u8>) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_width(STAR_WIDTH);
        area.set_content_height(STAR_HEIGHT);
        area.add_css_class("rating-stars");
        area.set_cursor_from_name(Some("pointer"));
        area.set_tooltip_text(Some(&tr(msgid("Rating"))));

        let value = Rc::new(Cell::new(rating.unwrap_or(0)));
        let preview = Rc::new(Cell::new(None));
        let drawn_value = Rc::clone(&value);
        let drawn_preview = Rc::clone(&preview);
        area.set_draw_func(move |area, context, width, height| {
            draw_stars(
                area,
                context,
                width,
                height,
                drawn_preview.get().unwrap_or(drawn_value.get()),
            );
        });

        let hovered = Rc::clone(&preview);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |controller, x, _| {
            hovered.set(Some(rating_at(x, widget_width(controller.widget()))));
            controller.widget().map(|widget| widget.queue_draw());
        });
        let left = Rc::clone(&preview);
        motion.connect_leave(move |controller| {
            left.set(None);
            controller.widget().map(|widget| widget.queue_draw());
        });
        area.add_controller(motion);

        Self {
            area,
            value,
            preview,
        }
    }

    pub(crate) fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub(crate) fn set_rating(&self, rating: Option<u8>) {
        self.value.set(rating.unwrap_or(0));
        if self.preview.get().is_none() {
            self.area.queue_draw();
        }
    }

    pub(crate) fn connect_commit(&self, commit: impl Fn(Option<u8>) + 'static) {
        let start = Rc::new(Cell::new(0.0));
        let drag = gtk::GestureDrag::new();
        let began_start = Rc::clone(&start);
        let began_preview = Rc::clone(&self.preview);
        drag.connect_drag_begin(move |gesture, x, _| {
            began_start.set(x);
            preview_at(gesture.widget(), &began_preview, x);
        });
        let updated_start = Rc::clone(&start);
        let updated_preview = Rc::clone(&self.preview);
        drag.connect_drag_update(move |gesture, offset, _| {
            preview_at(
                gesture.widget(),
                &updated_preview,
                updated_start.get() + offset,
            );
        });
        let value = Rc::clone(&self.value);
        let preview = Rc::clone(&self.preview);
        drag.connect_drag_end(move |gesture, offset, _| {
            let rating = rating_at(start.get() + offset, widget_width(gesture.widget()));
            value.set(rating);
            preview.set(Some(rating));
            gesture.widget().map(|widget| widget.queue_draw());
            commit(Some(rating));
        });
        self.area.add_controller(drag);
    }
}

fn preview_at(widget: Option<gtk::Widget>, preview: &Cell<Option<u8>>, x: f64) {
    preview.set(Some(rating_at(x, widget_width(widget.clone()))));
    widget.map(|widget| widget.queue_draw());
}

fn widget_width(widget: Option<gtk::Widget>) -> i32 {
    widget.map_or(STAR_WIDTH, |widget| widget.width())
}

fn rating_at(x: f64, width: i32) -> u8 {
    ((x / f64::from(width.max(1)) * 10.0).ceil() as u8).clamp(1, 10)
}

fn draw_stars(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    rating: u8,
) {
    let color = area.color();
    let outline = area.parent().map(|parent| parent.color()).unwrap_or(color);
    context.set_source_rgba(
        f64::from(outline.red()),
        f64::from(outline.green()),
        f64::from(outline.blue()),
        f64::from(outline.alpha()) * 0.45,
    );
    context.set_line_width(1.5);
    star_paths(context, width, height);
    let _ = context.stroke();

    let _ = context.save();
    context.rectangle(
        0.0,
        0.0,
        f64::from(width) * f64::from(rating.min(10)) / 10.0,
        f64::from(height),
    );
    context.clip();
    context.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
    star_paths(context, width, height);
    let _ = context.fill();
    let _ = context.restore();
}

fn star_paths(context: &cairo::Context, width: i32, height: i32) {
    let cell = f64::from(width) / 5.0;
    let outer = (cell.min(f64::from(height)) - 4.0) / 2.0;
    for star in 0..5 {
        let center_x = cell * (f64::from(star) + 0.5);
        let center_y = f64::from(height) / 2.0;
        for point in 0..10 {
            let radius = if point % 2 == 0 { outer } else { outer * 0.45 };
            let angle = -PI / 2.0 + f64::from(point) * PI / 5.0;
            let x = center_x + radius * angle.cos();
            let y = center_y + radius * angle.sin();
            if point == 0 {
                context.move_to(x, y);
            } else {
                context.line_to(x, y);
            }
        }
        context.close_path();
    }
}

pub(crate) fn context_rating_row(
    rating: Option<u8>,
    popover: &gtk::PopoverMenu,
    commit: impl Fn(Option<u8>) + 'static,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    row.set_hexpand(true);
    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.set_margin_bottom(2);
    row.append(&separator);

    let control = RatingControl::new(rating);
    control.widget().set_hexpand(true);
    control.widget().set_halign(gtk::Align::Fill);
    control.widget().set_margin_start(10);
    control.widget().set_margin_end(10);
    control.widget().set_margin_top(2);
    control.widget().set_margin_bottom(2);
    let popover = popover.downgrade();
    control.connect_commit(move |rating| {
        if let Some(popover) = popover.upgrade() {
            crate::interactions::popdown_native_menu(&popover);
        }
        commit(rating);
    });
    row.append(control.widget());
    row
}

impl crate::shell::Shell {
    pub(crate) fn rating_available(&self, item: &FavoriteItemId) -> bool {
        let configured = self.source.configured.borrow();
        let Some(source) = configured
            .sources
            .iter()
            .find(|source| configured.selected_source_id.as_ref() == Some(&source.id))
        else {
            return false;
        };
        if source.kind != "local" {
            return true;
        }
        let FavoriteItemId::Track(track_id) = item else {
            return false;
        };
        self.metadata_editing_available(MetadataItemId::Track(track_id.clone()))
    }

    pub(crate) fn set_rating(&self, item: FavoriteItemId, rating: Option<u8>) {
        if let Some(source) = self.selected_source_operations() {
            source.set_rating(item, rating);
        }
    }

    pub(crate) fn set_current_track_rating(&self, rating: Option<u8>) {
        if let Some(track_id) = self
            .selected_playback()
            .as_deref()
            .and_then(|player| player.transport.current.as_ref())
            .map(|entry| entry.track.id.clone())
        {
            self.set_rating(FavoriteItemId::Track(track_id), rating);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rating_at;

    #[test]
    fn pointer_position_selects_half_stars() {
        assert_eq!(rating_at(9.5, 95), 1);
        assert_eq!(rating_at(19.0, 95), 2);
        assert_eq!(rating_at(47.5, 95), 5);
        assert_eq!(rating_at(57.0, 95), 6);
    }
}
