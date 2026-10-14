use fluzo_core::{
    configuration::*,
    settings::{SettingOrigin, SettingValue, Settings},
};
use fluzo_runtime::{
    config::{encode_settings, parse_settings},
    configuration::{ConfigurationService, WritePolicy},
};
use fluzo_tui::{
    configuration::ConfigurationView,
    input::{KeyCode, KeyEvent, KeyModifiers},
    workspace::Workspace,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fluzo-s3-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn service(&self, policy: WritePolicy) -> ConfigurationService {
        ConfigurationService::start(self.0.clone(), None, vec![], policy).unwrap()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn settle(view: &mut ConfigurationView, service: &mut ConfigurationService) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        view.poll(service);
        if view.snapshot.is_some() && !view.is_pending() {
            break;
        }
        assert!(Instant::now() < deadline, "configuration did not settle");
        std::thread::yield_now();
    }
    view.poll(service);
}
fn view(service: &mut ConfigurationService) -> ConfigurationView {
    let mut view = ConfigurationView::default();
    settle(&mut view, service);
    view.show("");
    view
}
fn set(view: &mut ConfigurationView, key: &str, value: SettingValue) {
    view.stage(Edit::Set {
        key: key.into(),
        value,
    })
    .unwrap();
}

#[test]
fn review_regression_removed_collection_does_not_poison_later_saves() {
    let root = Root::new();
    fs::write(
        root.0.join(".fluzo"),
        "schema_version = 1\n[capacity_pools.old]\n",
    )
    .unwrap();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut view = view(&mut service);
    view.stage(Edit::RemovePool { name: "old".into() }).unwrap();
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Saved"));
    set(
        &mut view,
        "tui.theme",
        SettingValue::Text("high-contrast".into()),
    );
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Saved"), "{}", view.status);
    let saved = parse_settings(&fs::read_to_string(root.0.join(".fluzo")).unwrap()).unwrap();
    assert!(saved.capacity_pools.is_empty());
    assert_eq!(saved.tui.theme, "high-contrast");
    view.apply(&mut service);
    settle(&mut view, &mut service);
    assert_eq!(
        view.snapshot.as_ref().unwrap().effective_ui.theme,
        "high-contrast"
    );
}

#[test]
fn review_regression_operational_save_does_not_poison_presentation_apply() {
    let root = Root::new();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut view = view(&mut service);
    set(&mut view, "harness.max_turns", SettingValue::Integer(45));
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Saved"));
    set(
        &mut view,
        "tui.theme",
        SettingValue::Text("high-contrast".into()),
    );
    view.apply(&mut service);
    settle(&mut view, &mut service);
    assert_eq!(
        view.snapshot.as_ref().unwrap().effective_ui.theme,
        "high-contrast",
        "{}",
        view.status
    );
    assert_eq!(
        view.snapshot.as_ref().unwrap().values["harness.max_turns"].effective,
        SettingValue::Integer(30)
    );
    view.save(&mut service);
    settle(&mut view, &mut service);
    let saved = parse_settings(&fs::read_to_string(root.0.join(".fluzo")).unwrap()).unwrap();
    assert_eq!(saved.harness.max_turns, 45);
    assert_eq!(saved.tui.theme, "high-contrast");
}

#[test]
fn normal_view_real_save_apply_preserves_redacted_values_and_comments() {
    let root = Root::new();
    let original = "schema_version = 1\n# retain this comment\n[project]\nname = 'private-fixture'\n[tools.shell.env]\nORIGINAL = 'unchanged'\n";
    fs::write(root.0.join(".fluzo"), original).unwrap();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut view = view(&mut service);
    set(
        &mut view,
        "tui.theme",
        SettingValue::Text("high-contrast".into()),
    );
    view.save(&mut service);
    settle(&mut view, &mut service);
    let saved = fs::read_to_string(root.0.join(".fluzo")).unwrap();
    assert!(saved.contains("# retain this comment"));
    let parsed = parse_settings(&saved).unwrap();
    assert_eq!(parsed.tools.shell.env["ORIGINAL"], "unchanged");
    assert_eq!(parsed.project.name, "private-fixture");
    assert_eq!(
        view.snapshot.as_ref().unwrap().effective_ui.theme,
        "default"
    );
    view.apply(&mut service);
    settle(&mut view, &mut service);
    assert_eq!(
        view.snapshot.as_ref().unwrap().effective_ui.theme,
        "high-contrast"
    );
    assert_eq!(fs::read_to_string(root.0.join(".fluzo")).unwrap(), saved);
}

