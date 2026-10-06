use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use fluzo_core::model_discovery::*;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use tokio::sync::watch;

const DEADLINE: Duration = Duration::from_secs(5);

pub struct ModelDiscoveryService {
    sender: Option<SyncSender<Request>>,
    receiver: Receiver<(u64, Result<Vec<Model>, Error>)>,
    cancellation: watch::Sender<u64>,
    worker: Option<JoinHandle<()>>,
    current: Option<(Request, Status)>,
    last_id: u64,
    attempts: usize,
}

impl ModelDiscoveryService {
    pub fn start() -> Result<Self, Error> {
        let (sender, requests) = mpsc::sync_channel::<Request>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let (cancellation, mut cancelled) = watch::channel(0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| Error::Unavailable)?;
        let worker = std::thread::Builder::new().name("fluzo-model-discovery".into()).spawn(move || {
            while let Ok(request) = requests.recv() {
                let result = runtime.block_on(async {
                    if *cancelled.borrow() == request.id || *cancelled.borrow() == u64::MAX {
                        return Err(Error::Cancelled);
                    }
                    tokio::select! {
                        biased;
                        _ = cancelled.wait_for(|value| *value == request.id || *value == u64::MAX) => Err(Error::Cancelled),
                        result = tokio::time::timeout(DEADLINE, discover(&request.endpoint, request.authorization_env.as_deref())) => result.unwrap_or(Err(Error::Timeout)),
                    }
                });
                if results.send((request.id, result)).is_err() {
                    break;
                }
            }
        }).map_err(|_| Error::Unavailable)?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            cancellation,
            worker: Some(worker),
            current: None,
            last_id: 0,
            attempts: 0,
        })
    }

    fn collect(&mut self) {
        while let Ok((id, result)) = self.receiver.try_recv() {
            if let Some((request, status)) = &mut self.current
                && request.id == id
            {
                *status = Status::Complete(result);
            }
        }
    }
}

impl ModelDiscoveryPort for ModelDiscoveryService {
    fn submit(&mut self, request: Request) -> Result<(), Error> {
        self.collect();
        if request.protocol != PROTOCOL {
            return Err(Error::UnsupportedProtocol);
        }
        if request.id == 0 || request.id == u64::MAX {
            return Err(Error::Capacity);
        }
        endpoint(&request.endpoint)?;
        if let Some(name) = &request.authorization_env
            && (name.is_empty()
                || name.len() > 128
                || !name.bytes().enumerate().all(|(index, byte)| {
                    byte == b'_'
                        || byte.is_ascii_alphabetic()
                        || (index > 0 && byte.is_ascii_digit())
                }))
        {
            return Err(Error::InvalidAuthorization);
        }
        if let Some((current, status)) = &self.current {
            if *current == request {
                return Ok(());
            }
            if *status == Status::Pending {
                return Err(Error::Busy);
            }
        }
        if request.id <= self.last_id {
            return Err(Error::Capacity);
        }
        if self.attempts >= 64 {
            return Err(Error::Capacity);
        }
        self.sender
            .as_ref()
            .ok_or(Error::Unavailable)?
            .try_send(request.clone())
            .map_err(|_| Error::Busy)?;
        self.last_id = request.id;
        self.attempts += 1;
        self.current = Some((request, Status::Pending));
        Ok(())
    }

    fn status(&mut self, id: u64) -> Status {
        self.collect();
        self.current
            .as_ref()
            .filter(|(request, _)| request.id == id)
            .map_or(Status::Unknown, |(_, status)| status.clone())
    }

    fn cancel(&mut self, id: u64) {
        if self
            .current
            .as_ref()
            .is_some_and(|(request, status)| request.id == id && *status == Status::Pending)
        {
            self.cancellation.send_replace(id);
        }
    }
}

impl Drop for ModelDiscoveryService {
    fn drop(&mut self) {
        self.cancellation.send_replace(u64::MAX);
        self.sender = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, PartialEq)]
struct Target {
    host: String,
    port: u16,
    authority: String,
    path: String,
}

