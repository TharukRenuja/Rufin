//! Durable source downloads exposed through Library's local playback path.

mod actor;
mod storage;

pub use actor::{
    DownloadEvent, DownloadFeedback, DownloadFeedbackKind, DownloadQuality, DownloadQueueItem,
    DownloadQueueSnapshot, DownloadQueueState, DownloadRule, DownloadSubject, Downloads,
};
