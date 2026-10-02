use super::*;
use ratatui::{Terminal, backend::TestBackend};

struct Port {
    snapshot: ConfigurationSnapshot,
    requests: Vec<ConfigurationRequest>,
    outcome: ConfigurationRequestStatus,
}
impl Port {
    fn new() -> Self {
        let settings = Settings::default();
        Self {
            snapshot: ConfigurationSnapshot {
                version: Version {
                    instance: 1,
                    revision: 1,
                },
                discovery: Discovery::Valid,
                problem: None,
                values: settings
                    .entries()
                    .into_iter()
                    .map(|(descriptor, value)| {
                        (
                            descriptor.key.clone(),
                            ConfigurationValue {
                                descriptor,
                                saved: value.clone(),
                                draft: value.clone(),
                                effective: value,
                                saved_origin: fluzo_core::settings::SettingOrigin::BuiltIn,
                                draft_origin: fluzo_core::settings::SettingOrigin::BuiltIn,
                                effective_origin: fluzo_core::settings::SettingOrigin::BuiltIn,
                                cli_locked: false,
                            },
                        )
                    })
                    .collect(),
                effective_ui: settings.tui,
                pending_restart: vec![],
                changes: vec![],
                dropped_changes: 0,
                setup: None,
                backup: None,
                save_unavailable: None,
                apply_unavailable: None,
                remaining_requests: MAX_REQUESTS,
                next_request_id: ConfigurationRequestId(1),
            },
            requests: vec![],
            outcome: ConfigurationRequestStatus::Accepted,
        }
    }
}
impl ConfigurationPort for Port {
    fn snapshot(&mut self) -> Result<ConfigurationSnapshot, ConfigurationError> {
        Ok(self.snapshot.clone())
    }
    fn status(
        &mut self,
        _: ConfigurationRequestId,
    ) -> Result<ConfigurationRequestStatus, ConfigurationError> {
        Ok(self.outcome.clone())
    }
    fn submit(
        &mut self,
        request: ConfigurationRequest,
    ) -> Result<ConfigurationRequestId, ConfigurationError> {
        let id = request.id;
        self.requests.push(request);
        Ok(id)
    }
}
#[test]
fn mvp_developer_entry_opens_real_presentation_settings_without_demo_actions() {
    let mut port = Port::new();
    assert!(port.snapshot.effective_ui.dev_menu);
    let mut workspace = crate::workspace::Workspace::new("/fixture", false).unwrap();
    workspace.poll(&mut port);
    workspace.key(
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        &mut port,
    );
    for character in "Developer menu".chars() {
        workspace.key(
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            &mut port,
        );
    }
    workspace.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
    assert!(workspace.configuration.open);
    assert_eq!(workspace.configuration.filter, "tui.");
    assert!(port.requests.is_empty());
}

