use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use source::{LyricLine, Lyrics};

const DEFAULT_LYRICS_SCROLL_ANIMATION_MS: u64 = 300;
const MIN_LYRICS_SCROLL_ANIMATION_MS: u64 = 80;
const LYRICS_SCROLL_MS: u64 = 200;
const LYRICS_USER_SCROLL_PAUSE_MS: u64 = 3_000;
const LYRICS_SCROLL_READY_RETRY_MS: u64 = 32;
const LYRICS_SCROLL_READY_RETRIES: u8 = 12;

#[derive(Clone)]
pub struct LyricsPane {
    root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    body: gtk::Box,
    title: gtk::Label,
    clear_auto_search_button: gtk::Button,
    search_button: gtk::Button,
    rows: Rc<RefCell<Vec<LyricsRow>>>,
    active_index: Rc<Cell<Option<usize>>>,
    scroll_generation: Rc<Cell<u64>>,
    follow_pause_until: Rc<Cell<Option<Instant>>>,
}

#[derive(Clone)]
struct LyricsRow {
    line_index: usize,
    row: gtk::Widget,
    label: gtk::Label,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LyricsFollowScrollPause {
    Inactive,
    Active,
    Expired,
}

impl LyricsPane {
    pub fn new(title: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.add_css_class("lyrics-panel");
        root.set_vexpand(true);
        root.set_margin_top(4);
        root.set_margin_start(8);
        root.set_margin_end(8);
        root.set_margin_bottom(8);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_valign(gtk::Align::Center);

        let title = gtk::Label::new(Some(title));
        title.add_css_class("panel-title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);

        let clear_auto_search_button = gtk::Button::from_icon_name("window-close-symbolic");
        clear_auto_search_button.add_css_class("icon-button");
        clear_auto_search_button.add_css_class("flat");
        clear_auto_search_button.add_css_class("circular");
        clear_auto_search_button.set_visible(false);
        header.append(&clear_auto_search_button);

        let search_button = gtk::Button::from_icon_name("system-search-symbolic");
        search_button.add_css_class("icon-button");
        search_button.add_css_class("flat");
        search_button.add_css_class("circular");
        header.append(&search_button);
        root.append(&header);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.set_vexpand(true);
        body.add_css_class("lyrics-lines");
        scroller.set_child(Some(&body));
        root.append(&scroller);

        let pane = Self {
            root,
            scroller,
            body,
            title,
            clear_auto_search_button,
            search_button,
            rows: Rc::new(RefCell::new(Vec::new())),
            active_index: Rc::new(Cell::new(None)),
            scroll_generation: Rc::new(Cell::new(0)),
            follow_pause_until: Rc::new(Cell::new(None)),
        };
        pane.connect_header_hover(&header);
        pane.connect_user_scroll_pause();
        pane
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_title(&self, title: &str) {
        self.title.set_text(title);
    }

    pub fn connect_search_clicked(&self, search: impl Fn() + 'static) {
        self.search_button.connect_clicked(move |_| search());
    }

    pub fn connect_clear_auto_search_clicked(&self, clear: impl Fn() + 'static) {
        self.clear_auto_search_button
            .connect_clicked(move |_| clear());
    }

    pub fn set_search_action(&self, label: &str, enabled: bool) {
        self.search_button.set_tooltip_text(Some(label));
        self.search_button
            .update_property(&[gtk::accessible::Property::Label(label)]);
        self.search_button.set_sensitive(enabled);
    }

    pub fn set_clear_auto_search_action(&self, label: &str, enabled: bool) {
        self.clear_auto_search_button.set_tooltip_text(Some(label));
        self.clear_auto_search_button
            .update_property(&[gtk::accessible::Property::Label(label)]);
        self.clear_auto_search_button.set_sensitive(enabled);
        if !enabled {
            self.clear_auto_search_button.set_visible(false);
        }
    }

    pub fn set_content(
        &self,
        lyrics: Option<&Lyrics>,
        loading: bool,
        empty_status: String,
        seek: Rc<dyn Fn(u64)>,
    ) {
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        self.rows.borrow_mut().clear();
        self.active_index.set(None);
        self.cancel_scroll_animation();
        if lyrics.is_none() {
            self.body.add_css_class("lyrics-placeholder");
        } else {
            self.body.remove_css_class("lyrics-placeholder");
        }

        if let Some(current_lyrics) = lyrics {
            for (line_index, line) in current_lyrics.lines.iter().enumerate() {
                if !lyric_line_has_text(line) {
                    continue;
                }
                let label = gtk::Label::new(Some(&line.text));
                label.set_wrap(true);
                label.set_xalign(0.5);
                label.set_justify(gtk::Justification::Center);
                label.set_hexpand(true);
                label.add_css_class("lyrics-line");

                let row: gtk::Widget = if let Some(start_millis) = line.start_millis {
                    let row = gtk::Button::new();
                    row.add_css_class("flat");
                    row.set_hexpand(true);
                    row.set_child(Some(&label));

                    let seek = Rc::clone(&seek);
                    row.connect_clicked(move |_| seek(start_millis));
                    row.upcast()
                } else {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    row.set_hexpand(true);
                    row.append(&label);
                    row.upcast()
                };
                row.add_css_class("lyrics-row");

                self.body.append(&row);
                self.rows.borrow_mut().push(LyricsRow {
                    line_index,
                    row,
                    label,
                });
            }
        } else if loading {
            let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            placeholder.set_halign(gtk::Align::Fill);
            placeholder.set_valign(gtk::Align::Fill);
            placeholder.set_hexpand(true);
            placeholder.set_vexpand(true);

            let spinner = gtk::Spinner::new();
            spinner.add_css_class("lyrics-loading-spinner");
            spinner.set_halign(gtk::Align::Center);
            spinner.set_valign(gtk::Align::Center);
            spinner.set_hexpand(true);
            spinner.set_vexpand(true);
            spinner.start();
            placeholder.append(&spinner);
            self.body.append(&placeholder);
        } else {
            let status = gtk::Label::new(Some(&empty_status));
            status.add_css_class("muted");
            status.set_wrap(true);
            status.set_justify(gtk::Justification::Center);
            status.set_valign(gtk::Align::Center);
            status.set_vexpand(true);
            self.body.append(&status);
        }
    }

    pub fn update_highlight(&self, lyrics: Option<&Lyrics>, position_millis: u64) {
        self.update_highlight_with_scroll_duration(lyrics, position_millis, None);
    }

    fn update_highlight_with_scroll_duration(
        &self,
        lyrics: Option<&Lyrics>,
        position_millis: u64,
        scroll_duration: Option<u64>,
    ) {
        let active_index = lyrics
            .and_then(|lyrics| active_lyrics_line_index(lyrics.lines.as_slice(), position_millis));
        let highlight_all_lines =
            lyrics.is_some_and(|lyrics| should_highlight_all_lyrics_lines(lyrics.lines.as_slice()));
        let previous_index = self.active_index.replace(active_index);
        let follow_pause = self.follow_scroll_pause();
        let scroll_target = {
            let rows = self.rows.borrow();
            for row in rows.iter() {
                let active = highlight_all_lines || Some(row.line_index) == active_index;
                if active {
                    row.row.add_css_class("lyrics-row-active");
                    row.label.add_css_class("lyrics-line-active");
                } else {
                    row.row.remove_css_class("lyrics-row-active");
                    row.label.remove_css_class("lyrics-line-active");
                }
            }

            lyrics_follow_scroll_target(active_index, previous_index, follow_pause).and_then(
                |index| {
                    let row = rows.iter().find(|row| row.line_index == index)?.row.clone();
                    let duration = scroll_duration.unwrap_or_else(|| {
                        lyrics
                            .map(|lyrics| {
                                lyrics_scroll_animation_millis(
                                    lyrics.lines.as_slice(),
                                    index,
                                    position_millis,
                                )
                            })
                            .unwrap_or(DEFAULT_LYRICS_SCROLL_ANIMATION_MS)
                    });
                    Some((row, duration))
                },
            )
        };

        if let Some((row, duration)) = scroll_target {
            self.scroll_row_into_view(row, duration);
        }
    }

    pub fn refocus_highlight(&self, lyrics: Option<&Lyrics>, position_millis: u64) {
        self.active_index.set(None);
        self.follow_pause_until.set(None);
        self.cancel_scroll_animation();
        self.update_highlight_with_scroll_duration(lyrics, position_millis, Some(0));
    }

    pub fn pause_follow_scroll(&self) {
        self.follow_pause_until.set(Some(
            Instant::now() + Duration::from_millis(LYRICS_USER_SCROLL_PAUSE_MS),
        ));
    }

    pub fn clear_follow_scroll_pause(&self) {
        self.follow_pause_until.set(None);
    }

    pub fn restart_follow_tracking(&self) {
        self.active_index.set(None);
        self.follow_pause_until.set(None);
        self.cancel_scroll_animation();
    }

    fn connect_user_scroll_pause(&self) {
        let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let pane = self.clone();
        controller.connect_scroll(move |_, _, _| {
            pane.pause_follow_scroll();
            glib::Propagation::Proceed
        });
        self.scroller.add_controller(controller);
    }

    fn connect_header_hover(&self, header: &gtk::Box) {
        let button = self.clear_auto_search_button.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            if button.is_sensitive() {
                button.set_visible(true);
            }
        });
        let button = self.clear_auto_search_button.clone();
        motion.connect_leave(move |_| {
            button.set_visible(false);
        });
        header.add_controller(motion);
    }

