use super::*;

fn checkpoint_event() -> sessions::SessionEvent {
    let store = sessions::SessionStore::new(10, 100);
    let session = store.start_session(Some("proj".to_string()), Some("projection".to_string()));
    let start = store
        .record_tool_call_started_with_metadata(
            Some(&session.session_id),
            sessions::SessionTransport::Mcp,
            "run_process",
            &json!({"project": "proj"}),
            Some("proj".to_string()),
            sessions::ToolCallRecorderMetadata {
                ack_session_context_revision: sessions::SessionContextRevisionAck::Unacknowledged,
                ..Default::default()
            },
            super::super::sessions::session_tool_contract("run_process"),
        )
        .expect("checkpoint start");
    store
        .record_model_facing_tool_call_finished(
            Some(start),
            true,
            &json!({"exit_code": 0}),
            None,
            None,
        )
        .expect("checkpoint finish");
    store
        .summary(&session.session_id, Some(20))
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.kind == "tool_call_finished")
        .expect("finished checkpoint event")
}

fn minimal_event() -> sessions::SessionEvent {
    let mut event = checkpoint_event();
    event.status = Some("succeeded".to_string());
    event.changed_paths.clear();
    event.job_id = None;
    event.error_kind = None;
    event.effect_evidence = None;
    event.context_result_summary = None;
    event.validation_output_summary = None;
    event
}

#[test]
fn minimal_context_recovery_event_projects_only_known_identity_and_status() {
    let event = minimal_event();
    let projected = model_facing_recovery_event(&event);
    assert_eq!(
        projected["context_revision"],
        event.context_revision.unwrap()
    );
    assert_eq!(projected["tool_name"], "run_process");
    assert_eq!(projected["status"], "succeeded");
    for absent in [
        "changed_paths",
        "job_id",
        "error_kind",
        "effect_evidence",
        "context_result",
        "execution_summary",
    ] {
        assert!(projected.get(absent).is_none(), "{absent}: {projected}");
    }
}

#[test]
fn context_recovery_event_preserves_all_available_optional_evidence() {
    let mut event = minimal_event();
    event.changed_paths = vec!["src/lib.rs".to_string(), "README.md".to_string()];
    event.job_id = Some("job-42".to_string());
    event.error_kind = Some("outcome_unknown".to_string());
    let mut effect = event.effect_evidence.clone().unwrap_or_default();
    effect.state_changed = Some(true);
    effect.command_started = Some(true);
    effect.command_completed = Some(false);
    effect.execution_state = Some("outcome_unknown".to_string());
    event.effect_evidence = Some(effect);
    event.context_result_summary = Some(json!({"head": "abc123", "clean": false}));
    event.validation_output_summary = Some(json!({"passed": false, "tests_run_count": 3}));

    let projected = model_facing_recovery_event(&event);
    assert_eq!(
        projected["changed_paths"],
        json!(["src/lib.rs", "README.md"])
    );
    assert_eq!(projected["job_id"], "job-42");
    assert_eq!(projected["error_kind"], "outcome_unknown");
    assert_eq!(
        projected["effect_evidence"],
        json!({
            "state_changed": true,
            "command_started": true,
            "command_completed": false,
            "execution_state": "outcome_unknown"
        })
    );
    assert_eq!(
        projected["context_result"],
        json!({"head": "abc123", "clean": false})
    );
    assert_eq!(
        projected["execution_summary"],
        json!({"passed": false, "tests_run_count": 3})
    );
}

#[test]
fn context_recovery_event_preserves_present_empty_evidence_and_unknown_status() {
    let mut event = minimal_event();
    event.status = None;
    event.effect_evidence = Some(Default::default());
    event.context_result_summary = Some(Value::Null);
    event.validation_output_summary = Some(json!({}));

    let projected = model_facing_recovery_event(&event);
    assert!(projected.get("status").is_none());
    assert_eq!(projected["effect_evidence"], json!({}));
    assert!(projected.get("context_result").is_some());
    assert!(projected["context_result"].is_null());
    assert_eq!(projected["execution_summary"], json!({}));
}

#[test]
fn sparse_context_recovery_events_retain_more_under_the_same_hard_budget() {
    let template = minimal_event();
    let evidence_chars =
        SESSION_CONTINUITY_RECOVERY_EVENT_BYTES / SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT - 128;
    let recovery_events = (1..=SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT)
        .map(|context_revision| {
            let mut event = template.clone();
            event.context_revision = Some(context_revision as u64);
            event.context_result_summary = Some(json!({"evidence": "x".repeat(evidence_chars)}));
            event
        })
        .collect::<Vec<_>>();
    let recorded = sessions::RecordedModelFacingToolCall {
        session_id: "wc_sess_projection".to_string(),
        context_revision: SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT as u64,
        pre_response_context_revision: SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT as u64,
        checkpoint_advanced: false,
        pre_call_context_revision: SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT as u64,
        ack_session_context_revision: sessions::SessionContextRevisionAck::Revision(0),
        recovery_events,
        history_lost: false,
    };

    let sparse = bounded_model_facing_recovery_events(&recorded);
    assert_eq!(sparse.len(), SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT);
    assert_eq!(
        sparse
            .iter()
            .map(|event| event["context_revision"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        (1..=SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT as u64).collect::<Vec<_>>()
    );
    assert!(serde_json::to_vec(&sparse).unwrap().len() <= SESSION_CONTINUITY_RECOVERY_EVENT_BYTES);

    let mut legacy_retained = Vec::new();
    for event in recorded
        .recovery_events
        .iter()
        .rev()
        .take(SESSION_CONTINUITY_RECOVERY_EVENT_LIMIT)
    {
        legacy_retained.insert(
            0,
            json!({
                "context_revision": event.context_revision,
                "tool_name": event.tool_name,
                "status": event.status,
                "changed_paths": event.changed_paths,
                "job_id": event.job_id,
                "error_kind": event.error_kind,
                "effect_evidence": event.effect_evidence,
                "context_result": event.context_result_summary,
                "execution_summary": event.validation_output_summary,
            }),
        );
        if serde_json::to_vec(&legacy_retained).unwrap().len()
            > SESSION_CONTINUITY_RECOVERY_EVENT_BYTES
        {
            legacy_retained.remove(0);
            break;
        }
    }
    assert!(legacy_retained.len() < sparse.len());
}
