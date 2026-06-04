use super::*;

impl AppController {
    #[cfg(test)]
    pub fn forget_active_server(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = self.sync_in_flight.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Snapshot(Box::new(
                    LibrarySnapshot::first_run(),
                )));
                return;
            };
            if let Err(error) = cancel_sync_if_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let result = store.with_store(|store| {
                store.forget_server(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_store_disk_cover_cache(&store, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            clear_queue_and_stop_playback(
                &queue,
                &playback_request_generation,
                &playback,
                &playback_snapshot,
                &auto_dj_enabled,
                &events,
            );
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(
                LibrarySnapshot::first_run(),
            )));
            delete_token_after_forget(secrets, saved.server.id);
        });
    }
    pub fn forget_server(&self, server_id: ServerId) {
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = self.sync_in_flight.clone();
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                let active_id = store.active_server()?.map(|saved| saved.server.id);
                let saved = store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id);
                Ok((saved, active_id))
            }) {
                Ok((Some(saved), active_id)) => (saved, active_id),
                Ok((None, _)) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected server is no longer saved.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let (saved, active_id) = saved;
            if let Err(error) = cancel_sync_if_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let mut settings = load_settings_from_store(&store);
            if settings.sources.selected
                == Some(LibrarySourceSelection::Server(saved.server.id.clone()))
            {
                settings.sources.selected = None;
                if let Err(error) = store.save_settings(&settings) {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.forget_server(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_store_disk_cover_cache(&store, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if active_id.as_ref() == Some(&saved.server.id) {
                clear_queue_and_stop_playback(
                    &queue,
                    &playback_request_generation,
                    &playback,
                    &playback_snapshot,
                    &auto_dj_enabled,
                    &events,
                );
            }
            emit_snapshot(&store, &events);
            delete_token_after_forget(secrets, saved.server.id);
        });
    }
    #[instrument(skip(self, request), fields(provider = request.provider.provider_id(), server_url = %request.server_url, username = %request.username, trust_invalid_cert = request.trust_invalid_cert))]
    pub fn login(&self, request: LoginRequest) {
        let LoginRequest {
            provider,
            server_url,
            username,
            password,
            trust_invalid_cert,
            local_access_root,
            path_replace_from,
        } = request;
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let runtime = Arc::clone(&sync_context.runtime);
        let secrets = Arc::clone(&sync_context.secrets);
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback_request_generation = Arc::clone(&self.playback_request_generation);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let provider_name = provider.title();
            let _sent = events.send(ControllerEvent::LoginStatus(format!(
                "Checking {provider_name} server…"
            )));
            let device_id = if provider == StreamingProvider::Jellyfin {
                match ensure_jellyfin_device_id(&store) {
                    Ok(device_id) => Some(device_id),
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                }
            } else {
                None
            };
            let result = runtime.block_on(login_provider(
                provider,
                server_url,
                username,
                password,
                trust_invalid_cert,
                device_id,
            ));

            let session = match result {
                Ok(session) => session,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error.to_string()));
                    return;
                }
            };

            let activation_context = LoginActivationContext {
                store: &store,
                queue: &queue,
                playback_request_generation: &playback_request_generation,
                playback: &playback,
                playback_snapshot: &playback_snapshot,
                auto_dj_enabled: &auto_dj_enabled,
                events: &events,
            };
            let activation_request = LoginActivationRequest {
                session: &session,
                trust_invalid_cert,
                local_access_root: local_access_root.as_deref(),
                path_replace_from: path_replace_from.as_deref(),
            };
            let saved = match activate_with_token(&activation_context, &secrets, activation_request)
            {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };

            start_sync_thread(sync_context, saved);
        });
    }
}

pub(in crate::controller) fn delete_token_after_forget(
    secrets: Arc<dyn SecretStore>,
    server_id: ServerId,
) {
    thread::spawn(move || {
        if let Err(error) = secrets.delete_token(&server_id) {
            warn!(%error, server_id = %server_id, "failed to delete forgotten server token");
        }
    });
}
