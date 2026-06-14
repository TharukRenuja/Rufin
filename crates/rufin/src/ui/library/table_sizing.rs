use super::*;

const FITTED_TABLE_WIDTH_PADDING: i32 = 2;
const TABLE_TARGET_WIDTH: i32 = 44;
const TABLE_MIN_WIDTH: i32 = 24;
const TABLE_EXPAND_MIN_WIDTH: i32 = 140;

#[derive(Clone)]
pub(in crate::ui) struct ColumnViewWidthFit {
    table: glib::WeakRef<gtk::ColumnView>,
    columns: Rc<Vec<(gtk::ColumnViewColumn, i32)>>,
}

impl ColumnViewWidthFit {
    fn apply(&self) -> bool {
        let Some(table) = self.table.upgrade() else {
            return false;
        };
        apply_column_view_width_fit(&table, self.columns.as_ref());
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum ColumnViewWidthMode {
    RouteScroller,
    EmbeddedScroller,
    Embedded,
}

pub(in crate::ui) fn route_column_view_initial_width(shell: &Shell) -> i32 {
    column_view_initial_width(shell, 0, ColumnViewWidthMode::RouteScroller)
}

pub(in crate::ui) fn route_column_view_initial_width_with_inset(
    shell: &Shell,
    content_inset: i32,
) -> i32 {
    column_view_initial_width(shell, content_inset, ColumnViewWidthMode::RouteScroller)
}

pub(in crate::ui) fn column_view_initial_width(
    shell: &Shell,
    content_inset: i32,
    mode: ColumnViewWidthMode,
) -> i32 {
    let scrollbar_width = if column_view_reserves_scrollbar(mode) {
        vertical_scrollbar_width()
    } else {
        0
    };
    route_content_width(shell)
        .saturating_sub(scrollbar_width)
        .saturating_sub(content_inset.max(0))
        .saturating_sub(FITTED_TABLE_WIDTH_PADDING)
        .max(1)
}

fn vertical_scrollbar_width() -> i32 {
    let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, None::<&gtk::Adjustment>);
    let (_, natural, _, _) = scrollbar.measure(gtk::Orientation::Horizontal, -1);
    natural.max(0)
}

fn column_view_reserves_scrollbar(mode: ColumnViewWidthMode) -> bool {
    matches!(
        mode,
        ColumnViewWidthMode::RouteScroller | ColumnViewWidthMode::EmbeddedScroller
    )
}

pub(in crate::ui) fn fitted_column_widths(base_widths: &[i32], available_width: i32) -> Vec<i32> {
    if base_widths.is_empty() {
        return Vec::new();
    }

    let available_width = available_width.max(base_widths.len() as i32);
    let base_total = base_widths.iter().sum::<i32>();
    if base_total <= available_width {
        let mut widths = base_widths.to_vec();
        distribute_column_width(&mut widths, available_width);
        return widths;
    }

    let min_widths = base_widths
        .iter()
        .map(|width| (*width).clamp(TABLE_MIN_WIDTH, TABLE_TARGET_WIDTH))
        .collect::<Vec<_>>();
    let min_total = min_widths.iter().sum::<i32>();
    if min_total >= available_width {
        return proportional_column_widths(&min_widths, available_width);
    }

    let remaining = available_width - min_total;
    let flex_weights = base_widths
        .iter()
        .zip(min_widths.iter())
        .map(|(base, minimum)| base.saturating_sub(*minimum))
        .collect::<Vec<_>>();
    let flex_total = flex_weights.iter().sum::<i32>();
    if flex_total <= 0 {
        return min_widths;
    }

    let mut widths = min_widths
        .iter()
        .zip(flex_weights.iter())
        .map(|(minimum, flex)| minimum + (flex * remaining / flex_total))
        .collect::<Vec<_>>();
    distribute_column_width_remainder(&mut widths, available_width);
    widths
}

fn proportional_column_widths(weights: &[i32], available_width: i32) -> Vec<i32> {
    let available_width = available_width.max(weights.len() as i32);
    let total = weights.iter().sum::<i32>().max(1);
    let mut widths = weights
        .iter()
        .map(|weight| (weight * available_width / total).max(1))
        .collect::<Vec<_>>();
    distribute_column_width_remainder(&mut widths, available_width);
    widths
}

fn distribute_column_width_remainder(widths: &mut [i32], target: i32) {
    let mut remainder = target - widths.iter().sum::<i32>();
    let mut index = 0;
    while remainder > 0 && !widths.is_empty() {
        let len = widths.len();
        widths[index % len] += 1;
        remainder -= 1;
        index += 1;
    }
    while remainder < 0 && widths.iter().any(|width| *width > 1) {
        let pos = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, width)| **width)
            .map(|(pos, _)| pos)
            .unwrap_or(0);
        widths[pos] -= 1;
        remainder += 1;
    }
}

