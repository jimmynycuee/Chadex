use super::agent_task::{AgentTaskAttemptStartMutation, NewAgentTask};
use super::agent_wait::*;
use super::agent_wake::AgentWakeState;
use super::communication::{
    CommunicationPrincipal, NewAgentEndpoint, NewAgentIdentity,
    COMMUNICATION_PRINCIPAL_DIGEST_PREFIX,
};
use super::Database;

fn principal(hex: char) -> CommunicationPrincipal {
    CommunicationPrincipal {
        kind: "user".to_string(),
        digest: format!(
            "{COMMUNICATION_PRINCIPAL_DIGEST_PREFIX}{}",
            hex.to_string().repeat(64)
        ),
    }
}

fn agent(db: &Database, owner: &CommunicationPrincipal, label: &str) -> String {
    db.create_agent_identity(
        owner,
        NewAgentIdentity {
            handle: label.to_string(),
            display_name: label.to_string(),
            description: String::new(),
            specialty_labels: Vec::new(),
            idempotency_key: format!("create-{label}"),
        },
    )
    .unwrap()
    .agent
    .agent_id
}

fn endpoint(
    db: &Database,
    owner: &CommunicationPrincipal,
    agent_id: &str,
    label: &str,
) -> super::communication::AgentEndpointRecord {
    db.attach_agent_endpoint(
        owner,
        NewAgentEndpoint {
            agent_id: agent_id.to_string(),
            host: "ChatGPT".to_string(),
            client_attachment_id: Some(label.to_string()),
            wake_capable: true,
            idempotency_key: format!("endpoint-{label}"),
        },
    )
    .unwrap()
    .endpoint
}

fn task(
    db: &Database,
    owner: &CommunicationPrincipal,
    assignee_agent_id: &str,
    label: &str,
) -> String {
    db.create_agent_task(
        owner,
        NewAgentTask {
            title: format!("Task {label}"),
            instruction: format!("PRIVATE instruction {label}"),
            assignee_agent_id: Some(assignee_agent_id.to_string()),
            source_conversation_id: None,
            source_message_id: None,
            referenced_project_id: Some(format!("agent:special:private-{label}")),
            idempotency_key: format!("task-{label}"),
        },
    )
    .unwrap()
    .task
    .summary
    .task_id
}

fn start(
    db: &Database,
    owner: &CommunicationPrincipal,
    task_id: &str,
    assignee_agent_id: &str,
    label: &str,
) -> AgentTaskAttemptStartMutation {
    db.start_agent_task_attempt(
        owner,
        task_id,
        assignee_agent_id,
        &format!("attempt-{label}"),
    )
    .unwrap()
}

fn complete(
    db: &Database,
    owner: &CommunicationPrincipal,
    task_id: &str,
    assignee_agent_id: &str,
    started: &AgentTaskAttemptStartMutation,
    label: &str,
) {
    db.complete_agent_task_attempt(
        owner,
        task_id,
        &started.attempt.attempt_id,
        assignee_agent_id,
        &started.attempt_fence,
        started.attempt.attempt_controller_generation,
        super::agent_task::AgentTaskState::Succeeded,
        Some(&format!("PRIVATE terminal result {label}")),
        Some(&format!("PRIVATE terminal reason {label}")),
        &format!("complete-{label}"),
    )
    .unwrap();
}

fn wait_input(
    target_agent_id: &str,
    endpoint: &super::communication::AgentEndpointRecord,
    task_ids: &[String],
    key: &str,
) -> NewAgentWait {
    NewAgentWait {
        target_agent_id: target_agent_id.to_string(),
        endpoint_id: endpoint.endpoint_id.clone(),
        expected_controller_generation: endpoint.controller_generation,
        events: task_ids
            .iter()
            .map(|task_id| AgentWaitEventSelector {
                kind: AGENT_WAIT_EVENT_KIND_AGENT_TASK_TERMINAL.to_string(),
                task_id: task_id.clone(),
            })
            .collect(),
        idempotency_key: key.to_string(),
    }
}

