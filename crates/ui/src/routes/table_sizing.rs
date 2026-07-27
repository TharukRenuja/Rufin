use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;

use crate::MIN_TABLE_COLUMN_WIDTH;
use crate::shell::Shell;
use crate::shell::layout::route_content_width;

use super::route_layout::PRIMARY_ROUTE_HORIZONTAL_INSET;

const FITTED_TABLE_WIDTH_PADDING: i32 = 2;
const TABLE_TARGET_WIDTH: i32 = 44;
const TABLE_EXPAND_MIN_WIDTH: i32 = 140;

pub(crate) fn route_column_view_initial_width(shell: &Shell) -> i32 {
    route_column_view_initial_width_with_inset(shell, PRIMARY_ROUTE_HORIZONTAL_INSET)
}

pub(crate) fn route_column_view_initial_width_with_inset(shell: &Shell, content_inset: i32) -> i32 {
    column_view_initial_width(shell, content_inset)
}

pub(crate) fn column_view_initial_width(shell: &Shell, content_inset: i32) -> i32 {
    route_content_width(shell)
        .saturating_sub(content_inset.max(0))
        .saturating_sub(FITTED_TABLE_WIDTH_PADDING)
        .max(1)
}

fn vertical_scrollbar_width() -> i32 {
    let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, None::<&gtk::Adjustment>);
    let (_, natural, _, _) = scrollbar.measure(gtk::Orientation::Horizontal, -1);
    natural.max(0)
}

