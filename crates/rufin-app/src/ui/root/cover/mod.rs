use super::*;

mod cache_lookup;
mod decode_queue;
mod size_helpers;
mod tiles;
mod warming;

pub(in crate::ui) use cache_lookup::{
    VisibleCoverCacheMissAction, visible_cover_cache_miss_action,
};
use size_helpers::*;

#[derive(Clone)]
pub(in crate::ui) struct CoverBinding {
    pub(in crate::ui) tile: ArtworkTileWeak,
    pub(in crate::ui) generation: u64,
    pub(in crate::ui) clear_on_failure: bool,
}

#[derive(Clone)]
pub(in crate::ui) struct DecodedCover {
    pub(in crate::ui) pixbuf: Pixbuf,
    pub(in crate::ui) size: i32,
    pub(in crate::ui) bytes: usize,
    pub(in crate::ui) last_used: u64,
    pub(in crate::ui) priority: CoverDecodePriority,
}

pub(in crate::ui) struct DecodedCoverOrderEntry {
    pub(in crate::ui) key: String,
    pub(in crate::ui) last_used: u64,
}

pub(in crate::ui) struct CoverDecodeJob {
    pub(in crate::ui) key: String,
    pub(in crate::ui) path: PathBuf,
    pub(in crate::ui) size: i32,
    pub(in crate::ui) priority: CoverDecodePriority,
    pub(in crate::ui) requires_live_binding: bool,
}

pub(in crate::ui) struct CoverWarmJob {
    pub(in crate::ui) key: String,
    pub(in crate::ui) image_ref: ImageRef,
    pub(in crate::ui) fetch_size: u32,
    pub(in crate::ui) size: i32,
}

pub(in crate::ui) struct CoverPathLookupRequest {
    pub(in crate::ui) key: String,
    pub(in crate::ui) image_ref: ImageRef,
    pub(in crate::ui) fetch_size: u32,
    pub(in crate::ui) size: i32,
    pub(in crate::ui) intent: CoverPathLookupIntent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum CoverPathLookupIntent {
    Visible,
    Priority,
    StartupPrime,
    Warm,
}

impl CoverPathLookupIntent {
    fn coalesce(self, next: Self) -> Self {
        match (self, next) {
            (Self::Visible, _) | (_, Self::Visible) => Self::Visible,
            (Self::Priority, _) | (_, Self::Priority) => Self::Priority,
            (Self::StartupPrime, _) | (_, Self::StartupPrime) => Self::StartupPrime,
            _ => Self::Warm,
        }
    }
}

pub(in crate::ui) fn record_cover_path_lookup_request(
    lookups: &mut HashMap<String, CoverPathLookupIntent>,
    key: String,
    intent: CoverPathLookupIntent,
) -> bool {
    if let Some(existing) = lookups.get_mut(&key) {
        *existing = existing.coalesce(intent);
        false
    } else {
        lookups.insert(key, intent);
        true
    }
}

pub(in crate::ui) struct FirstRunCoverPrimeJob {
    pub(in crate::ui) key: String,
    pub(in crate::ui) image_ref: ImageRef,
    pub(in crate::ui) fetch_size: u32,
    pub(in crate::ui) size: i32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::ui) enum CoverDecodePriority {
    Visible,
    Warm,
}

impl CoverDecodePriority {
    pub(in crate::ui) fn glib_priority(self) -> glib::Priority {
        match self {
            Self::Visible => glib::Priority::DEFAULT_IDLE,
            Self::Warm => glib::Priority::LOW,
        }
    }
}

pub(in crate::ui) fn retain_current_priority_cover_work(
    lookups: &mut HashMap<String, CoverPathLookupIntent>,
    queue: &mut VecDeque<CoverDecodeJob>,
    keep: &HashSet<String>,
) {
    lookups.retain(|key, intent| {
        !matches!(
            intent,
            CoverPathLookupIntent::Priority | CoverPathLookupIntent::StartupPrime
        ) || keep.contains(key)
    });
    queue.retain(|job| {
        job.priority != CoverDecodePriority::Visible
            || job.requires_live_binding
            || keep.contains(&job.key)
    });
}

pub(in crate::ui) fn clear_queued_route_cover_work(
    lookups: &mut HashMap<String, CoverPathLookupIntent>,
    queue: &mut VecDeque<CoverDecodeJob>,
) {
    lookups.retain(|_, intent| *intent == CoverPathLookupIntent::Warm);
    queue.retain(|job| job.priority == CoverDecodePriority::Warm);
}

pub(in crate::ui) fn queue_cover_decode_job(
    queue: &mut VecDeque<CoverDecodeJob>,
    job: CoverDecodeJob,
) {
    if job.priority == CoverDecodePriority::Visible {
        let insertion_index = queue
            .iter()
            .position(|queued| queued.priority == CoverDecodePriority::Warm)
            .unwrap_or(queue.len());
        queue.insert(insertion_index, job);
    } else {
        queue.push_back(job);
    }
}

pub(in crate::ui) fn cover_decode_has_capacity(
    active: &HashMap<String, CoverDecodePriority>,
    priority: CoverDecodePriority,
) -> bool {
    match priority {
        CoverDecodePriority::Visible => {
            active
                .values()
                .filter(|active_priority| **active_priority == CoverDecodePriority::Visible)
                .count()
                < COVER_VISIBLE_DECODE_MAX_IN_FLIGHT
        }
        CoverDecodePriority::Warm => active.len() < COVER_DECODE_MAX_IN_FLIGHT,
    }
}

#[cfg(test)]
mod priority_work_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn visible_viewport_work_drops_stale_priority_backlog() {
        let mut lookups = HashMap::from([
            ("old-priority".to_string(), CoverPathLookupIntent::Priority),
            (
                "current-priority".to_string(),
                CoverPathLookupIntent::Priority,
            ),
            ("live-visible".to_string(), CoverPathLookupIntent::Visible),
            ("background-warm".to_string(), CoverPathLookupIntent::Warm),
        ]);
        let mut queue = VecDeque::from([
            CoverDecodeJob {
                key: "old-priority".to_string(),
                path: PathBuf::from("/tmp/old-cover.jpg"),
                size: 96,
                priority: CoverDecodePriority::Visible,
                requires_live_binding: false,
            },
            CoverDecodeJob {
                key: "current-priority".to_string(),
                path: PathBuf::from("/tmp/current-cover.jpg"),
                size: 96,
                priority: CoverDecodePriority::Visible,
                requires_live_binding: false,
            },
            CoverDecodeJob {
                key: "live-visible".to_string(),
                path: PathBuf::from("/tmp/live-cover.jpg"),
                size: 96,
                priority: CoverDecodePriority::Visible,
                requires_live_binding: true,
            },
            CoverDecodeJob {
                key: "background-warm".to_string(),
                path: PathBuf::from("/tmp/warm-cover.jpg"),
                size: 96,
                priority: CoverDecodePriority::Warm,
                requires_live_binding: false,
            },
        ]);
        let keep = HashSet::from(["current-priority".to_string()]);

