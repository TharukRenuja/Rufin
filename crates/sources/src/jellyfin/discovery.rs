use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use crate::remote_http::{self, BodyLimit, RemoteHttpPolicy};
use crate::{SourceError, SourceResult};
use if_addrs::IfAddr;
use reqwest::{Client, StatusCode, header, redirect};
use serde::Deserialize;
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};
use tracing::instrument;

use super::normalize_base_url;

const JELLYFIN_DISCOVERY_PORT: u16 = 7359;
const JELLYFIN_DISCOVERY_PACKET_LIMIT: usize = 4096;
const JELLYFIN_DISCOVERY_MESSAGES: &[&[u8]] =
    &[b"Who is JellyfinServer?", b"who is JellyfinServer?"];
const JELLYFIN_LOCALHOST_URL: &str = "http://localhost:8096";
const JELLYFIN_LOCALHOST_TARGETS: &[&str] = &["http://127.0.0.1:8096", "http://[::1]:8096"];
const JELLYFIN_LOCALHOST_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const JELLYFIN_LOCALHOST_RESPONSE: BodyLimit = BodyLimit {
    max_bytes: 16 * 1024,
    context: "Jellyfin discovery response",
};
const JELLYFIN_DISCOVERY_HTTP: RemoteHttpPolicy = RemoteHttpPolicy {
    service: "Jellyfin discovery",
    auth_context: "Jellyfin discovery returned",
    error_body: JELLYFIN_LOCALHOST_RESPONSE,
    redact_error_url: None,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredJellyfinServer {
    pub id: Option<String>,
    pub name: String,
    pub address: String,
}

#[instrument(skip_all, fields(timeout_ms = timeout.as_millis()))]
pub async fn discover_jellyfin_servers(
    timeout: Duration,
) -> SourceResult<Vec<DiscoveredJellyfinServer>> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|error| map_io_error("bind Jellyfin discovery socket", error))?;
    socket
        .set_broadcast(true)
        .map_err(|error| map_io_error("enable Jellyfin discovery broadcast", error))?;

    let mut sent_any = false;
    let mut last_send_error = None;
    for target in discovery_targets() {
        for message in JELLYFIN_DISCOVERY_MESSAGES {
            match socket.send_to(message, target).await {
                Ok(_) => sent_any = true,
                Err(error) => last_send_error = Some(error),
            }
        }
    }
    if !sent_any {
        return Err(map_io_error(
            "send Jellyfin discovery broadcast",
            last_send_error.expect("fixed discovery targets are not empty"),
        ));
    }

    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; JELLYFIN_DISCOVERY_PACKET_LIMIT];
    let mut servers = Vec::new();
    while Instant::now() < deadline {
        match timeout_at(deadline, socket.recv_from(&mut buffer)).await {
            Ok(Ok((size, _source))) => {
                if let Some(server) = discovered_server_from_packet(&buffer[..size]) {
                    push_server(&mut servers, server);
                }
            }
            Ok(Err(error)) => {
                return Err(map_io_error("receive Jellyfin discovery response", error));
            }
            Err(_) => break,
        }
    }

    if let Ok(client) = Client::builder()
        .no_proxy()
        .redirect(redirect::Policy::none())
        .connect_timeout(JELLYFIN_LOCALHOST_PROBE_TIMEOUT)
        .timeout(JELLYFIN_LOCALHOST_PROBE_TIMEOUT)
        .build()
    {
        for target in JELLYFIN_LOCALHOST_TARGETS {
            if let Some(server) = probe_localhost_server(&client, target).await {
                push_server(&mut servers, server);
            }
        }
    }

    servers.sort_by_key(|server| (server.name.to_lowercase(), server.address.clone()));
    Ok(servers)
}

fn discovery_targets() -> Vec<SocketAddrV4> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(interfaces) => interfaces
            .into_iter()
            .filter_map(|interface| {
                let is_up = interface.is_oper_up();
                let is_point_to_point = interface.is_p2p();
                let IfAddr::V4(address) = interface.addr else {
                    return None;
                };
                Some(DiscoveryIpv4Interface {
                    address: address.ip,
                    broadcast: address.broadcast,
                    is_up,
                    is_point_to_point,
                })
            })
            .collect(),
        Err(error) => {
            tracing::debug!(
                %error,
                "could not enumerate interface broadcasts for Jellyfin discovery"
            );
            Vec::new()
        }
    };
    discovery_targets_for(interfaces)
}

