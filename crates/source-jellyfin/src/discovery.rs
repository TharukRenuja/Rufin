use std::io;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use serde::Deserialize;
use source::{SourceError, SourceResult};
use tracing::instrument;

use crate::root::normalize_base_url;

const JELLYFIN_DISCOVERY_PORT: u16 = 7359;
const JELLYFIN_DISCOVERY_PACKET_LIMIT: usize = 4096;
const JELLYFIN_DISCOVERY_TIMEOUT_SLICE: Duration = Duration::from_millis(200);
const JELLYFIN_DISCOVERY_MESSAGES: &[&[u8]] =
    &[b"Who is JellyfinServer?", b"who is JellyfinServer?"];
const JELLYFIN_LOCALHOST_PORTS: &[u16] = &[8096];
const JELLYFIN_LOCALHOST_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const JELLYFIN_LOCALHOST_RESPONSE_LIMIT: u64 = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredJellyfinServer {
    pub id: Option<String>,
    pub name: String,
    pub address: String,
    pub endpoint_address: Option<String>,
}

#[instrument(skip_all, fields(timeout_ms = timeout.as_millis()))]
pub fn discover_jellyfin_servers(timeout: Duration) -> SourceResult<Vec<DiscoveredJellyfinServer>> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|error| map_io_error("bind Jellyfin discovery socket", error))?;
    socket
        .set_broadcast(true)
        .map_err(|error| map_io_error("enable Jellyfin discovery broadcast", error))?;
    socket
        .set_read_timeout(Some(JELLYFIN_DISCOVERY_TIMEOUT_SLICE))
        .map_err(|error| map_io_error("set Jellyfin discovery timeout", error))?;

    let mut sent_any = false;
    let mut last_send_error = None;
    for target in discovery_targets() {
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

    for server in discover_localhost_servers() {
        push_discovered_server(&mut servers, server);
    }

    servers.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.address.cmp(&right.address))
    });
    Ok(servers)
}

fn discovery_targets() -> Vec<SocketAddrV4> {
    vec![
        SocketAddrV4::new(Ipv4Addr::BROADCAST, JELLYFIN_DISCOVERY_PORT),
        SocketAddrV4::new(Ipv4Addr::new(127, 255, 255, 255), JELLYFIN_DISCOVERY_PORT),
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, JELLYFIN_DISCOVERY_PORT),
    ]
}

fn map_io_error(context: &str, error: io::Error) -> SourceError {
    SourceError::Network(format!("{context}: {error}"))
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

fn discover_localhost_servers() -> Vec<DiscoveredJellyfinServer> {
    localhost_probe_targets()
        .into_iter()
        .filter_map(probe_localhost_server)
        .collect()
}

fn localhost_probe_targets() -> Vec<LocalhostProbeTarget> {
    JELLYFIN_LOCALHOST_PORTS
        .iter()
        .flat_map(|port| {
            [
                LocalhostProbeTarget {
                    socket: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), *port),
                    host_header: format!("localhost:{port}"),
                    base_url: format!("http://localhost:{port}"),
                },
                LocalhostProbeTarget {
                    socket: SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), *port),
                    host_header: format!("localhost:{port}"),
                    base_url: format!("http://localhost:{port}"),
                },
            ]
        })
        .collect()
}

