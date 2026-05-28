use super::*;

impl AppController {
    pub(in crate::controller) fn with_queue_mut<T>(
        &self,
        operation: impl FnOnce(&mut QueueEngine) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "queue lock was poisoned".to_string())?;
        let Some(queue) = queue.as_mut() else {
            return Err("No active queue is available.".to_string());
        };
        operation(queue)
    }
}
