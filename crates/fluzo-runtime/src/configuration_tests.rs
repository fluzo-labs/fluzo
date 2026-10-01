use super::*;
use std::fs;
use std::time::{Duration, Instant};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fluzo-config-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn state(&self) -> State {
        State::open(
            &self.0,
            None,
            vec![],
            WritePolicy::CoordinatedLocalWriters,
            99,
        )
        .unwrap()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn set(key: &str, value: SettingValue) -> Edit {
    Edit::Set {
        key: key.into(),
        value,
    }
}
fn keys(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}
fn execute(
    state: &mut State,
    action: ConfigurationAction,
) -> Result<ConfigurationOutcome, ConfigurationError> {
    state.execute(&ConfigurationRequest {
        protocol: CONFIGURATION_PROTOCOL,
        id: ConfigurationRequestId(1),
        action,
    })
}
fn edit(state: &mut State, edits: Vec<Edit>) {
    execute(
        state,
        ConfigurationAction::Edit {
            expected: state.version,
            edits,
        },
    )
    .unwrap();
}

#[test]
fn discovery_distinguishes_missing_valid_invalid_and_inaccessible() {
    let root = Root::new();
    assert_eq!(root.state().discovery, Discovery::Missing);
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n").unwrap();
    assert_eq!(root.state().discovery, Discovery::Valid);
    fs::write(root.0.join(".fluzo"), "broken = [").unwrap();
    let mut state = root.state();
    assert_eq!(state.discovery, Discovery::Invalid);
    assert!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: Version {
                    instance: 99,
                    revision: 1
                },
                keys: vec![]
            }
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.0.join(".fluzo")).unwrap(),
        "broken = ["
    );
    fs::remove_file(root.0.join(".fluzo")).unwrap();
    fs::create_dir(root.0.join(".fluzo")).unwrap();
    assert_eq!(root.state().discovery, Discovery::Inaccessible);
}

#[test]
fn selected_save_apply_cancel_and_cli_are_independent() {
    let root = Root::new();
    let original = "schema_version = 1\n# keep\n[project]\nname = 'original' # retained\n";
    fs::write(root.0.join(".fluzo"), original).unwrap();
    let mut state = State::open(
        &root.0,
        None,
        vec![set("tui.animation_fps", SettingValue::Integer(15))],
        WritePolicy::CoordinatedLocalWriters,
        99,
    )
    .unwrap();
    let effective = state.effective.clone();
    edit(
        &mut state,
        vec![
            set("tui.theme", SettingValue::Text("high-contrast".into())),
            set("tui.animation_fps", SettingValue::Integer(0)),
        ],
    );
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: keys(&["tui.theme"])
            }
        ),
        Ok(ConfigurationOutcome::Saved)
    );
    let saved = fs::read_to_string(root.0.join(".fluzo")).unwrap();
    assert!(saved.contains("# retained") && saved.contains("# keep"));
    assert_eq!(parse_settings(&saved).unwrap().tui.animation_fps, 60);
    assert_eq!(state.effective, effective);
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Apply {
                expected: version,
                keys: keys(&["tui.animation_fps"])
            }
        ),
        Err(ConfigurationError::CliLocked)
    );
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Apply {
                expected: version,
                keys: keys(&["tui.theme"])
            }
        ),
        Ok(ConfigurationOutcome::Applied)
    );
    assert_eq!(state.effective.tui.theme, "high-contrast");
    let version = state.version;
    execute(
        &mut state,
        ConfigurationAction::Cancel { expected: version },
    )
    .unwrap();
    assert_eq!(state.draft.tui.animation_fps, 60);
    assert_eq!(state.effective.tui.animation_fps, 15);
    assert_eq!(fs::read_to_string(root.0.join(".fluzo")).unwrap(), saved);
    let view = state.snapshot();
    assert!(view.values["tui.animation_fps"].cli_locked);
    assert_eq!(
        view.values["tui.theme"].effective_origin,
        SettingOrigin::ActiveSnapshot
    );
    assert_eq!(
        view.values["tui.theme"].saved_origin,
        SettingOrigin::SavedFuture
    );
}