pub(crate) fn fitted_column_widths(base_widths: &[i32], available_width: i32) -> Vec<i32> {
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
        .map(|width| (*width).clamp(MIN_TABLE_COLUMN_WIDTH, TABLE_TARGET_WIDTH))
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

#[derive(Clone)]
pub(crate) struct ColumnViewWidthFit {
    table: gtk::glib::WeakRef<gtk::ColumnView>,
    state: Rc<ColumnViewWidthState>,
    fallback_width: i32,
    last_width: Rc<Cell<i32>>,
}

struct ColumnViewWidthState {
    columns: RefCell<Vec<(gtk::ColumnViewColumn, i32)>>,
    available_width: Cell<i32>,
    applying: Cell<bool>,
    user_resized: Cell<bool>,
}

impl ColumnViewWidthFit {
    pub(crate) fn replace(&self, columns: Vec<(gtk::ColumnViewColumn, i32)>) {
        disable_column_expansion(&columns);
        *self.state.columns.borrow_mut() = columns;
        self.state.user_resized.set(false);
        connect_resizable_column_widths(&self.state);
        fit_preferred_column_widths(&self.state);
        if let Some(table) = self.table.upgrade() {
            table.queue_resize();
        }
    }

    pub(crate) fn fit_scroller_allocation(
        &self,
        scroller: &gtk::ScrolledWindow,
        allocation_width: i32,
    ) {
        let Some(table) = self.table.upgrade() else {
            return;
        };
        let available_width = fitted_table_available_width(
            table.width(),
            Some(allocation_width),
            table.margin_start(),
            table.margin_end(),
            scroller_vertical_scrollbar_width(scroller),
            self.fallback_width,
        );
        if available_width <= 1 {
            return;
        }
        self.state.available_width.set(available_width);
        if self.last_width.replace(available_width) == available_width {
            return;
        }
        fit_preferred_column_widths(&self.state);
    }

    pub(crate) fn set_preferred_widths(&self, widths: &[i32]) {
        let mut columns = self.state.columns.borrow_mut();
        if columns.len() != widths.len() {
            return;
        }
        for ((_, preferred_width), width) in columns.iter_mut().zip(widths) {
            *preferred_width = (*width).max(1);
        }
        drop(columns);
        self.state.user_resized.set(false);
        fit_preferred_column_widths(&self.state);
    }

    pub(crate) fn take_user_resized_widths(&self) -> Option<Vec<i32>> {
        self.state.user_resized.replace(false).then(|| {
            self.state
                .columns
                .borrow()
                .iter()
                .map(|(_, width)| *width)
                .collect()
        })
    }
}

pub(crate) fn install_column_view_width_fit(
    table: &gtk::ColumnView,
    columns: Vec<(gtk::ColumnViewColumn, i32)>,
    initial_width: i32,
) -> ColumnViewWidthFit {
    disable_column_expansion(&columns);
    let state = Rc::new(ColumnViewWidthState {
        columns: RefCell::new(columns),
        available_width: Cell::new(initial_width.max(1)),
        applying: Cell::new(false),
        user_resized: Cell::new(false),
    });
    connect_resizable_column_widths(&state);
    fit_preferred_column_widths(&state);
    ColumnViewWidthFit {
        table: table.downgrade(),
        state,
        fallback_width: initial_width,
        last_width: Rc::new(Cell::new(initial_width)),
    }
}

pub(crate) fn connect_column_width_save(
    table: &gtk::ColumnView,
    width_fit: &ColumnViewWidthFit,
    save: impl Fn(Vec<i32>) + 'static,
) {
    let events = gtk::EventControllerLegacy::new();
    events.set_propagation_phase(gtk::PropagationPhase::Capture);
    let width_fit = width_fit.clone();
    let save = Rc::new(save);
    events.connect_event(move |_, event| {
        if matches!(
            event.event_type(),
            gtk::gdk::EventType::ButtonRelease | gtk::gdk::EventType::KeyRelease
        ) {
            let width_fit = width_fit.clone();
            let save = Rc::clone(&save);
            gtk::glib::idle_add_local_once(move || {
                if let Some(widths) = width_fit.take_user_resized_widths() {
                    save(widths);
                }
            });
        }
        gtk::glib::Propagation::Proceed
    });
    table.add_controller(events);
}

fn disable_column_expansion(columns: &[(gtk::ColumnViewColumn, i32)]) {
    for (column, _) in columns {
        column.set_expand(false);
    }
}

fn scroller_vertical_scrollbar_width(scroller: &gtk::ScrolledWindow) -> i32 {
    let (_, vertical_policy) = scroller.policy();
    scroller_vertical_scrollbar_width_for_fit(vertical_policy, scroller.is_overlay_scrolling())
}

fn scroller_vertical_scrollbar_width_for_fit(
    vertical_policy: gtk::PolicyType,
    overlay_scrolling: bool,
) -> i32 {
    if overlay_scrolling {
        return 0;
    }
    if matches!(
        vertical_policy,
        gtk::PolicyType::Always | gtk::PolicyType::Automatic
    ) {
        vertical_scrollbar_width()
    } else {
        0
    }
}

fn fitted_table_available_width(
    table_width: i32,
    viewport_width: Option<i32>,
    margin_start: i32,
    margin_end: i32,
    scrollbar_width: i32,
    fallback_width: i32,
) -> i32 {
    if let Some(viewport_width) = viewport_width.filter(|width| *width > 1) {
        return viewport_width
            .saturating_sub(margin_start.max(0))
            .saturating_sub(margin_end.max(0))
            .saturating_sub(scrollbar_width.max(0))
            .saturating_sub(FITTED_TABLE_WIDTH_PADDING)
            .max(1);
    }

    if viewport_width.is_some() {
        return fallback_width.max(1);
    }

    table_width
        .max(fallback_width)
        .saturating_sub(FITTED_TABLE_WIDTH_PADDING)
        .max(1)
}

fn connect_resizable_column_widths(state: &Rc<ColumnViewWidthState>) {
    let columns = state
        .columns
        .borrow()
        .iter()
        .map(|(column, _)| column.clone())
        .collect::<Vec<_>>();
    for column in columns {
        if !column.is_resizable() {
            continue;
        }
        let state = Rc::downgrade(state);
        column.connect_fixed_width_notify(move |changed| {
            let Some(state) = state.upgrade() else {
                return;
            };
            record_resized_column(&state, changed);
        });
    }
}

fn record_resized_column(state: &ColumnViewWidthState, changed: &gtk::ColumnViewColumn) {
    if state.applying.get() {
        return;
    }

    let (current_widths, resizable, changed_position) = {
        let columns = state.columns.borrow();
        let changed_position = columns.iter().position(|(column, _)| column == changed);
        (
            columns
                .iter()
                .map(|(column, _)| column.fixed_width().max(1))
                .collect::<Vec<_>>(),
            columns
                .iter()
                .map(|(column, _)| column.is_resizable())
                .collect::<Vec<_>>(),
            changed_position,
        )
    };
    let Some(changed_position) = changed_position else {
        return;
    };
    let widths = rebalanced_column_widths(
        &current_widths,
        &resizable,
        changed_position,
        state.available_width.get(),
    );
    for ((_, preferred_width), width) in state.columns.borrow_mut().iter_mut().zip(&widths) {
        *preferred_width = *width;
    }
    state.user_resized.set(true);
    apply_column_widths(state, &widths);
}

fn fit_preferred_column_widths(state: &ColumnViewWidthState) {
    let preferred_widths = state
        .columns
        .borrow()
        .iter()
        .map(|(_, width)| *width)
        .collect::<Vec<_>>();
    let widths = fitted_column_widths(&preferred_widths, state.available_width.get());
    apply_column_widths(state, &widths);
}

fn apply_column_widths(state: &ColumnViewWidthState, widths: &[i32]) {
    state.applying.set(true);
    for ((column, _), width) in state.columns.borrow().iter().zip(widths) {
        let width = (*width).max(1);
        if column.fixed_width() != width {
            column.set_fixed_width(width);
        }
    }
    state.applying.set(false);
}

fn rebalanced_column_widths(
    current_widths: &[i32],
    resizable: &[bool],
    changed_position: usize,
    available_width: i32,
) -> Vec<i32> {
    if current_widths.len() != resizable.len()
        || !resizable.get(changed_position).copied().unwrap_or(false)
    {
        return fitted_column_widths(current_widths, available_width);
    }

    let mut widths = current_widths
        .iter()
        .map(|width| (*width).max(1))
        .collect::<Vec<_>>();
    let available_width = available_width.max(widths.len() as i32);
    let difference = available_width - widths.iter().sum::<i32>();
    if difference == 0 {
        return widths;
    }

    let right_resizable = resizable
        .iter()
        .enumerate()
        .skip(changed_position + 1)
        .filter_map(|(position, resizable)| (*resizable).then_some(position))
        .collect::<Vec<_>>();

    if difference > 0 {
        let recipient = right_resizable.first().copied().unwrap_or(changed_position);
        widths[recipient] += difference;
        return widths;
    }

    let mut overflow = -difference;
    for position in right_resizable {
        let shrink = (widths[position] - MIN_TABLE_COLUMN_WIDTH)
            .max(0)
            .min(overflow);
        widths[position] -= shrink;
        overflow -= shrink;
        if overflow == 0 {
            return widths;
        }
    }
    let shrink = (widths[changed_position] - 1).min(overflow);
    widths[changed_position] -= shrink;
    overflow -= shrink;
    if overflow > 0 {
        return fitted_column_widths(&widths, available_width);
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::{
        fitted_table_available_width, rebalanced_column_widths,
        scroller_vertical_scrollbar_width_for_fit,
    };

    #[test]
    fn scrollable_route_tables_use_overlay_scrollbar_width() {
        assert_eq!(
            scroller_vertical_scrollbar_width_for_fit(gtk::PolicyType::Always, true),
            0
        );
        assert_eq!(
            scroller_vertical_scrollbar_width_for_fit(gtk::PolicyType::External, false),
            0
        );
    }

    #[test]
    fn viewport_width_wins_over_stale_table_width() {
        let available = fitted_table_available_width(900, Some(520), 24, 0, 14, 900);

        assert_eq!(available, 480);
    }

    #[test]
    fn initial_width_seeds_unallocated_tables() {
        let available = fitted_table_available_width(900, Some(1), 24, 0, 14, 620);

        assert_eq!(available, 620);
    }

    #[test]
    fn shrinking_one_column_gives_space_to_its_right_neighbor() {
        let widths = rebalanced_column_widths(&[44, 120, 256], &[false, true, true], 1, 600);

        assert_eq!(widths, vec![44, 120, 436]);
    }

    #[test]
    fn expanding_one_column_takes_space_from_its_right_neighbor() {
        let widths = rebalanced_column_widths(&[44, 540, 100], &[false, true, true], 1, 600);

        assert_eq!(widths, vec![44, 532, 24]);
        assert_eq!(widths.iter().sum::<i32>(), 600);
    }

    #[test]
    fn resizing_a_middle_column_does_not_move_columns_to_its_left() {
        let expanded =
            rebalanced_column_widths(&[44, 180, 200, 120], &[false, true, true, true], 2, 500);
        let shrunk =
            rebalanced_column_widths(&[44, 180, 80, 120], &[false, true, true, true], 2, 500);

        assert_eq!(expanded, vec![44, 180, 200, 76]);
        assert_eq!(shrunk, vec![44, 180, 80, 196]);
    }

    #[test]
    fn resizing_the_last_column_keeps_the_right_edge_fixed() {
        let widths = rebalanced_column_widths(&[44, 220, 230], &[false, true, true], 2, 444);

        assert_eq!(widths, vec![44, 220, 180]);
    }
}
