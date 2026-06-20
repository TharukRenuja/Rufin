use crate::{ImageBytes, ProviderError, ProviderResult};
use reqwest::{Client, StatusCode, header};
use serde::de::DeserializeOwned;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BodyLimit {
    pub max_bytes: usize,
    pub context: &'static str,
}

#[derive(Clone, Copy)]
pub struct RemoteHttpPolicy {
    pub auth_context: &'static str,
    pub error_body: BodyLimit,
    pub redact_error_url: Option<fn(&mut reqwest::Url)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteTimeouts {
    pub connect: Duration,
    pub request: Duration,
}

pub fn build_client(
    trust_invalid_cert: bool,
    timeouts: RemoteTimeouts,
    policy: RemoteHttpPolicy,
) -> ProviderResult<Client> {
    Client::builder()
        .danger_accept_invalid_certs(trust_invalid_cert)
        .connect_timeout(timeouts.connect)
        .timeout(timeouts.request)
        .build()
        .map_err(|error| map_reqwest_error(error, policy))
}

pub async fn json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    policy: RemoteHttpPolicy,
    limit: BodyLimit,
) -> ProviderResult<T> {
    let response = checked_response(request, policy).await?;
    let bytes = response_bytes_bounded(response, policy, limit).await?;
    serde_json::from_slice::<T>(&bytes).map_err(|error| ProviderError::Other(error.to_string()))
}

pub async fn unit(
    request: reqwest::RequestBuilder,
    policy: RemoteHttpPolicy,
) -> ProviderResult<()> {
    checked_response(request, policy).await?;
    Ok(())
}

pub async fn bytes(
    request: reqwest::RequestBuilder,
    policy: RemoteHttpPolicy,
    limit: BodyLimit,
) -> ProviderResult<ImageBytes> {
    let response = checked_response(request, policy).await?;
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response_bytes_bounded(response, policy, limit).await?;
    Ok(ImageBytes {
        bytes,
        content_type,
    })
}

pub fn map_reqwest_error(mut error: reqwest::Error, policy: RemoteHttpPolicy) -> ProviderError {
    if let Some(redact) = policy.redact_error_url
        && let Some(url) = error.url_mut()
    {
        redact(url);
    }
    let message = error.to_string();
    let lowered = message.to_lowercase();
    if lowered.contains("certificate") || lowered.contains("tls") {
        ProviderError::Tls(message)
    } else if error.is_connect() || error.is_request() || error.is_timeout() {
        ProviderError::Network(message)
    } else if let Some(status) = error.status() {
        ProviderError::Server {
            status: status.as_u16(),
            message,
        }
    } else {
        ProviderError::Other(message)
    }
}

async fn checked_response(
    request: reqwest::RequestBuilder,
    policy: RemoteHttpPolicy,
) -> ProviderResult<reqwest::Response> {
    let response = request
        .send()
        .await
        .map_err(|error| map_reqwest_error(error, policy))?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "{} {}",
            policy.auth_context,
            status.as_u16()
        )));
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ProviderError::NotFound);
    }
    if status.is_client_error() || status.is_server_error() {
        let message = response_text_or_status(response, status, policy).await;
        return Err(ProviderError::Server {
            status: status.as_u16(),
            message,
        });
    }
    Ok(response)
}

async fn response_text_or_status(
    response: reqwest::Response,
    status: StatusCode,
    policy: RemoteHttpPolicy,
) -> String {
    match response_bytes_bounded(response, policy, policy.error_body).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => status.to_string(),
    }
}

async fn response_bytes_bounded(
    mut response: reqwest::Response,
    policy: RemoteHttpPolicy,
    limit: BodyLimit,
) -> ProviderResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit.max_bytes as u64)
    {
        return Err(size_error(limit));
    }

    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(limit.max_bytes as u64) as usize,
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| map_reqwest_error(error, policy))?
    {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > limit.max_bytes)
        {
            return Err(size_error(limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn size_error(limit: BodyLimit) -> ProviderError {
    ProviderError::Other(format!(
        "{} exceeded {} MiB limit",
        limit.context,
        limit.max_bytes / 1024 / 1024
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SMALL_BODY: BodyLimit = BodyLimit {
        max_bytes: 3,
        context: "test response",
    };
    const POLICY: RemoteHttpPolicy = RemoteHttpPolicy {
        auth_context: "Test server returned",
        error_body: BodyLimit {
            max_bytes: 32,
            context: "test error response",
        },
        redact_error_url: None,
    };

    #[derive(Debug, Deserialize)]
    struct Payload {
        value: String,
    }

    #[tokio::test]
    async fn json_reads_bounded_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/payload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": "ok"
            })))
            .mount(&server)
            .await;

        let client = Client::new();
        let url = format!("{}/payload", server.uri());
        let payload: Payload = json(
            client.get(url),
            POLICY,
            BodyLimit {
                max_bytes: 32,
                context: "test JSON response",
            },
        )
        .await
        .expect("payload");

        assert_eq!(payload.value, "ok");
    }

    #[tokio::test]
    async fn status_errors_map_to_provider_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/broken"))
            .respond_with(ResponseTemplate::new(500).set_body_string("broken"))
            .mount(&server)
            .await;

        let client = Client::new();
        let auth = unit(client.get(format!("{}/auth", server.uri())), POLICY)
            .await
            .expect_err("auth error");
        let missing = unit(client.get(format!("{}/missing", server.uri())), POLICY)
            .await
            .expect_err("missing error");
        let broken = unit(client.get(format!("{}/broken", server.uri())), POLICY)
            .await
            .expect_err("server error");

        assert!(matches!(auth, ProviderError::Auth(_)));
        assert!(matches!(missing, ProviderError::NotFound));
        assert!(matches!(
            broken,
            ProviderError::Server {
                status: 500,
                message
            } if message == "broken"
        ));
    }

    #[tokio::test]
    async fn bytes_preserve_content_type_and_limit_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/image"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(vec![1_u8, 2, 3]),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/large"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0_u8; 4]))
            .mount(&server)
            .await;

        let client = Client::new();
        let image = bytes(
            client.get(format!("{}/image", server.uri())),
            POLICY,
            SMALL_BODY,
        )
        .await
        .expect("image");
        let large = bytes(
            client.get(format!("{}/large", server.uri())),
            POLICY,
            SMALL_BODY,
        )
        .await
        .expect_err("oversized body");

        assert_eq!(image.bytes, vec![1, 2, 3]);
        assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
        assert!(large.to_string().contains("test response exceeded"));
    }

    #[tokio::test]
    async fn timeout_maps_to_network_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(200))
                    .set_body_string("{}"),
            )
            .mount(&server)
            .await;
        let client = build_client(
            false,
            RemoteTimeouts {
                connect: Duration::from_secs(1),
                request: Duration::from_millis(20),
            },
            POLICY,
        )
        .expect("client");

        let error = unit(client.get(format!("{}/slow", server.uri())), POLICY)
            .await
            .expect_err("timeout");

        assert!(matches!(error, ProviderError::Network(_)));
    }
}
