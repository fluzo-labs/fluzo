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
                external_change: None,
                external_sequence: 0,
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
    assert!(!workspace.configuration.open);
    workspace.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
    assert!(workspace.configuration.open);
    assert_eq!(workspace.configuration.filter, "tui.");
    assert!(port.requests.is_empty());
}

#[test]
fn discovered_model_staging_is_atomic_and_rejects_existing_aliases() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.stage_discovered_model("fixture", "http://127.0.0.1:1234/v1", "synthetic")
        .unwrap();
    let batch = view.batch();
    assert_eq!(batch.len(), 6);
    assert!(
        view.stage_discovered_model("fixture", "http://127.0.0.1:1234/v1", "other")
            .is_err()
    );
    assert_eq!(view.batch(), batch);
    let selection = |alias: &str| crate::model_wizard::Selection {
        alias: alias.into(),
        endpoint: "http://127.0.0.1:1234/v1".into(),
        model: "synthetic".into(),
        authorization_env: Some("FIXTURE_AUTH".into()),
    };
    assert!(
        view.stage_discovered_models(&[selection("newmodel"), selection("fixture")])
            .is_err()
    );
    assert_eq!(view.batch(), batch);
    assert!(port.requests.is_empty());
    view.collections.clear();
    view.edits.clear();
    for index in 0..MAX_EDITS - 2 {
        view.edits
            .insert(format!("fixture.{index}"), SettingValue::Integer(1));
    }
    let before = view.batch();
    assert!(
        view.stage_discovered_model("another", "http://127.0.0.1:1234/v1", "synthetic")
            .is_err()
    );
    assert_eq!(view.batch(), before);
}

#[test]
fn every_configuration_surface_uses_shared_dialogs_and_preserves_background() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    for filter in [
        "harness.",
        "capacity_pools.",
        "models.",
        "storage.",
        "telemetry.",
        "tui.theme",
        "tui.notifications.",
        "tui.dev_menu",
        "tui.",
        "",
    ] {
        view.show(filter);
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            for (color, ascii) in [(true, false), (false, false), (false, true)] {
                let mut shell = crate::shell::Shell::configuration_shell().unwrap();
                shell.preferences.ascii = ascii;
                shell.truecolor = color;
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                for mode in 0..4 {
                    view.help = mode == 1;
                    view.input = match mode {
                        2 => Some(Input::Pool),
                        3 => Some(Input::Field("tui.theme".into())),
                        _ => None,
                    };
                    terminal
                        .draw(|frame| {
                            frame
                                .render_widget(Paragraph::new("retained background"), frame.area());
                            view.render_dialog(frame, &shell, color);
                        })
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    let left = (width - (width - 2).min(100)) / 2;
                    let top = (height - (height - 2).min(34)) / 2;
                    assert_eq!(buffer[(left, top)].symbol(), if ascii { "+" } else { "╭" });
                    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                    assert!(text.contains("retained background"));
                    assert!(text.contains(if ascii { "/" } else { "╱" }));
                    assert!(text.contains(match mode {
                        1 => "Configuration help",
                        2 => "Add pool name",
                        _ => "FLUZO / Configuration",
                    }));
                    assert!(text.contains("Esc"));
                }
            }
        }
    }
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
fn rows(
    view: &ConfigurationView,
    width: u16,
    height: u16,
    color: bool,
    ascii: bool,
) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| view.render(frame, color, ascii))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|row| {
            (0..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}
fn restart_line(
    view: &ConfigurationView,
    width: u16,
    height: u16,
    color: bool,
    ascii: bool,
) -> String {
    rows(view, width, height, color, ascii)
        .iter()
        .find_map(|row| {
            row.find("Restart pending:").map(|start| {
                row[start..]
                    .trim_end_matches([' ', '\u{2502}', '|'])
                    .to_string()
            })
        })
        .unwrap_or_else(|| panic!("restart-pending line missing from the settings details area"))
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
fn enumerated_choices_require_focus_and_keep_selected_option_visible() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    for descriptor in view.descriptors().values() {
        let SettingKind::Choice(options) = &descriptor.kind else {
            continue;
        };
        view.show(&descriptor.key);
        view.selected = descriptor.key.clone();
        let before = view.batch();
        press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(view.batch(), before);
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        if !mutable(descriptor) {
            assert!(view.choice_input.is_none());
            continue;
        }
        let original = view.choice_input.clone();
        view.paste("invalid");
        press(&mut view, &mut port, KeyCode::Backspace, KeyModifiers::NONE);
        press(
            &mut view,
            &mut port,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(view.choice_input, original);
        for _ in 0..options.len() {
            press(&mut view, &mut port, KeyCode::Left, KeyModifiers::NONE);
        }
        for (selection, option) in options.iter().enumerate() {
            assert_eq!(view.choice_input, Some((descriptor.key.clone(), selection)));
            for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
                for color in [false, true] {
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal
                        .draw(|frame| view.render(frame, color, !color))
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    let output: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                    assert!(output.contains(option));
                    if color {
                        assert!(
                            buffer
                                .content
                                .iter()
                                .any(|cell| cell.bg == ratatui::style::Color::Cyan)
                        );
                    } else {
                        assert!(output.contains(&format!("[x {option}]")));
                    }
                }
            }
            press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
        }
        assert_eq!(view.batch(), before);
        press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(view.batch(), before);
        assert!(view.open && view.choice_input.is_none());
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        for _ in 0..options.len() {
            press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
        }
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert!(view.choice_input.is_none());
        assert_eq!(
            view.edits[&descriptor.key],
            SettingValue::Text(options.last().unwrap().clone())
        );
    }
    assert!(port.requests.is_empty());
}

#[test]
fn theme_preview_and_exit_prompt_preserve_authority_and_drafts() {
    for locked in [false, true] {
        let mut port = Port::new();
        port.snapshot
            .values
            .get_mut("tui.theme")
            .unwrap()
            .cli_locked = locked;
        let mut workspace = crate::workspace::Workspace::new("fixture", false).unwrap();
        workspace.poll(&mut port);
        workspace.configuration.show("tui.theme");
        for code in [KeyCode::Enter, KeyCode::Right, KeyCode::Enter] {
            workspace.key(KeyEvent::new(code, KeyModifiers::NONE), &mut port);
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| workspace.render(frame, true))
            .unwrap();
        assert_eq!(
            workspace.shell.preferences.effective().theme,
            if locked { "default" } else { "high-contrast" }
        );
        assert_eq!(workspace.shell.preferences.applied().theme, "default");
        assert!(port.requests.is_empty());
        workspace.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        assert!(workspace.configuration.open);
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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
            assert!(output.contains("Save changes?"));
            assert!(output.contains("Back to editing"));
        }
        workspace.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        assert!(workspace.configuration.open && workspace.configuration.close_prompt.is_none());
        workspace.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        workspace.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &mut port);
        workspace.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        assert!(!workspace.configuration.open);
        terminal
            .draw(|frame| workspace.render(frame, true))
            .unwrap();
        assert_eq!(workspace.shell.preferences.effective().theme, "default");
        assert!(workspace.configuration.has_unsaved_changes());
        assert!(port.requests.is_empty());
    }
}