#[test]
fn collection_edits_validate_together_and_selected_save_reloads() {
    let root = Root::new();
    let mut state = root.state();
    edit(
        &mut state,
        vec![
            Edit::AddModel {
                name: "coder".into(),
            },
            set(
                "models.coder.base_url",
                SettingValue::Text("http://127.0.0.1:1/v1".into()),
            ),
            set("models.coder.model", SettingValue::Text("fixture".into())),
            set(
                "models.coder.capacity_id",
                SettingValue::Text("fixture".into()),
            ),
            set("models.coder.pool", SettingValue::Text("local".into())),
            Edit::AddPool {
                name: "local".into(),
            },
            set("agent.model", SettingValue::Text("coder".into())),
        ],
    );
    let version = state.version;
    assert!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: keys(&["models.coder"])
            }
        )
        .is_err()
    );
    assert!(!root.0.join(".fluzo").exists());
    execute(
        &mut state,
        ConfigurationAction::Save {
            expected: version,
            keys: keys(&["models.coder", "capacity_pools.local", "agent.model"]),
        },
    )
    .unwrap();
    let reloaded = root.state();
    assert_eq!(reloaded.saved, state.draft);
    assert_eq!(
        state.snapshot().values["models.coder.base_url"].draft,
        SettingValue::Text("[redacted]".into())
    );
    edit(
        &mut state,
        vec![
            set("agent.model", SettingValue::Unset),
            Edit::RemoveModel {
                name: "coder".into(),
            },
            Edit::RemovePool {
                name: "local".into(),
            },
        ],
    );
    let version = state.version;
    execute(
        &mut state,
        ConfigurationAction::Save {
            expected: version,
            keys: keys(&["agent.model", "models.coder", "capacity_pools.local"]),
        },
    )
    .unwrap();
    assert!(root.state().saved.models.is_empty());
}

#[test]
fn external_create_edit_remove_and_stale_versions_never_clobber() {
    let root = Root::new();
    let mut state = root.state();
    edit(
        &mut state,
        vec![set("tui.animation_fps", SettingValue::Integer(0))],
    );
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n# external\n").unwrap();
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: keys(&["tui.animation_fps"])
            }
        ),
        Err(ConfigurationError::Conflict)
    );
    execute(&mut state, ConfigurationAction::Reload).unwrap();
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Cancel { expected: version }
        ),
        Err(ConfigurationError::Conflict)
    );
    let version = state.version;
    fs::remove_file(root.0.join(".fluzo")).unwrap();
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: vec![]
            }
        ),
        Err(ConfigurationError::Conflict)
    );
    assert!(!root.0.join(".fluzo").exists());
}

#[test]
fn operational_apply_is_unavailable_and_restart_is_only_pending() {
    let root = Root::new();
    let mut state = root.state();
    let active = state.effective.clone();
    let consumed = (123u64, Duration::from_secs(8), 2usize);
    let dispatched_deadline = Duration::from_secs(40);
    edit(
        &mut state,
        vec![set("harness.max_turns", SettingValue::Integer(2))],
    );
    let version = state.version;
    execute(
        &mut state,
        ConfigurationAction::Save {
            expected: version,
            keys: keys(&["harness.max_turns"]),
        },
    )
    .unwrap();
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Apply {
                expected: version,
                keys: keys(&["harness.max_turns"])
            }
        ),
        Err(ConfigurationError::Unavailable)
    );
    assert_eq!(state.effective, active);
    assert_eq!(state.saved.harness.max_turns, 2);
    assert_eq!(consumed, (123, Duration::from_secs(8), 2));
    assert_eq!(dispatched_deadline, Duration::from_secs(40));
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Apply {
                expected: version,
                keys: keys(&["schema_version"])
            }
        ),
        Ok(ConfigurationOutcome::Applied)
    );
    assert!(state.pending_restart.is_empty());
    state.pending_restart = keys(&["agent.runtime"]);
    let version = state.version;
    execute(
        &mut state,
        ConfigurationAction::Apply {
            expected: version,
            keys: keys(&["tui.theme"]),
        },
    )
    .unwrap();
    assert_eq!(state.pending_restart, keys(&["agent.runtime"]));
}

