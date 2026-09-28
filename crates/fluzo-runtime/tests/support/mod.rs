use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body, Bytes, Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

pub const DEADLINE: Duration = Duration::from_secs(5);
pub const MAX_BYTES: usize = 64 * 1024;
const MAX_STEPS: usize = 32;
const MAX_CONNECTIONS: usize = 64;
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    UnexpectedRequest { step: usize },
    InvalidRequest { step: usize },
    MissingSteps { remaining: usize },
    IncompleteBody { step: usize },
    Transport,
    Limit,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub version: u32,
    pub requests: usize,
    pub consumed: usize,
    pub completed: usize,
    pub cancelled: usize,
    pub failures: Vec<Failure>,
}

impl Report {
    fn fail(&mut self, failure: Failure) {
        if self.failures.len() < MAX_CONNECTIONS {
            self.failures.push(failure);
        }
    }
}

pub enum Chunk {
    Data(Bytes),
    Gate(oneshot::Receiver<()>),
    Disconnect,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Chunk>,
    pub expect_cancel: bool,
}

impl Reply {
    pub fn bytes(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            chunks: vec![Chunk::Data(body.into())],
            expect_cancel: false,
        }
    }
}

pub struct Step {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    pub reply: Reply,
}

pub struct Scenario {
    pub version: u32,
    pub steps: Vec<Step>,
}

struct State {
    steps: VecDeque<Step>,
    report: Report,
    stopping: bool,
}

struct ScriptBody {
    chunks: VecDeque<Chunk>,
    state: Arc<Mutex<State>>,
    step: Option<usize>,
    terminal: bool,
    expect_cancel: bool,
    progress: watch::Sender<usize>,
}

impl Body for ScriptBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        loop {
            match self.chunks.front_mut() {
                Some(Chunk::Gate(receiver)) => match Pin::new(receiver).poll(context) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(())) => {
                        self.chunks.pop_front();
                    }
                    Poll::Ready(Err(_)) => {
                        return Poll::Ready(Some(Err(io::Error::other("script gate abandoned"))));
                    }
                },
                Some(Chunk::Data(_)) => {
                    let Some(Chunk::Data(bytes)) = self.chunks.pop_front() else {
                        unreachable!()
                    };
                    self.progress.send_modify(|count| *count += 1);
                    return Poll::Ready(Some(Ok(Frame::data(bytes))));
                }
                Some(Chunk::Disconnect) => {
                    self.chunks.clear();
                    self.terminal = true;
                    return Poll::Ready(Some(Err(io::Error::other("scripted disconnect"))));
                }
                None => {
                    self.terminal = true;
                    return Poll::Ready(None);
                }
            }
        }
    }
}

impl Drop for ScriptBody {
    fn drop(&mut self) {
        if let Some(step) = self.step {
            let mut state = self.state.lock().unwrap();
            if self.terminal && !self.expect_cancel {
                state.report.completed += 1;
            } else if !self.terminal && self.expect_cancel && !state.stopping {
                state.report.cancelled += 1;
            } else {
                state.report.fail(Failure::IncompleteBody { step });
            }
        }
    }
}