#[test]
fn exit_save_waits_for_completion_and_retains_draft_on_rejection() {
    for failure in [false, true] {
        let mut port = Port::new();
        let mut view = view(&mut port);
        view.stage(Edit::Set {
            key: "tui.theme".into(),
            value: SettingValue::Text("high-contrast".into()),
        })
        .unwrap();
        view.checked.insert("tui.animation_fps".into());
        press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
        view.close_prompt = Some(0);
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert!(view.open && view.close_after_save);
        port.snapshot.values.get_mut("tui.theme").unwrap().draft =
            SettingValue::Text("high-contrast".into());
        port.outcome = ConfigurationRequestStatus::Completed {
            version: port.snapshot.version,
            outcome: ConfigurationOutcome::DraftUpdated,
        };
        view.poll(&mut port);
        assert!(
            matches!(&port.requests[1].action, ConfigurationAction::Save { keys, .. } if keys == &vec!["tui.theme".to_owned()])
        );
        assert!(view.open);
        port.outcome = if failure {
            ConfigurationRequestStatus::Failed(ConfigurationError::ReadOnly)
        } else {
            ConfigurationRequestStatus::Completed {
                version: port.snapshot.version,
                outcome: ConfigurationOutcome::Saved,
            }
        };
        view.poll(&mut port);
        assert_eq!(view.open, failure);
        assert_eq!(view.has_unsaved_changes(), failure);
        assert!(!view.close_after_save);
    }
    let mut port = Port::new();
    port.snapshot.save_unavailable = Some(ConfigurationError::ReadOnly);
    let mut view = view(&mut port);
    view.stage(Edit::Set {
        key: "tui.theme".into(),
        value: SettingValue::Text("high-contrast".into()),
    })
    .unwrap();
    press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
    view.close_prompt = Some(0);
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    assert!(view.open && view.has_unsaved_changes());
    assert!(view.status.contains("ReadOnly"));
    assert!(port.requests.is_empty());
}

