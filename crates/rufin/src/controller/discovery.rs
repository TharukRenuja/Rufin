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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerDiscoveryStatus {
    Idle,
    Searching,
    Empty,
    Found(u64),
    Failed(String),
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
                status: ServerDiscoveryStatus::Searching,
                running: true,
            });

            match discover_jellyfin_servers(SERVER_DISCOVERY_TIMEOUT) {
                Ok(servers) => {
                    let servers: Vec<DiscoveredServer> =
                        servers.into_iter().map(DiscoveredServer::from).collect();
                    let status = discovery_finished_status(servers.len());
                    let _sent = events.send(ControllerEvent::ServerDiscovery {
                        servers,
                        status,
                        running: false,
                    });
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::ServerDiscovery {
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
