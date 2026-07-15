use std::thread;
use std::time::Duration;

use super::SourceCommands;
use sources::{
    DiscoveredServer, ServerDiscoveryStatus, ServerDiscoveryUpdate,
    jellyfin::discover_jellyfin_servers,
};

const SERVER_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(1_800);

impl SourceCommands {
    pub fn discover_servers(&self) {
        let discovery_events = self.source_events.discovery.clone();
        thread::spawn(move || {
            let _sent = discovery_events.try_send(ServerDiscoveryUpdate {
                servers: Vec::new(),
                status: ServerDiscoveryStatus::Searching,
                running: true,
            });

            match discover_jellyfin_servers(SERVER_DISCOVERY_TIMEOUT) {
                Ok(servers) => {
                    let servers: Vec<DiscoveredServer> =
                        servers.into_iter().map(DiscoveredServer::from).collect();
                    let status = discovery_finished_status(servers.len());
                    let _sent = discovery_events.try_send(ServerDiscoveryUpdate {
                        servers,
                        status,
                        running: false,
                    });
                }
                Err(error) => {
                    let _sent = discovery_events.try_send(ServerDiscoveryUpdate {
                        servers: Vec::new(),
                        status: ServerDiscoveryStatus::Failed(error.to_string()),
                        running: false,
                    });
                }
            }
        });
    }
}

fn discovery_finished_status(count: usize) -> ServerDiscoveryStatus {
    match count {
        0 => ServerDiscoveryStatus::Empty,
        count => ServerDiscoveryStatus::Found(count as u64),
    }
}