fn ready(service: &mut ConfigurationService) -> ConfigurationSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.snapshot() {
            Ok(snapshot) => return snapshot,
            Err(ConfigurationError::Busy) => {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            other => panic!("{other:?}"),
        }
    }
}
fn completed(
    service: &mut ConfigurationService,
    id: ConfigurationRequestId,
) -> ConfigurationRequestStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = service.status(id).unwrap();
        if status != ConfigurationRequestStatus::Accepted {
            return status;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn worker_port_bounds_replay_and_quiesce_preserve_completion() {
    let root = Root::new();
    let mut service = ConfigurationService::start(
        root.0.clone(),
        None,
        vec![],
        WritePolicy::CoordinatedLocalWriters,
    )
    .unwrap();
    let snapshot = ready(&mut service);
    let request = ConfigurationRequest {
        protocol: CONFIGURATION_PROTOCOL,
        id: ConfigurationRequestId(1),
        action: ConfigurationAction::Save {
            expected: snapshot.version,
            keys: vec![],
        },
    };
    let wire = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<ConfigurationRequest>(&wire).unwrap(),
        request
    );
    assert_eq!(service.submit(request.clone()), Ok(request.id));
    assert!(matches!(
        completed(&mut service, request.id),
        ConfigurationRequestStatus::Completed {
            outcome: ConfigurationOutcome::Saved,
            ..
        }
    ));
    let saved = fs::read_to_string(root.0.join(".fluzo")).unwrap();
    assert_eq!(service.submit(request.clone()), Ok(request.id));
    let mut different = request.clone();
    different.action = ConfigurationAction::Reload;
    assert_eq!(
        service.submit(different),
        Err(ConfigurationError::InvalidRequest)
    );
    assert_eq!(fs::read_to_string(root.0.join(".fluzo")).unwrap(), saved);
    for sequence in 2..=MAX_REQUESTS {
        let request = ConfigurationRequest {
            protocol: CONFIGURATION_PROTOCOL,
            id: ConfigurationRequestId(sequence as u64),
            action: ConfigurationAction::Reload,
        };
        service.submit(request.clone()).unwrap();
        completed(&mut service, request.id);
    }
    assert_eq!(
        service.submit(ConfigurationRequest {
            protocol: CONFIGURATION_PROTOCOL,
            id: ConfigurationRequestId(100),
            action: ConfigurationAction::Reload
        }),
        Err(ConfigurationError::Capacity)
    );
    service.quiesce();
    assert_eq!(service.submit(request.clone()), Ok(request.id));
    assert_eq!(
        service.status(ConfigurationRequestId(999)).unwrap(),
        ConfigurationRequestStatus::Unknown
    );
    let snapshot = ready(&mut service);
    assert_eq!(
        serde_json::from_str::<ConfigurationSnapshot>(&serde_json::to_string(&snapshot).unwrap())
            .unwrap(),
        snapshot
    );
}