        retain_current_priority_cover_work(&mut lookups, &mut queue, &keep);

        assert!(!lookups.contains_key("old-priority"));
        assert!(lookups.contains_key("current-priority"));
        assert!(lookups.contains_key("live-visible"));
        assert!(lookups.contains_key("background-warm"));

        let queued_keys = queue.iter().map(|job| job.key.as_str()).collect::<Vec<_>>();
        assert_eq!(
            queued_keys,
            vec!["current-priority", "live-visible", "background-warm"]
        );
    }

    #[test]
    fn visible_decode_jobs_keep_request_order_ahead_of_warm_work() {
        let mut queue = VecDeque::from([decode_job("warm-old", CoverDecodePriority::Warm)]);

        queue_cover_decode_job(
            &mut queue,
            decode_job("visible-first", CoverDecodePriority::Visible),
        );
        queue_cover_decode_job(
            &mut queue,
            decode_job("visible-second", CoverDecodePriority::Visible),
        );
        queue_cover_decode_job(
            &mut queue,
            decode_job("warm-new", CoverDecodePriority::Warm),
        );

        let queued_keys = queue.iter().map(|job| job.key.as_str()).collect::<Vec<_>>();
        assert_eq!(
            queued_keys,
            vec!["visible-first", "visible-second", "warm-old", "warm-new"]
        );
    }

    #[test]
    fn visible_decode_capacity_is_independent_from_warm_lane() {
        let active = (0..COVER_DECODE_MAX_IN_FLIGHT)
            .map(|index| (format!("warm-{index}"), CoverDecodePriority::Warm))
            .collect::<HashMap<_, _>>();

        assert!(cover_decode_has_capacity(
            &active,
            CoverDecodePriority::Visible
        ));
        assert!(!cover_decode_has_capacity(
            &active,
            CoverDecodePriority::Warm
        ));
    }

    #[test]
    fn route_change_drops_visible_work_but_keeps_background_warm_work() {
        let mut lookups = HashMap::from([
            ("old-visible".to_string(), CoverPathLookupIntent::Visible),
            ("old-priority".to_string(), CoverPathLookupIntent::Priority),
            ("background-warm".to_string(), CoverPathLookupIntent::Warm),
        ]);
        let mut queue = VecDeque::from([
            decode_job("old-visible", CoverDecodePriority::Visible),
            decode_job("background-warm", CoverDecodePriority::Warm),
        ]);

        clear_queued_route_cover_work(&mut lookups, &mut queue);

        assert_eq!(
            lookups,
            HashMap::from([("background-warm".to_string(), CoverPathLookupIntent::Warm)])
        );
        let queued_keys = queue.iter().map(|job| job.key.as_str()).collect::<Vec<_>>();
        assert_eq!(queued_keys, vec!["background-warm"]);
    }

    fn decode_job(key: &str, priority: CoverDecodePriority) -> CoverDecodeJob {
        CoverDecodeJob {
            key: key.to_string(),
            path: PathBuf::from("/tmp/cached-cover.jpg"),
            size: 96,
            priority,
            requires_live_binding: false,
        }
    }
}