fn wait_wake_id(db: &Database, wait_id: &str) -> String {
    db.conn_for_tests()
        .query_row(
            "SELECT wake_id FROM wc_agent_wakes WHERE source_wait_id = ?1",
            [wait_id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn create_wait_is_exact_keyed_private_and_snapshots_already_terminal_sources() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-create.db")).unwrap();
    let owner = principal('1');
    let watcher = agent(&db, &owner, "wait-create-watcher");
    let worker = agent(&db, &owner, "wait-create-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-create-view");
    let task_a = task(&db, &owner, &worker, "wait-create-a");
    let task_b = task(&db, &owner, &worker, "wait-create-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-create-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-create-b");
    complete(&db, &owner, &task_a, &worker, &a, "wait-create-a");
    complete(&db, &owner, &task_b, &worker, &b, "wait-create-b");

    let input = wait_input(
        &watcher,
        &endpoint,
        &[task_a.clone(), task_b.clone()],
        "wait-create-key",
    );
    let created = db.create_agent_wait(&owner, input.clone()).unwrap();
    assert_eq!(created.agent_wait.state, AgentWaitState::Triggered);
    assert_eq!(created.agent_wait.source_count, 2);
    assert_eq!(created.agent_wait.match_count, 2);
    assert!(created.schedule_required);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1 AND state = 'pending'",
                [created.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "multiple already-terminal selectors must create exactly one queueable Wake"
    );

    let replay = db.create_agent_wait(&owner, input).unwrap();
    assert!(replay.replayed);
    assert!(!replay.state_changed);
    assert_eq!(replay.agent_wait.wait_id, created.agent_wait.wait_id);

    let mut changed = wait_input(&watcher, &endpoint, &[task_a], "wait-create-key");
    changed.expected_controller_generation = endpoint.controller_generation;
    let conflict = db.create_agent_wait(&owner, changed).unwrap_err();
    assert_eq!(conflict.code(), "communication_idempotency_conflict");

    let serialized = serde_json::to_string(&created.agent_wait).unwrap();
    for private in [
        "PRIVATE instruction",
        "PRIVATE terminal result",
        "PRIVATE terminal reason",
        "agent:special:private-",
    ] {
        assert!(!serialized.contains(private));
    }
}

#[test]
fn wait_source_authority_is_independent_and_foreign_task_is_existence_hidden() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-authority.db")).unwrap();
    let alice = principal('2');
    let bob = principal('3');
    let alice_agent = agent(&db, &alice, "wait-authority-alice");
    let bob_agent = agent(&db, &bob, "wait-authority-bob");
    let endpoint = endpoint(&db, &alice, &alice_agent, "wait-authority-view");
    let foreign_task = task(&db, &bob, &bob_agent, "wait-authority-foreign");
    let foreign = db
        .create_agent_wait(
            &alice,
            wait_input(
                &alice_agent,
                &endpoint,
                &[foreign_task],
                "wait-authority-foreign",
            ),
        )
        .unwrap_err();
    let missing = db
        .create_agent_wait(
            &alice,
            wait_input(
                &alice_agent,
                &endpoint,
                &["wc_agent_task_________________".to_string()],
                "wait-authority-missing",
            ),
        )
        .unwrap_err();
    assert_eq!(foreign.code(), "agent_task_not_found");
    assert_eq!(missing.code(), "agent_task_not_found");
    assert_eq!(foreign.message(), missing.message());
}

#[test]
fn future_matches_coalesce_only_before_prepare_and_exact_consume_resumes_once() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-coalesce.db")).unwrap();
    let owner = principal('4');
    let watcher = agent(&db, &owner, "wait-coalesce-watcher");
    let worker = agent(&db, &owner, "wait-coalesce-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-coalesce-view");
    let task_a = task(&db, &owner, &worker, "wait-coalesce-a");
    let task_b = task(&db, &owner, &worker, "wait-coalesce-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-coalesce-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-coalesce-b");
    let created = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b.clone()],
                "wait-coalesce",
            ),
        )
        .unwrap();
    assert_eq!(created.agent_wait.state, AgentWaitState::Waiting);

    complete(&db, &owner, &task_a, &worker, &a, "wait-coalesce-a");
    let wake_id = wait_wake_id(&db, &created.agent_wait.wait_id);
    let first = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(first.wait_match_count_snapshot, Some(1));
    let explicit = db
        .accept_explicit_agent_wake_activation(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            "wait-coalesce-explicit-activation",
        )
        .unwrap_err();
    assert_eq!(
        explicit.code(),
        "agent_wait_wake_requires_endpoint_dispatch"
    );
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    assert_eq!(claim.wake.wake_id, wake_id);

    complete(&db, &owner, &task_b, &worker, &b, "wait-coalesce-b");
    let coalesced = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(coalesced.state, AgentWakeState::Claimed);
    assert_eq!(coalesced.wait_match_count_snapshot, Some(2));
    assert!(coalesced.revision > first.revision);

    let prepared = db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
            &claim.consume_token,
        )
        .unwrap();
    assert!(prepared
        .envelope
        .resume_hint
        .contains(&created.agent_wait.wait_id));
    assert!(prepared.envelope.resume_hint.contains("match_count=2"));
    assert!(prepared
        .envelope
        .resume_hint
        .contains("read_agent_wait(wait_id)"));
    assert!(!prepared.envelope.resume_hint.contains("PRIVATE"));

    let consumed = db
        .consume_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.consume_token,
        )
        .unwrap();
    assert!(!consumed.already_consumed);
    assert_eq!(
        db.read_agent_wait(&owner, &created.agent_wait.wait_id)
            .unwrap()
            .state,
        AgentWaitState::Resumed
    );
    let late_ack = db
        .complete_agent_wake_delivery(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
        )
        .unwrap();
    assert_eq!(late_ack.state, AgentWakeState::Consumed);
    assert_eq!(
        db.read_agent_wait(&owner, &created.agent_wait.wait_id)
            .unwrap()
            .state,
        AgentWaitState::Resumed,
        "a late Host ACK after exact consume must remain idempotent and never regress the one-shot Wait"
    );
    let replay = db
        .consume_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.consume_token,
        )
        .unwrap();
    assert!(replay.already_consumed);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [created.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn prepared_wait_batch_is_sealed_and_post_fence_cancel_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-sealed.db")).unwrap();
    let owner = principal('5');
    let watcher = agent(&db, &owner, "wait-sealed-watcher");
    let worker = agent(&db, &owner, "wait-sealed-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-sealed-view");
    let task_a = task(&db, &owner, &worker, "wait-sealed-a");
    let task_b = task(&db, &owner, &worker, "wait-sealed-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-sealed-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-sealed-b");
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b.clone()],
                "wait-sealed",
            ),
        )
        .unwrap()
        .agent_wait;
    complete(&db, &owner, &task_a, &worker, &a, "wait-sealed-a");
    let wake_id = wait_wake_id(&db, &wait.wait_id);
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    db.prepare_agent_wake_dispatch(
        &owner,
        &watcher,
        &endpoint.endpoint_id,
        endpoint.controller_generation,
        &wake_id,
        &claim.attempt.attempt_id,
        &claim.claim_fence,
        &claim.consume_token,
    )
    .unwrap();
    let sealed = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(sealed.wait_match_count_snapshot, Some(1));

    complete(&db, &owner, &task_b, &worker, &b, "wait-sealed-b");
    let after = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(after.state, AgentWakeState::Prepared);
    assert_eq!(after.wait_match_count_snapshot, Some(1));
    assert_eq!(
        db.read_agent_wait(&owner, &wait.wait_id)
            .unwrap()
            .match_count,
        2
    );
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "sealed one-shot Wait must not manufacture a successor model opportunity"
    );

    let cancel = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-sealed-cancel")
        .unwrap_err();
    assert_eq!(cancel.code(), "agent_wait_dispatch_fence_crossed");
    assert_eq!(
        db.read_agent_wait(&owner, &wait.wait_id).unwrap().state,
        AgentWaitState::Triggered
    );
}