#[test]
fn file_failures_preserve_or_reconcile_without_claiming_rollback() {
    use crate::configuration_file::{FAILURE, Fault};
    for fault in [
        Fault::FileSync,
        Fault::BeforeReplace,
        Fault::AfterReplace,
        Fault::DirectorySync,
    ] {
        for existing in [false, true] {
            let root = Root::new();
            if existing {
                fs::write(root.0.join(".fluzo"), "schema_version = 1\n# retained\n").unwrap();
            }
            let before = fs::read(root.0.join(".fluzo")).ok();
            let mut state = root.state();
            let active = state.effective.clone();
            edit(
                &mut state,
                vec![set("tui.animation_fps", SettingValue::Integer(0))],
            );
            let version = state.version;
            FAILURE.with(|value| value.set(Some(fault)));
            let result = execute(
                &mut state,
                ConfigurationAction::Save {
                    expected: version,
                    keys: keys(&["tui.animation_fps"]),
                },
            );
            FAILURE.with(|value| value.set(None));
            let uncertain = matches!(fault, Fault::AfterReplace | Fault::DirectorySync);
            assert_eq!(
                result,
                Err(if uncertain {
                    ConfigurationError::Uncertain
                } else {
                    ConfigurationError::WriteFailed
                })
            );
            assert_eq!(state.effective, active);
            assert_eq!(state.saved.tui.animation_fps, 60);
            assert_eq!(state.draft.tui.animation_fps, 0);
            if uncertain {
                assert_eq!(root.state().saved.tui.animation_fps, 0);
                let version = state.version;
                assert_eq!(
                    execute(
                        &mut state,
                        ConfigurationAction::Save {
                            expected: version,
                            keys: vec![]
                        }
                    ),
                    Err(ConfigurationError::Uncertain)
                );
                execute(&mut state, ConfigurationAction::Reload).unwrap();
                assert_eq!(state.saved.tui.animation_fps, 0);
                assert_eq!(state.effective, active);
            } else {
                assert_eq!(fs::read(root.0.join(".fluzo")).ok(), before);
            }
            assert_eq!(
                fs::read_dir(&root.0).unwrap().count(),
                usize::from(existing || uncertain)
            );
        }
    }
}

#[test]
fn file_lock_symlink_hardlink_and_replacement_checks_fail_closed() {
    use std::os::unix::fs::symlink;
    let root = Root::new();
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n").unwrap();
    let mut state = root.state();
    let lock = fs::File::open(&root.0).unwrap();
    lock.try_lock().unwrap();
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: vec![]
            }
        ),
        Err(ConfigurationError::Busy)
    );
    lock.unlock().unwrap();
    fs::write(root.0.join("replacement"), "schema_version = 1\n").unwrap();
    fs::rename(root.0.join("replacement"), root.0.join(".fluzo")).unwrap();
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: vec![]
            }
        ),
        Err(ConfigurationError::Conflict)
    );
    fs::hard_link(root.0.join(".fluzo"), root.0.join("alias")).unwrap();
    assert_eq!(root.state().discovery, Discovery::Inaccessible);
    fs::remove_file(root.0.join("alias")).unwrap();
    fs::remove_file(root.0.join(".fluzo")).unwrap();
    fs::write(root.0.join("target"), "schema_version = 1\n").unwrap();
    symlink("target", root.0.join(".fluzo")).unwrap();
    assert_eq!(root.state().discovery, Discovery::Inaccessible);
    symlink(&root.0, root.0.join("directory-link")).unwrap();
    assert!(
        State::open(
            &root.0,
            Some(Path::new("directory-link/target")),
            vec![],
            WritePolicy::ReadOnly,
            2
        )
        .is_err()
    );
}

#[test]
fn rejected_batch_is_atomic_and_projections_are_sanitized_bounded() {
    let root = Root::new();
    let mut state = root.state();
    let before = state.snapshot();
    let version = state.version;
    let edits = vec![
        set("tui.theme", SettingValue::Text("high-contrast".into())),
        set("tui.notifications.max_visible", SettingValue::Integer(6)),
    ];
    assert!(
        execute(
            &mut state,
            ConfigurationAction::Edit {
                expected: version,
                edits
            }
        )
        .is_err()
    );
    assert_eq!(state.snapshot(), before);
    edit(
        &mut state,
        vec![set(
            "project.name",
            SettingValue::Text("safe\u{1b}[2J\u{202e}".into()),
        )],
    );
    let wire = serde_json::to_string(&state.snapshot()).unwrap();
    assert!(!wire.contains("\\u001b") && !wire.contains('\u{202e}'));
    for sequence in 1..=100 {
        state.record(
            ConfigurationRequestId(sequence),
            ConfigurationOutcome::DraftUpdated,
        );
    }
    assert_eq!(state.snapshot().changes.len(), MAX_CHANGES);
    assert_eq!(state.snapshot().dropped_changes, 36);
    assert!(
        validate_edits(&vec![
            set("tui.animation_fps", SettingValue::Integer(0));
            MAX_EDITS + 1
        ])
        .is_err()
    );
    assert!(
        validate_edits(&[set(
            "project.name",
            SettingValue::Text("x".repeat(MAX_REQUEST_BYTES + 1))
        )])
        .is_err()
    );
}

