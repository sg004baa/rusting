//! Request assembly, transmission, and response conversion.

use std::error::Error as StdError;
use std::time::{Duration, Instant};

use diqwest::WithDigestAuth;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use rusting_core::{
    AuthKind, BodyContent, KeyValue, RequestModel, config::SslSettings, model::RUSTING_VERSION,
};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use crate::client;
use crate::timing::Recorder;
use crate::types::{Phase, PhaseEvent, Response, SendError, SentRequest};

/// Builds and sends one already-templated request.
///
/// `cookie_jar` is session state and is attached only when the request's
/// `attach_cookies` option is enabled. Progress delivery is best-effort: a
/// dropped receiver does not cancel the request.
pub async fn send(
    request: &RequestModel,
    settings: &SslSettings,
    cookie_jar: &[KeyValue],
    progress: Option<UnboundedSender<PhaseEvent>>,
) -> Result<Response, SendError> {
    let timeout = client::validated_timeout(request.options.timeout)?;
    let http_client = client::build(request, settings)?;
    let mut url = Url::parse(&request.url)
        .map_err(|error| SendError::InvalidRequest(format!("invalid URL: {error}")))?;
    let mut parameters = request.enabled_params().peekable();
    if parameters.peek().is_some() {
        let mut query = url.query_pairs_mut();
        for parameter in parameters {
            query.append_pair(&parameter.name, &parameter.value);
        }
    }

    let mut headers = request_headers(request)?;
    attach_cookies(request, cookie_jar, &mut headers)?;
    if !headers.contains_key(USER_AGENT) {
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&format!("rusting/{RUSTING_VERSION}")).map_err(|error| {
                SendError::InvalidRequest(format!("invalid generated User-Agent: {error}"))
            })?,
        );
    }

    let body = encoded_body(request);
    if !request.has_explicit_content_type()
        && let Some(content_type) = request.body.as_ref().and_then(BodyContent::content_type)
    {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(content_type).map_err(|error| {
                SendError::InvalidRequest(format!("invalid body content type: {error}"))
            })?,
        );
    }

    let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
        .map_err(|error| SendError::InvalidRequest(format!("invalid HTTP method: {error}")))?;
    let mut builder = http_client.request(method, url).headers(headers);
    if let Some(body) = body {
        builder = builder.body(body);
    }

    let digest_credentials = match request.auth.as_ref().and_then(|auth| auth.kind) {
        Some(AuthKind::Basic) => {
            let auth = request
                .auth
                .as_ref()
                .and_then(|auth| auth.basic.as_ref())
                .ok_or_else(|| {
                    SendError::InvalidRequest(
                        "Basic auth is selected but has no credentials".into(),
                    )
                })?;
            builder = builder.basic_auth(&auth.username, Some(&auth.password));
            None
        }
        Some(AuthKind::BearerToken) => {
            let auth = request
                .auth
                .as_ref()
                .and_then(|auth| auth.bearer_token.as_ref())
                .ok_or_else(|| {
                    SendError::InvalidRequest("Bearer auth is selected but has no token".into())
                })?;
            builder = builder.bearer_auth(&auth.token);
            None
        }
        Some(AuthKind::Digest) => {
            let auth = request
                .auth
                .as_ref()
                .and_then(|auth| auth.digest.as_ref())
                .ok_or_else(|| {
                    SendError::InvalidRequest(
                        "Digest auth is selected but has no credentials".into(),
                    )
                })?;
            Some((auth.username.as_str(), auth.password.as_str()))
        }
        None => None,
    };

    let snapshot = builder
        .try_clone()
        .ok_or_else(|| SendError::InvalidRequest("request body could not be replayed".into()))?
        .build()
        .map_err(|error| classify_reqwest(error, timeout, false))?;
    let sent = snapshot_request(&snapshot);

    // A fresh reqwest client guarantees a fresh connection. reqwest does not
    // expose connector milestones, so Connect is the supported aggregate from
    // dispatch through response headers; DNS and TLS remain honestly Skipped.
    let total_started = Instant::now();
    let mut timing = Recorder::new(progress);
    timing.start(Phase::Connect);
    timing.start(Phase::TimeToFirstByte);
    let response_result = if let Some(credentials) = digest_credentials {
        builder
            .send_digest_auth(credentials)
            .await
            .map_err(|error| classify_digest(error, timeout, !request.options.proxy_url.is_empty()))
    } else {
        builder.send().await.map_err(|error| {
            classify_reqwest(error, timeout, !request.options.proxy_url.is_empty())
        })
    };

    let response = match response_result {
        Ok(response) => response,
        Err(error) => {
            timing.fail(Phase::Connect);
            timing.fail(Phase::TimeToFirstByte);
            return Err(error);
        }
    };
    timing.complete(Phase::Connect, total_started);
    timing.complete(Phase::TimeToFirstByte, total_started);

    let status = response.status();
    let reason = status.canonical_reason().unwrap_or("").to_owned();
    let response_url = response.url().to_string();
    let response_headers = response.headers().clone();
    let headers = convert_headers(&response_headers);
    let cookies = response
        .cookies()
        .map(|cookie| KeyValue::new(cookie.name(), cookie.value()))
        .collect();

    let download_started = Instant::now();
    timing.start(Phase::Download);
    let body = match response.bytes().await {
        Ok(body) => body.to_vec(),
        Err(error) => {
            timing.fail(Phase::Download);
            return Err(classify_reqwest(
                error,
                timeout,
                !request.options.proxy_url.is_empty(),
            ));
        }
    };
    timing.complete(Phase::Download, download_started);
    let timings = timing.finish(total_started.elapsed());

    Ok(Response {
        status: status.as_u16(),
        reason,
        url: response_url,
        headers,
        cookies,
        body,
        timings,
        sent,
    })
}

