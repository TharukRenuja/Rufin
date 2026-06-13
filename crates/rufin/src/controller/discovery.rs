use std::thread;
use std::time::Duration;

use super::{AppController, ControllerEvent};
use crate::providers::{DiscoveredJellyfinServer, discover_jellyfin_servers};

const SERVER_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(1_800);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredServer {
    pub provider: String,
    pub name: String,
    pub address: String,
    pub id: Option<String>,
}

impl From<DiscoveredJellyfinServer> for DiscoveredServer {
    fn from(server: DiscoveredJellyfinServer) -> Self {
        Self {
            provider: "Jellyfin".to_string(),
            name: server.name,
            address: server.address,
            id: server.id,
        }
    }
}

impl AppController {
    pub fn discover_servers(&self) {
        let events = self.events.clone();
        thread::spawn(move || {
            let _sent = events.send(ControllerEvent::ServerDiscovery {
                servers: Vec::new(),
                status: "Searching for Jellyfin servers on the local network…".to_string(),
                running: true,
            });

            match discover_jellyfin_servers(SERVER_DISCOVERY_TIMEOUT) {
                Ok(servers) => {
                    let servers: Vec<DiscoveredServer> =
                        servers.into_iter().map(DiscoveredServer::from).collect();
                    let status = discovery_finished_status(&servers);
                    let _sent = events.send(ControllerEvent::ServerDiscovery {
                        servers,
                        status,
                        running: false,
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::ServerDiscovery {
                        servers: Vec::new(),
                        status: format!("Server discovery failed: {error}"),
                        running: false,
                    });
                }
            }
        });
    }
}

fn discovery_finished_status(servers: &[DiscoveredServer]) -> String {
    match servers.len() {
        0 => "No Jellyfin servers found. Enter the address manually or search again".to_string(),
        1 => "Found 1 Jellyfin server".to_string(),
        count => format!("Found {count} Jellyfin servers"),
    }
}
