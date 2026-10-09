use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fluzo_core::model_discovery::*;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::header::HeaderValue;
use hyper_util::rt::TokioIo;
use tokio::sync::watch;

const DEADLINE: Duration = Duration::from_secs(5);

/// Per-run consent bookkeeping. This lives only in process memory: nothing here is
/// ever written to `.fluzo` or any other file, so every run starts unaccepted.
#[derive(Default)]
struct Consent {
    probed: BTreeMap<String, AddressClass>,
    accepted: BTreeSet<(String, AddressClass)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Job {
    Probe {
        id: u64,
        endpoint: String,
        credential_attached: bool,
    },
    Discover(Request),
}

impl Job {
    fn id(&self) -> u64 {
        match self {
            Job::Probe { id, .. } => *id,
            Job::Discover(request) => request.id,
        }
    }

    fn endpoint(&self) -> &str {
        match self {
            Job::Probe { endpoint, .. } => endpoint,
            Job::Discover(request) => &request.endpoint,
        }
    }
}

pub struct ModelDiscoveryService {
    sender: Option<SyncSender<Job>>,
    receiver: Receiver<(u64, Status)>,
    cancellation: watch::Sender<u64>,
    worker: Option<JoinHandle<()>>,
    consent: Arc<Mutex<Consent>>,
    current: Option<(Job, Status)>,
    last_id: u64,
    attempts: usize,
}

impl ModelDiscoveryService {
    pub fn start() -> Result<Self, Error> {
        Self::spawn(Transport::System)
    }

    #[cfg(test)]
    fn scripted(fixture: Arc<Fixture>) -> Result<Self, Error> {
        Self::spawn(Transport::Scripted(fixture))
    }

    fn spawn(transport: Transport) -> Result<Self, Error> {
        let (sender, requests) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let (cancellation, mut cancelled) = watch::channel(0);
        let consent = Arc::new(Mutex::new(Consent::default()));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| Error::Unavailable)?;
        let worker_consent = Arc::clone(&consent);
        let worker = std::thread::Builder::new().name("fluzo-model-discovery".into()).spawn(move || {
            while let Ok(job) = requests.recv() {
                let status = runtime.block_on(async {
                    if *cancelled.borrow() == job.id() || *cancelled.borrow() == u64::MAX {
                        return Status::Complete(Err(Error::Cancelled));
                    }
                    tokio::select! {
                        biased;
                        _ = cancelled.wait_for(|value| *value == job.id() || *value == u64::MAX) => Status::Complete(Err(Error::Cancelled)),
                        result = tokio::time::timeout(DEADLINE, run_job(&transport, &worker_consent, &job)) => result.unwrap_or(Status::Complete(Err(Error::Timeout))),
                    }
                });
                if results.send((job.id(), status)).is_err() {
                    break;
                }
            }
        }).map_err(|_| Error::Unavailable)?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            cancellation,
            worker: Some(worker),
            consent,
            current: None,
            last_id: 0,
            attempts: 0,
        })
    }

    fn collect(&mut self) {
        while let Ok((id, status)) = self.receiver.try_recv() {
            if let Some((job, current)) = &mut self.current
                && job.id() == id
            {
                *current = status;
            }
        }
    }

    /// Refuse before queueing when the last resolution already showed this exact
    /// endpoint to be non-loopback and no acceptance covers that pair. Nothing is
    /// queued, so no socket can be attempted for the request.
    fn consent_gate(&self, value: &str) -> Result<(), Error> {
        let consent = self.consent.lock().map_err(|_| Error::Unavailable)?;
        let Some(class) = consent.probed.get(value).copied() else {
            return Ok(());
        };
        if class == AddressClass::Loopback || consent.accepted.contains(&(value.to_owned(), class))
        {
            Ok(())
        } else {
            Err(Error::ConsentRequired)
        }
    }

    fn dispatch(&mut self, job: Job) -> Result<(), Error> {
        if let Some((current, status)) = &self.current {
            if *current == job {
                return Ok(());
            }
            if *status == Status::Pending {
                return Err(Error::Busy);
            }
        }
        if job.id() <= self.last_id {
            return Err(Error::Capacity);
        }
        if self.attempts >= 64 {
            return Err(Error::Capacity);
        }
        self.sender
            .as_ref()
            .ok_or(Error::Unavailable)?
            .try_send(job.clone())
            .map_err(|_| Error::Busy)?;
        self.last_id = job.id();
        self.attempts += 1;
        self.current = Some((job, Status::Pending));
        Ok(())
    }
}