fn request_headers(request: &RequestModel) -> Result<HeaderMap, SendError> {
    let mut headers = HeaderMap::new();
    for header in request.enabled_headers() {
        let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|error| {
            SendError::InvalidRequest(format!("invalid header name {:?}: {error}", header.name))
        })?;
        let value = HeaderValue::from_str(&header.value).map_err(|error| {
            SendError::InvalidRequest(format!("invalid value for header {}: {error}", header.name))
        })?;
        headers.append(name, value);
    }
    Ok(headers)
}

fn attach_cookies(
    request: &RequestModel,
    cookie_jar: &[KeyValue],
    headers: &mut HeaderMap,
) -> Result<(), SendError> {
    if !request.options.attach_cookies {
        return Ok(());
    }
    let mut cookie = String::new();
    for item in cookie_jar.iter().filter(|item| item.enabled) {
        if !cookie.is_empty() {
            cookie.push_str("; ");
        }
        cookie.push_str(&item.name);
        cookie.push('=');
        cookie.push_str(&item.value);
    }
    if !cookie.is_empty() {
        headers.append(
            COOKIE,
            HeaderValue::from_str(&cookie).map_err(|error| {
                SendError::InvalidRequest(format!("invalid session cookie: {error}"))
            })?,
        );
    }
    Ok(())
}

fn encoded_body(request: &RequestModel) -> Option<Vec<u8>> {
    match request.body.as_ref()? {
        BodyContent::Raw { content, .. } => Some(content.as_bytes().to_vec()),
        BodyContent::Form { form_data, .. } => {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for item in form_data.iter().filter(|item| item.enabled) {
                serializer.append_pair(&item.name, &item.value);
            }
            Some(serializer.finish().into_bytes())
        }
    }
}

fn snapshot_request(request: &reqwest::Request) -> SentRequest {
    SentRequest {
        method: request.method().as_str().to_owned(),
        url: request.url().to_string(),
        headers: convert_headers(request.headers()),
        body: request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .map(|body| String::from_utf8_lossy(body).into_owned()),
    }
}

fn convert_headers(headers: &HeaderMap) -> Vec<KeyValue> {
    headers
        .iter()
        .map(|(name, value)| {
            KeyValue::new(name.as_str(), String::from_utf8_lossy(value.as_bytes()))
        })
        .collect()
}

fn classify_digest(
    error: diqwest::error::Error,
    timeout: Duration,
    using_proxy: bool,
) -> SendError {
    let message = error.to_string();
    match error {
        diqwest::error::Error::Reqwest(error) => classify_reqwest(error, timeout, using_proxy),
        diqwest::error::Error::MissingHost
        | diqwest::error::Error::RequestBuilderNotCloneable
        | diqwest::error::Error::InvalidHeaderValue(_)
        | diqwest::error::Error::ToStr(_) => {
            SendError::InvalidRequest(format!("digest authentication failed: {message}"))
        }
        diqwest::error::Error::DigestAuth(_)
        | diqwest::error::Error::AuthHeaderMissing
        | diqwest::error::Error::LockPoisoned => {
            SendError::Other(format!("digest authentication failed: {message}"))
        }
    }
}

fn classify_reqwest(error: reqwest::Error, timeout: Duration, using_proxy: bool) -> SendError {
    let message = error_chain(&error);
    if error.is_timeout() {
        SendError::Timeout(timeout)
    } else if client::looks_like_tls_error(&message) {
        SendError::Tls(message)
    } else if error.is_connect() && using_proxy {
        SendError::Proxy(message)
    } else if error.is_connect() {
        SendError::Connect(message)
    } else if error.is_builder() || error.is_request() || error.is_redirect() {
        SendError::InvalidRequest(message)
    } else {
        SendError::Other(message)
    }
}

