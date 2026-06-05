use super::*;

impl AppController {
    pub fn clear_app_cache() -> Result<(), String> {
        let store = StoreHandle::open_for_app()?;
        let Some(saved) = store.with_store(|store| store.active_server())? else {
            return Err("No active server is saved.".to_string());
        };
        store.with_store(|store| {
            store.clear_library_cache(&saved.server.id)?;
            Ok(())
        })?;
        clear_disk_cover_cache(&saved.server.id)?;
        clear_disk_waveform_cache(&saved.server.id)?;
        Ok(())
    }
    pub fn forget_active_server_for_app() -> Result<(), String> {
        let store = StoreHandle::open_for_app()?;
        let Some(saved) = store.with_store(|store| store.active_server())? else {
            return Err("No active server is saved.".to_string());
        };
        store.with_store(|store| {
            store.forget_server(&saved.server.id)?;
            Ok(())
        })?;
        clear_disk_cover_cache(&saved.server.id)?;
        clear_disk_waveform_cache(&saved.server.id)?;
        if let Err(error) = platform_token_store().delete_token(&saved.server.id) {
            warn!(%error, server_id = %saved.server.id, "failed to delete forgotten server token");
        }
        Ok(())
    }
}