#[test]
fn disabled_values_are_gray_without_losing_focus_or_fixed_markers() {
    for theme_name in ["default", "high-contrast"] {
        let mut port = Port::new();
        port.snapshot.effective_ui.theme = theme_name.into();
        let mut view = view(&mut port);
        for key in ["storage.auto_expire", "schema_version"] {
            view.show(key);
            view.selected = key.into();
            for color in [false, true] {
                let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
                terminal
                    .draw(|frame| view.render(frame, color, !color))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let value = &buffer[(42, 6)];
                if color {
                    assert_eq!(value.fg, ratatui::style::Color::Gray);
                    assert_eq!(value.bg, buffer[(3, 6)].bg);
                } else {
                    assert!(value.modifier.contains(ratatui::style::Modifier::DIM));
                }
                let output: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(output.contains("[fixed]"));
            }
            press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
            assert!(
                view.input.is_none() && view.boolean_input.is_none() && view.choice_input.is_none()
            );
        }
        assert!(port.requests.is_empty());
    }
}

#[test]
fn inline_deletion_clears_old_value_from_the_screen() {
    for deletion in [KeyCode::Backspace, KeyCode::Delete] {
        for color in [false, true] {
            let mut port = Port::new();
            let mut view = view(&mut port);
            view.show("tui.animation_fps");
            view.stage(Edit::Set {
                key: "tui.animation_fps".into(),
                value: SettingValue::Integer(123),
            })
            .unwrap();
            press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut view, &mut port, deletion, KeyModifiers::NONE);
            assert!(view.editor.text.is_empty());
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|frame| view.render(frame, color, false))
                .unwrap();
            let buffer = terminal.backend().buffer();
            for column in 42..77 {
                assert_eq!(buffer[(column, 6)].symbol(), " ");
            }
            view.paste("9");
            terminal
                .draw(|frame| view.render(frame, color, false))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(42, 6)].symbol(), "9");
            assert_eq!(buffer[(43, 6)].symbol(), " ");
            assert_eq!(buffer[(44, 6)].symbol(), " ");
            press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(view.edits["tui.animation_fps"], SettingValue::Integer(123));
            assert!(port.requests.is_empty());
        }
    }
}

#[test]
fn inline_values_replace_cancel_validate_and_restore_defaults() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    for (key, replacement) in [
        ("tui.animation_fps", "17"),
        ("context.compaction_threshold", "0.7"),
        ("agent.model", "fixture"),
        ("tools.shell.inherit_env", "LANG\nTERM"),
        ("tools.shell.env", "KEY=value"),
    ] {
        view.show(key);
        view.selected = key.into();
        let before = view.batch();
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        view.paste(replacement);
        assert_eq!(view.editor.text, replacement);
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| view.render(frame, true, false))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| cell.bg == ratatui::style::Color::Cyan)
            );
            let output: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(output.contains("FLUZO / Configuration"));
            assert!(output.contains("Empty restores default"));
        }
        press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(view.batch(), before);
        assert!(view.open && view.input.is_none());
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        view.paste(replacement);
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert!(view.input.is_none());
        let expected = parse_value(&view.descriptors()[key], replacement).unwrap();
        assert_eq!(view.edits[key], expected);
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        press(
            &mut view,
            &mut port,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        );
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(view.edits[key], view.descriptors()[key].default);
        assert!(view.input.is_none());
    }
    view.show("tui.animation_fps");
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    view.paste("invalid");
    let before = view.batch();
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    assert!(view.input.is_some());
    assert_eq!(view.batch(), before);
    assert!(port.requests.is_empty());
}