    fn follow_scroll_pause(&self) -> LyricsFollowScrollPause {
        let pause = lyrics_follow_scroll_pause_state(self.follow_pause_until.get(), Instant::now());
        if pause == LyricsFollowScrollPause::Expired {
            self.follow_pause_until.set(None);
        }
        pause
    }

    fn cancel_scroll_animation(&self) {
        self.scroll_generation
            .set(self.scroll_generation.get().saturating_add(1));
    }

    fn scroll_row_into_view(&self, row: gtk::Widget, duration_millis: u64) {
        let scroller = self.scroller.clone();
        let generation = self.scroll_generation.get().saturating_add(1);
        self.scroll_generation.set(generation);
        let scroll_generation = Rc::clone(&self.scroll_generation);
        scroll_row_into_view_when_ready(
            scroller,
            row,
            duration_millis,
            scroll_generation,
            generation,
            LYRICS_SCROLL_READY_RETRIES,
        );
    }
}

fn scroll_row_into_view_when_ready(
    scroller: gtk::ScrolledWindow,
    row: gtk::Widget,
    duration_millis: u64,
    scroll_generation: Rc<Cell<u64>>,
    generation: u64,
    retries_left: u8,
) {
    glib::idle_add_local_once(move || {
        if scroll_generation.get() != generation {
            return;
        }

        let bounds = row.compute_bounds(&scroller);
        let adjustment = scroller.vadjustment();
        let ready = bounds.is_some() && scroller.height() > 1 && adjustment.page_size() > 1.0;
        if !ready && retries_left > 0 {
            glib::timeout_add_local_once(
                Duration::from_millis(LYRICS_SCROLL_READY_RETRY_MS),
                move || {
                    scroll_row_into_view_when_ready(
                        scroller,
                        row,
                        duration_millis,
                        scroll_generation,
                        generation,
                        retries_left - 1,
                    );
                },
            );
            return;
        }

        let Some(bounds) = bounds else {
            return;
        };
        let viewport_height = f64::from(scroller.height().max(1));
        let row_center = adjustment.value() + f64::from(bounds.y() + bounds.height() / 2.0);
        let target = row_center - viewport_height / 2.0;
        let upper = adjustment.upper() - adjustment.page_size();
        let target = target.clamp(adjustment.lower(), upper.max(adjustment.lower()));
        let start = adjustment.value();
        let delta = target - start;
        if duration_millis == 0 || delta.abs() < 1.0 {
            adjustment.set_value(target);
            return;
        }
        let started_at = Instant::now();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if scroll_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let elapsed = started_at.elapsed().as_millis() as f64;
            let progress = (elapsed / duration_millis as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - progress).powi(3);
            adjustment.set_value(start + delta * eased);
            if progress >= 1.0 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    });
}

pub fn active_lyrics_line_index(lines: &[LyricLine], position_millis: u64) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let start = line.start_millis?;
            (start <= position_millis).then_some((
                lyric_line_has_text(line).then_some(index),
                start,
                index,
            ))
        })
        .max_by_key(|(_, start, index)| (*start, *index))
        .and_then(|(index, _, _)| index)
}