fn error_chain(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        let detail = error.to_string();
        if !detail.is_empty() && !message.contains(&detail) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        source = error.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use rusting_core::{Auth, HttpMethod, Options};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::types::PhaseOutcome;

    struct LocalServer {
        url: String,
        requests: tokio::task::JoinHandle<Vec<String>>,
    }

    async fn server(responses: Vec<(Duration, String)>) -> LocalServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local server");
        let address = listener.local_addr().expect("local address");
        let requests = tokio::spawn(async move {
            let mut captured = Vec::new();
            for (delay, response) in responses {
                let (mut stream, _) = listener.accept().await.expect("accept request");
                let request = read_request(&mut stream).await;
                captured.push(request);
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
                stream.shutdown().await.expect("close response");
            }
            captured
        });
        LocalServer {
            url: format!("http://{address}"),
            requests,
        }
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut buffer).await.expect("read request");
            assert_ne!(read, 0, "connection closed before request headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buffer).await.expect("read request body");
            assert_ne!(read, 0, "connection closed before request body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn request(url: String) -> RequestModel {
        RequestModel {
            url,
            options: Options {
                timeout: 1.0,
                ..Options::default()
            },
            ..RequestModel::default()
        }
    }

    #[tokio::test]
    async fn get_merges_query_headers_cookies_and_reports_timings() {
        let local = server(vec![(
            Duration::ZERO,
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: sid=abc; Path=/\r\nConnection: close\r\n\r\nok".into(),
        )])
        .await;
        let mut model = request(format!("{}/items?existing=1", local.url));
        model.params = vec![KeyValue::new("added", "two words")];
        model.headers = vec![KeyValue::new("X-Test", "yes")];
        let cookies = vec![KeyValue::new("session", "value")];
        let (progress, mut events) = tokio::sync::mpsc::unbounded_channel();

        let response = send(&model, &SslSettings::default(), &cookies, Some(progress))
            .await
            .expect("send GET");
        let wire = local.requests.await.expect("server task").remove(0);
        assert!(wire.starts_with("GET /items?existing=1&added=two+words HTTP/1.1\r\n"));
        let wire_lower = wire.to_ascii_lowercase();
        assert!(wire_lower.contains("x-test: yes\r\n"));
        assert!(wire_lower.contains("cookie: session=value\r\n"));
        assert!(
            wire_lower
                .contains(&format!("user-agent: rusting/{RUSTING_VERSION}").to_ascii_lowercase())
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"ok");
        assert_eq!(response.cookies, vec![KeyValue::new("sid", "abc")]);
        assert_eq!(
            response.sent.url,
            format!("{}/items?existing=1&added=two+words", local.url)
        );
        assert!(matches!(
            response.timings.outcome(Phase::Connect),
            PhaseOutcome::Completed(_)
        ));
        assert_eq!(response.timings.outcome(Phase::Dns), PhaseOutcome::Skipped);
        assert_eq!(response.timings.outcome(Phase::Tls), PhaseOutcome::Skipped);
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        assert!(received.contains(&PhaseEvent::Started(Phase::Download)));
    }

    #[tokio::test]
    async fn post_sends_raw_body_and_basic_auth_preemptively() {
        let local = server(vec![(Duration::ZERO, ok("done"))]).await;
        let mut model = request(format!("{}/submit", local.url));
        model.method = HttpMethod::Post;
        model.body = Some(BodyContent::Raw {
            content: "hello".into(),
            content_type: Some("text/plain".into()),
        });
        model.auth = Some(Auth::basic("aladdin", "open sesame"));

        let response = send(&model, &SslSettings::default(), &[], None)
            .await
            .expect("send POST");
        let wire = local.requests.await.expect("server task").remove(0);
        let wire_lower = wire.to_ascii_lowercase();
        assert!(
            wire.starts_with("POST /submit HTTP/1.1\r\n"),
            "unexpected request: {wire:?}"
        );
        assert!(wire.ends_with("\r\n\r\nhello"));
        assert!(wire_lower.contains("content-type: text/plain\r\n"));
        assert!(wire_lower.contains("authorization: basic ywxhzgrpbjpvcgvuihnlc2ftzq==\r\n"));
        assert_eq!(response.sent.body.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn bearer_auth_is_preemptive() {
        let local = server(vec![(Duration::ZERO, ok(""))]).await;
        let mut model = request(local.url.clone());
        model.auth = Some(Auth::bearer("token-value"));
        send(&model, &SslSettings::default(), &[], None)
            .await
            .expect("send bearer request");
        let wire = local.requests.await.expect("server task").remove(0);
        assert!(
            wire.to_ascii_lowercase()
                .contains("authorization: bearer token-value\r\n")
        );
    }

    #[tokio::test]
    async fn digest_auth_replays_after_a_local_challenge() {
        let local = server(vec![
            (
                Duration::ZERO,
                concat!(
                    "HTTP/1.1 401 Unauthorized\r\n",
                    "WWW-Authenticate: Digest realm=\"local\", nonce=\"abcdef\", ",
                    "algorithm=MD5, qop=\"auth\"\r\n",
                    "Content-Length: 0\r\n",
                    "Connection: close\r\n\r\n"
                )
                .into(),
            ),
            (Duration::ZERO, ok("authenticated")),
        ])
        .await;
        let mut model = request(format!("{}/digest", local.url));
        model.auth = Some(Auth::digest("user", "password"));
        let response = send(&model, &SslSettings::default(), &[], None)
            .await
            .expect("complete digest exchange");
        let requests = local.requests.await.expect("server task");
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].to_ascii_lowercase().contains("authorization:"));
        let authorization = requests[1]
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with("authorization: digest ")
            })
            .expect("digest Authorization header");
        assert!(authorization.contains("username=\"user\""));
        assert_eq!(response.body, b"authenticated");
    }

    #[tokio::test]
    async fn form_encodes_enabled_rows_and_respects_explicit_content_type() {
        let local = server(vec![(Duration::ZERO, ok(""))]).await;
        let mut disabled = KeyValue::new("skip", "me");
        disabled.enabled = false;
        let mut model = request(format!("{}/form", local.url));
        model.method = HttpMethod::Post;
        model.headers = vec![KeyValue::new("Content-Type", "application/custom")];
        model.body = Some(BodyContent::Form {
            form_data: vec![KeyValue::new("a", "two words"), disabled],
            content_type: Some(BodyContent::FORM_CONTENT_TYPE.into()),
        });

        send(&model, &SslSettings::default(), &[], None)
            .await
            .expect("send form");
        let wire = local.requests.await.expect("server task").remove(0);
        let lower = wire.to_ascii_lowercase();
        assert!(wire.ends_with("\r\n\r\na=two+words"));
        assert!(lower.contains("content-type: application/custom\r\n"));
        assert!(!lower.contains(BodyContent::FORM_CONTENT_TYPE));
    }

    #[tokio::test]
    async fn redirect_policy_is_request_scoped() {
        let first = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect target");
        let target_address = first.local_addr().expect("target address");
        let target = tokio::spawn(async move {
            let (mut stream, _) = first.accept().await.expect("accept redirect target");
            let request = read_request(&mut stream).await;
            stream
                .write_all(ok("final").as_bytes())
                .await
                .expect("write final");
            request
        });
        let redirect = server(vec![(
            Duration::ZERO,
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )])
        .await;
        let followed = send(
            &request(format!("{}/start", redirect.url)),
            &SslSettings::default(),
            &[],
            None,
        )
        .await
        .expect("follow redirect");
        assert_eq!(followed.status, 200);
        assert_eq!(followed.url, format!("http://{target_address}/final"));
        assert!(
            target
                .await
                .expect("target task")
                .starts_with("GET /final ")
        );
        redirect.requests.await.expect("redirect task");

        let redirect = server(vec![(
            Duration::ZERO,
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        )])
        .await;
        let mut not_followed = request(format!("{}/start", redirect.url));
        not_followed.options.follow_redirects = false;
        let response = send(&not_followed, &SslSettings::default(), &[], None)
            .await
            .expect("do not follow redirect");
        assert_eq!(response.status, 302);
        redirect.requests.await.expect("redirect task");
    }

    #[tokio::test]
    async fn timeout_is_classified_with_configured_duration() {
        let local = server(vec![(Duration::from_millis(150), ok("late"))]).await;
        let mut model = request(local.url.clone());
        model.options.timeout = 0.03;
        let error = send(&model, &SslSettings::default(), &[], None)
            .await
            .expect_err("request should time out");
        assert!(matches!(
            error,
            SendError::Timeout(duration) if duration == Duration::from_millis(30)
        ));
        local.requests.abort();
    }

    #[tokio::test]
    async fn cookie_attachment_can_be_disabled() {
        let local = server(vec![(Duration::ZERO, ok(""))]).await;
        let mut model = request(local.url.clone());
        model.options.attach_cookies = false;
        send(
            &model,
            &SslSettings::default(),
            &[KeyValue::new("secret", "no")],
            None,
        )
        .await
        .expect("send without cookies");
        let wire = local.requests.await.expect("server task").remove(0);
        let headers = wire.split("\r\n\r\n").next().expect("request headers");
        assert!(!headers.to_ascii_lowercase().contains("cookie:"));
    }
}