#[test]
fn queue_saturation_is_nonblocking_and_acknowledgement_is_not_completion() {
    let (sender, requests) = mpsc::sync_channel(MAX_PENDING);
    let (_results, receiver) = mpsc::sync_channel(MAX_PENDING + 1);
    let mut service = ConfigurationService {
        sender: Some(sender),
        receiver,
        records: vec![],
        projection: None,
        unavailable: false,
    };
    for sequence in 1..=MAX_PENDING {
        let id = ConfigurationRequestId(sequence as u64);
        service
            .submit(ConfigurationRequest {
                protocol: CONFIGURATION_PROTOCOL,
                id,
                action: ConfigurationAction::Reload,
            })
            .unwrap();
        assert_eq!(
            service.status(id).unwrap(),
            ConfigurationRequestStatus::Accepted
        );
    }
    assert_eq!(
        service.submit(ConfigurationRequest {
            protocol: CONFIGURATION_PROTOCOL,
            id: ConfigurationRequestId(100),
            action: ConfigurationAction::Reload
        }),
        Err(ConfigurationError::Busy)
    );
    assert_eq!(requests.try_iter().count(), MAX_PENDING);
}

#[test]
fn lost_acknowledgement_and_worker_disconnect_never_replay_effects() {
    let root = Root::new();
    let mut service = ConfigurationService::start(
        root.0.clone(),
        None,
        vec![],
        WritePolicy::CoordinatedLocalWriters,
    )
    .unwrap();
    let snapshot = ready(&mut service);
    let request = ConfigurationRequest {
        protocol: CONFIGURATION_PROTOCOL,
        id: ConfigurationRequestId(20),
        action: ConfigurationAction::Save {
            expected: snapshot.version,
            keys: vec![],
        },
    };
    service.submit(request.clone()).unwrap();
    assert_eq!(service.submit(request.clone()), Ok(request.id));
    assert!(matches!(
        completed(&mut service, request.id),
        ConfigurationRequestStatus::Completed {
            outcome: ConfigurationOutcome::Saved,
            ..
        }
    ));
    assert_eq!(
        service
            .snapshot()
            .unwrap()
            .changes
            .iter()
            .filter(|change| change.request == request.id)
            .count(),
        1
    );
    let (sender, _requests) = mpsc::sync_channel(MAX_PENDING);
    let (results, receiver) = mpsc::sync_channel(MAX_PENDING + 1);
    let mut disconnected = ConfigurationService {
        sender: Some(sender),
        receiver,
        records: vec![(request.clone(), ConfigurationRequestStatus::Accepted)],
        projection: None,
        unavailable: false,
    };
    drop(results);
    assert_eq!(
        disconnected.status(request.id).unwrap(),
        ConfigurationRequestStatus::Failed(ConfigurationError::Uncertain)
    );
    assert_eq!(disconnected.submit(request.clone()), Ok(request.id));
    assert_eq!(
        disconnected.status(request.id).unwrap(),
        ConfigurationRequestStatus::Failed(ConfigurationError::Uncertain)
    );
}

#[test]
fn reload_keeps_effective_provenance_and_invalid_input_keeps_draft() {
    let root = Root::new();
    let mut state = root.state();
    edit(
        &mut state,
        vec![set("tui.theme", SettingValue::Text("high-contrast".into()))],
    );
    fs::write(
        root.0.join(".fluzo"),
        "schema_version = 1\n[tui]\nanimation_fps = 30\n",
    )
    .unwrap();
    execute(&mut state, ConfigurationAction::Reload).unwrap();
    assert_eq!(state.effective.tui.animation_fps, 60);
    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.values["tui.animation_fps"].saved_origin,
        SettingOrigin::Repository
    );
    assert_eq!(
        snapshot.values["tui.animation_fps"].effective_origin,
        SettingOrigin::BuiltIn
    );
    edit(
        &mut state,
        vec![set("tui.theme", SettingValue::Text("high-contrast".into()))],
    );
    let draft = state.draft.clone();
    fs::write(root.0.join(".fluzo"), "invalid = [").unwrap();
    execute(&mut state, ConfigurationAction::Reload).unwrap();
    assert_eq!(state.draft, draft);
    assert_eq!(state.discovery, Discovery::Invalid);
}