async fn respond(
    request: Request<Incoming>,
    state: Arc<Mutex<State>>,
    progress: watch::Sender<usize>,
) -> Result<Response<ScriptBody>, Infallible> {
    let (parts, body) = request.into_parts();
    let body = timeout(DEADLINE, Limited::new(body, MAX_BYTES).collect()).await;
    let mut state_guard = state.lock().unwrap();
    let step_index = state_guard.report.consumed;
    state_guard.report.requests += 1;
    let bytes = match body {
        Ok(Ok(body)) => Some(body.to_bytes()),
        _ => None,
    };
    let matched = state_guard.steps.front().is_some_and(|step| {
        parts.method.as_str() == step.method
            && parts
                .uri
                .path_and_query()
                .is_some_and(|path| path.as_str() == step.path)
            && step.headers.iter().all(|(name, value)| {
                parts
                    .headers
                    .get(name)
                    .is_some_and(|actual| actual == value.as_str())
            })
            && bytes.as_ref().is_some_and(|bytes| match &step.body {
                Some(expected) => {
                    serde_json::from_slice::<Value>(bytes).is_ok_and(|actual| actual == *expected)
                }
                None => bytes.is_empty(),
            })
    });
    let (reply, step) = if matched {
        state_guard.report.consumed += 1;
        (
            state_guard.steps.pop_front().unwrap().reply,
            Some(step_index),
        )
    } else {
        let failure = if bytes.is_none() {
            Failure::InvalidRequest { step: step_index }
        } else {
            Failure::UnexpectedRequest { step: step_index }
        };
        state_guard.report.fail(failure);
        (
            Reply::bytes(StatusCode::BAD_REQUEST, "scenario request rejected"),
            None,
        )
    };
    drop(state_guard);
    let mut response = Response::builder().status(reply.status);
    for (name, value) in reply.headers {
        response = response.header(name, value);
    }
    Ok(response
        .body(ScriptBody {
            chunks: reply.chunks.into(),
            state,
            step,
            terminal: false,
            expect_cancel: reply.expect_cancel,
            progress,
        })
        .expect("validated response headers"))
}

pub struct Fixture {
    address: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    progress: watch::Receiver<usize>,
    active: watch::Receiver<usize>,
    pub root: PathBuf,
}

impl Fixture {
    pub async fn start(scenario: Scenario) -> io::Result<Self> {
        if scenario.version != 1 || scenario.steps.len() > MAX_STEPS {
            return Err(io::Error::other(
                "unsupported scenario version or step limit",
            ));
        }
        let mut total = 0usize;
        for step in &scenario.steps {
            total =
                total.saturating_add(step.body.as_ref().map_or(0, |body| body.to_string().len()));
            if step.reply.chunks.len() > 128
                || step.headers.len() > 16
                || step.reply.headers.len() > 16
            {
                return Err(io::Error::other("scenario count limit"));
            }
            for chunk in &step.reply.chunks {
                if let Chunk::Data(bytes) = chunk {
                    total = total.saturating_add(bytes.len());
                }
            }
            for (name, value) in step.headers.iter().chain(&step.reply.headers) {
                total = total.saturating_add(name.len()).saturating_add(value.len());
                if hyper::header::HeaderName::from_bytes(name.as_bytes()).is_err()
                    || hyper::header::HeaderValue::from_str(value).is_err()
                {
                    return Err(io::Error::other("invalid scenario header"));
                }
            }
            if total > MAX_BYTES || step.path.len() > 1024 || step.method.len() > 16 {
                return Err(io::Error::other("scenario byte limit"));
            }
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let root = std::env::temp_dir().join(format!(
            "fluzo-http-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root)?;
        let state = Arc::new(Mutex::new(State {
            steps: scenario.steps.into(),
            stopping: false,
            report: Report {
                version: scenario.version,
                requests: 0,
                consumed: 0,
                completed: 0,
                cancelled: 0,
                failures: Vec::new(),
            },
        }));
        let (stop, mut stopped) = oneshot::channel();
        let (progress_sender, progress) = watch::channel(0);
        let (active_sender, active) = watch::channel(0);
        let worker_state = state.clone();
        let worker = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let mut accepted = 0;
            let lifetime = tokio::time::sleep(DEADLINE);
            tokio::pin!(lifetime);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    _ = &mut lifetime => {
                        worker_state.lock().unwrap().report.fail(Failure::Transport);
                        break;
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        if result.is_err() { worker_state.lock().unwrap().report.fail(Failure::Transport); }
                    }
                    incoming = listener.accept() => {
                        let Ok((socket, _)) = incoming else {
                            worker_state.lock().unwrap().report.fail(Failure::Transport);
                            break;
                        };
                        accepted += 1;
                        if accepted > MAX_CONNECTIONS || connections.len() >= 8 {
                            worker_state.lock().unwrap().report.fail(Failure::Limit);
                            drop(socket);
                            continue;
                        }
                        let request_state = worker_state.clone();
                        let connection_state = worker_state.clone();
                        let progress = progress_sender.clone();
                        let active = active_sender.clone();
                        active.send_modify(|count| *count += 1);
                        connections.spawn(async move {
                            let handled = Arc::new(AtomicBool::new(false));
                            let observed = handled.clone();
                            let service = service_fn(move |request| {
                                observed.store(true, Ordering::Relaxed);
                                respond(request, request_state.clone(), progress.clone())
                            });
                            let connection = hyper::server::conn::http1::Builder::new()
                                .max_buf_size(16 * 1024).keep_alive(false).auto_date_header(false)
                                .serve_connection(TokioIo::new(socket), service);
                            match timeout(DEADLINE, connection).await {
                                Err(_) => connection_state.lock().unwrap().report.fail(Failure::Transport),
                                Ok(Err(_)) if !handled.load(Ordering::Relaxed) => connection_state.lock().unwrap().report.fail(Failure::Transport),
                                _ => {}
                            }
                            active.send_modify(|count| *count -= 1);
                        });
                    }
                }
            }
            worker_state.lock().unwrap().stopping = true;
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        let fixture = Self {
            address,
            state,
            stop: Some(stop),
            worker: Some(worker),
            progress,
            active,
            root,
        };
        for directory in ["home", "config", "data", "workspace"] {
            std::fs::create_dir(fixture.root.join(directory))?;
        }
        Ok(fixture)
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .current_dir(self.root.join("workspace"))
            .stdin(std::process::Stdio::null());
        command
    }

    pub async fn socket(&self, endpoint: &str) -> io::Result<TcpStream> {
        if endpoint != self.endpoint() || self.stop.is_none() {
            return Err(io::Error::other(
                "destination is not registered for this fixture",
            ));
        }
        timeout(DEADLINE, TcpStream::connect(self.address)).await?
    }

    pub async fn connect(&self, endpoint: &str) -> io::Result<Client> {
        let socket = self.socket(endpoint).await?;
        let (sender, connection) = timeout(
            DEADLINE,
            hyper::client::conn::http1::handshake(TokioIo::new(socket)),
        )
        .await?
        .map_err(|_| io::Error::other("fixture handshake failed"))?;
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Client {
            sender,
            driver,
            address: self.address,
        })
    }

