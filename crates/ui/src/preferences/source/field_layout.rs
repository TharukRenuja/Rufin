use std::cell::Cell;

use adw::prelude::*;

use crate::layout::{AllocationOwner, width_allocation_owner};

const COMPACT_FIELD_ROW_STACK_WIDTH: i32 = 560;

pub(super) fn style_compact_field_row(row: &impl IsA<gtk::Widget>) {
    row.add_css_class("compact-field-row");
}

pub(super) fn compact_field_row_group(row: &impl IsA<gtk::Widget>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_hexpand(true);
    group.add(row);
    group
}

pub(super) fn install_compact_field_row_responsiveness(fields: &gtk::Box) -> AllocationOwner {
    let resize_fields = fields.clone();
    let stacked = Cell::new(false);
    width_allocation_owner(fields, move |width| {
        let stack = width < COMPACT_FIELD_ROW_STACK_WIDTH;
        if stacked.replace(stack) != stack {
            apply_compact_field_row_layout(&resize_fields, stack);
        }
    })
}

fn apply_compact_field_row_layout(fields: &gtk::Box, stack: bool) {
    fields.set_orientation(if stack {
        gtk::Orientation::Vertical
    } else {
        gtk::Orientation::Horizontal
    });
    fields.set_homogeneous(!stack);
    fields.set_spacing(if stack { 8 } else { 12 });
}