fn probe_localhost_server(target: LocalhostProbeTarget) -> Option<DiscoveredJellyfinServer> {
    let mut stream =
        TcpStream::connect_timeout(&target.socket, JELLYFIN_LOCALHOST_PROBE_TIMEOUT).ok()?;
    stream
        .set_read_timeout(Some(JELLYFIN_LOCALHOST_PROBE_TIMEOUT))
        .ok()?;
    stream
        .set_write_timeout(Some(JELLYFIN_LOCALHOST_PROBE_TIMEOUT))
        .ok()?;
    let request = format!(
        "GET /System/Info/Public HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        target.host_header
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream
        .take(JELLYFIN_LOCALHOST_RESPONSE_LIMIT)
        .read_to_end(&mut response)
        .ok()?;
    let body = http_response_body(&response)?;
    discovered_server_from_public_info(target.base_url.as_str(), target.socket.to_string(), &body)
}

fn http_response_body(response: &[u8]) -> Option<Vec<u8>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?
        + 4;
    let status_line = response.split(|byte| *byte == b'\n').next()?;
    if !status_line.starts_with(b"HTTP/1.1 200") && !status_line.starts_with(b"HTTP/1.0 200") {
        return None;
    }
    let headers = String::from_utf8_lossy(&response[..header_end]).to_ascii_lowercase();
    let body = &response[header_end..];
    if headers.contains("\r\ntransfer-encoding: chunked") {
        decode_chunked_body(body)
    } else {
        Some(body.to_vec())
    }
}

fn decode_chunked_body(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body.windows(2).position(|window| window == b"\r\n")?;
        let size_text = std::str::from_utf8(&body[..line_end])
            .ok()?
            .split(';')
            .next()?
            .trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Some(decoded);
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            return None;
        }
        decoded.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

fn discovered_server_from_public_info(
    base_url: &str,
    endpoint_address: String,
    body: &[u8],
) -> Option<DiscoveredJellyfinServer> {
    let response: JellyfinPublicSystemInfo = serde_json::from_slice(body).ok()?;
    Some(DiscoveredJellyfinServer {
        id: response
            .id
            .or(response.source_id)
            .filter(|id| !id.trim().is_empty()),
        name: response
            .server_name
            .or(response.local_address)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Jellyfin".to_string()),
        address: normalize_discovered_address(base_url)?,
        endpoint_address: Some(endpoint_address),
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
    if servers
        .iter()
        .any(|existing| existing.address == server.address)
    {
        return;
    }
    servers.push(server);
}

#[derive(Clone, Debug)]
struct LocalhostProbeTarget {
    socket: SocketAddr,
    host_header: String,
    base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinDiscoveryResponse {
    address: Option<String>,
    id: Option<String>,
    name: Option<String>,
    endpoint_address: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinPublicSystemInfo {
    id: Option<String>,
    source_id: Option<String>,
    server_name: Option<String>,
    local_address: Option<String>,
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
    fn discovery_fall_address() {
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
    fn discovery_keep_server() {
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

        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].address, "http://music.local:8096");
        assert_eq!(servers[1].address, "http://192.0.2.10:8096");
    }

    #[test]
    fn discovery_include_localhost() {
        let targets = discovery_targets();

        assert!(targets.contains(&SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            JELLYFIN_DISCOVERY_PORT
        )));
    }

    #[test]
    fn localhost_public_info_maps_server() {
        let body = serde_json::json!({
            "Id": "server-one",
            "ServerName": "Local Jellyfin",
            "LocalAddress": "http://127.0.0.1:8096"
        })
        .to_string();

        let server = discovered_server_from_public_info(
            "http://localhost:8096",
            "127.0.0.1:8096".to_string(),
            body.as_bytes(),
        )
        .expect("localhost server");

        assert_eq!(server.id.as_deref(), Some("server-one"));
        assert_eq!(server.name, "Local Jellyfin");
        assert_eq!(server.address, "http://localhost:8096");
        assert_eq!(server.endpoint_address.as_deref(), Some("127.0.0.1:8096"));
    }

    #[test]
    fn discovery_accept_http() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nf\r\n{\"ServerName\":\"\r\n10\r\nLocal Jellyfin\"}\r\n0\r\n\r\n";

        let body = http_response_body(response).expect("response body");
        let server = discovered_server_from_public_info(
            "http://localhost:8096",
            "127.0.0.1:8096".to_string(),
            &body,
        )
        .expect("localhost server");

        assert_eq!(server.name, "Local Jellyfin");
    }
}