#[test]
fn settings_rows_align_values_and_only_show_cyan_while_editing() {
    for width in [54, 74, 96] {
        for theme_name in ["default", "high-contrast"] {
            let mut settings = Settings::default();
            settings.tui.theme = theme_name.into();
            let theme = Theme::new(&settings.tui, true);
            for color in [false, true] {
                let theme = if color {
                    theme
                } else {
                    Theme::new(&settings.tui, false)
                };
                for editing in [None, Some(true), Some(false)] {
                    let mut terminal = Terminal::new(TestBackend::new(width, 3)).unwrap();
                    terminal
                        .draw(|frame| {
                            frame.render_widget(
                                Paragraph::new(vec![
                                    setting_row(
                                        "auto expire",
                                        "false",
                                        editing,
                                        width,
                                        true,
                                        theme,
                                        color,
                                    ),
                                    setting_row(
                                        "storage.min_free_bytes",
                                        "123",
                                        None,
                                        width,
                                        false,
                                        theme,
                                        color,
                                    ),
                                    setting_row(
                                        "wide 界 e\u{301}",
                                        "456",
                                        None,
                                        width,
                                        false,
                                        theme,
                                        color,
                                    ),
                                ]),
                                frame.area(),
                            );
                        })
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    let value_x = 4 + (width - 4) / 2;
                    assert_eq!(buffer[(value_x, 1)].symbol(), "1");
                    assert_eq!(buffer[(value_x, 2)].symbol(), "4");
                    if editing.is_none() {
                        assert_eq!(buffer[(value_x, 0)].symbol(), "f");
                        let background = buffer[(0, 0)].bg;
                        for column in 0..width {
                            assert_eq!(buffer[(column, 0)].bg, background);
                        }
                        assert!(
                            !buffer
                                .content
                                .iter()
                                .any(|cell| cell.bg == ratatui::style::Color::Cyan)
                        );
                    } else if color {
                        let selected_x = value_x + if editing == Some(true) { 1 } else { 8 };
                        assert_eq!(buffer[(selected_x, 0)].bg, ratatui::style::Color::Cyan);
                        assert_ne!(buffer[(0, 0)].bg, ratatui::style::Color::Cyan);
                    }
                }
            }
        }
    }
}

#[test]
fn boolean_controls_require_edit_focus_and_explicit_confirmation() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    let descriptors = view.descriptors();
    for descriptor in descriptors
        .values()
        .filter(|field| field.kind == SettingKind::Boolean)
    {
        view.show(&descriptor.key);
        view.selected = descriptor.key.clone();
        let original = view.snapshot.as_ref().unwrap().values[&descriptor.key]
            .draft
            .clone();
        press(&mut view, &mut port, KeyCode::Left, KeyModifiers::NONE);
        assert!(!view.edits.contains_key(&descriptor.key));
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        if !mutable(descriptor) {
            assert!(view.boolean_input.is_none());
            assert!(text(&view, 80, 24, false).contains("[fixed]"));
            continue;
        }
        let initial = original == SettingValue::Boolean(true);
        assert_eq!(view.boolean_input, Some((descriptor.key.clone(), initial)));
        view.paste("not a boolean\n");
        press(
            &mut view,
            &mut port,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(view.boolean_input, Some((descriptor.key.clone(), initial)));
        let arrow = if initial {
            KeyCode::Right
        } else {
            KeyCode::Left
        };
        press(&mut view, &mut port, arrow, KeyModifiers::NONE);
        press(&mut view, &mut port, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(view.selected, descriptor.key);
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            for (color, ascii) in [(true, false), (false, false), (false, true)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| view.render(frame, color, ascii))
                    .unwrap();
                let rendered: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                if color {
                    assert!(rendered.contains(" true   false "));
                    assert!(!rendered.contains("[x true]") && !rendered.contains("[x false]"));
                    let buffer = terminal.backend().buffer();
                    let start = buffer
                        .content
                        .windows(14)
                        .position(|cells| {
                            cells.iter().map(|cell| cell.symbol()).collect::<String>()
                                == " true   false "
                        })
                        .unwrap();
                    let true_bg = buffer.content[start + 1].bg;
                    let false_bg = buffer.content[start + 8].bg;
                    assert_ne!(true_bg, false_bg);
                    assert_eq!(
                        if initial { false_bg } else { true_bg },
                        ratatui::style::Color::Cyan
                    );
                    let label_bg = buffer.content[start - 1].bg;
                    assert_ne!(label_bg, ratatui::style::Color::Cyan);
                    assert_ne!(label_bg, buffer.content[start + 6].bg);
                    assert_eq!(
                        if initial { true_bg } else { false_bg },
                        buffer.content[start + 6].bg
                    );
                } else {
                    assert!(rendered.contains(boolean_control("", !initial).trim_start()));
                }
                assert!(rendered.contains("Esc cancels"));
            }
        }
        assert!(!view.edits.contains_key(&descriptor.key));
        press(&mut view, &mut port, KeyCode::Esc, KeyModifiers::NONE);
        assert!(view.open && view.boolean_input.is_none());
        assert!(!view.edits.contains_key(&descriptor.key));
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut view, &mut port, arrow, KeyModifiers::NONE);
        press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(view.edits[&descriptor.key], SettingValue::Boolean(!initial));
        assert!(view.boolean_input.is_none());
        view.edits.clear();
    }
    assert!(port.requests.is_empty());
}