pub fn should_highlight_all_lyrics_lines(lines: &[LyricLine]) -> bool {
    !lines.is_empty() && lines.iter().all(|line| line.start_millis.is_none())
}

pub fn next_lyrics_line_start_after(lines: &[LyricLine], position_millis: u64) -> Option<u64> {
    lines
        .iter()
        .filter_map(|line| line.start_millis)
        .filter(|start| *start > position_millis)
        .min()
}

pub fn lyrics_follow_scroll_pause_state(
    paused_until: Option<Instant>,
    now: Instant,
) -> LyricsFollowScrollPause {
    match paused_until {
        Some(paused_until) if now < paused_until => LyricsFollowScrollPause::Active,
        Some(_) => LyricsFollowScrollPause::Expired,
        None => LyricsFollowScrollPause::Inactive,
    }
}

pub fn lyrics_follow_scroll_target(
    active_index: Option<usize>,
    previous_index: Option<usize>,
    follow_pause: LyricsFollowScrollPause,
) -> Option<usize> {
    if follow_pause == LyricsFollowScrollPause::Active {
        return None;
    }
    active_index.filter(|index| {
        follow_pause == LyricsFollowScrollPause::Expired || Some(*index) != previous_index
    })
}

pub fn lyrics_scroll_animation_millis(
    lines: &[LyricLine],
    active_index: usize,
    position_millis: u64,
) -> u64 {
    let budget = lines
        .iter()
        .skip(active_index + 1)
        .filter(|line| lyric_line_has_text(line))
        .filter_map(|line| line.start_millis)
        .find(|start| *start > position_millis)
        .and_then(|next_start| {
            next_start
                .saturating_sub(position_millis)
                .checked_sub(LYRICS_SCROLL_MS)
        });
    budget
        .map(|budget| {
            budget.clamp(
                MIN_LYRICS_SCROLL_ANIMATION_MS,
                DEFAULT_LYRICS_SCROLL_ANIMATION_MS,
            )
        })
        .unwrap_or(DEFAULT_LYRICS_SCROLL_ANIMATION_MS)
}