fn press(view: &mut ConfigurationView, port: &mut Port, code: KeyCode, modifiers: KeyModifiers) {
    view.key(KeyEvent::new(code, modifiers), port);
}
fn view(port: &mut Port) -> ConfigurationView {
    let mut view = ConfigurationView::default();
    view.poll(port);
    view.show("");
    view
}
fn text(view: &ConfigurationView, width: u16, height: u16, ascii: bool) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| view.render(frame, false, ascii))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn review_regression_effective_only_collections_can_be_recreated_after_reload() {
    for (group, shortcut) in [("models", 'n'), ("capacity_pools", 'p')] {
        let mut port = Port::new();
        let mut settings = Settings::default();
        settings.models.insert("old".into(), Default::default());
        settings
            .capacity_pools
            .insert("old".into(), Default::default());
        for (descriptor, effective) in settings
            .entries()
            .into_iter()
            .filter(|(descriptor, _)| descriptor.key.starts_with(&format!("{group}.old.")))
        {
            port.snapshot.values.insert(
                descriptor.key.clone(),
                ConfigurationValue {
                    descriptor,
                    saved: SettingValue::Unset,
                    draft: SettingValue::Unset,
                    effective,
                    saved_origin: fluzo_core::settings::SettingOrigin::SavedFuture,
                    draft_origin: fluzo_core::settings::SettingOrigin::SavedFuture,
                    effective_origin: fluzo_core::settings::SettingOrigin::ActiveSnapshot,
                    cli_locked: false,
                },
            );
        }
        let mut view = view(&mut port);
        press(
            &mut view,
            &mut port,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
        );
        press(
            &mut view,
            &mut port,
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
        );
        port.outcome = ConfigurationRequestStatus::Completed {
            version: port.snapshot.version,
            outcome: ConfigurationOutcome::Reloaded,
        };
        view.poll(&mut port);
        press(
            &mut view,
            &mut port,
            KeyCode::Char(shortcut),
            KeyModifiers::CONTROL,
        );
        view.paste("old");
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            view.collections.get(&format!("{group}.old")),
            Some(&true),
            "{}",
            view.status
        );
        assert!(
            view.snapshot
                .as_ref()
                .unwrap()
                .values
                .keys()
                .any(|key| key.starts_with(&format!("{group}.old.")))
        );
        press(
            &mut view,
            &mut port,
            KeyCode::Char(shortcut),
            KeyModifiers::CONTROL,
        );
        view.paste("old");
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert!(view.status.contains("already exists"));
    }
}

#[test]
fn review_regression_partial_completion_preserves_other_pending_scopes() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.staged.extend([
        "tui.theme".into(),
        "tui.reduced_motion".into(),
        "harness.max_turns".into(),
    ]);
    view.unapplied
        .extend(["tui.theme".into(), "tui.reduced_motion".into()]);
    view.checked.insert("tui.theme".into());
    view.save(&mut port);
    port.outcome = ConfigurationRequestStatus::Completed {
        version: port.snapshot.version,
        outcome: ConfigurationOutcome::Saved,
    };
    view.poll(&mut port);
    assert!(!view.staged.contains("tui.theme"));
    assert!(view.staged.contains("tui.reduced_motion"));
    assert!(view.staged.contains("harness.max_turns"));
    assert!(view.unapplied.contains("tui.theme"));
    assert!(view.checked.is_empty());
    view.checked.insert("tui.reduced_motion".into());
    view.apply(&mut port);
    port.outcome = ConfigurationRequestStatus::Failed(ConfigurationError::Unavailable);
    view.poll(&mut port);
    assert!(view.unapplied.contains("tui.reduced_motion"));
    assert!(view.checked.contains("tui.reduced_motion"));
    view.apply(&mut port);
    port.outcome = ConfigurationRequestStatus::Completed {
        version: port.snapshot.version,
        outcome: ConfigurationOutcome::Applied,
    };
    view.poll(&mut port);
    assert!(!view.unapplied.contains("tui.reduced_motion"));
    assert!(view.unapplied.contains("tui.theme"));
    assert!(view.staged.contains("tui.reduced_motion"));
    assert!(view.checked.is_empty());
    view.save(&mut port);
    assert!(
        matches!(&port.requests.last().unwrap().action, ConfigurationAction::Save { keys, .. } if keys == &vec!["harness.max_turns".to_owned(), "tui.reduced_motion".to_owned()])
    );
}