#[test]
fn typing_is_local_and_late_validation_after_close_never_dispatches_save() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    view.show("tui.theme");
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
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
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
    press(&mut view, &mut port, KeyCode::Right, KeyModifiers::NONE);
    press(&mut view, &mut port, KeyCode::Enter, KeyModifiers::NONE);
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
fn restart_pending_line_lists_exactly_the_snapshot_keys() {
    let mut port = Port::new();
    port.snapshot.pending_restart = vec!["agent.runtime".into(), "harness.max_turns".into()];
    let mut view = view(&mut port);
    view.show("tui.theme");
    assert_eq!(
        restart_line(&view, 120, 40, false, false),
        "Restart pending: agent.runtime, harness.max_turns"
    );
    let rows = rows(&view, 120, 40, false, false);
    let effective = rows
        .iter()
        .position(|row| row.contains("Effective "))
        .expect("selected key details");
    let restart = rows
        .iter()
        .position(|row| row.contains("Restart pending:"))
        .expect("restart-pending line");
    let footer = rows
        .iter()
        .position(|row| row.contains("Enter edit"))
        .expect("footer");
    assert!(restart > effective && restart < footer);
}

#[test]
fn restart_pending_empty_state_is_explicit() {
    let mut port = Port::new();
    assert!(port.snapshot.pending_restart.is_empty());
    let view = view(&mut port);
    for ascii in [false, true] {
        let line = restart_line(&view, 120, 40, false, ascii);
        assert_eq!(line, "Restart pending: none");
    }
    assert!(!text(&view, 120, 40, false).contains("Restart pending: \u{2502}"));
}

#[test]
fn restart_pending_line_survives_an_unrelated_presentation_apply() {
    let mut port = Port::new();
    port.snapshot.pending_restart = vec!["agent.runtime".into()];
    let mut view = view(&mut port);
    view.show("tui.theme");
    view.checked.insert("tui.theme".into());
    view.apply(&mut port);
    assert!(
        matches!(
            &port.requests.last().unwrap().action,
            ConfigurationAction::Apply { keys, .. } if keys == &vec!["tui.theme".to_owned()]
        ),
        "a presentation Apply must name only the selected key, never the restart key"
    );
    assert_eq!(
        restart_line(&view, 120, 40, false, false),
        "Restart pending: agent.runtime"
    );
    port.outcome = ConfigurationRequestStatus::Completed {
        version: port.snapshot.version,
        outcome: ConfigurationOutcome::Applied,
    };
    assert!(view.poll(&mut port));
    assert!(!view.checked.contains("tui.theme"));
    assert!(!view.unapplied.contains("tui.theme"));
    assert_eq!(
        restart_line(&view, 120, 40, false, false),
        "Restart pending: agent.runtime"
    );
}

#[test]
fn restart_pending_line_stays_readable_across_sizes_and_fallbacks() {
    let mut port = Port::new();
    port.snapshot.pending_restart = vec!["agent.runtime".into(), "harness.max_turns".into()];
    let view = view(&mut port);
    for (width, height) in [(60u16, 16u16), (80, 24), (120, 40), (160, 50)] {
        for color in [false, true] {
            for ascii in [false, true] {
                assert_eq!(
                    restart_line(&view, width, height, color, ascii),
                    "Restart pending: agent.runtime, harness.max_turns",
                    "{width}x{height} color={color} ascii={ascii}"
                );
            }
        }
    }
    port.snapshot.external_change = Some(ExternalChange::Modified);
    port.snapshot.external_sequence = 1;
    let mut notified = ConfigurationView::default();
    notified.poll(&mut port);
    notified.show("");
    for (width, height) in [(60u16, 16u16), (80, 24)] {
        assert_eq!(
            restart_line(&notified, width, height, false, false),
            "Restart pending: agent.runtime, harness.max_turns",
            "{width}x{height} with an outside-change notice row"
        );
    }
}