#[test]
fn cancellation_is_keyed_and_retires_only_predispatch_wait_wake() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-cancel.db")).unwrap();
    let owner = principal('6');
    let watcher = agent(&db, &owner, "wait-cancel-watcher");
    let worker = agent(&db, &owner, "wait-cancel-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-cancel-view");
    let task_id = task(&db, &owner, &worker, "wait-cancel-task");
    let started = start(&db, &owner, &task_id, &worker, "wait-cancel-task");
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input(&watcher, &endpoint, &[task_id.clone()], "wait-cancel"),
        )
        .unwrap()
        .agent_wait;
    complete(&db, &owner, &task_id, &worker, &started, "wait-cancel-task");
    let wake_id = wait_wake_id(&db, &wait.wait_id);
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    let cancelled = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-cancel-key")
        .unwrap();
    assert_eq!(cancelled.agent_wait.state, AgentWaitState::Cancelled);
    assert_eq!(
        db.agent_wake(&wake_id).unwrap().unwrap().state,
        AgentWakeState::Retired
    );
    assert_eq!(
        db.agent_wake_attempts(&wake_id).unwrap()[0].state,
        super::agent_wake::AgentWakeAttemptState::Revoked
    );
    let replay = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-cancel-key")
        .unwrap();
    assert!(replay.replayed);
    assert!(!replay.state_changed);
    assert_eq!(
        claim.wake.source_wait_id.as_deref(),
        Some(wait.wait_id.as_str())
    );
}

