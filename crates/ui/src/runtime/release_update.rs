//! Release-update results presented by the UI.
//!
//! Rufin owns acquisition and notification policy. UI owns only when to start
//! the launch check and how to present its complete result.

use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseNote {
    pub version: String,
    pub date: String,
    pub summary: Option<String>,
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseUpdate {
    pub notes: Arc<[ReleaseNote]>,
    pub notification_version: Option<String>,
}

pub trait ReleaseUpdatePort: Send + Sync {
    fn check(&self);
    fn mark_seen(&self, version: String) -> Result<(), String>;
}

pub type ReleaseUpdateHandle = Arc<dyn ReleaseUpdatePort>;