#[test]
fn create_only_policy_refuses_replacement_but_applies_presentation() {
    let root = Root::new();
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n").unwrap();
    let mut service = root.service(WritePolicy::CreateOnly);
    let mut view = view(&mut service);
    assert_eq!(
        view.snapshot.as_ref().unwrap().save_unavailable,
        Some(ConfigurationError::ReadOnly)
    );
    set(
        &mut view,
        "tui.notifications.duration_seconds",
        SettingValue::Integer(12),
    );
    view.save(&mut service);
    assert!(!view.is_pending());
    view.apply(&mut service);
    settle(&mut view, &mut service);
    assert_eq!(
        view.snapshot
            .as_ref()
            .unwrap()
            .effective_ui
            .notifications
            .duration_seconds,
        12
    );
    assert_eq!(
        fs::read_to_string(root.0.join(".fluzo")).unwrap(),
        "schema_version = 1\n"
    );
}

#[test]
fn collection_batch_and_all_value_types_round_trip_without_partial_save() {
    let root = Root::new();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut view = view(&mut service);
    view.stage(Edit::AddPool {
        name: "local".into(),
    })
    .unwrap();
    view.stage(Edit::AddModel {
        name: "coder".into(),
    })
    .unwrap();
    set(
        &mut view,
        "models.coder.base_url",
        SettingValue::Text("http://127.0.0.1:1/v1".into()),
    );
    set(
        &mut view,
        "models.coder.model",
        SettingValue::Text("fixture".into()),
    );
    set(
        &mut view,
        "models.coder.capacity_id",
        SettingValue::Text("fixture".into()),
    );
    set(
        &mut view,
        "models.coder.pool",
        SettingValue::Text("local".into()),
    );
    set(&mut view, "agent.model", SettingValue::Text("coder".into()));
    set(
        &mut view,
        "context.compaction_threshold",
        SettingValue::Fraction(0.7),
    );
    set(
        &mut view,
        "tools.shell.inherit_env",
        SettingValue::TextList(vec!["LANG".into()]),
    );
    set(
        &mut view,
        "tools.shell.env",
        SettingValue::TextMap([("RUST_BACKTRACE".into(), "full".into())].into()),
    );
    set(&mut view, "tui.reduced_motion", SettingValue::Boolean(true));
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Saved"), "{}", view.status);
    let saved = parse_settings(&fs::read_to_string(root.0.join(".fluzo")).unwrap()).unwrap();
    assert_eq!(saved.agent.model.as_deref(), Some("coder"));
    assert_eq!(saved.context.compaction_threshold, 0.7);
    assert_eq!(saved.tools.shell.inherit_env, ["LANG"]);
    assert_eq!(saved.tools.shell.env["RUST_BACKTRACE"], "full");
    view.stage(Edit::RemovePool {
        name: "local".into(),
    })
    .unwrap();
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Validation"));
    assert_eq!(
        parse_settings(&fs::read_to_string(root.0.join(".fluzo")).unwrap()).unwrap(),
        saved
    );
}

#[test]
fn conflict_and_operational_apply_do_not_change_effective_or_replay() {
    let root = Root::new();
    fs::write(
        root.0.join(".fluzo"),
        encode_settings(&Settings::default()).unwrap(),
    )
    .unwrap();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut view = view(&mut service);
    set(&mut view, "harness.max_turns", SettingValue::Integer(45));
    view.apply(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Unavailable"));
    assert_eq!(
        view.snapshot.as_ref().unwrap().values["harness.max_turns"].effective,
        SettingValue::Integer(30)
    );
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n# concurrent\n").unwrap();
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("Conflict"));
    assert_eq!(
        fs::read_to_string(root.0.join(".fluzo")).unwrap(),
        "schema_version = 1\n# concurrent\n"
    );
}