fn lyric_line_has_text(line: &LyricLine) -> bool {
    !line.text.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        LyricsFollowScrollPause, active_lyrics_line_index, lyrics_follow_scroll_pause_state,
        lyrics_follow_scroll_target, lyrics_scroll_animation_millis, next_lyrics_line_start_after,
        should_highlight_all_lyrics_lines,
    };
    use source::LyricLine;
    use std::time::{Duration, Instant};

    #[test]
    fn sync_lyrics_started() {
        let lines = vec![
            LyricLine {
                text: "intro".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "verse".to_string(),
                start_millis: Some(5_500),
            },
            LyricLine {
                text: "unsynced".to_string(),
                start_millis: None,
            },
            LyricLine {
                text: "chorus".to_string(),
                start_millis: Some(9_000),
            },
        ];

        assert_eq!(active_lyrics_line_index(&lines, 999), None);
        assert_eq!(active_lyrics_line_index(&lines, 1_000), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 5_499), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 5_500), Some(1));
        assert_eq!(active_lyrics_line_index(&lines, 8_999), Some(1));
        assert_eq!(active_lyrics_line_index(&lines, 9_000), Some(3));
    }

    #[test]
    fn lyrics_blank_line_clears_highlight() {
        let lines = vec![
            LyricLine {
                text: "current".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "".to_string(),
                start_millis: Some(5_000),
            },
            LyricLine {
                text: "next".to_string(),
                start_millis: Some(9_000),
            },
        ];

        assert_eq!(active_lyrics_line_index(&lines, 4_999), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 5_000), None);
        assert_eq!(active_lyrics_line_index(&lines, 8_999), None);
        assert_eq!(active_lyrics_line_index(&lines, 9_000), Some(2));
    }

    #[test]
    fn lyrics_keep_active() {
        let lines = vec![
            LyricLine {
                text: "last".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: " ".to_string(),
                start_millis: Some(5_000),
            },
        ];

        assert_eq!(active_lyrics_line_index(&lines, 5_000), None);
        assert_eq!(active_lyrics_line_index(&lines, 50_000), None);
    }

    #[test]
    fn unsynchronized_lyrics_timed() {
        let lines = vec![LyricLine {
            text: "plain".to_string(),
            start_millis: None,
        }];

        assert_eq!(active_lyrics_line_index(&lines, 0), None);
    }

    #[test]
    fn unsynchronized_lyrics_highlight() {
        let lines = vec![
            LyricLine {
                text: "first".to_string(),
                start_millis: None,
            },
            LyricLine {
                text: "second".to_string(),
                start_millis: None,
            },
        ];

        assert!(should_highlight_all_lyrics_lines(&lines));
    }

    #[test]
    fn sync_lyrics_every() {
        let lines = vec![
            LyricLine {
                text: "first".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "untimed note".to_string(),
                start_millis: None,
            },
        ];

        assert!(!should_highlight_all_lyrics_lines(&lines));
        assert!(!should_highlight_all_lyrics_lines(&[]));
    }

    #[test]
    fn lyrics_schedule_line() {
        let lines = vec![
            LyricLine {
                text: "intro".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "verse".to_string(),
                start_millis: Some(5_500),
            },
            LyricLine {
                text: "unsynced".to_string(),
                start_millis: None,
            },
            LyricLine {
                text: "chorus".to_string(),
                start_millis: Some(9_000),
            },
        ];

        assert_eq!(next_lyrics_line_start_after(&lines, 999), Some(1_000));
        assert_eq!(next_lyrics_line_start_after(&lines, 1_000), Some(5_500));
        assert_eq!(next_lyrics_line_start_after(&lines, 5_499), Some(5_500));
        assert_eq!(next_lyrics_line_start_after(&lines, 5_500), Some(9_000));
        assert_eq!(next_lyrics_line_start_after(&lines, 9_000), None);
    }

    #[test]
    fn lyrics_schedule_boundary() {
        let lines = vec![
            LyricLine {
                text: "current".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "".to_string(),
                start_millis: Some(5_000),
            },
            LyricLine {
                text: "next".to_string(),
                start_millis: Some(9_000),
            },
        ];

        assert_eq!(next_lyrics_line_start_after(&lines, 4_999), Some(5_000));
        assert_eq!(next_lyrics_line_start_after(&lines, 5_000), Some(9_000));
    }

    #[test]
    fn lyrics_finish_line() {
        let lines = vec![
            LyricLine {
                text: "current".to_string(),
                start_millis: Some(5_500),
            },
            LyricLine {
                text: "next".to_string(),
                start_millis: Some(6_000),
            },
        ];

        let duration = lyrics_scroll_animation_millis(&lines, 0, 5_500);

        assert!(duration <= 300);
        assert!(duration >= 80);
        assert_eq!(
            lyrics_scroll_animation_millis(&lines, 0, 5_501),
            duration - 1
        );
    }

    #[test]
    fn lyrics_follow_scroll_pause_expires() {
        let now = Instant::now();

        assert_eq!(
            lyrics_follow_scroll_pause_state(None, now),
            LyricsFollowScrollPause::Inactive
        );
        assert_eq!(
            lyrics_follow_scroll_pause_state(Some(now + Duration::from_millis(1)), now),
            LyricsFollowScrollPause::Active
        );
        assert_eq!(
            lyrics_follow_scroll_pause_state(Some(now), now),
            LyricsFollowScrollPause::Expired
        );
    }

    #[test]
    fn lyrics_ignore_line() {
        assert_eq!(
            lyrics_follow_scroll_target(Some(3), Some(3), LyricsFollowScrollPause::Inactive),
            None
        );
        assert_eq!(
            lyrics_follow_scroll_target(Some(4), Some(3), LyricsFollowScrollPause::Inactive),
            Some(4)
        );
        assert_eq!(
            lyrics_follow_scroll_target(Some(3), Some(3), LyricsFollowScrollPause::Expired),
            Some(3)
        );
        assert_eq!(
            lyrics_follow_scroll_target(Some(4), Some(3), LyricsFollowScrollPause::Active),
            None
        );
    }
}