#[test]
fn registry_coverage_and_typed_inputs_include_collections_optional_and_escaped_maps() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.stage(Edit::AddModel {
        name: "fixture".into(),
    })
    .unwrap();
    view.stage(Edit::AddPool {
        name: "local".into(),
    })
    .unwrap();
    let mut settings = Settings::default();
    settings.models.insert("fixture".into(), Default::default());
    settings
        .capacity_pools
        .insert("local".into(), Default::default());
    let descriptors = view.descriptors();
    assert_eq!(descriptors.len(), settings.descriptors().len());
    for descriptor in settings.descriptors() {
        assert_eq!(descriptors[&descriptor.key], descriptor);
    }
    let map = &descriptors["tools.shell.env"];
    assert_eq!(
        parse_value(map, "NAME=a\\=b\\nline\nEMPTY=\\e").unwrap(),
        SettingValue::TextMap(
            [
                ("NAME".into(), "a=b\nline".into()),
                ("EMPTY".into(), String::new())
            ]
            .into()
        )
    );
    assert!(parse_value(map, "A=1\nA=2").is_err());
    assert_eq!(
        parse_value(&descriptors["tools.shell.inherit_env"], "LANG\n\\e").unwrap(),
        SettingValue::TextList(vec!["LANG".into(), String::new()])
    );
    assert_eq!(
        parse_value(&descriptors["agent.model"], "").unwrap(),
        SettingValue::Unset
    );
    for invalid in ["0", "1", "NaN", "inf"] {
        assert!(parse_value(&descriptors["context.compaction_threshold"], invalid).is_err());
    }
    assert!(parse_value(&descriptors["tui.notifications.duration_seconds"], "31").is_err());
    assert!(!mutable(&descriptors["schema_version"]));
    assert!(!mutable(&descriptors["storage.auto_expire"]));
}

#[test]
fn typing_is_local_and_late_validation_after_close_never_dispatches_save() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.show("tui.theme");
    press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
    assert!(port.requests.is_empty());
    view.save(&mut port);
    assert_eq!(port.requests.len(), 1);
    assert!(matches!(
        port.requests[0].action,
        ConfigurationAction::Edit { .. }
    ));
    press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
    port.outcome = ConfigurationRequestStatus::Completed {
        version: Version {
            instance: 1,
            revision: 2,
        },
        outcome: ConfigurationOutcome::DraftUpdated,
    };
    view.poll(&mut port);
    assert_eq!(port.requests.len(), 1);
    assert!(!view.open);
    assert!(view.status.contains("not dispatched"));
}

#[test]
fn reopen_before_late_validation_does_not_restore_save_intent() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.show("tui.theme");
    press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
    view.save(&mut port);
    press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
    view.show("tui.theme");
    port.outcome = ConfigurationRequestStatus::Completed {
        version: Version {
            instance: 1,
            revision: 2,
        },
        outcome: ConfigurationOutcome::DraftUpdated,
    };
    view.poll(&mut port);
    assert_eq!(port.requests.len(), 1);
    assert!(view.status.contains("validated"));
}

#[test]
fn read_only_capacity_conflict_and_unknown_keep_drafts() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.stage(Edit::Set {
        key: "tui.theme".into(),
        value: SettingValue::Text("high-contrast".into()),
    })
    .unwrap();
    port.snapshot.save_unavailable = Some(ConfigurationError::ReadOnly);
    view.poll(&mut port);
    view.save(&mut port);
    assert!(port.requests.is_empty());
    assert_eq!(view.edits.len(), 1);
    port.snapshot.save_unavailable = None;
    port.snapshot.remaining_requests = 1;
    view.poll(&mut port);
    view.save(&mut port);
    assert!(port.requests.is_empty());
    assert_eq!(view.edits.len(), 1);
    port.snapshot.remaining_requests = MAX_REQUESTS;
    port.snapshot.version.revision = 2;
    view.poll(&mut port);
    view.validate(&mut port);
    assert!(view.status.contains("Conflict"));
    assert!(port.requests.is_empty());
}

