use std::sync::Arc;

pub trait DiagnosticsPort: Send + Sync {
    fn debug_enabled(&self) -> bool;
    fn set_debug_enabled(&self, enabled: bool) -> Result<(), String>;
    fn revision(&self) -> u64;
    fn snapshot(&self) -> String;
}

pub type DiagnosticsHandle = Arc<dyn DiagnosticsPort>;