#[test]
fn wait_and_pending_wake_survive_reopen_and_can_resume_on_new_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("wait-restart.db");
    let owner = principal('7');
    let (watcher, worker, wait_id) = {
        let db = Database::open(&path).unwrap();
        let watcher = agent(&db, &owner, "wait-restart-watcher");
        let worker = agent(&db, &owner, "wait-restart-worker");
        let endpoint = endpoint(&db, &owner, &watcher, "wait-restart-view-a");
        let task_id = task(&db, &owner, &worker, "wait-restart-task");
        let started = start(&db, &owner, &task_id, &worker, "wait-restart-task");
        let wait = db
            .create_agent_wait(
                &owner,
                wait_input(&watcher, &endpoint, &[task_id.clone()], "wait-restart"),
            )
            .unwrap()
            .agent_wait;
        complete(
            &db,
            &owner,
            &task_id,
            &worker,
            &started,
            "wait-restart-task",
        );
        (watcher, worker, wait.wait_id)
    };
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.read_agent_wait(&owner, &wait_id).unwrap().state,
        AgentWaitState::Triggered
    );
    let replacement = endpoint(&db, &owner, &watcher, "wait-restart-view-b");
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &replacement.endpoint_id,
            replacement.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    assert_eq!(claim.wake.source_wait_id.as_deref(), Some(wait_id.as_str()));
    let prepared = db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &replacement.endpoint_id,
            replacement.controller_generation,
            &claim.wake.wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
            &claim.consume_token,
        )
        .unwrap();
    assert!(prepared.envelope.resume_hint.contains(&wait_id));
    db.consume_agent_wake(
        &owner,
        &watcher,
        &replacement.endpoint_id,
        replacement.controller_generation,
        &claim.wake.wake_id,
        &claim.consume_token,
    )
    .unwrap();
    assert_eq!(
        db.read_agent_wait(&owner, &wait_id).unwrap().state,
        AgentWaitState::Resumed
    );
    assert!(!worker.is_empty());
}

#[test]
fn wait_creation_enforces_selector_agent_and_endpoint_bounds() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-bounds.db")).unwrap();
    let owner = principal('9');
    let watcher = agent(&db, &owner, "wait-bounds-watcher");
    let worker = agent(&db, &owner, "wait-bounds-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-bounds-view");
    let empty = db
        .create_agent_wait(
            &owner,
            wait_input(&watcher, &endpoint, &[], "wait-bounds-empty"),
        )
        .unwrap_err();
    assert_eq!(empty.code(), "invalid_agent_wait_events");

    let nine = (0..9)
        .map(|index| {
            format!(
                "wc_agent_task_{}",
                webcodex_core::compact::encode(&(index as u128).to_be_bytes()[4..])
            )
        })
        .collect::<Vec<_>>();
    let oversized = db
        .create_agent_wait(
            &owner,
            wait_input(&watcher, &endpoint, &nine, "wait-bounds-nine"),
        )
        .unwrap_err();
    assert_eq!(oversized.code(), "invalid_agent_wait_events");

    let real_task = task(&db, &owner, &worker, "wait-bounds-real");
    let mut stale = wait_input(
        &watcher,
        &endpoint,
        std::slice::from_ref(&real_task),
        "wait-bounds-stale",
    );
    stale.expected_controller_generation += 1;
    assert!(db.create_agent_wait(&owner, stale).is_err());

    for index in 0..MAX_ACTIVE_AGENT_WAITS_PER_AGENT {
        let task_id = task(&db, &owner, &worker, &format!("wait-bounds-agent-{index}"));
        db.create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                std::slice::from_ref(&task_id),
                &format!("wait-bounds-agent-{index}"),
            ),
        )
        .unwrap();
    }
    let overflow_task = task(&db, &owner, &worker, "wait-bounds-agent-overflow");
    let overflow = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                std::slice::from_ref(&overflow_task),
                "wait-bounds-agent-overflow",
            ),
        )
        .unwrap_err();
    assert_eq!(overflow.code(), "agent_wait_agent_capacity_reached");
}

