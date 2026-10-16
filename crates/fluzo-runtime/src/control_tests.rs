//! RUN-04 P3 evidence: the private owner-verified control channel.
//!
//! Real Unix sockets and real process locks, no mocks. The child-role
//! fixture at the bottom is `#[ignore]`d and only runs when spawned by
//! its parent; running it alone fails its role assertion.

use crate::control::{
    self, ControlClient, ControlCommand, ControlError, ControlRequest, ControlServer,
    MAX_FRAME_BYTES, PROTOCOL_VERSION, SOCKET_NAME, code,
};
use crate::store::DRIVE_DIRECTORY;
use crate::workspace::{OwnershipError, WriterOwnership};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(700);

struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-ctl-{tag}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs_create(&path);
        Self(path)
    }

    fn at(tag: &str, extra_depth: usize) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let mut path = base.join(format!("fluzo-ctl-{tag}-{}", std::process::id()));
        for index in 0..extra_depth {
            path = path.join(format!("depth-{index}"));
        }
        fs_create(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn socket_path(&self) -> PathBuf {
        self.0.join(DRIVE_DIRECTORY).join(SOCKET_NAME)
    }
}

fn fs_create(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bind_server(root: &Path) -> ControlServer {
    let ownership = WriterOwnership::acquire(root).expect("acquire writer ownership");
    ControlServer::bind(ownership).expect("bind control server")
}

fn envelope_json(protocol: u64, owner: &str, request: &str) -> String {
    format!("{{\"protocol\":{protocol},\"owner\":\"{owner}\",\"request\":{request}}}")
}

fn frame(json: &str) -> Vec<u8> {
    let bytes = json.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

/// What the client actually saw: the server-side outcome plus the raw
/// response body, so a test can assert on the wire rather than on an
/// internal return value.
struct Wire {
    served: Result<control::ServedRequest, ControlError>,
    body: String,
}

impl Wire {
    /// The code as it appears on the wire, whether the transport
    /// succeeded with a refusal response or failed outright.
    fn code(&self) -> String {
        let value: serde_json::Value =
            serde_json::from_str(&self.body).expect("response frame is json");
        value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn ok(&self) -> bool {
        let value: serde_json::Value =
            serde_json::from_str(&self.body).expect("response frame is json");
        value
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    }
}

/// Serves one pre-framed request over a socketpair.
///
/// The write half is shut down after sending so a deliberately short or
/// malformed frame reaches the server as EOF. Without it the server
/// would block waiting for body bytes that no test intends to send, and
/// the fixture would hang instead of failing.
fn exchange(server: &ControlServer, request_bytes: &[u8]) -> Wire {
    let (mut client, server_side) = UnixStream::pair().expect("socketpair");
    client.write_all(request_bytes).expect("write request");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close the client write side");
    let served = server.serve_stream(server_side);
    // The server greets with the protocol version before anything
    // else; consume it or the frame header would read its bytes.
    let mut greeting = [0_u8; 8];
    client.read_exact(&mut greeting).expect("read greeting");
    assert_eq!(
        u64::from_be_bytes(greeting),
        PROTOCOL_VERSION,
        "the served socket should greet with the expected protocol"
    );
    let mut header = [0_u8; 4];
    client
        .read_exact(&mut header)
        .expect("read response header");
    let length = u32::from_be_bytes(header) as usize;
    let mut body = vec![0_u8; length];
    client.read_exact(&mut body).expect("read response body");
    Wire {
        served,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// Binding
// ---------------------------------------------------------------------------

#[test]
fn socket_is_created_owner_only_inside_the_protected_drive() {
    let root = Root::new("mode");
    let server = bind_server(root.path());
    let socket = server.socket_path();
    assert_eq!(socket, root.socket_path());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "the control socket must not be group or other accessible"
        );
    }
}

#[test]
fn bind_refuses_a_symlink_at_the_socket_path() {
    let root = Root::new("symlink");
    let ownership = WriterOwnership::acquire(root.path()).unwrap();
    let socket = root.socket_path();
    std::fs::create_dir_all(root.path().join(DRIVE_DIRECTORY)).unwrap();
    std::os::unix::fs::symlink("/tmp/definitely-not-a-fluzo-socket", &socket).unwrap();
    let error =
        ControlServer::bind(ownership).expect_err("a symlinked socket path must be refused");
    assert_eq!(error.code, code::SYMLINK_REFUSED, "{error}");
}

#[test]
fn bind_refuses_a_path_over_the_unix_socket_limit() {
    // `.fluzodrive/control.sock` is 25 bytes, so a root past 75 bytes
    // crosses the ~104-byte kernel limit. Refusing beats truncating.
    let root = Root::at("long", 1);
    let deep = {
        let mut path = root.path().to_path_buf();
        // Pad the final component until the socket path exceeds the cap.
        let mut name = path.file_name().unwrap().to_str().unwrap().to_owned();
        while path.as_os_str().len() + 25 <= control::MAX_SOCKET_PATH_BYTES {
            name.push('x');
            let parent = path.parent().unwrap().to_path_buf();
            path = parent.join(&name);
        }
        fs_create(&path);
        path
    };
    assert!(
        deep.join(DRIVE_DIRECTORY)
            .join(SOCKET_NAME)
            .as_os_str()
            .len()
            > control::MAX_SOCKET_PATH_BYTES,
        "fixture did not produce an over-limit socket path"
    );
    let ownership = WriterOwnership::acquire(&deep).unwrap();
    let error =
        ControlServer::bind(ownership).expect_err("an over-limit socket path must be refused");
    assert_eq!(error.code, code::PATH_TOO_LONG, "{error}");
}

// ---------------------------------------------------------------------------
// Owner verification
// ---------------------------------------------------------------------------

#[test]
fn a_request_from_the_bound_owner_is_served() {
    let root = Root::new("served");
    let server = bind_server(root.path());
    let token = server.owner_token().to_owned();
    let wire = exchange(
        &server,
        &frame(&envelope_json(
            PROTOCOL_VERSION,
            &token,
            "{\"kind\":\"task_status\",\"task_id\":\"task-1\"}",
        )),
    );
    let served = wire.served.expect("owner request should be served");
    assert!(served.response.ok);
    assert_eq!(
        served.request,
        ControlRequest::TaskStatus {
            task_id: "task-1".to_owned()
        }
    );
}

#[test]
fn a_wrong_owner_token_is_refused() {
    let root = Root::new("bad-token");
    let server = bind_server(root.path());
    let wire = exchange(
        &server,
        &frame(&envelope_json(
            PROTOCOL_VERSION,
            "not-this-acquisition",
            "{\"kind\":\"task_status\",\"task_id\":\"task-1\"}",
        )),
    );
    assert!(!wire.ok(), "a wrong token must not be served");
    assert_eq!(wire.code(), code::OWNER_MISMATCH);
    assert!(
        server.commands().try_recv().is_err(),
        "a refused request must signal nothing"
    );
}

#[test]
fn a_stale_owner_is_refused_even_with_the_right_token() {
    let root = Root::new("stale");
    let server = bind_server(root.path());
    let token = server.owner_token().to_owned();
    let envelope = ControlRequest::Cancel {
        task_id: "task-1".to_owned(),
    };
    let envelope = control::Envelope {
        protocol: PROTOCOL_VERSION,
        owner: token.clone(),
        request: envelope,
    };
    // Right token, lock no longer held: the request must not reach the
    // command channel.
    let response =
        server.handle_verifying(&envelope, || Err("lock lost to reconciliation".to_owned()));
    assert!(!response.ok);
    assert_eq!(response.code, code::STALE_OWNER);
    assert!(
        server.commands().try_recv().is_err(),
        "a stale-owner cancellation must never be delivered"
    );
    // And the same envelope with the real live check does get through.
    let live = server.handle(&envelope);
    assert!(live.ok, "the live lock should serve the same request");
    assert_eq!(
        server.commands().try_recv().unwrap(),
        ControlCommand::Cancel {
            task_id: "task-1".to_owned()
        }
    );
}

#[test]
fn a_wrong_protocol_version_is_refused_before_anything_else() {
    let root = Root::new("version");
    let server = bind_server(root.path());
    let token = server.owner_token().to_owned();
    // Version 2 with an otherwise valid owner: version wins, and the
    // request is never interpreted.
    let wire = exchange(
        &server,
        &frame(&envelope_json(
            PROTOCOL_VERSION + 1,
            &token,
            "{\"kind\":\"cancel\",\"task_id\":\"task-1\"}",
        )),
    );
    assert_eq!(wire.code(), code::UNSUPPORTED_VERSION);
    assert!(
        server.commands().try_recv().is_err(),
        "nothing was dispatched"
    );
}

// ---------------------------------------------------------------------------
// Closed request set
// ---------------------------------------------------------------------------

#[test]
fn the_request_set_is_closed_and_names_no_privilege_escalation() {
    let kinds = [
        ControlRequest::InspectSession {
            session_id: "s".into(),
        }
        .kind(),
        ControlRequest::TaskStatus {
            task_id: "t".into(),
        }
        .kind(),
        ControlRequest::Cancel {
            task_id: "t".into(),
        }
        .kind(),
        ControlRequest::ForceStop {
            task_id: "t".into(),
            confirmed: true,
        }
        .kind(),
    ];
    assert_eq!(
        kinds,
        ["inspect_session", "task_status", "cancel", "force_stop"],
        "the wire vocabulary must be exactly these four"
    );
    for forbidden in ["yolo", "set_yolo", "acquire_writer", "resume", "dispatch"] {
        assert!(
            !kinds.contains(&forbidden),
            "{forbidden} must not be expressible on the control channel"
        );
    }
}

#[test]
fn an_unknown_request_kind_is_refused_rather_than_mapped() {
    let root = Root::new("unknown-kind");
    let server = bind_server(root.path());
    let token = server.owner_token().to_owned();
    let wire = exchange(
        &server,
        &frame(&envelope_json(
            PROTOCOL_VERSION,
            &token,
            "{\"kind\":\"set_yolo\",\"enabled\":true}",
        )),
    );
    assert_eq!(wire.code(), code::UNKNOWN_REQUEST);
    assert!(server.commands().try_recv().is_err());
}

#[test]
fn read_only_requests_never_reach_the_command_channel() {
    let root = Root::new("read-only");
    let server = bind_server(root.path());
    let token = server.owner_token().to_owned();
    for request in [
        ControlRequest::InspectSession {
            session_id: "s-1".into(),
        },
        ControlRequest::TaskStatus {
            task_id: "t-1".into(),
        },
    ] {
        assert!(request.is_read_only());
        let response = server.handle(&control::Envelope {
            protocol: PROTOCOL_VERSION,
            owner: token.clone(),
            request,
        });
        assert!(response.ok);
        assert!(
            server.commands().try_recv().is_err(),
            "an inspection request must not enqueue a command"
        );
    }
}

// ---------------------------------------------------------------------------
// Force-stop confirmation
// ---------------------------------------------------------------------------

#[test]
fn force_stop_without_confirmation_returns_a_structured_refusal_and_signals_nothing() {
    let root = Root::new("force-unconfirmed");
    let server = bind_server(root.path());
    let response = server.handle(&control::Envelope {
        protocol: PROTOCOL_VERSION,
        owner: server.owner_token().to_owned(),
        request: ControlRequest::ForceStop {
            task_id: "task-9".into(),
            confirmed: false,
        },
    });
    assert!(!response.ok);
    assert_eq!(response.code, code::CONFIRMATION_REQUIRED);
    assert!(
        server.commands().try_recv().is_err(),
        "an unconfirmed force stop must not reach the runtime"
    );
}

#[test]
fn force_stop_with_confirmation_is_delivered_once() {
    let root = Root::new("force-confirmed");
    let server = bind_server(root.path());
    let response = server.handle(&control::Envelope {
        protocol: PROTOCOL_VERSION,
        owner: server.owner_token().to_owned(),
        request: ControlRequest::ForceStop {
            task_id: "task-9".into(),
            confirmed: true,
        },
    });
    assert!(response.ok);
    assert_eq!(
        server.commands().try_recv().unwrap(),
        ControlCommand::ForceStop {
            task_id: "task-9".to_owned()
        }
    );
    assert!(
        server.commands().try_recv().is_err(),
        "one request must enqueue exactly one command"
    );
}

// ---------------------------------------------------------------------------
// Framing limits
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_declared_length_is_refused_before_allocation() {
    let root = Root::new("oversize");
    let server = bind_server(root.path());
    let token = server.owner_token().to_owned();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_be_bytes());
    bytes.extend_from_slice(
        envelope_json(
            PROTOCOL_VERSION,
            &token,
            "{\"kind\":\"cancel\",\"task_id\":\"t\"}",
        )
        .as_bytes(),
    );
    let wire = exchange(&server, &bytes);
    assert_eq!(wire.code(), code::FRAME_TOO_LARGE);
    assert!(server.commands().try_recv().is_err());
}

#[test]
fn a_truncated_frame_is_refused() {
    let root = Root::new("truncated");
    let server = bind_server(root.path());
    let wire = exchange(&server, &[0, 0, 0, 40]);
    assert_eq!(wire.code(), code::TRUNCATED);
}

#[test]
fn a_non_json_body_is_refused_as_malformed() {
    let root = Root::new("malformed");
    let server = bind_server(root.path());
    let wire = exchange(&server, &frame("this is not json"));
    assert_eq!(wire.code(), code::MALFORMED);
}

#[test]
fn a_refusal_does_not_echo_the_callers_input() {
    let root = Root::new("no-echo");
    let server = bind_server(root.path());
    let secret = "sk-super-secret-session-token";
    let wire = exchange(
        &server,
        &frame(&envelope_json(
            PROTOCOL_VERSION,
            secret,
            "{\"kind\":\"task_status\",\"task_id\":\"t\"}",
        )),
    );
    assert!(
        !wire.body.contains(secret),
        "a refusal must return a code, not echo what the caller sent"
    );
    assert!(wire.body.contains(code::OWNER_MISMATCH));
}

// ---------------------------------------------------------------------------
// Client round trip over the real socket
// ---------------------------------------------------------------------------

#[test]
fn client_and_server_round_trip_over_the_real_socket() {
    let root = Root::new("roundtrip");
    // `mpsc::Receiver` is not `Sync`, so the server is shared through
    // a mutex rather than a bare Arc.
    let server: Arc<std::sync::Mutex<ControlServer>> =
        Arc::new(std::sync::Mutex::new(bind_server(root.path())));
    let token = server.lock().unwrap().owner_token().to_owned();
    let socket = server.lock().unwrap().socket_path().to_owned();
    let serve_server = Arc::clone(&server);
    let served = std::thread::spawn(move || serve_server.lock().unwrap().serve_one());

    let mut client = ControlClient::connect(&socket).expect("connect");
    let response = client
        .request(
            &token,
            ControlRequest::TaskStatus {
                task_id: "t-7".into(),
            },
        )
        .expect("request");
    assert!(response.ok, "{response:?}");
    assert_eq!(response.detail, "status of t-7");

    let served = served.join().expect("serve thread").expect("served");
    assert_eq!(
        served.request,
        ControlRequest::TaskStatus {
            task_id: "t-7".to_owned()
        }
    );
}

#[test]
fn a_client_connecting_with_a_mismatched_greeting_fails_before_requesting() {
    // A plain listener that greets with the wrong version must be
    // rejected before the client sends a request it could act on.
    let root = Root::new("bad-greeting");
    let listener = bind_plain_listener(&root.socket_path());
    let socket = root.socket_path();
    let server_side = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut stream = stream;
        stream.write_all(&99_u64.to_be_bytes()).expect("greeting");
        // Keep the connection open so the client cannot mistake a
        // closed socket for a version problem.
        std::thread::sleep(std::time::Duration::from_millis(200));
    });
    let error = ControlClient::connect(&socket).expect_err("a wrong greeting must be refused");
    assert_eq!(error.code, code::UNSUPPORTED_VERSION, "{error}");
    server_side.join().unwrap();
}