#[test]
fn selected_default_cancel_and_redacted_input_never_reuse_projection() {
    let mut port = Port::new();
    port.snapshot
        .values
        .get_mut("tools.shell.env")
        .unwrap()
        .draft = SettingValue::Text("[redacted]".into());
    let mut view = view(&mut port);
    view.show("tools.shell.env");
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    assert!(view.editor.text.is_empty());
    view.paste("KEY=synthetic-private\u{1b}]52;c;payload\u{7}");
    assert!(!text(&view, 80, 24, false).contains("synthetic-private"));
    press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
    assert!(view.edits.is_empty());
    press(
        &mut view,
        &mut port,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
    );
    assert_eq!(view.edits.len(), 1);
    assert!(port.requests.is_empty());
    press(
        &mut view,
        &mut port,
        KeyCode::Char('x'),
        KeyModifiers::CONTROL,
    );
    assert!(port.requests.is_empty());
    press(
        &mut view,
        &mut port,
        KeyCode::Char('x'),
        KeyModifiers::CONTROL,
    );
    assert!(matches!(
        port.requests[0].action,
        ConfigurationAction::Cancel { .. }
    ));
}

#[test]
fn failures_and_closed_view_completion_are_not_replayed() {
    for error in [
        ConfigurationError::WriteFailed,
        ConfigurationError::Uncertain,
    ] {
        let mut port = Port::new();
        let mut view = view(&mut port);
        view.staged.insert("tui.theme".into());
        view.save(&mut port);
        assert_eq!(port.requests.len(), 1);
        view.open = false;
        port.outcome = ConfigurationRequestStatus::Failed(error.clone());
        view.poll(&mut port);
        view.poll(&mut port);
        assert_eq!(port.requests.len(), 1);
        assert!(view.status.contains(&format!("{error:?}")));
    }
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.staged.insert("tui.theme".into());
    view.save(&mut port);
    view.open = false;
    port.outcome = ConfigurationRequestStatus::Completed {
        version: Version {
            instance: 1,
            revision: 2,
        },
        outcome: ConfigurationOutcome::Saved,
    };
    view.poll(&mut port);
    assert!(view.status.contains("Saved"));
    assert!(!view.open);
    assert_eq!(port.requests.len(), 1);
    view.show("");
    view.save(&mut port);
    port.outcome = ConfigurationRequestStatus::Unknown;
    view.poll(&mut port);
    view.save(&mut port);
    assert_eq!(port.requests.len(), 2);
    assert!(view.status.contains("Conflict"));
}

#[test]
fn oversized_batch_and_paste_never_split_or_submit() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    for index in 0..=MAX_EDITS {
        view.collections
            .insert(format!("capacity_pools.pool{index}"), true);
    }
    view.save(&mut port);
    assert!(port.requests.is_empty());
    assert!(view.status.contains("not split"));
    view.show("tui.theme");
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    let original = view.editor.text.clone();
    view.paste(&"x".repeat(1024 * 1024));
    assert_eq!(view.editor.text, original);
    assert!(port.requests.is_empty());
}

#[test]
fn layouts_keyboard_and_hostile_text_remain_bounded() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.show("tui.notifications");
    for (width, height) in [(40, 10), (60, 16), (80, 24), (120, 40), (160, 50)] {
        for ascii in [false, true] {
            let output = text(&view, width, height, ascii);
            assert!(output.contains(if width < 60 {
                "resize required"
            } else {
                "Configuration"
            }));
            assert!(!output.contains('\u{1b}'));
        }
    }
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    view.paste("\u{1b}]52;c;synthetic\u{7}");
    assert!(!text(&view, 120, 40, false).contains('\u{1b}'));
    assert!(port.requests.is_empty());
}

#[test]
fn workspace_has_no_synthetic_tasks_and_preserves_composer_across_settings() {
    let mut port = Port::new();
    let mut workspace = crate::workspace::Workspace::new("fixture", false).unwrap();
    workspace.poll(&mut port);
    workspace.paste("retained composer");
    workspace.key(
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        &mut port,
    );
    workspace.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
    assert!(workspace.configuration.open);
    workspace.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|frame| workspace.render(frame, false))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(output.contains("retained composer"));
    assert!(!output.contains("Demo session"));
    assert!(!output.contains("synthetic"));
    assert!(port.requests.is_empty());
}
