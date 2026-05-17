use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use rufin_provider::{ProviderError, ProviderResult};
use serde::Deserialize;
use tracing::instrument;

use crate::normalize_base_url;

const JELLYFIN_DISCOVERY_PORT: u16 = 7359;
const JELLYFIN_DISCOVERY_PACKET_LIMIT: usize = 4096;
const JELLYFIN_DISCOVERY_TIMEOUT_SLICE: Duration = Duration::from_millis(200);
const JELLYFIN_DISCOVERY_MESSAGES: &[&[u8]] =
    &[b"Who is JellyfinServer?", b"who is JellyfinServer?"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredJellyfinServer {
    pub id: Option<String>,
    pub name: String,
    pub address: String,
    pub endpoint_address: Option<String>,
}

#[instrument(skip_all, fields(timeout_ms = timeout.as_millis()))]
pub fn discover_jellyfin_servers(
    timeout: Duration,
) -> ProviderResult<Vec<DiscoveredJellyfinServer>> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|error| map_io_error("bind Jellyfin discovery socket", error))?;
    socket
        .set_broadcast(true)
        .map_err(|error| map_io_error("enable Jellyfin discovery broadcast", error))?;
    socket
        .set_read_timeout(Some(JELLYFIN_DISCOVERY_TIMEOUT_SLICE))
        .map_err(|error| map_io_error("set Jellyfin discovery timeout", error))?;

    let targets = [
        SocketAddrV4::new(Ipv4Addr::BROADCAST, JELLYFIN_DISCOVERY_PORT),
        SocketAddrV4::new(Ipv4Addr::new(127, 255, 255, 255), JELLYFIN_DISCOVERY_PORT),
    ];
    let mut sent_any = false;
    let mut last_send_error = None;
    for target in targets {
        for message in JELLYFIN_DISCOVERY_MESSAGES {
            match socket.send_to(message, target) {
                Ok(_) => sent_any = true,
                Err(error) => last_send_error = Some(error),
            }
        }
    }
    if !sent_any {
        return Err(map_io_error(
            "send Jellyfin discovery broadcast",
            last_send_error.unwrap_or_else(|| io::Error::other("no discovery targets")),
        ));
    }

    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; JELLYFIN_DISCOVERY_PACKET_LIMIT];
    let mut servers = Vec::new();
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((size, _source)) => {
                if let Some(server) = discovered_server_from_packet(&buffer[..size]) {
                    push_discovered_server(&mut servers, server);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                return Err(map_io_error("receive Jellyfin discovery response", error));
            }
        }
    }

    servers.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.address.cmp(&right.address))
    });
    Ok(servers)
}

fn map_io_error(context: &str, error: io::Error) -> ProviderError {
    ProviderError::Network(format!("{context}: {error}"))
}

fn discovered_server_from_packet(packet: &[u8]) -> Option<DiscoveredJellyfinServer> {
    let response: JellyfinDiscoveryResponse = serde_json::from_slice(packet).ok()?;
    let address = response
        .address
        .as_deref()
        .and_then(normalize_discovered_address)
        .or_else(|| {
            response
                .endpoint_address
                .as_deref()
                .and_then(normalize_discovered_address)
        })?;
    Some(DiscoveredJellyfinServer {
        id: response.id.filter(|id| !id.trim().is_empty()),
        name: response
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Jellyfin".to_string()),
        address,
        endpoint_address: response.endpoint_address,
    })
}

fn normalize_discovered_address(raw: &str) -> Option<String> {
    normalize_base_url(raw)
        .ok()
        .map(|url| url.as_str().trim_end_matches('/').to_string())
}

fn push_discovered_server(
    servers: &mut Vec<DiscoveredJellyfinServer>,
    server: DiscoveredJellyfinServer,
) {
    if servers.iter().any(|existing| {
        existing.address == server.address
            || existing
                .id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty() && server.id.as_deref() == Some(id))
    }) {
        return;
    }
    servers.push(server);
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinDiscoveryResponse {
    address: Option<String>,
    id: Option<String>,
    name: Option<String>,
    endpoint_address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_response_maps_server_address() {
        let packet = serde_json::json!({
            "Address": "http://music.local:8096/",
            "Id": "server-one",
            "Name": "Music Box",
            "EndpointAddress": "192.0.2.10:8096"
        })
        .to_string();

        let server = discovered_server_from_packet(packet.as_bytes()).expect("discovered server");

        assert_eq!(server.id.as_deref(), Some("server-one"));
        assert_eq!(server.name, "Music Box");
        assert_eq!(server.address, "http://music.local:8096");
        assert_eq!(server.endpoint_address.as_deref(), Some("192.0.2.10:8096"));
    }

    #[test]
    fn discovery_response_falls_back_to_endpoint_address() {
        let packet = serde_json::json!({
            "Id": "server-one",
            "EndpointAddress": "192.0.2.10:8096"
        })
        .to_string();

        let server = discovered_server_from_packet(packet.as_bytes()).expect("discovered server");

        assert_eq!(server.name, "Jellyfin");
        assert_eq!(server.address, "http://192.0.2.10:8096");
    }

    #[test]
    fn discovery_results_deduplicate_by_id_and_address() {
        let mut servers = Vec::new();
        push_discovered_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://music.local:8096".to_string(),
                endpoint_address: None,
            },
        );
        push_discovered_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://192.0.2.10:8096".to_string(),
                endpoint_address: None,
            },
        );
        push_discovered_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: None,
                name: "Music Box".to_string(),
                address: "http://music.local:8096".to_string(),
                endpoint_address: None,
            },
        );

        assert_eq!(servers.len(), 1);
    }
}