fn bind_plain_listener(path: &Path) -> std::os::unix::net::UnixListener {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::net::UnixListener::bind(path).unwrap()
}

// ---------------------------------------------------------------------------
// Cross-process: an inspection client cannot become a writer
// ---------------------------------------------------------------------------

/// Serves one request from a background thread. `mpsc::Receiver` is
/// not `Sync`, so the server is shared through a mutex; the parent
/// keeps the handle so it can still assert on its own ownership after
/// the child has been served.
fn serve_one_in_background(
    server: ControlServer,
) -> (
    Arc<std::sync::Mutex<ControlServer>>,
    std::thread::JoinHandle<control::ServedRequest>,
) {
    let server = Arc::new(std::sync::Mutex::new(server));
    let serving = Arc::clone(&server);
    let handle =
        std::thread::spawn(move || serving.lock().unwrap().serve_one().expect("serve one"));
    (server, handle)
}

/// PRD 17.2: opening an inspection channel must not start a second
/// writer. The child connects as a control client, gets its answer,
/// and is then refused the writer lock that the parent still holds.
#[test]
fn an_inspection_client_cannot_take_the_writer_lock() {
    let root = Root::new("crossproc");
    let (server, served) = serve_one_in_background(bind_server(root.path()));
    let token = server.lock().unwrap().owner_token().to_owned();
    let socket = server.lock().unwrap().socket_path().to_owned();

    let mut child = ChildProcess::start("inspect_only", root.path(), &socket, &token);
    let reply = child.read_line("STATUS ");
    assert_eq!(
        reply.trim(),
        "STATUS ok",
        "the control client should have been served"
    );
    let acquire = child.read_line("ACQUIRE ");
    assert!(
        acquire.starts_with("ACQUIRE held_by"),
        "the inspection client must not obtain writer ownership, got {acquire}"
    );
    served.join().expect("serve thread");
    // The parent still owns the workspace: serving an inspection
    // client neither transferred nor released anything.
    assert!(
        server
            .lock()
            .unwrap()
            .ownership()
            .verify_still_owned()
            .is_ok()
    );
    child.kill_and_wait();
}