    pub async fn wait_for_chunks(&mut self, count: usize) {
        timeout(
            DEADLINE,
            self.progress.wait_for(|current| *current >= count),
        )
        .await
        .unwrap()
        .unwrap();
    }

    pub async fn wait_for_idle(&mut self) {
        timeout(DEADLINE, self.active.wait_for(|count| *count == 0))
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn finish(&mut self) -> Report {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut worker) = self.worker.take() {
            match timeout(DEADLINE, &mut worker).await {
                Ok(Ok(())) => {}
                _ => {
                    worker.abort();
                    self.state.lock().unwrap().report.fail(Failure::Transport);
                }
            }
        }
        let mut state = self.state.lock().unwrap();
        let remaining = state.steps.len();
        if remaining != 0
            && !state
                .report
                .failures
                .contains(&Failure::MissingSteps { remaining })
        {
            state.report.fail(Failure::MissingSteps { remaining });
        }
        state.report.clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub struct Client {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    driver: JoinHandle<()>,
    address: SocketAddr,
}

impl Client {
    pub async fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Bytes,
    ) -> io::Result<Response<Incoming>> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.len() > 1024
            || body.len() > MAX_BYTES
        {
            return Err(io::Error::other("invalid fixture request"));
        }
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.address.to_string())
            .header("content-type", "application/json")
            .body(Full::new(body))
            .map_err(|_| io::Error::other("invalid fixture request"))?;
        timeout(DEADLINE, self.sender.send_request(request))
            .await?
            .map_err(|_| io::Error::other("fixture request failed"))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

pub async fn collect(response: Response<Incoming>) -> io::Result<Bytes> {
    timeout(
        DEADLINE,
        Limited::new(response.into_body(), MAX_BYTES).collect(),
    )
    .await?
    .map(|body| body.to_bytes())
    .map_err(|_| io::Error::other("incomplete fixture response"))
}