#[test]
fn wait_and_task_wakes_share_the_existing_one_dispatched_wake_fence() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-competition.db")).unwrap();
    let owner = principal('a');
    let watcher = agent(&db, &owner, "wait-competition-watcher");
    let worker = agent(&db, &owner, "wait-competition-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-competition-view");

    let source_task = task(&db, &owner, &worker, "wait-competition-source");
    let source_attempt = start(
        &db,
        &owner,
        &source_task,
        &worker,
        "wait-competition-source",
    );
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                std::slice::from_ref(&source_task),
                "wait-competition-wait",
            ),
        )
        .unwrap()
        .agent_wait;
    complete(
        &db,
        &owner,
        &source_task,
        &worker,
        &source_attempt,
        "wait-competition-source",
    );

    let task_wake_task = task(&db, &owner, &watcher, "wait-competition-task-wake");
    let task_wake_attempt = start(
        &db,
        &owner,
        &task_wake_task,
        &watcher,
        "wait-competition-task-wake",
    );
    db.start_agent_task_endpoint_continuation(
        &owner,
        &task_wake_task,
        &task_wake_attempt.attempt.attempt_id,
        &watcher,
        &task_wake_attempt.attempt_fence,
        task_wake_attempt.attempt.attempt_controller_generation,
    )
    .unwrap();

    let first = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    let second = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    assert_ne!(first.wake.wake_id, second.wake.wake_id);
    assert!(
        [
            first.wake.source_wait_id.as_deref(),
            second.wake.source_wait_id.as_deref()
        ]
        .contains(&Some(wait.wait_id.as_str())),
        "one claimed Wake must be the Wait-origin opportunity"
    );

    db.prepare_agent_wake_dispatch(
        &owner,
        &watcher,
        &endpoint.endpoint_id,
        endpoint.controller_generation,
        &first.wake.wake_id,
        &first.attempt.attempt_id,
        &first.claim_fence,
        &first.consume_token,
    )
    .unwrap();
    assert!(db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &second.wake.wake_id,
            &second.attempt.attempt_id,
            &second.claim_fence,
            &second.consume_token,
        )
        .is_err());
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes
                 WHERE target_agent_id = ?1 AND state IN ('prepared', 'delivered', 'delivery_unknown')",
                [watcher.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "Wait-origin Wakes must reuse the existing Agent-global durable dispatch fence"
    );
}

#[test]
fn source_fanout_is_bounded_at_wait_admission() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-fanout.db")).unwrap();
    let owner = principal('8');
    let worker = agent(&db, &owner, "wait-fanout-worker");
    let task_id = task(&db, &owner, &worker, "wait-fanout-task");
    let watchers = [
        agent(&db, &owner, "wait-fanout-a"),
        agent(&db, &owner, "wait-fanout-b"),
    ];
    let endpoints = [
        endpoint(&db, &owner, &watchers[0], "wait-fanout-a"),
        endpoint(&db, &owner, &watchers[1], "wait-fanout-b"),
    ];
    for index in 0..MAX_AGENT_WAITS_PER_SOURCE {
        let slot = (index as usize) % watchers.len();
        db.create_agent_wait(
            &owner,
            wait_input(
                &watchers[slot],
                &endpoints[slot],
                std::slice::from_ref(&task_id),
                &format!("wait-fanout-{index}"),
            ),
        )
        .unwrap();
    }
    let overflow = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watchers[0],
                &endpoints[0],
                std::slice::from_ref(&task_id),
                "wait-fanout-overflow",
            ),
        )
        .unwrap_err();
    assert_eq!(overflow.code(), "agent_wait_source_capacity_reached");
}