#[test]
fn a_cross_process_client_with_a_stale_token_is_refused() {
    let root = Root::new("crossproc-stale");
    let (server, served) = serve_one_in_background(bind_server(root.path()));
    let socket = server.lock().unwrap().socket_path().to_owned();
    let mut child = ChildProcess::start("stale_token", root.path(), &socket, "stale-token-value");
    let reply = child.read_line("STATUS ");
    assert_eq!(
        reply.trim(),
        "STATUS owner_mismatch",
        "a copied token must not control another process's workspace"
    );
    served.join().expect("serve thread");
    assert!(
        server.lock().unwrap().commands().try_recv().is_err(),
        "a refused cross-process request must enqueue nothing"
    );
    child.kill_and_wait();
}

// ---------------------------------------------------------------------------
// Child plumbing
// ---------------------------------------------------------------------------

struct ChildProcess {
    child: Child,
    reader: BufReader<ChildStdout>,
}

impl ChildProcess {
    fn start(role: &str, root: &Path, socket: &Path, token: &str) -> Self {
        let exe = std::env::current_exe().expect("test exe");
        let mut child = Command::new(exe)
            .arg("--ignored")
            .arg("--exact")
            .arg("control_tests::control_child_role")
            .arg("--nocapture")
            .env("FLUZO_CTL_ROLE", role)
            .env("FLUZO_CTL_ROOT", root)
            .env("FLUZO_CTL_SOCKET", socket)
            .env("FLUZO_CTL_TOKEN", token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn control child");
        let stdout = child.stdout.take().expect("child stdout");
        Self {
            child,
            reader: BufReader::new(stdout),
        }
    }

    fn read_line(&mut self, prefix: &str) -> String {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    let status = self.child.wait().expect("wait after early exit");
                    panic!("control child exited {status} before announcing {prefix:?}");
                }
                Ok(_) => {
                    if line.starts_with(prefix) {
                        return line;
                    }
                }
                Err(error) => panic!("failed reading control child output: {error}"),
            }
        }
    }

    fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Entry point for the child processes above. `#[ignore]`d so it never