#[test]
fn editor_reconciles_real_write_failure_without_partial_file_changes() {
    let root = Root::new();
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n").unwrap();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut view = view(&mut service);
    let collision = root.0.join(format!(".fluzo-save-{}-1", std::process::id()));
    fs::write(&collision, "independent collision").unwrap();
    set(
        &mut view,
        "tui.theme",
        SettingValue::Text("high-contrast".into()),
    );
    view.save(&mut service);
    settle(&mut view, &mut service);
    assert!(view.status.contains("WriteFailed"), "{}", view.status);
    assert_eq!(
        fs::read_to_string(root.0.join(".fluzo")).unwrap(),
        "schema_version = 1\n"
    );
    assert_eq!(
        fs::read_to_string(collision).unwrap(),
        "independent collision"
    );
}

#[test]
fn request_exhaustion_is_projected_without_evicting_results() {
    let root = Root::new();
    let mut service = root.service(WritePolicy::CreateOnly);
    let mut view = view(&mut service);
    for index in 0..MAX_REQUESTS {
        set(
            &mut view,
            "tui.animation_fps",
            SettingValue::Integer((index % 61) as u64),
        );
        view.validate(&mut service);
        settle(&mut view, &mut service);
    }
    assert_eq!(view.snapshot.as_ref().unwrap().remaining_requests, 0);
    set(&mut view, "tui.animation_fps", SettingValue::Integer(15));
    view.save(&mut service);
    assert!(!view.is_pending());
    assert!(!root.0.join(".fluzo").exists());
    assert!(matches!(
        service.status(ConfigurationRequestId(1)).unwrap(),
        ConfigurationRequestStatus::Completed { .. }
    ));
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn open_developer_presentation_settings(
    workspace: &mut Workspace,
    service: &mut ConfigurationService,
) {
    workspace.key(key(KeyCode::Char('p'), KeyModifiers::CONTROL), service);
    for character in "Developer menu".chars() {
        workspace.key(key(KeyCode::Char(character), KeyModifiers::NONE), service);
    }
    workspace.key(key(KeyCode::Enter, KeyModifiers::NONE), service);
    workspace.key(key(KeyCode::Enter, KeyModifiers::NONE), service);
    assert!(workspace.configuration.open);
    assert_eq!(workspace.configuration.filter, "tui.");
}

#[test]
fn developer_menu_route_saves_and_applies_only_selected_ui_preferences() {
    let root = Root::new();
    fs::write(
        root.0.join(".fluzo"),
        "schema_version = 1\n# keep this comment\n[project]\nname = 'fixture' # retained\n[tools.shell.env]\nORIGINAL = 'unchanged'\n",
    )
    .unwrap();
    let mut service = ConfigurationService::start(
        root.0.clone(),
        None,
        vec![Edit::Set {
            key: "tui.animation_fps".into(),
            value: SettingValue::Integer(0),
        }],
        WritePolicy::LocalWorkspace,
    )
    .unwrap();
    let mut workspace = Workspace::new(root.0.to_str().unwrap(), false).unwrap();
    settle(&mut workspace.configuration, &mut service);
    open_developer_presentation_settings(&mut workspace, &mut service);

    let before = workspace.configuration.snapshot.clone().unwrap();
    assert_eq!(before.effective_ui.theme, "default");

    set(
        &mut workspace.configuration,
        "tui.theme",
        SettingValue::Text("high-contrast".into()),
    );
    set(
        &mut workspace.configuration,
        "tui.notifications.duration_seconds",
        SettingValue::Integer(9),
    );
    set(
        &mut workspace.configuration,
        "tui.animation_fps",
        SettingValue::Integer(15),
    );
    workspace.key(key(KeyCode::Char('s'), KeyModifiers::CONTROL), &mut service);
    settle(&mut workspace.configuration, &mut service);
    assert!(
        workspace.configuration.status.contains("Saved"),
        "{}",
        workspace.configuration.status
    );

    let saved = fs::read_to_string(root.0.join(".fluzo")).unwrap();
    assert!(saved.contains("# keep this comment") && saved.contains("# retained"));
    let parsed = parse_settings(&saved).unwrap();
    assert_eq!(parsed.tui.theme, "high-contrast");
    assert_eq!(parsed.tui.notifications.duration_seconds, 9);
    assert_eq!(parsed.tui.animation_fps, 15);
    assert_eq!(parsed.project.name, "fixture");
    let lowered = saved.to_lowercase();
    for forbidden in ["synthetic", "preview", "scenario", "playback"] {
        assert!(
            !lowered.contains(forbidden),
            "persisted {forbidden} content"
        );
    }

    let snap = workspace.configuration.snapshot.clone().unwrap();
    assert_eq!(snap.effective_ui.theme, "default");
    assert_eq!(
        snap.values["tui.theme"].effective,
        SettingValue::Text("default".into())
    );
    assert_eq!(
        snap.values["tui.theme"].saved,
        SettingValue::Text("high-contrast".into())
    );
    assert_eq!(
        snap.values["tui.theme"].saved_origin,
        SettingOrigin::SavedFuture
    );
    assert!(snap.values["tui.animation_fps"].cli_locked);
    assert_eq!(
        snap.values["tui.animation_fps"].effective,
        SettingValue::Integer(0)
    );

    workspace.key(key(KeyCode::Char('a'), KeyModifiers::CONTROL), &mut service);
    settle(&mut workspace.configuration, &mut service);
    let snap = workspace.configuration.snapshot.clone().unwrap();
    assert_eq!(snap.effective_ui.theme, "high-contrast");
    assert_eq!(
        snap.values["tui.notifications.duration_seconds"].effective,
        SettingValue::Integer(9)
    );
    assert_eq!(
        snap.values["tui.animation_fps"].effective,
        SettingValue::Integer(0)
    );

    workspace.poll(&mut service);
    assert_eq!(workspace.shell.preferences.applied().theme, "high-contrast");
    assert_eq!(workspace.shell.preferences.effective().animation_fps, 0);

    let outcomes: Vec<ConfigurationOutcome> = snap
        .changes
        .iter()
        .map(|change| change.outcome.clone())
        .collect();
    assert!(
        outcomes.contains(&ConfigurationOutcome::Saved),
        "{outcomes:?}"
    );
    assert_eq!(*outcomes.last().unwrap(), ConfigurationOutcome::Applied);
}

#[test]
fn developer_menu_route_cancel_and_close_never_undo_a_completed_save() {
    let root = Root::new();
    fs::write(root.0.join(".fluzo"), "schema_version = 1\n# keep\n").unwrap();
    let mut service = root.service(WritePolicy::LocalWorkspace);
    let mut workspace = Workspace::new(root.0.to_str().unwrap(), false).unwrap();
    settle(&mut workspace.configuration, &mut service);
    open_developer_presentation_settings(&mut workspace, &mut service);

    set(
        &mut workspace.configuration,
        "tui.theme",
        SettingValue::Text("high-contrast".into()),
    );
    workspace.key(key(KeyCode::Char('s'), KeyModifiers::CONTROL), &mut service);
    settle(&mut workspace.configuration, &mut service);
    assert!(workspace.configuration.status.contains("Saved"));
    let saved = fs::read_to_string(root.0.join(".fluzo")).unwrap();
    assert!(saved.contains("high-contrast"));

    set(
        &mut workspace.configuration,
        "tui.theme",
        SettingValue::Text("default".into()),
    );
    workspace.key(key(KeyCode::Char('x'), KeyModifiers::CONTROL), &mut service);
    workspace.key(key(KeyCode::Char('x'), KeyModifiers::CONTROL), &mut service);
    settle(&mut workspace.configuration, &mut service);
    let snap = workspace.configuration.snapshot.clone().unwrap();
    assert_eq!(
        snap.values["tui.theme"].draft,
        snap.values["tui.theme"].saved
    );
    assert_eq!(fs::read_to_string(root.0.join(".fluzo")).unwrap(), saved);

    workspace.key(key(KeyCode::Esc, KeyModifiers::NONE), &mut service);
    assert!(!workspace.configuration.open);
    assert_eq!(fs::read_to_string(root.0.join(".fluzo")).unwrap(), saved);
}