#[test]
fn review_regression_apply_does_not_expand_valid_sparse_configuration() {
    let root = Root::new();
    let mut source = String::from("schema_version = 1\n");
    for index in 0..9000 {
        source.push_str(&format!("[capacity_pools.pool{index}]\n"));
    }
    let settings = parse_settings(&source).unwrap();
    assert!(source.len() < crate::config::MAX_CONFIG_BYTES);
    assert!(encode_settings(&settings).is_err());
    fs::write(root.0.join(".fluzo"), &source).unwrap();
    let mut state = root.state();
    edit(
        &mut state,
        vec![set("tui.animation_fps", SettingValue::Integer(0))],
    );
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Apply {
                expected: version,
                keys: keys(&["tui.animation_fps"])
            }
        ),
        Ok(ConfigurationOutcome::Applied)
    );
    assert_eq!(state.effective.tui.animation_fps, 0);
    assert_eq!(state.effective.capacity_pools, settings.capacity_pools);
    assert_eq!(fs::read_to_string(root.0.join(".fluzo")).unwrap(), source);
}

#[test]
fn review_regression_failed_reload_preserves_saved_and_draft_origins() {
    for explicit in [false, true] {
        let root = Root::new();
        fs::write(
            root.0.join(".fluzo"),
            "schema_version = 1\n[tui]\nanimation_fps = 30\n",
        )
        .unwrap();
        let mut state = State::open(
            &root.0,
            explicit.then_some(Path::new(".fluzo")),
            vec![],
            WritePolicy::ReadOnly,
            12,
        )
        .unwrap();
        edit(
            &mut state,
            vec![set("tui.theme", SettingValue::Text("high-contrast".into()))],
        );
        let before = state.snapshot().values;
        for source in ["schema_version = 1\nunknown = true\n", "broken = ["] {
            fs::write(root.0.join(".fluzo"), source).unwrap();
            execute(&mut state, ConfigurationAction::Reload).unwrap();
            assert_eq!(state.snapshot().values, before);
            assert_eq!(state.discovery, Discovery::Invalid);
        }
        fs::remove_file(root.0.join(".fluzo")).unwrap();
        fs::create_dir(root.0.join(".fluzo")).unwrap();
        execute(&mut state, ConfigurationAction::Reload).unwrap();
        assert_eq!(state.snapshot().values, before);
        assert_eq!(state.discovery, Discovery::Inaccessible);
    }
}

#[test]
fn review_regression_diagnostics_keep_safe_code_and_location() {
    for (source, code, key) in [
        ("schema_version = 1\nbroken = [", "Syntax", ""),
        (
            "[tui]\nanimation_fps = 30\n",
            "MissingVersion",
            "schema_version",
        ),
        (
            "schema_version = 1\n[tui]\nanimation_fps = 'synthetic-private-value'\n",
            "InvalidType",
            "tui.animation_fps",
        ),
    ] {
        let original = parse_settings(source).unwrap_err();
        let error = invalid(original.clone());
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["Invalid"]["code"], code);
        assert_eq!(json["Invalid"]["key"], key);
        assert_eq!(
            json["Invalid"]["span"],
            serde_json::to_value(original.span).unwrap()
        );
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("synthetic-private-value")
        );
        assert_eq!(
            serde_json::from_value::<ConfigurationError>(json).unwrap(),
            error
        );
    }
    let error =
        invalid(parse_settings(&" ".repeat(crate::config::MAX_CONFIG_BYTES + 1)).unwrap_err());
    assert_eq!(
        serde_json::to_value(error).unwrap()["Invalid"]["code"],
        "TooLarge"
    );
}