/// runs as a normal test; the fixtures invoke it directly.
#[test]
#[ignore = "child role driven by the control fixtures, not run standalone"]
fn control_child_role() {
    let role = std::env::var("FLUZO_CTL_ROLE").expect("FLUZO_CTL_ROLE");
    let socket = PathBuf::from(std::env::var("FLUZO_CTL_SOCKET").expect("FLUZO_CTL_SOCKET"));
    let token = std::env::var("FLUZO_CTL_TOKEN").expect("FLUZO_CTL_TOKEN");
    let root = PathBuf::from(std::env::var("FLUZO_CTL_ROOT").expect("FLUZO_CTL_ROOT"));
    let mut client = ControlClient::connect(&socket).expect("child connects to the control socket");
    match role.as_str() {
        "inspect_only" => {
            let response = client
                .request(
                    &token,
                    ControlRequest::TaskStatus {
                        task_id: "t-1".into(),
                    },
                )
                .expect("child request");
            announce(&format!("STATUS {}", response.code));
            // The same process now tries to become the writer. It must
            // be refused while the parent holds the lock.
            match WriterOwnership::acquire(&root) {
                Ok(_) => announce("ACQUIRE granted"),
                Err(OwnershipError::HeldBy { .. }) => announce("ACQUIRE held_by"),
                Err(other) => announce(&format!("ACQUIRE other_{other:?}")),
            }
        }
        "stale_token" => {
            let response = client
                .request(
                    &token,
                    ControlRequest::Cancel {
                        task_id: "t-1".into(),
                    },
                )
                .expect("child request");
            announce(&format!("STATUS {}", response.code));
        }
        other => panic!("unknown control child role {other:?}"),
    }
    // Stay alive so the parent can inspect its own state before reaping.
    std::thread::sleep(std::time::Duration::from_secs(30));
}

fn announce(text: &str) {
    println!("{text}");
    let _ = std::io::stdout().flush();
}