impl ModelDiscoveryPort for ModelDiscoveryService {
    fn probe(&mut self, probe: Probe) -> Result<(), Error> {
        self.collect();
        if probe.protocol != PROTOCOL {
            return Err(Error::UnsupportedProtocol);
        }
        if probe.id == 0 || probe.id == u64::MAX {
            return Err(Error::Capacity);
        }
        endpoint(&probe.endpoint)?;
        validate_authorization(probe.authorization_env.as_deref())?;
        self.dispatch(Job::Probe {
            id: probe.id,
            endpoint: probe.endpoint,
            credential_attached: probe.authorization_env.is_some(),
        })
    }

    fn accept(&mut self, value: &str, class: AddressClass) -> Result<(), Error> {
        endpoint(value)?;
        if class == AddressClass::Unresolved {
            return Err(Error::InvalidEndpoint);
        }
        self.consent
            .lock()
            .map_err(|_| Error::Unavailable)?
            .accepted
            .insert((value.to_owned(), class));
        Ok(())
    }

    fn submit(&mut self, request: Request) -> Result<(), Error> {
        self.collect();
        if request.protocol != PROTOCOL {
            return Err(Error::UnsupportedProtocol);
        }
        if request.id == 0 || request.id == u64::MAX {
            return Err(Error::Capacity);
        }
        endpoint(&request.endpoint)?;
        validate_authorization(request.authorization_env.as_deref())?;
        self.consent_gate(&request.endpoint)?;
        self.dispatch(Job::Discover(request))
    }

    fn status(&mut self, id: u64) -> Status {
        self.collect();
        self.current
            .as_ref()
            .filter(|(job, _)| job.id() == id)
            .map_or(Status::Unknown, |(_, status)| status.clone())
    }