fn endpoint(value: &str) -> Result<Target, Error> {
    if value.len() > 2048 || !value.is_ascii() || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(Error::InvalidEndpoint);
    }
    let uri: hyper::Uri = value.parse().map_err(|_| Error::InvalidEndpoint)?;
    if uri.scheme_str() != Some("http") || uri.query().is_some() || value.contains(['#', '@', '%'])
    {
        return Err(Error::InvalidEndpoint);
    }
    let host = uri.host().ok_or(Error::InvalidEndpoint)?;
    if host.trim_matches(['[', ']']).is_empty() || uri.port_u16() == Some(0) {
        return Err(Error::InvalidEndpoint);
    }
    let base = uri.path().trim_end_matches('/');
    if !base.is_empty() && base != "/v1" {
        return Err(Error::InvalidEndpoint);
    }
    Ok(Target {
        host: host.trim_matches(['[', ']']).to_owned(),
        port: uri.port_u16().unwrap_or(80),
        authority: uri.authority().ok_or(Error::InvalidEndpoint)?.to_string(),
        path: "/v1/models".to_owned(),
    })
}

async fn discover(value: &str, authorization_env: Option<&str>) -> Result<Vec<Model>, Error> {
    let target = endpoint(value)?;
    let authorization = authorization_env
        .map(|name| {
            let value = std::env::var(name).map_err(|_| Error::AuthorizationUnavailable)?;
            if value.is_empty()
                || value.len() > 4096
                || value.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(Error::InvalidAuthorization);
            }
            let mut header = hyper::header::HeaderValue::from_str(&value)
                .map_err(|_| Error::InvalidAuthorization)?;
            header.set_sensitive(true);
            Ok(header)
        })
        .transpose()?;
    let stream = tokio::net::TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|_| Error::Connection)?;
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(16 * 1024)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| Error::Connection)?;
    let exchange = async {
        let mut builder = hyper::Request::builder();
        if let Some(header) = authorization {
            builder = builder.header(hyper::header::AUTHORIZATION, header);
        }
        let request = builder
            .method("GET")
            .uri(target.path)
            .header("Host", target.authority)
            .header("Accept", "application/json")
            .header("Connection", "close")
            .body(Empty::<Bytes>::new())
            .map_err(|_| Error::InvalidEndpoint)?;
        let mut response = sender
            .send_request(request)
            .await
            .map_err(|_| Error::Connection)?;
        if response.status() != hyper::StatusCode::OK {
            return Err(Error::Http(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(frame) = response.body_mut().frame().await {
            let frame = frame.map_err(|_| Error::InvalidResponse)?;
            if let Some(data) = frame.data_ref() {
                if bytes.len().saturating_add(data.len()) > MAX_RESPONSE_BYTES {
                    return Err(Error::TooLarge);
                }
                bytes.extend_from_slice(data);
            }
        }
        parse_models(&bytes)
    };
    tokio::pin!(exchange);
    tokio::pin!(connection);
    tokio::select! {
        result = &mut exchange => result,
        result = &mut connection => {
            result.map_err(|_| Error::Connection)?;
            exchange.await
        }
    }
}

fn parse_models(bytes: &[u8]) -> Result<Vec<Model>, Error> {
    let document: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    let data = document
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or(Error::InvalidResponse)?;
    if data.len() > MAX_MODELS {
        return Err(Error::TooLarge);
    }
    let mut models = std::collections::BTreeMap::new();
    for item in data {
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or(Error::InvalidResponse)?;
        if id.is_empty() || id.len() > MAX_TEXT_BYTES || id.chars().any(|character| character.is_control() || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
            return Err(Error::InvalidResponse);
        }
        let positive = |key: &str| {
            item.get(key)
                .and_then(serde_json::Value::as_u64)
                .filter(|value| *value > 0)
        };
        models.entry(id.to_owned()).or_insert_with(|| Model {
            id: id.to_owned(),
            context_window: positive("max_context_length").or_else(|| positive("context_length")),
            max_output_tokens: positive("max_output_tokens"),
            slots: positive("slots").and_then(|value| value.try_into().ok()),
        });
    }
    Ok(models.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn endpoint_policy_allows_dns_names_and_rejects_ambiguous_or_credential_urls() {
        for value in [
            "http://localhost:8080",
            "http://192.168.1.2:8080/v1/",
            "http://[::1]:8080/v1",
            "http://server.local/v1",
            "http://halo.totoshome.duckdns.org/",
            "http://8.8.8.8/v1",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(endpoint(value).is_ok(), "{value}");
        }
        for value in [
            "https://localhost/v1",
            "http://user:secret@localhost",
            "http://localhost/v1?key=x",
            "http://localhost:0",
            "http://localhost/other",
            "http://localhost/#fragment",
            "http:///v1",
            "http://%/v1",
        ] {
            assert_eq!(endpoint(value), Err(Error::InvalidEndpoint), "{value}");
        }
    }

    #[test]
    fn endpoint_keeps_hostname_and_authority_for_virtual_host_routing() {
        let target = endpoint("http://halo.totoshome.duckdns.org/").unwrap();
        assert_eq!(target.host, "halo.totoshome.duckdns.org");
        assert_eq!(target.authority, "halo.totoshome.duckdns.org");
        assert_eq!(target.port, 80);
        assert_eq!(target.path, "/v1/models");
        let target = endpoint("http://llm.internal:8080/v1").unwrap();
        assert_eq!(target.host, "llm.internal");
        assert_eq!(target.authority, "llm.internal:8080");
        assert_eq!(target.port, 8080);
        let target = endpoint("http://[::1]:8080/v1").unwrap();
        assert_eq!(target.host, "::1");
        assert_eq!(target.authority, "[::1]:8080");
        assert_eq!(target.port, 8080);
    }

    #[test]
    fn real_http_catalog_through_hostname_sends_hostname_authority() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let mut stream = accept(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
            assert!(
                request.contains(&format!("host: localhost:{port}\r\n").to_lowercase()),
                "{request}"
            );
            let body = r#"{"data":[{"id":"hostname-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let mut service = ModelDiscoveryService::start().unwrap();
        service
            .submit(Request {
                protocol: PROTOCOL,
                id: 1,
                endpoint: format!("http://localhost:{port}/v1"),
                authorization_env: None,
            })
            .unwrap();
        assert_eq!(
            wait(&mut service, 1),
            Status::Complete(Ok(vec!["hostname-model".into()]))
        );
        worker.join().unwrap();
    }

    #[test]
    fn unresolvable_hostname_fails_closed_without_hanging() {
        let mut service = ModelDiscoveryService::start().unwrap();
        service
            .submit(Request {
                protocol: PROTOCOL,
                id: 1,
                endpoint: "http://fluzo-unresolvable.invalid/v1".into(),
                authorization_env: None,
            })
            .unwrap();
        match wait(&mut service, 1) {
            Status::Complete(Err(_)) => {}
            other => panic!("expected a failed lookup, got {other:?}"),
        }
    }

    fn wait(service: &mut ModelDiscoveryService, id: u64) -> Status {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            let status = service.status(id);
            if status != Status::Pending {
                return status;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    fn accept(listener: &std::net::TcpListener) -> std::net::TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Err(error) => panic!("fixture accept: {error}"),
            }
        }
    }

    #[test]
    fn real_http_catalog_redirect_rate_limit_and_cancel_never_retry() {
        use std::io::{Read, Write};
        for code in [200, 302, 429] {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let target = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            target.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
            let redirect = format!("http://{}/v1/models", target.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let mut stream = accept(&listener);
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
                assert!(!request.to_lowercase().contains("authorization"));
                let body = r#"{"data":[{"id":"fixture-model"}]}"#;
                write!(stream, "HTTP/1.1 {code} Fixture\r\nContent-Length: {}\r\nLocation: {redirect}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                listener.set_nonblocking(true).unwrap();
                listener
            });
            let mut service = ModelDiscoveryService::start().unwrap();
            let request = Request {
                protocol: PROTOCOL,
                id: 1,
                endpoint,
                authorization_env: None,
            };
            service.submit(request.clone()).unwrap();
            let result = wait(&mut service, 1);
            assert_eq!(
                result,
                Status::Complete(if code == 200 {
                    Ok(vec!["fixture-model".into()])
                } else {
                    Err(Error::Http(code))
                })
            );
            service.submit(request).unwrap();
            assert_eq!(service.status(1), result);
            let listener = worker.join().unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert_eq!(
                target.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut service = ModelDiscoveryService::start().unwrap();
        service
            .submit(Request {
                protocol: PROTOCOL,
                id: 1,
                endpoint: format!("http://{}", listener.local_addr().unwrap()),
                authorization_env: None,
            })
            .unwrap();
        let stream = accept(&listener);
        service.cancel(1);
        assert_eq!(
            wait(&mut service, 1),
            Status::Complete(Err(Error::Cancelled))
        );
        drop(stream);
    }

    #[test]
    fn stalled_response_times_out_and_oversized_body_is_rejected() {
        use std::io::Write;
        for oversized in [false, true] {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let mut service = ModelDiscoveryService::start().unwrap();
            service
                .submit(Request {
                    protocol: PROTOCOL,
                    id: 1,
                    endpoint: format!("http://{}", listener.local_addr().unwrap()),
                    authorization_env: None,
                })
                .unwrap();
            let mut stream = accept(&listener);
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            if oversized {
                use std::io::Read;
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    MAX_RESPONSE_BYTES + 1
                )
                .unwrap();
                let _ = stream.write_all(&vec![b' '; MAX_RESPONSE_BYTES + 1]);
            }
            assert_eq!(
                wait(&mut service, 1),
                Status::Complete(Err(if oversized {
                    Error::TooLarge
                } else {
                    Error::Timeout
                }))
            );
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn authorization_reference_validation_and_protocol_fail_before_connect() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut service = ModelDiscoveryService::start().unwrap();
        let mut request = Request {
            protocol: 1,
            id: 1,
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
            authorization_env: None,
        };
        assert_eq!(
            service.submit(request.clone()),
            Err(Error::UnsupportedProtocol)
        );
        request.protocol = PROTOCOL;
        request.authorization_env = Some("Bearer token\r\nInjected".into());
        assert_eq!(
            service.submit(request.clone()),
            Err(Error::InvalidAuthorization)
        );
        request.authorization_env = Some("FLUZO_TEST_MISSING_AUTH_71BEA304".into());
        assert!(std::env::var_os("FLUZO_TEST_MISSING_AUTH_71BEA304").is_none());
        service.submit(request).unwrap();
        assert_eq!(
            wait(&mut service, 1),
            Status::Complete(Err(Error::AuthorizationUnavailable))
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn response_limits_ids_and_deduplicates_without_inventing_metadata() {
        let models = parse_models(br#"{"data":[{"id":"known","max_context_length":65536,"max_output_tokens":8192,"slots":2},{"id":"unknown","slots":-1}]}"#).unwrap();
        assert_eq!(models[0].context_window, Some(65536));
        assert_eq!(models[0].max_output_tokens, Some(8192));
        assert_eq!(models[0].slots, Some(2));
        assert_eq!(models[1].context_window, None);
        assert_eq!(models[1].slots, None);
        assert_eq!(
            parse_models(br#"{"data":[{"id":"b"},{"id":"a"},{"id":"b"}]}"#),
            Ok(vec!["a".into(), "b".into()])
        );
        for bytes in [
            br#"{}"#.as_slice(),
            br#"{"data":[{}]}"#,
            br#"{"data":[{"id":"\u001b"}]}"#,
        ] {
            assert_eq!(parse_models(bytes), Err(Error::InvalidResponse));
        }
        let data = serde_json::json!({"data": vec![serde_json::json!({"id":"a"}); MAX_MODELS + 1]});
        assert_eq!(
            parse_models(data.to_string().as_bytes()),
            Err(Error::TooLarge)
        );
    }
}