#[test]
fn restart_pending_keys_are_sanitized_like_other_configuration_text() {
    let mut port = Port::new();
    port.snapshot.pending_restart = vec![
        "\u{1b}[31m\u{7}evil".to_string(),
        "\u{202e}evil\u{200e}".to_string(),
    ];
    let view = view(&mut port);
    assert_eq!(
        restart_line(&view, 120, 40, false, false),
        r"Restart pending: \u{1b}[31m\u{7}evil, \u{202e}evil\u{200e}"
    );
    for ascii in [false, true] {
        let screen = text(&view, 120, 40, ascii);
        for character in ['\u{1b}', '\u{7}', '\u{202e}', '\u{200e}'] {
            assert!(
                !screen.contains(character),
                "raw {character:?} reached the screen"
            );
        }
    }
}

#[test]
fn command_menu_routes_each_root_without_implicit_effects() {
    for root in [
        "Models",
        "Developer Menu",
        "Quit",
        "Plugins",
        "Providers",
        "Configuration",
    ] {
        let mut port = Port::new();
        let mut workspace = crate::workspace::Workspace::new("fixture", false).unwrap();
        workspace.poll(&mut port);
        workspace.key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &mut port,
        );
        for character in root.chars() {
            workspace.key(
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                &mut port,
            );
        }
        workspace.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        assert_eq!(workspace.shell.quit, root == "Quit");
        assert!(!workspace.configuration.open);
        assert!(!workspace.model_wizard.open);
        if root == "Quit" {
            continue;
        }
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| workspace.render(frame, false))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains(&format!("Commands / {root}")));
            assert!(text.contains(match root {
                "Configuration" => "Limits",
                "Models" => "Configured models",
                "Developer Menu" => "Presentation settings",
                "Plugins" => "Plugins are not implemented",
                _ => "Provider management is not implemented",
            }));
            assert!(text.contains("Esc returns"));
        }
        if matches!(root, "Plugins" | "Providers") {
            workspace.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            assert!(!workspace.configuration.open && !workspace.model_wizard.open);
        }
        workspace.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| workspace.render(frame, false))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains(&format!("Search: {root}")));
        assert!(!text.contains(&format!("Commands / {root}")));
        assert!(port.requests.is_empty());
    }
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
    assert!(!workspace.configuration.open);
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

#[test]
fn outside_change_shows_a_banner_until_keep_ours_is_pressed() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    assert!(view.external_notice().is_none());
    port.snapshot.external_change = Some(ExternalChange::Modified);
    view.poll(&mut port);
    assert!(
        view.external_notice()
            .is_some_and(|notice| notice.contains("changed outside Fluzo"))
    );
    let rendered = text(&view, 120, 40, true);
    assert!(rendered.contains("changed outside Fluzo"), "{rendered}");
    port.snapshot.external_change = Some(ExternalChange::Removed);
    port.snapshot.external_sequence = 2;
    view.poll(&mut port);
    assert!(
        view.external_notice()
            .is_some_and(|notice| notice.contains("deleted outside Fluzo"))
    );
    press(
        &mut view,
        &mut port,
        KeyCode::Char('k'),
        KeyModifiers::CONTROL,
    );
    assert!(view.external_notice().is_none());
    assert!(view.status.contains("Keeping our draft"));
    port.snapshot.external_sequence = 3;
    view.poll(&mut port);
    assert!(
        view.external_notice()
            .is_some_and(|notice| notice.contains("deleted outside Fluzo")),
        "a second outside edit of the same kind must surface again after acknowledgement"
    );
}

#[test]
fn keep_ours_needs_an_outside_change_and_waits_for_pending_writes() {
    let mut port = Port::new();
    let mut view = view(&mut port);
    press(
        &mut view,
        &mut port,
        KeyCode::Char('k'),
        KeyModifiers::CONTROL,
    );
    assert!(view.status.contains("No outside change to acknowledge"));
    assert!(!view.external_ack);
    port.snapshot.external_change = Some(ExternalChange::Appeared);
    view.poll(&mut port);
    view.pending = Some(Pending {
        id: ConfigurationRequestId(7),
        intent: Intent::Save,
        editing: false,
        keys: vec![],
    });
    press(
        &mut view,
        &mut port,
        KeyCode::Char('k'),
        KeyModifiers::CONTROL,
    );
    assert!(!view.external_ack);
    assert!(view.status.contains("Wait for pending completion"));
    view.pending = None;
    press(
        &mut view,
        &mut port,
        KeyCode::Char('k'),
        KeyModifiers::CONTROL,
    );
    assert!(view.external_ack);
}
