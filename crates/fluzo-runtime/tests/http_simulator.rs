mod support;

use http_body_util::BodyExt;
use hyper::StatusCode;
use hyper::body::Bytes;
use serde_json::{Value, json};
use support::{Chunk, DEADLINE, Failure, Fixture, MAX_BYTES, Reply, Scenario, Step, collect};
use tokio::sync::oneshot;
use tokio::time::timeout;

fn step(path: &str, body: Option<Value>, reply: Reply) -> Step {
    Step {
        method: if body.is_some() { "POST" } else { "GET" }.into(),
        path: path.into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body,
        reply,
    }
}

async fn fixture(steps: Vec<Step>) -> Fixture {
    Fixture::start(Scenario { version: 1, steps })
        .await
        .unwrap()
}

fn chat(stream: bool) -> Value {
    json!({"model":"synthetic", "stream":stream, "messages":[{"role":"user","content":"Inspect the fixture"}]})
}

#[tokio::test(flavor = "current_thread")]
async fn discovery_chat_and_tool_result_follow_versioned_semantic_steps() {
    let first = chat(false);
    let assistant = json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/lib.rs\"}"}}]});
    let second = json!({"model":"synthetic","stream":false,"messages":[first["messages"][0].clone(),assistant.clone(),{"role":"tool","tool_call_id":"call-1","content":"fixture text"}]});
    let answer = json!({"id":"chat-1","object":"chat.completion","model":"synthetic","choices":[{"index":0,"message":assistant,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":8,"completion_tokens":4,"total_tokens":12}});
    let mut server = fixture(vec![
        step("/v1/models", None, Reply::bytes(StatusCode::OK, r#"{"object":"list","data":[{"id":"synthetic","object":"model"}]}"#)),
        step("/v1/chat/completions", Some(first.clone()), Reply::bytes(StatusCode::OK, answer.to_string())),
        step("/v1/chat/completions", Some(second.clone()), Reply::bytes(StatusCode::OK, r#"{"choices":[{"message":{"role":"assistant","content":"Inspected"},"finish_reason":"stop"}]}"#)),
    ]).await;
    let protected = server.root.join("workspace/source.rs");
    std::fs::write(&protected, "untouched fixture").unwrap();
    for (method, path, body) in [
        ("GET", "/v1/models", Bytes::new()),
        ("POST", "/v1/chat/completions", first.to_string().into()),
        ("POST", "/v1/chat/completions", second.to_string().into()),
    ] {
        let mut client = server.connect(&server.endpoint()).await.unwrap();
        let response = client.request(method, path, body).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            serde_json::from_slice::<Value>(&collect(response).await.unwrap())
                .unwrap()
                .is_object()
        );
    }
    let report = server.finish().await;
    assert_eq!(report.version, 1);
    assert_eq!(
        (report.requests, report.consumed, report.completed),
        (3, 3, 3)
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(
        std::fs::read_to_string(protected).unwrap(),
        "untouched fixture"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn wrong_missing_duplicate_and_secret_requests_fail_without_echoing_content() {
    for body in [json!({"secret":"synthetic-secret"}), chat(true)] {
        let mut server = fixture(vec![step(
            "/v1/chat/completions",
            Some(chat(false)),
            Reply::bytes(StatusCode::OK, "{}"),
        )])
        .await;
        let mut client = server.connect(&server.endpoint()).await.unwrap();
        let response = client
            .request("POST", "/v1/chat/completions", body.to_string().into())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            collect(response).await.unwrap(),
            "scenario request rejected"
        );
        let report = server.finish().await;
        assert_eq!(
            report.failures,
            vec![
                Failure::UnexpectedRequest { step: 0 },
                Failure::MissingSteps { remaining: 1 }
            ]
        );
        assert!(!format!("{report:?}").contains("synthetic-secret"));
    }
    let mut server = fixture(vec![step(
        "/v1/models",
        None,
        Reply::bytes(StatusCode::OK, "{}"),
    )])
    .await;
    for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
        let mut client = server.connect(&server.endpoint()).await.unwrap();
        let response = client
            .request("GET", "/v1/models", Bytes::new())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        collect(response).await.unwrap();
    }
    let report = server.finish().await;
    assert_eq!(
        report.failures,
        vec![Failure::UnexpectedRequest { step: 1 }]
    );
    let mut missing = fixture(vec![step(
        "/v1/models",
        None,
        Reply::bytes(StatusCode::OK, "{}"),
    )])
    .await;
    assert_eq!(
        missing.finish().await.failures,
        vec![Failure::MissingSteps { remaining: 1 }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fragments_are_released_by_acknowledgement_not_sleep() {
    let (release, held) = oneshot::channel();
    let prefix = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n";
    let suffix = "\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"src/lib.rs\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":8,\"completion_tokens\":4,\"total_tokens\":12}}\n\ndata: [DONE]\n\n";
    let mut reply = Reply::bytes(StatusCode::OK, Bytes::new());
    reply
        .headers
        .push(("content-type".into(), "text/event-stream".into()));
    reply.chunks = vec![
        Chunk::Data(prefix.into()),
        Chunk::Gate(held),
        Chunk::Data(suffix.into()),
    ];
    let mut server = fixture(vec![step("/v1/chat/completions", Some(chat(true)), reply)]).await;
    let mut client = server.connect(&server.endpoint()).await.unwrap();
    let response = client
        .request(
            "POST",
            "/v1/chat/completions",
            chat(true).to_string().into(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();
    let first = timeout(DEADLINE, body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert_eq!(first, prefix);
    server.wait_for_chunks(1).await;
    release.send(()).unwrap();
    let remaining = timeout(DEADLINE, body.collect())
        .await
        .unwrap()
        .unwrap()
        .to_bytes();
    assert_eq!(remaining, suffix);
    let stream = format!("{prefix}{suffix}");
    let events: Vec<_> = stream
        .split("\n\n")
        .filter_map(|event| event.strip_prefix("data: "))
        .collect();
    assert_eq!(events.last(), Some(&"[DONE]"));
    let deltas: Vec<Value> = events[..3]
        .iter()
        .map(|event| serde_json::from_str(event).unwrap())
        .collect();
    let arguments = format!(
        "{}{}",
        deltas[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap(),
        deltas[1]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&arguments).unwrap(),
        json!({"path":"src/lib.rs"})
    );
    assert_eq!(deltas[1]["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(deltas[2]["usage"]["total_tokens"], 12);
    assert!(server.finish().await.failures.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn errors_malformed_decisions_and_laya_routes_are_scripted_not_repaired() {
    for path in ["/v1/systemone", "/upstream/laya/v1/systemone"] {
        for (status, payload) in [
            (StatusCode::OK, "{malformed"),
            (StatusCode::OK, r#"{"decision":42}"#),
            (StatusCode::UNAUTHORIZED, "{}"),
            (StatusCode::SERVICE_UNAVAILABLE, "{}"),
            (
                StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":{"type":"capacity_exceeded"}}"#,
            ),
        ] {
            let request = json!({"request_id":"synthetic-1","context":{"task":"inspect"}});
            let mut server = fixture(vec![step(
                path,
                Some(request.clone()),
                Reply::bytes(status, payload),
            )])
            .await;
            let mut client = server.connect(&server.endpoint()).await.unwrap();
            let response = client
                .request("POST", path, request.to_string().into())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(collect(response).await.unwrap(), payload);
            let report = server.finish().await;
            assert_eq!(report.requests, 1);
            assert!(report.failures.is_empty());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn registered_destinations_reject_redirects_foreign_runs_and_retries() {
    let mut target = fixture(vec![]).await;
    let mut redirect = Reply::bytes(StatusCode::TEMPORARY_REDIRECT, "redirect rejected");
    redirect.headers.push((
        "location".into(),
        format!("{}/v1/models", target.endpoint()),
    ));
    let mut origin = fixture(vec![step("/v1/models", None, redirect)]).await;
    assert!(origin.connect(&target.endpoint()).await.is_err());
    assert!(
        origin
            .connect(&origin.endpoint().replacen("http:", "https:", 1))
            .await
            .is_err()
    );
    let mut client = origin.connect(&origin.endpoint()).await.unwrap();
    assert!(
        client
            .request("GET", &target.endpoint(), Bytes::new())
            .await
            .is_err()
    );
    let response = client
        .request("GET", "/v1/models", Bytes::new())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    collect(response).await.unwrap();
    assert!(origin.finish().await.failures.is_empty());
    assert_eq!(target.finish().await.requests, 0);
    assert!(origin.connect(&origin.endpoint()).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn disconnect_and_cancellation_preserve_partial_output() {
    for cancel in [false, true] {
        let (release, held) = oneshot::channel();
        let mut reply = Reply::bytes(StatusCode::OK, "partial");
        reply.expect_cancel = cancel;
        reply.chunks.push(Chunk::Gate(held));
        reply.chunks.push(Chunk::Disconnect);
        let mut server = fixture(vec![step("/v1/models", None, reply)]).await;
        let mut client = server.connect(&server.endpoint()).await.unwrap();
        let response = client
            .request("GET", "/v1/models", Bytes::new())
            .await
            .unwrap();
        let mut body = response.into_body();
        let partial = timeout(DEADLINE, body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(partial, "partial");
        server.wait_for_chunks(1).await;
        if cancel {
            drop(body);
            drop(client);
            server.wait_for_idle().await;
            drop(release);
        } else {
            release.send(()).unwrap();
            assert!(timeout(DEADLINE, body.collect()).await.unwrap().is_err());
        }
        let report = server.finish().await;
        assert!(report.failures.is_empty(), "{report:?}");
        assert_eq!(report.cancelled, usize::from(cancel));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn independent_roots_environments_and_parallel_listeners_are_cleaned() {
    let (mut first, mut second) = tokio::join!(fixture(vec![]), fixture(vec![]));
    assert_ne!(first.endpoint(), second.endpoint());
    assert_ne!(first.root, second.root);
    let command = first.command("synthetic-child-not-executed");
    let environment: Vec<_> = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.unwrap().to_owned()))
        .collect();
    assert_eq!(environment.len(), 3);
    assert_eq!(
        command.get_current_dir(),
        Some(first.root.join("workspace").as_path())
    );
    assert!(
        environment
            .iter()
            .all(|(_, value)| std::path::Path::new(value).starts_with(&first.root))
    );
    assert!(first.finish().await.failures.is_empty());
    assert!(second.finish().await.failures.is_empty());
    let roots = [first.root.clone(), second.root.clone()];
    drop(first);
    drop(second);
    assert!(roots.iter().all(|root| !root.exists()));
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_oversized_wrong_route_and_wrong_header_requests_are_rejected() {
    for (method, path, body, wrong_header) in [
        (
            "POST",
            "/v1/chat/completions",
            Bytes::from_static(b"{bad"),
            false,
        ),
        ("GET", "/v1/chat/completions", Bytes::new(), false),
        (
            "POST",
            "/unexpected",
            Bytes::from(chat(false).to_string()),
            false,
        ),
        (
            "POST",
            "/v1/chat/completions",
            Bytes::from(chat(false).to_string()),
            true,
        ),
    ] {
        let mut expected = step(
            "/v1/chat/completions",
            Some(chat(false)),
            Reply::bytes(StatusCode::OK, "{}"),
        );
        if wrong_header {
            expected
                .headers
                .push(("x-scenario".into(), "required".into()));
        }
        let mut server = fixture(vec![expected]).await;
        let mut client = server.connect(&server.endpoint()).await.unwrap();
        let response = client.request(method, path, body).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        collect(response).await.unwrap();
        assert!(
            server
                .finish()
                .await
                .failures
                .contains(&Failure::UnexpectedRequest { step: 0 })
        );
    }
    let mut server = fixture(vec![step(
        "/v1/chat/completions",
        Some(chat(false)),
        Reply::bytes(StatusCode::OK, "{}"),
    )])
    .await;
    let mut client = server.connect(&server.endpoint()).await.unwrap();
    assert!(
        client
            .request(
                "POST",
                "/v1/chat/completions",
                vec![0; MAX_BYTES + 1].into()
            )
            .await
            .is_err()
    );
    assert_eq!(server.finish().await.requests, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn server_rejects_oversized_bodies_and_malformed_http_independently() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for oversized in [false, true] {
        let mut server = fixture(vec![step(
            "/v1/chat/completions",
            Some(chat(false)),
            Reply::bytes(StatusCode::OK, "{}"),
        )])
        .await;
        let mut socket = server.socket(&server.endpoint()).await.unwrap();
        let request = if oversized {
            format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                MAX_BYTES + 1,
                "x".repeat(MAX_BYTES + 1)
            )
        } else {
            "BROKEN HTTP\r\n\r\n".to_owned()
        };
        timeout(DEADLINE, socket.write_all(request.as_bytes()))
            .await
            .unwrap()
            .unwrap();
        let mut received = Vec::new();
        timeout(
            DEADLINE,
            socket.take(MAX_BYTES as u64).read_to_end(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        server.wait_for_idle().await;
        let report = server.finish().await;
        if oversized {
            assert!(
                report
                    .failures
                    .contains(&Failure::InvalidRequest { step: 0 }),
                "{report:?}"
            );
            assert!(received.starts_with(b"HTTP/1.1 400"));
        } else {
            assert!(report.failures.contains(&Failure::Transport), "{report:?}");
        }
        assert!(
            report
                .failures
                .contains(&Failure::MissingSteps { remaining: 1 })
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_is_not_evidence_of_client_cancellation() {
    let (_release, held) = oneshot::channel();
    let mut reply = Reply::bytes(StatusCode::OK, "partial");
    reply.expect_cancel = true;
    reply.chunks.push(Chunk::Gate(held));
    let mut server = fixture(vec![step("/v1/models", None, reply)]).await;
    let mut client = server.connect(&server.endpoint()).await.unwrap();
    let response = client
        .request("GET", "/v1/models", Bytes::new())
        .await
        .unwrap();
    server.wait_for_chunks(1).await;
    let report = server.finish().await;
    assert_eq!(report.cancelled, 0);
    assert_eq!(report.failures, vec![Failure::IncompleteBody { step: 0 }]);
    drop(response);
}

#[tokio::test(flavor = "current_thread")]
async fn response_gates_do_not_block_other_connections_and_429_retries_fail() {
    let (release, held) = oneshot::channel();
    let mut slow = Reply::bytes(StatusCode::OK, "slow");
    slow.chunks.insert(0, Chunk::Gate(held));
    let mut server = fixture(vec![
        step("/slow", None, slow),
        step(
            "/limited",
            None,
            Reply::bytes(StatusCode::TOO_MANY_REQUESTS, "capacity rejected"),
        ),
    ])
    .await;
    let mut slow_client = server.connect(&server.endpoint()).await.unwrap();
    let slow_response = slow_client
        .request("GET", "/slow", Bytes::new())
        .await
        .unwrap();
    let mut fast_client = server.connect(&server.endpoint()).await.unwrap();
    let rejected = fast_client
        .request("GET", "/limited", Bytes::new())
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    collect(rejected).await.unwrap();
    release.send(()).unwrap();
    assert_eq!(collect(slow_response).await.unwrap(), "slow");
    let mut retry = server.connect(&server.endpoint()).await.unwrap();
    let response = retry
        .request("GET", "/limited", Bytes::new())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    collect(response).await.unwrap();
    assert_eq!(
        server.finish().await.failures,
        vec![Failure::UnexpectedRequest { step: 2 }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_limits_and_unfinished_responses_fail_closed() {
    assert!(
        Fixture::start(Scenario {
            version: 2,
            steps: vec![]
        })
        .await
        .is_err()
    );
    assert!(
        Fixture::start(Scenario {
            version: 1,
            steps: vec![step(
                "/v1/models",
                None,
                Reply::bytes(StatusCode::OK, vec![0; MAX_BYTES + 1])
            )]
        })
        .await
        .is_err()
    );
    let (_release, held) = oneshot::channel();
    let mut reply = Reply::bytes(StatusCode::OK, "partial");
    reply.chunks.push(Chunk::Gate(held));
    let mut server = fixture(vec![step("/v1/models", None, reply)]).await;
    let mut client = server.connect(&server.endpoint()).await.unwrap();
    let response = client
        .request("GET", "/v1/models", Bytes::new())
        .await
        .unwrap();
    server.wait_for_chunks(1).await;
    let report = server.finish().await;
    assert_eq!(report.failures, vec![Failure::IncompleteBody { step: 0 }]);
    drop(response);
}