#[test]
fn review_regression_public_port_retains_diagnostics_and_rejects_old_protocol() {
    let root = Root::new();
    let source = "schema_version = 1\nbroken = [";
    fs::write(root.0.join(".fluzo"), source).unwrap();
    let mut service =
        ConfigurationService::start(root.0.clone(), None, vec![], WritePolicy::ReadOnly).unwrap();
    let snapshot = ready(&mut service);
    assert_eq!(
        snapshot.problem,
        Some(invalid(parse_settings(source).unwrap_err()))
    );
    assert_eq!(
        service.submit(ConfigurationRequest {
            protocol: 1,
            id: ConfigurationRequestId(1),
            action: ConfigurationAction::Reload,
        }),
        Err(ConfigurationError::UnsupportedProtocol)
    );
    fs::write(root.0.join(".fluzo"), [0xff, 0xfe]).unwrap();
    let id = ConfigurationRequestId(2);
    service
        .submit(ConfigurationRequest {
            protocol: CONFIGURATION_PROTOCOL,
            id,
            action: ConfigurationAction::Reload,
        })
        .unwrap();
    assert!(matches!(
        completed(&mut service, id),
        ConfigurationRequestStatus::Completed { .. }
    ));
    assert!(matches!(
        service.snapshot().unwrap().problem,
        Some(ConfigurationError::Invalid {
            code: ConfigErrorCode::InvalidEncoding,
            span: None,
            ..
        })
    ));
}

#[test]
fn process_lock_helper() {
    use std::io::{Read, Write};
    let Some(path) = std::env::var_os("FLUZO_CONFIG_LOCK_FIXTURE") else {
        return;
    };
    let directory = fs::File::open(path).unwrap();
    directory.try_lock().unwrap();
    std::io::stdout().write_all(b"LOCKED\n").unwrap();
    std::io::stdout().flush().unwrap();
    let mut release = [0];
    std::io::stdin().read_exact(&mut release).unwrap();
    directory.unlock().unwrap();
}

#[test]
fn cooperating_processes_cannot_write_while_directory_is_locked() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let root = Root::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "configuration::tests::process_lock_helper",
            "--nocapture",
        ])
        .env_clear()
        .env("FLUZO_CONFIG_LOCK_FIXTURE", &root.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            if line.contains("LOCKED") {
                let _ = ready_sender.send(());
            }
            line.clear();
        }
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ready_receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut state = root.state();
        let version = state.version;
        assert_eq!(
            execute(
                &mut state,
                ConfigurationAction::Save {
                    expected: version,
                    keys: vec![]
                }
            ),
            Err(ConfigurationError::Busy)
        );
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            execute(
                &mut state,
                ConfigurationAction::Save {
                    expected: version,
                    keys: vec![]
                }
            ),
            Ok(ConfigurationOutcome::Saved)
        );
    }));
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    child.wait().unwrap();
    reader.join().unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[test]
fn read_only_and_explicit_selection_are_enforced() {
    let root = Root::new();
    fs::write(
        root.0.join("selected.toml"),
        "schema_version = 1\n[tui]\nanimation_fps = 30\n",
    )
    .unwrap();
    let mut state = State::open(
        &root.0,
        Some(Path::new("selected.toml")),
        vec![],
        WritePolicy::ReadOnly,
        9,
    )
    .unwrap();
    assert_eq!(state.effective.tui.animation_fps, 30);
    assert_eq!(
        state.snapshot().values["tui.animation_fps"].saved_origin,
        SettingOrigin::ExplicitFile
    );
    let version = state.version;
    assert_eq!(
        execute(
            &mut state,
            ConfigurationAction::Save {
                expected: version,
                keys: vec![]
            }
        ),
        Err(ConfigurationError::ReadOnly)
    );
    assert!(
        State::open(
            &root.0,
            Some(Path::new("../outside")),
            vec![],
            WritePolicy::ReadOnly,
            10
        )
        .is_err()
    );
    assert!(!root.0.join(".fluzo").exists());
}