    fn cancel(&mut self, id: u64) {
        if self
            .current
            .as_ref()
            .is_some_and(|(job, status)| job.id() == id && *status == Status::Pending)
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

/// The only destination a discovery job may reach. `System` performs real name
/// resolution and HTTP; the scripted variant exists so the consent decisions
/// around it can be asserted without a network.
#[derive(Clone)]
enum Transport {
    System,
    #[cfg(test)]
    Scripted(Arc<Fixture>),
}

#[cfg(test)]
struct Fixture {
    addresses: Mutex<Vec<IpAddr>>,
    connects: Mutex<Vec<SocketAddr>>,
    models: Vec<Model>,
    delay: Option<Duration>,
}

#[cfg(test)]
impl Fixture {
    fn new(addresses: Vec<IpAddr>, delay: Option<Duration>) -> Arc<Self> {
        Arc::new(Self {
            addresses: Mutex::new(addresses),
            connects: Mutex::new(Vec::new()),
            models: vec!["fixture-model".into()],
            delay,
        })
    }

    fn set_addresses(&self, addresses: Vec<IpAddr>) {
        *self.addresses.lock().unwrap() = addresses;
    }

    fn connects(&self) -> Vec<SocketAddr> {
        self.connects.lock().unwrap().clone()
    }
}

impl Transport {
    /// Resolve without connecting. A failed lookup yields no addresses, which
    /// classifies as `Unresolved` rather than inventing a destination.
    async fn resolve(&self, host: &str, port: u16) -> Vec<SocketAddr> {
        match self {
            Transport::System => {
                let mut addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
                    .await
                    .map(|resolved| resolved.collect())
                    .unwrap_or_default();
                addresses.sort();
                addresses
            }
            #[cfg(test)]
            Transport::Scripted(fixture) => {
                if let Some(delay) = fixture.delay {
                    tokio::time::sleep(delay).await;
                }
                fixture
                    .addresses
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|address| SocketAddr::new(*address, port))
                    .collect()
            }
        }
    }

    async fn exchange(
        &self,
        addresses: &[SocketAddr],
        target: &Target,
        authorization: Option<HeaderValue>,
    ) -> Result<Vec<Model>, Error> {
        match self {
            Transport::System => connect_and_exchange(addresses, target, authorization).await,
            #[cfg(test)]
            Transport::Scripted(fixture) => {
                let Some(address) = addresses.first() else {
                    return Err(Error::Connection);
                };
                fixture.connects.lock().unwrap().push(*address);
                Ok(fixture.models.clone())
            }
        }
    }
}

async fn run_job(transport: &Transport, consent: &Mutex<Consent>, job: &Job) -> Status {
    let target = match endpoint(job.endpoint()) {
        Ok(target) => target,
        Err(error) => return Status::Complete(Err(error)),
    };
    let addresses = transport.resolve(&target.host, target.port).await;
    let class = classify(&addresses);
    let recorded = consent
        .lock()
        .map(|mut consent| {
            consent.probed.insert(job.endpoint().to_owned(), class);
        })
        .is_ok();
    if !recorded {
        return Status::Complete(Err(Error::Unavailable));
    }
    match job {
        Job::Probe {
            credential_attached,
            ..
        } => Status::Notice(Notice {
            endpoint: job.endpoint().to_owned(),
            resolved: resolved_names(&addresses),
            class,
            credential_attached: *credential_attached,
        }),
        Job::Discover(request) => {
            if class == AddressClass::Unresolved {
                return Status::Complete(Err(Error::Connection));
            }
            if class != AddressClass::Loopback {
                let decision = consent.lock().map(|consent| {
                    (
                        consent
                            .accepted
                            .contains(&(request.endpoint.clone(), class)),
                        consent
                            .accepted
                            .iter()
                            .any(|(accepted, _)| *accepted == request.endpoint),
                    )
                });
                let Ok((accepted, previously_accepted)) = decision else {
                    return Status::Complete(Err(Error::Unavailable));
                };
                if !accepted {
                    if previously_accepted {
                        return Status::Complete(Err(Error::AddressClassChanged));
                    }
                    return Status::Notice(Notice {
                        endpoint: request.endpoint.clone(),
                        resolved: resolved_names(&addresses),
                        class,
                        credential_attached: request.authorization_env.is_some(),
                    });
                }
            }
            let authorization = match authorization_header(request.authorization_env.as_deref()) {
                Ok(authorization) => authorization,
                Err(error) => return Status::Complete(Err(error)),
            };
            match transport.exchange(&addresses, &target, authorization).await {
                Ok(models) => Status::Complete(Ok(models)),
                Err(error) => Status::Complete(Err(error)),
            }
        }
    }
}

/// Conservative address classification: any public address, or a mix of loopback
/// and non-loopback, is reported as public.
fn classify(addresses: &[SocketAddr]) -> AddressClass {
    if addresses.is_empty() {
        return AddressClass::Unresolved;
    }
    if addresses.iter().all(|address| is_loopback(address.ip())) {
        return AddressClass::Loopback;
    }
    if addresses.iter().all(|address| is_private(address.ip())) {
        return AddressClass::Private;
    }
    AddressClass::Public
}

fn resolved_names(addresses: &[SocketAddr]) -> Vec<String> {
    addresses
        .iter()
        .map(|address| address.ip().to_string())
        .collect()
}

/// IPv4-mapped IPv6 addresses are classified as the IPv4 address they carry.
fn normalize(address: IpAddr) -> IpAddr {
    if let IpAddr::V6(address) = address
        && let Some(mapped) = address.to_ipv4_mapped()
    {
        return IpAddr::V4(mapped);
    }
    address
}

fn is_loopback(address: IpAddr) -> bool {
    match normalize(address) {
        IpAddr::V4(address) => address.is_loopback(),
        IpAddr::V6(address) => address.is_loopback(),
    }
}

fn is_private(address: IpAddr) -> bool {
    match normalize(address) {
        IpAddr::V4(address) => {
            address.is_private() || address.is_link_local() || is_carrier_grade_nat(&address)
        }
        IpAddr::V6(address) => address.is_unique_local() || address.is_unicast_link_local(),
    }
}

/// RFC 6598 carrier-grade NAT space, which `Ipv4Addr::is_private` does not cover.
fn is_carrier_grade_nat(address: &Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && second & 0b1100_0000 == 0b0100_0000
}

fn validate_authorization(name: Option<&str>) -> Result<(), Error> {
    let Some(name) = name else {
        return Ok(());
    };
    if name.is_empty()
        || name.len() > 128
        || !name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
    {
        return Err(Error::InvalidAuthorization);
    }
    Ok(())
}

fn authorization_header(name: Option<&str>) -> Result<Option<HeaderValue>, Error> {
    name.map(|name| {
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
    .transpose()
}

async fn connect_and_exchange(
    addresses: &[SocketAddr],
    target: &Target,
    authorization: Option<HeaderValue>,
) -> Result<Vec<Model>, Error> {
    let mut connected = None;
    for address in addresses {
        if let Ok(stream) = tokio::net::TcpStream::connect(*address).await {
            connected = Some(stream);
            break;
        }
    }
    let stream = connected.ok_or(Error::Connection)?;
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
            .uri(target.path.clone())
            .header("Host", target.authority.clone())
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

    const PRIVATE_ENDPOINT: &str = "http://192.168.222.33:8080/v1";
    const LOOPBACK_ENDPOINT: &str = "http://127.0.0.1:8080/v1";
    const AUTH_REFERENCE: &str = "FLUZO_TEST_ENDPOINT_CONSENT_AUTH";

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn sock(value: &str) -> SocketAddr {
        SocketAddr::new(ip(value), 8080)
    }

    fn request(id: u64, endpoint: &str, authorization_env: Option<&str>) -> Request {
        Request {
            protocol: PROTOCOL,
            id,
            endpoint: endpoint.into(),
            authorization_env: authorization_env.map(str::to_owned),
        }
    }

    fn probe(id: u64, endpoint: &str, authorization_env: Option<&str>) -> Probe {
        Probe {
            protocol: PROTOCOL,
            id,
            endpoint: endpoint.into(),
            authorization_env: authorization_env.map(str::to_owned),
        }
    }

    fn notice(endpoint: &str, resolved: &[&str], class: AddressClass, credential: bool) -> Status {
        Status::Notice(Notice {
            endpoint: endpoint.into(),
            resolved: resolved.iter().map(|value| (*value).to_owned()).collect(),
            class,
            credential_attached: credential,
        })
    }

    #[test]
    fn classify_is_conservative_across_address_ranges() {
        assert_eq!(classify(&[]), AddressClass::Unresolved);
        for value in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
            assert_eq!(classify(&[sock(value)]), AddressClass::Loopback, "{value}");
        }
        for value in [
            "10.0.0.5",
            "172.16.9.9",
            "192.168.222.33",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "fc00::1",
            "fd12:3456::7",
            "fe80::1",
            "::ffff:192.168.1.1",
        ] {
            assert_eq!(classify(&[sock(value)]), AddressClass::Private, "{value}");
        }
        for value in [
            "8.8.8.8",
            "203.0.113.9",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.0.1",
            "169.253.0.1",
        ] {
            assert_eq!(classify(&[sock(value)]), AddressClass::Public, "{value}");
        }
        assert_eq!(
            classify(&[sock("127.0.0.1"), sock("127.0.0.53")]),
            AddressClass::Loopback
        );
        // A mix of loopback and non-loopback is never reported as loopback.
        assert_eq!(
            classify(&[sock("127.0.0.1"), sock("10.0.0.1")]),
            AddressClass::Public
        );
        assert_eq!(
            classify(&[sock("192.168.1.2"), sock("203.0.113.9")]),
            AddressClass::Public
        );
        assert_eq!(
            classify(&[sock("127.0.0.1"), sock("203.0.113.9")]),
            AddressClass::Public
        );
    }

    #[test]
    fn probe_reports_a_notice_and_never_connects() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service
            .probe(probe(1, PRIVATE_ENDPOINT, Some(AUTH_REFERENCE)))
            .unwrap();
        let status = wait(&mut service, 1);
        assert_eq!(
            status,
            notice(
                PRIVATE_ENDPOINT,
                &["192.168.222.33"],
                AddressClass::Private,
                true
            )
        );
        assert!(fixture.connects().is_empty());
        // The reference name is operator-supplied; the resolved value never is.
        let rendered = format!("{status:?}");
        assert!(rendered.contains("Private"));
        assert!(!rendered.contains(AUTH_REFERENCE));
    }

    #[test]
    fn loopback_probe_and_submit_need_no_acceptance() {
        let fixture = Fixture::new(vec![ip("127.0.0.1")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service.probe(probe(1, LOOPBACK_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 1),
            notice(
                LOOPBACK_ENDPOINT,
                &["127.0.0.1"],
                AddressClass::Loopback,
                false
            )
        );
        assert!(fixture.connects().is_empty());
        service.submit(request(2, LOOPBACK_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 2),
            Status::Complete(Ok(vec!["fixture-model".into()]))
        );
        assert_eq!(fixture.connects(), vec![sock("127.0.0.1")]);
    }

    #[test]
    fn unaccepted_non_loopback_notices_then_refuses_with_zero_connects() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service.submit(request(1, PRIVATE_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 1),
            notice(
                PRIVATE_ENDPOINT,
                &["192.168.222.33"],
                AddressClass::Private,
                false
            )
        );
        assert!(fixture.connects().is_empty());
        // A retry is refused synchronously: nothing is queued, so nothing can connect.
        assert_eq!(
            service.submit(request(2, PRIVATE_ENDPOINT, None)),
            Err(Error::ConsentRequired)
        );
        assert_eq!(service.status(2), Status::Unknown);
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn acceptance_is_bound_to_the_exact_endpoint_string() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        let other = "http://192.168.222.99:8080/v1";
        service
            .accept(PRIVATE_ENDPOINT, AddressClass::Private)
            .unwrap();
        service.submit(request(1, other, None)).unwrap();
        assert!(matches!(wait(&mut service, 1), Status::Notice(_)));
        assert!(fixture.connects().is_empty());
        service.submit(request(2, PRIVATE_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 2),
            Status::Complete(Ok(vec!["fixture-model".into()]))
        );
        assert_eq!(fixture.connects(), vec![sock("192.168.222.33")]);
    }

    #[test]
    fn class_change_after_acceptance_aborts_before_any_request() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service.probe(probe(1, PRIVATE_ENDPOINT, None)).unwrap();
        assert!(matches!(wait(&mut service, 1), Status::Notice(_)));
        service
            .accept(PRIVATE_ENDPOINT, AddressClass::Private)
            .unwrap();
        fixture.set_addresses(vec![ip("203.0.113.9")]);
        service.submit(request(2, PRIVATE_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 2),
            Status::Complete(Err(Error::AddressClassChanged))
        );
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn unresolved_host_fails_closed_and_cannot_be_accepted() {
        let fixture = Fixture::new(vec![], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service.probe(probe(1, PRIVATE_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 1),
            notice(PRIVATE_ENDPOINT, &[], AddressClass::Unresolved, false)
        );
        assert_eq!(
            service.accept(PRIVATE_ENDPOINT, AddressClass::Unresolved),
            Err(Error::InvalidEndpoint)
        );
        // The recorded unresolved class makes a later submit refuse before queueing.
        assert_eq!(
            service.submit(request(2, PRIVATE_ENDPOINT, None)),
            Err(Error::ConsentRequired)
        );
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn unprobed_unresolvable_submit_fails_without_hanging_or_connecting() {
        let fixture = Fixture::new(vec![], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service.submit(request(1, PRIVATE_ENDPOINT, None)).unwrap();
        assert_eq!(
            wait(&mut service, 1),
            Status::Complete(Err(Error::Connection))
        );
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn acceptance_never_crosses_a_service_instance() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], None);
        let mut first = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        first
            .accept(PRIVATE_ENDPOINT, AddressClass::Private)
            .unwrap();
        drop(first);
        let mut second = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        second.submit(request(1, PRIVATE_ENDPOINT, None)).unwrap();
        assert!(matches!(wait(&mut second, 1), Status::Notice(_)));
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn declined_attempt_carries_neither_endpoint_nor_credential_reference() {
        let rendered = format!("{:?}", Error::ConsentRequired);
        assert_eq!(rendered, "ConsentRequired");
        assert!(!rendered.contains(PRIVATE_ENDPOINT));
        assert!(!rendered.contains("192.168.222.33"));
        assert!(!rendered.contains(AUTH_REFERENCE));
    }

    #[test]
    fn cancel_reaches_a_pending_probe() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], Some(Duration::from_millis(400)));
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        service.probe(probe(1, PRIVATE_ENDPOINT, None)).unwrap();
        service.cancel(1);
        assert_eq!(
            wait(&mut service, 1),
            Status::Complete(Err(Error::Cancelled))
        );
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn protocol_two_is_rejected_after_the_bump() {
        let fixture = Fixture::new(vec![ip("127.0.0.1")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        let mut submit = request(1, LOOPBACK_ENDPOINT, None);
        submit.protocol = 2;
        assert_eq!(service.submit(submit), Err(Error::UnsupportedProtocol));
        let mut probe = probe(2, LOOPBACK_ENDPOINT, None);
        probe.protocol = 2;
        assert_eq!(service.probe(probe), Err(Error::UnsupportedProtocol));
        assert!(fixture.connects().is_empty());
    }

    #[test]
    fn probe_validates_before_touching_the_worker() {
        let fixture = Fixture::new(vec![ip("192.168.222.33")], None);
        let mut service = ModelDiscoveryService::scripted(Arc::clone(&fixture)).unwrap();
        let mut probe = probe(0, PRIVATE_ENDPOINT, None);
        assert_eq!(service.probe(probe.clone()), Err(Error::Capacity));
        probe.id = 1;
        probe.endpoint = "https://192.168.222.33/v1".into();
        assert_eq!(service.probe(probe.clone()), Err(Error::InvalidEndpoint));
        probe.endpoint = PRIVATE_ENDPOINT.into();
        probe.authorization_env = Some("bad name\r\nInjected".into());
        assert_eq!(service.probe(probe), Err(Error::InvalidAuthorization));
        assert!(fixture.connects().is_empty());
    }
}
