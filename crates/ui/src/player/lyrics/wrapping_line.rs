use std::cell::RefCell;
use std::ops::Range;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

const ROW_SPACING: i32 = 2;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct WrappingLine {
        pub(super) children: RefCell<Vec<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WrappingLine {
        const NAME: &'static str = "RufinLyricsWrappingLine";
        type Type = super::WrappingLine;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for WrappingLine {
        fn dispose(&self) {
            for child in self.children.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for WrappingLine {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = self.children.borrow();
            if children.is_empty() {
                return (0, 0, -1, -1);
            }
            let widths = child_widths(&children);
            if orientation == gtk::Orientation::Horizontal {
                let minimum = children
                    .iter()
                    .map(|child| child.measure(orientation, -1).0)
                    .max()
                    .unwrap_or_default();
                let natural = widths.iter().copied().fold(0_i32, i32::saturating_add);
                return (minimum, natural, -1, -1);
            }

            let available_width = if for_size < 0 {
                widths.iter().copied().fold(0_i32, i32::saturating_add)
            } else {
                for_size.max(1)
            };
            let mut minimum = 0_i32;
            let mut natural = 0_i32;
            for (row_index, range) in wrapped_row_ranges(&widths, available_width)
                .into_iter()
                .enumerate()
            {
                if row_index > 0 {
                    minimum = minimum.saturating_add(super::ROW_SPACING);
                    natural = natural.saturating_add(super::ROW_SPACING);
                }
                let (row_minimum, row_natural) = row_height(&children, &widths, range);
                minimum = minimum.saturating_add(row_minimum);
                natural = natural.saturating_add(row_natural);
            }
            (minimum, natural, -1, -1)
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let children = self.children.borrow();
            let widths = child_widths(&children)
                .into_iter()
                .map(|child_width| child_width.min(width.max(1)))
                .collect::<Vec<_>>();
            let mut y = 0_i32;
            for range in wrapped_row_ranges(&widths, width.max(1)) {
                let row_width = widths[range.clone()]
                    .iter()
                    .copied()
                    .fold(0_i32, i32::saturating_add);
                let (_, row_height) = row_height(&children, &widths, range.clone());
                let mut x = (width.saturating_sub(row_width) / 2).max(0);
                for index in range {
                    let child_width = widths[index];
                    let (_, child_height, _, _) =
                        children[index].measure(gtk::Orientation::Vertical, child_width);
                    let transform = gtk::gsk::Transform::new()
                        .translate(&gtk::graphene::Point::new(x as f32, y as f32));
                    children[index].allocate(child_width, child_height, -1, Some(transform));
                    x = x.saturating_add(child_width);
                }
                y = y
                    .saturating_add(row_height)
                    .saturating_add(super::ROW_SPACING);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            for child in self.children.borrow().iter() {
                self.obj().snapshot_child(child, snapshot);
            }
        }
    }

    fn child_widths(children: &[gtk::Widget]) -> Vec<i32> {
        children
            .iter()
            .map(|child| child.measure(gtk::Orientation::Horizontal, -1).1.max(1))
            .collect()
    }

    fn row_height(children: &[gtk::Widget], widths: &[i32], range: Range<usize>) -> (i32, i32) {
        range
            .map(|index| {
                let (minimum, natural, _, _) =
                    children[index].measure(gtk::Orientation::Vertical, widths[index]);
                (minimum, natural)
            })
            .fold(
                (0, 0),
                |(min_height, natural_height), (minimum, natural)| {
                    (min_height.max(minimum), natural_height.max(natural))
                },
            )
    }
}

glib::wrapper! {
    pub struct WrappingLine(ObjectSubclass<imp::WrappingLine>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl WrappingLine {
    pub(super) fn new() -> Self {
        let line: Self = glib::Object::new();
        line.set_accessible_role(gtk::AccessibleRole::Presentation);
        line
    }

    pub(super) fn append(&self, child: &impl IsA<gtk::Widget>) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;

        let child = child.clone().upcast::<gtk::Widget>();
        child.set_parent(self);
        self.imp().children.borrow_mut().push(child);
        self.queue_resize();
    }
}

fn wrapped_row_ranges(widths: &[i32], available_width: i32) -> Vec<Range<usize>> {
    if widths.is_empty() {
        return Vec::new();
    }
    let available_width = available_width.max(1);
    let mut rows = Vec::new();
    let mut row_start = 0;
    let mut row_width = 0_i32;
    for (index, width) in widths.iter().copied().enumerate() {
        let width = width.min(available_width);
        if index > row_start && row_width.saturating_add(width) > available_width {
            rows.push(row_start..index);
            row_start = index;
            row_width = 0;
        }
        row_width = row_width.saturating_add(width);
    }
    rows.push(row_start..widths.len());
    rows
}

#[cfg(test)]
mod tests {
    use super::wrapped_row_ranges;

    #[test]
    fn wrapping_keeps_each_reading_with_its_native_token() {
        assert_eq!(wrapped_row_ranges(&[30, 20, 40, 10], 60), vec![0..2, 2..4]);
        assert_eq!(wrapped_row_ranges(&[80, 10], 60), vec![0..1, 1..2]);
    }
}