fn distribute_column_width(widths: &mut [i32], target: i32) {
    let remainder = target - widths.iter().sum::<i32>();
    if remainder <= 0 || widths.is_empty() {
        return;
    }
    let flex_positions = widths
        .iter()
        .enumerate()
        .filter_map(|(pos, width)| (*width >= TABLE_EXPAND_MIN_WIDTH).then_some(pos))
        .collect::<Vec<_>>();
    if !flex_positions.is_empty() {
        distribute_weighted_column_width(widths, &flex_positions, remainder);
        return;
    }

    let pos = widths
        .iter()
        .enumerate()
        .max_by_key(|(_, width)| **width)
        .map(|(pos, _)| pos)
        .unwrap_or(0);
    widths[pos] += remainder;
}

fn distribute_weighted_column_width(widths: &mut [i32], positions: &[usize], extra: i32) {
    let total = positions.iter().map(|pos| widths[*pos]).sum::<i32>().max(1);
    let mut applied = 0;
    for pos in positions {
        let add = widths[*pos] * extra / total;
        widths[*pos] += add;
        applied += add;
    }

    let mut remainder = extra - applied;
    let mut index = 0;
    while remainder > 0 {
        let pos = positions[index % positions.len()];
        widths[pos] += 1;
        remainder -= 1;
        index += 1;
    }
}

pub(in crate::ui) fn install_column_view_width_fit(
    shell: &Rc<Shell>,
    table: &gtk::ColumnView,
    columns: Vec<(gtk::ColumnViewColumn, i32)>,
    initial_width: i32,
) {
    if columns.is_empty() {
        return;
    }

    let columns = Rc::new(columns);
    fit_column_widths(columns.as_ref(), table.width().max(initial_width));
    shell
        .state
        .column_view_width_fits
        .borrow_mut()
        .push(ColumnViewWidthFit {
            table: table.downgrade(),
            columns: Rc::clone(&columns),
        });
    let columns_for_map = Rc::clone(&columns);
    table.connect_map(move |table| {
        apply_column_view_width_fit(table, columns_for_map.as_ref());
    });
    let columns_for_resize = Rc::clone(&columns);
    table.connect_notify_local(Some("width"), move |table, _| {
        apply_column_view_width_fit(table, columns_for_resize.as_ref());
    });
    let columns_for_tick = Rc::clone(&columns);
    table.add_tick_callback(move |table, _| {
        apply_column_view_width_fit(table, columns_for_tick.as_ref());
        if table.width() > 1 {
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
}

pub(in crate::ui) fn refit_column_view_width_fits(fits: &mut Vec<ColumnViewWidthFit>) {
    fits.retain(ColumnViewWidthFit::apply);
}

fn apply_column_view_width_fit(table: &gtk::ColumnView, columns: &[(gtk::ColumnViewColumn, i32)]) {
    let available_width = table.width().saturating_sub(FITTED_TABLE_WIDTH_PADDING);
    if available_width <= 1 {
        return;
    }

    fit_column_widths(columns, available_width);
}

fn fit_column_widths(columns: &[(gtk::ColumnViewColumn, i32)], available_width: i32) {
    if available_width <= 1 {
        return;
    }
    let base_widths = columns.iter().map(|(_, width)| *width).collect::<Vec<_>>();
    let fitted_widths = fitted_column_widths(&base_widths, available_width);
    for ((column, _), width) in columns.iter().zip(fitted_widths) {
        column.set_fixed_width(width.max(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollable_embedded_tables_reserve_scrollbar_width() {
        assert!(column_view_reserves_scrollbar(
            ColumnViewWidthMode::EmbeddedScroller
        ));
        assert!(!column_view_reserves_scrollbar(
            ColumnViewWidthMode::Embedded
        ));
    }
}