fn discovery_targets_for(
    interfaces: impl IntoIterator<Item = DiscoveryIpv4Interface>,
) -> Vec<SocketAddrV4> {
    let mut targets = vec![
        SocketAddrV4::new(Ipv4Addr::BROADCAST, JELLYFIN_DISCOVERY_PORT),
        SocketAddrV4::new(Ipv4Addr::new(127, 255, 255, 255), JELLYFIN_DISCOVERY_PORT),
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, JELLYFIN_DISCOVERY_PORT),
    ];
    for interface in interfaces {
        if !interface.is_up || interface.address.is_loopback() || interface.is_point_to_point {
            continue;
        }
        let Some(broadcast) = interface.broadcast else {
            continue;
        };
        let target = SocketAddrV4::new(broadcast, JELLYFIN_DISCOVERY_PORT);
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

fn map_io_error(context: &str, error: io::Error) -> SourceError {
    SourceError::Network(format!("{context}: {error}"))
}

fn discovered_server_from_packet(packet: &[u8]) -> Option<DiscoveredJellyfinServer> {
    let response: JellyfinDiscoveryResponse = serde_json::from_slice(packet).ok()?;
    let address = response.address?;
    discovered_server(response.id, response.name, &address)
}

async fn probe_localhost_server(client: &Client, target: &str) -> Option<DiscoveredJellyfinServer> {
    let response = client
        .get(format!("{target}/System/Info/Public"))
        .header(header::ACCEPT, "application/json")
        .header(header::HOST, "localhost:8096")
        .send()
        .await
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let body = remote_http::bounded_response_body(
        response,
        JELLYFIN_DISCOVERY_HTTP,
        JELLYFIN_LOCALHOST_RESPONSE,
    )
    .await
    .ok()?;
    let response: JellyfinPublicSystemInfo = serde_json::from_slice(&body).ok()?;
    discovered_server(
        response.id.or(response.source_id),
        response.server_name.or(response.local_address),
        JELLYFIN_LOCALHOST_URL,
    )
}

fn discovered_server(
    id: Option<String>,
    name: Option<String>,
    address: &str,
) -> Option<DiscoveredJellyfinServer> {
    Some(DiscoveredJellyfinServer {
        id: id.filter(|id| !id.trim().is_empty()),
        name: name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Jellyfin".to_string()),
        address: normalize_base_url(address)
            .ok()?
            .as_str()
            .trim_end_matches('/')
            .to_string(),
    })
}

fn push_server(servers: &mut Vec<DiscoveredJellyfinServer>, server: DiscoveredJellyfinServer) {
    if !servers
        .iter()
        .any(|existing| same_endpoint(&existing.address, &server.address))
    {
        servers.push(server);
    }
}

fn same_endpoint(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let (Ok(left), Ok(right)) = (normalize_base_url(left), normalize_base_url(right)) else {
        return false;
    };
    is_loopback_endpoint(&left)
        && is_loopback_endpoint(&right)
        && left.scheme() == right.scheme()
        && left.port_or_known_default() == right.port_or_known_default()
        && left.path() == right.path()
}

fn is_loopback_endpoint(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[derive(Clone, Copy, Debug)]
struct DiscoveryIpv4Interface {
    address: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
    is_up: bool,
    is_point_to_point: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinDiscoveryResponse {
    address: Option<String>,
    id: Option<String>,
    name: Option<String>,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn discovery_response_maps_server_address() {
        let packet = serde_json::json!({
            "Address": "http://192.0.2.20:8096/",
            "Id": "server-one",
            "Name": "Music Box",
            "EndpointAddress": "127.0.0.1:8096"
        })
        .to_string();

        let server = discovered_server_from_packet(packet.as_bytes()).expect("discovered server");

        assert_eq!(server.id.as_deref(), Some("server-one"));
        assert_eq!(server.name, "Music Box");
        assert_eq!(server.address, "http://192.0.2.20:8096");
    }

    #[test]
    fn discovery_response_requires_an_advertised_address() {
        let packet = serde_json::json!({
            "Id": "server-one",
            "EndpointAddress": "192.0.2.10:8096"
        })
        .to_string();

        assert!(discovered_server_from_packet(packet.as_bytes()).is_none());
    }

    #[test]
    fn discovery_keeps_distinct_urls_and_one_loopback() {
        let mut servers = Vec::new();
        push_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://music.local:8096".to_string(),
            },
        );
        push_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://192.0.2.10:8096".to_string(),
            },
        );
        push_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://127.0.0.1:8096".to_string(),
            },
        );
        push_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://localhost:8096".to_string(),
            },
        );
        push_server(
            &mut servers,
            DiscoveredJellyfinServer {
                id: Some("server-one".to_string()),
                name: "Music Box".to_string(),
                address: "http://[::1]:8096".to_string(),
            },
        );

        assert_eq!(servers.len(), 3);
        assert_eq!(servers[0].address, "http://music.local:8096");
        assert_eq!(servers[1].address, "http://192.0.2.10:8096");
        assert_eq!(servers[2].address, "http://127.0.0.1:8096");
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
    fn discovery_targets_active_interface_broadcasts() {
        let targets = discovery_targets_for([
            DiscoveryIpv4Interface {
                address: Ipv4Addr::new(192, 168, 1, 103),
                broadcast: Some(Ipv4Addr::new(192, 168, 1, 255)),
                is_up: true,
                is_point_to_point: false,
            },
            DiscoveryIpv4Interface {
                address: Ipv4Addr::new(10, 2, 0, 2),
                broadcast: Some(Ipv4Addr::new(10, 2, 0, 2)),
                is_up: true,
                is_point_to_point: true,
            },
            DiscoveryIpv4Interface {
                address: Ipv4Addr::new(198, 51, 100, 20),
                broadcast: Some(Ipv4Addr::new(198, 51, 100, 255)),
                is_up: false,
                is_point_to_point: false,
            },
        ]);

        assert!(targets.contains(&SocketAddrV4::new(
            Ipv4Addr::new(192, 168, 1, 255),
            JELLYFIN_DISCOVERY_PORT
        )));
        assert!(!targets.contains(&SocketAddrV4::new(
            Ipv4Addr::new(10, 2, 0, 2),
            JELLYFIN_DISCOVERY_PORT
        )));
        assert!(!targets.contains(&SocketAddrV4::new(
            Ipv4Addr::new(198, 51, 100, 255),
            JELLYFIN_DISCOVERY_PORT
        )));
    }

    #[test]
    fn localhost_public_info_maps_server() {
        let response: JellyfinPublicSystemInfo = serde_json::from_value(serde_json::json!({
            "Id": "server-one",
            "ServerName": "Local Jellyfin",
            "LocalAddress": "http://127.0.0.1:8096"
        }))
        .expect("public system info");

        let server = discovered_server(
            response.id.or(response.source_id),
            response.server_name.or(response.local_address),
            "http://localhost:8096",
        )
        .expect("localhost server");

        assert_eq!(server.id.as_deref(), Some("server-one"));
        assert_eq!(server.name, "Local Jellyfin");
        assert_eq!(server.address, "http://localhost:8096");
    }

    #[tokio::test]
    async fn localhost_probe_accepts_only_bounded_public_info() {
        let client = Client::builder().no_proxy().build().expect("client");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/System/Info/Public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Id": "server-one",
                "ServerName": "Local Jellyfin"
            })))
            .mount(&server)
            .await;

        let discovered = probe_localhost_server(&client, &server.uri())
            .await
            .expect("localhost server");
        assert_eq!(discovered.id.as_deref(), Some("server-one"));
        assert_eq!(discovered.name, "Local Jellyfin");

        let oversized = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/System/Info/Public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ServerName": "x".repeat(JELLYFIN_LOCALHOST_RESPONSE.max_bytes)
            })))
            .mount(&oversized)
            .await;
        assert!(
            probe_localhost_server(&client, &oversized.uri())
                .await
                .is_none()
        );
    }
}
