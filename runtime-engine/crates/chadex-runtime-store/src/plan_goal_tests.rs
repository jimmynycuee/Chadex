use super::goal_run::*;
use super::plan::*;
use super::Database;
use serde_json::json;

const T0: i64 = 1_000_000;
const OWNER: &str = "owner-a";
const PROJECT: &str = "agent:client-1:repo";

fn db() -> (tempfile::TempDir, Database) {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("plans.db")).unwrap();
    (temp, db)
}

fn fresh_db() -> (tempfile::TempDir, Database) {
    db()
}

fn spec() -> PlanSpec {
    PlanSpec {
        acceptance: vec![AcceptanceCheck {
            name: "tests".to_string(),
            argv: vec!["cargo".to_string(), "test".to_string()],
            cwd: None,
            timeout_secs: 600,
            lane: AcceptanceLane::default(),
            kind: AcceptanceKind::default(),
            loopback_ports: vec![],
            container_image: None,
            depends_on: vec![],
            external_gate: None,
        }],
        budget: GoalBudget::default(),
        workspace_requirements: vec![],
        dependencies: vec![],
    }
}

fn binding() -> PlanApprovalBinding {
    PlanApprovalBinding {
        project_id: PROJECT.to_string(),
        runner_client_id: "client-1".to_string(),
        project_root: "/repo".to_string(),
        base_commit: "abc123".to_string(),
    }
}

fn env() -> GoalStartEnvironment {
    GoalStartEnvironment {
        runner_client_id: "client-1".to_string(),
        project_root: "/repo".to_string(),
    }
}

fn started_plan(db: &Database) -> (String, PlanRevisionOutcome) {
    started_plan_with_spec(db, spec())
}

fn started_plan_with_spec(db: &Database, plan_spec: PlanSpec) -> (String, PlanRevisionOutcome) {
    let start = db.plan_start(OWNER, PROJECT, "Add feature", T0).unwrap();
    assert!(start.created);
    assert!(start.plan.plan_mode_active);
    let saved = db
        .plan_save_revision(OWNER, &start.plan.plan_id, 0, "# Plan v1\nstep one", plan_spec, T0 + 1)
        .unwrap();
    (start.plan.plan_id, saved)
}

fn approve(db: &Database, plan_id: &str, saved: &PlanRevisionOutcome, now: i64) -> PlanDecisionRecord {
    db.plan_approve(
        plan_id,
        saved.revision.revision,
        &saved.revision.content_sha256,
        binding(),
        "operator",
        DEFAULT_PLAN_APPROVAL_TTL_MS,
        now,
    )
    .unwrap()
}

fn start_run(db: &Database, plan_id: &str, now: i64) -> Result<GoalRunStartOutcome, PlanStoreError> {
    start_run_provider(db, plan_id, "codex", now)
}

fn start_run_provider(
    db: &Database,
    plan_id: &str,
    provider: &str,
    now: i64,
) -> Result<GoalRunStartOutcome, PlanStoreError> {
    db.goal_run_start(OWNER, plan_id, &env(), &json!({"kind": "test"}), provider, now)
}

fn record_commit_checkpoint(
    db: &Database,
    run_id: &str,
    lease: &GoalLease,
    round: i64,
    now: i64,
) -> String {
    let op_key = format!("commit:{round}@g{}", lease.generation);
    db.goal_op_intent(run_id, lease, &op_key, "commit", round, now).unwrap();
    db.goal_op_finish(run_id, lease, &op_key, GoalOpState::Succeeded, &json!({}), now)
        .unwrap();
    let checkpoint = "a".repeat(40);
    db.goal_run_checkpoint_record(run_id, lease, round, &checkpoint, now)
        .unwrap();
    checkpoint
}

fn record_commit_and_pass(
    db: &Database,
    run_id: &str,
    lease: &GoalLease,
    round: i64,
    now: i64,
) {
    let checkpoint = record_commit_checkpoint(db, run_id, lease, round, now);
    let check = &spec().acceptance[0];
    db.goal_acceptance_receipt_record(
        run_id,
        lease,
        GoalAcceptanceReceiptObservation {
            acceptance_revision: 1,
            checkpoint_round: round,
            checkpoint_sha: checkpoint,
            check_name: check.name.clone(),
            argv_sha256: goal_acceptance_argv_sha256(&check.argv).unwrap(),
            exit_code: Some(0),
            timed_out: false,
            output_sha256: "b".repeat(64),
            observed_at_unix_ms: now,
            external_attestation: None,
        },
    )
    .unwrap();
}

fn external_gate_spec() -> PlanSpec {
    PlanSpec {
        acceptance: vec![AcceptanceCheck {
            name: "release-attestation".to_string(),
            argv: vec!["chadex-gate".to_string(), "release".to_string()],
            cwd: None,
            timeout_secs: 60,
            lane: AcceptanceLane::ExternalAttestation,
            kind: AcceptanceKind::ExternalGate,
            loopback_ports: vec![],
            container_image: None,
            depends_on: vec![],
            external_gate: Some(ExternalGateManifest {
                owner: "release manager".to_string(),
                action: "approve release".to_string(),
                evidence: "signed release record".to_string(),
                next_step: "attach evidence and retry acceptance".to_string(),
            }),
        }],
        budget: GoalBudget::default(),
        workspace_requirements: vec![],
        dependencies: vec![],
    }
}

fn park_run_at_checkpoint(
    db: &Database,
    plan_id: &str,
    now: i64,
) -> (GoalRunRecord, GoalLease, String) {
    let run = start_run(db, plan_id, now).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", now + 1, 30_000, 1).unwrap().remove(0);
    let checkpoint = record_commit_checkpoint(db, &run.goal_run_id, &lease, 1, now + 1);
    let parked = db
        .goal_run_transition(
            &run.goal_run_id,
            &lease,
            GoalRunState::NeedsAttention,
            "acceptance_blocked",
            now + 2,
        )
        .unwrap();
    (parked, lease, checkpoint)
}

fn concurrent_start_pair(
    left_provider: &str,
    right_provider: &str,
) -> (
    (PlanDecisionRecord, GoalRunStartOutcome),
    (PlanDecisionRecord, GoalRunStartOutcome),
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plans.db");
    let db = Database::open(&path).unwrap();
    let saved = db
        .goal_request_create(OWNER, PROJECT, "Concurrent Goal", "Fix add", spec(), &json!({"kind": "test"}), T0)
        .unwrap();
    let plan_id = saved.plan.plan_id.clone();
    let revision = saved.revision.revision;
    let content_sha256 = saved.revision.content_sha256.clone();
    drop(db);

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let left = {
        let path = path.clone();
        let plan_id = plan_id.clone();
        let content_sha256 = content_sha256.clone();
        let barrier = barrier.clone();
        let provider = left_provider.to_string();
        std::thread::spawn(move || {
            let db = Database::open(&path).unwrap();
            barrier.wait();
            let approval = db
                .plan_approve(
                    &plan_id,
                    revision,
                    &content_sha256,
                    binding(),
                    "operator",
                    DEFAULT_PLAN_APPROVAL_TTL_MS,
                    T0 + 2,
                )
                .unwrap();
            let outcome = db
                .goal_run_start(
                    OWNER,
                    &plan_id,
                    &env(),
                    &json!({"mode": provider}),
                    &provider,
                    T0 + 3,
                )
                .unwrap();
            (approval, outcome)
        })
    };
    let right = {
        let path = path.clone();
        let plan_id = plan_id.clone();
        let barrier = barrier.clone();
        let provider = right_provider.to_string();
        std::thread::spawn(move || {
            let db = Database::open(&path).unwrap();
            barrier.wait();
            let approval = db
                .plan_approve(
                    &plan_id,
                    revision,
                    &content_sha256,
                    binding(),
                    "operator",
                    DEFAULT_PLAN_APPROVAL_TTL_MS,
                    T0 + 2,
                )
                .unwrap();
            let outcome = db
                .goal_run_start(
                    OWNER,
                    &plan_id,
                    &env(),
                    &json!({"mode": provider}),
                    &provider,
                    T0 + 3,
                )
                .unwrap();
            (approval, outcome)
        })
    };
    barrier.wait();
    (left.join().unwrap(), right.join().unwrap())
}

#[test]
fn revisions_are_append_only_cas_checked_and_diffable() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    assert_eq!(v1.revision.revision, 1);
    // Stale writer cannot replace a newer plan.
    let stale = db
        .plan_save_revision(OWNER, &plan_id, 0, "other", spec(), T0 + 2)
        .unwrap_err();
    assert_eq!(stale.code(), "plan_revision_changed");
    let v2 = db
        .plan_save_revision(OWNER, &plan_id, 1, "# Plan v2\nstep one\nstep two", spec(), T0 + 3)
        .unwrap();
    assert_eq!(v2.revision.revision, 2);
    assert_ne!(v1.revision.content_sha256, v2.revision.content_sha256);
    // Old revision is still readable byte-for-byte.
    let old = db
        .plan_read(PlanReader::Owner(OWNER), &plan_id, Some(1), T0 + 4)
        .unwrap();
    assert_eq!(old.revision.unwrap().content, "# Plan v1\nstep one");
    // SQLite itself refuses in-place replacement or deletion.
    {
        let conn = db.lock_connection(crate::StoreDomain::Goal);
        let update = conn.execute(
            "UPDATE wc_plan_revisions SET content = 'swapped' WHERE plan_id = ?1 AND revision = 1",
            [&plan_id],
        );
        assert!(update.unwrap_err().to_string().contains("immutable"));
        let delete = conn.execute("DELETE FROM wc_plan_revisions WHERE plan_id = ?1", [&plan_id]);
        assert!(delete.unwrap_err().to_string().contains("immutable"));
    }
    let diff = plan_line_diff("# Plan v1\nstep one", "# Plan v2\nstep one\nstep two").unwrap();
    assert_eq!(diff, vec!["-# Plan v1", "+# Plan v2", " step one", "+step two"]);
    // Another principal cannot see the plan.
    let hidden = db
        .plan_read(PlanReader::Owner("owner-b"), &plan_id, None, T0 + 5)
        .unwrap_err();
    assert_eq!(hidden.code(), "plan_not_found");
}

#[test]
fn approval_binds_exact_revision_and_hash_and_new_revision_invalidates_it() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    // Wrong hash or stale revision is refused.
    let wrong = db
        .plan_approve(&plan_id, 1, "deadbeef", binding(), "operator", 60_000, T0 + 2)
        .unwrap_err();
    assert_eq!(wrong.code(), "plan_content_hash_mismatch");
    approve(&db, &plan_id, &v1, T0 + 3);
    let approved = db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 4).unwrap();
    assert_eq!(approved.plan.state, PlanState::Approved);
    assert!(!approved.plan.plan_mode_active, "approval releases the read-only lock");

    // v1 approved, then v2 saved: the v1 approval must not carry over.
    let v2 = db
        .plan_save_revision(OWNER, &plan_id, 1, "# Plan v2", spec(), T0 + 5)
        .unwrap();
    assert_eq!(v2.superseded_approvals, 1);
    assert!(v2.plan.plan_mode_active, "editing re-engages Plan Mode");
    let refused = start_run(&db, &plan_id, T0 + 6).unwrap_err();
    assert_eq!(refused.code(), "plan_not_approved");
    // Approving the stale v1 is refused as well.
    let stale = db
        .plan_approve(&plan_id, 1, &v1.revision.content_sha256, binding(), "operator", 60_000, T0 + 7)
        .unwrap_err();
    assert_eq!(stale.code(), "plan_revision_stale");
    approve(&db, &plan_id, &v2, T0 + 8);
    let run = start_run(&db, &plan_id, T0 + 9).unwrap();
    assert!(run.created);
    assert_eq!(run.run.revision, 2);
    assert_eq!(run.run.content_sha256, v2.revision.content_sha256);
}

#[test]
fn approval_requires_acceptance_and_matching_project() {
    let (_temp, db) = db();
    let start = db.plan_start(OWNER, PROJECT, "No checks", T0).unwrap();
    let saved = db
        .plan_save_revision(OWNER, &start.plan.plan_id, 0, "plan", PlanSpec::default(), T0 + 1)
        .unwrap();
    let err = db
        .plan_approve(
            &start.plan.plan_id,
            1,
            &saved.revision.content_sha256,
            binding(),
            "operator",
            60_000,
            T0 + 2,
        )
        .unwrap_err();
    assert_eq!(err.code(), "plan_missing_acceptance");

    let (plan_id, v1) = {
        let other = db.plan_start("owner-c", PROJECT, "x", T0).unwrap();
        let saved = db
            .plan_save_revision("owner-c", &other.plan.plan_id, 0, "plan", spec(), T0 + 1)
            .unwrap();
        (other.plan.plan_id, saved)
    };
    let mut wrong = binding();
    wrong.project_id = "agent:client-1:other".to_string();
    let err = db
        .plan_approve(&plan_id, 1, &v1.revision.content_sha256, wrong, "operator", 60_000, T0 + 2)
        .unwrap_err();
    assert_eq!(err.code(), "plan_project_mismatch");
}

#[test]
fn approval_is_single_use_expiring_revocable_and_environment_bound() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    // Duplicate start replays the same run instead of creating a second one.
    let first = start_run(&db, &plan_id, T0 + 3).unwrap();
    let again = start_run(&db, &plan_id, T0 + 4).unwrap();
    assert!(first.created);
    assert!(!again.created);
    assert_eq!(first.run.goal_run_id, again.run.goal_run_id);
    // Another principal can neither start nor replay it.
    let foreign = db
        .goal_run_start("owner-b", &plan_id, &env(), &json!({}), "codex", T0 + 5)
        .unwrap_err();
    assert_eq!(foreign.code(), "plan_not_found");

    // Revoked approval cannot start.
    let (_temp2, db2) = self::db();
    let (plan_id2, v1b) = started_plan(&db2);
    let decision = approve(&db2, &plan_id2, &v1b, T0 + 2);
    db2.plan_revoke_approval(&decision.approval_id, T0 + 3).unwrap();
    assert_eq!(start_run(&db2, &plan_id2, T0 + 4).unwrap_err().code(), "plan_not_approved");

    // Expired approval cannot start.
    let decision = approve(&db2, &plan_id2, &v1b, T0 + 5);
    let after_expiry = decision.expires_at_unix_ms.unwrap() + 1;
    assert_eq!(
        start_run(&db2, &plan_id2, after_expiry).unwrap_err().code(),
        "plan_not_approved"
    );

    // Runner/root changed since approval.
    approve(&db2, &plan_id2, &v1b, after_expiry + 1);
    let changed = db2
        .goal_run_start(
            OWNER,
            &plan_id2,
            &GoalStartEnvironment {
                runner_client_id: "client-2".to_string(),
                project_root: "/repo".to_string(),
            },
            &json!({}),
            "codex",
            after_expiry + 2,
        )
        .unwrap_err();
    assert_eq!(changed.code(), "plan_approval_environment_changed");
}

#[test]
fn reapproving_same_revision_does_not_create_a_second_goal_run() {
    let (_temp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    let first_approval = approve(&db, &plan_id, &saved, T0 + 2);
    let first = db
        .goal_run_start(
            OWNER,
            &plan_id,
            &env(),
            &json!({"mode": "codex", "attempt": 1}),
            "codex",
            T0 + 3,
        )
        .unwrap();
    assert!(first.created);

    // A repeated Start click issues a fresh approval_id. It must not bypass
    // idempotency for the same plan revision and content hash.
    let second_approval = approve(&db, &plan_id, &saved, T0 + 4);
    assert_eq!(second_approval.approval_id, first_approval.approval_id);
    let replay = db
        .goal_run_start(
            OWNER,
            &plan_id,
            &env(),
            &json!({"mode": "chatgpt", "attempt": 2}),
            "chatgpt",
            T0 + 5,
        )
        .unwrap();

    assert!(!replay.created, "same revision must replay the original run");
    assert_eq!(replay.run.goal_run_id, first.run.goal_run_id);
    assert_eq!(replay.run.approval_id, first_approval.approval_id);
    assert_eq!(replay.run.provider, "codex");
    assert_eq!(replay.run.dispatch_auth, json!({"mode": "codex", "attempt": 1}));
    assert_eq!(replay.run.binding, binding());

    let mut changed_binding = binding();
    changed_binding.base_commit = "different-commit".to_string();
    let changed_approval = db
        .plan_approve(
            &plan_id,
            saved.revision.revision,
            &saved.revision.content_sha256,
            changed_binding,
            "operator",
            DEFAULT_PLAN_APPROVAL_TTL_MS,
            T0 + 5,
        )
        .unwrap_err();
    assert_eq!(changed_approval.code(), "plan_approval_environment_changed");
    let changed_environment = db
        .goal_run_start(
            OWNER,
            &plan_id,
            &GoalStartEnvironment {
                runner_client_id: "client-2".to_string(),
                project_root: "/repo".to_string(),
            },
            &json!({}),
            "chatgpt",
            T0 + 5,
        )
        .unwrap_err();
    assert_eq!(changed_environment.code(), "plan_approval_environment_changed");
    assert_eq!(
        db.goal_run_start("owner-b", &plan_id, &env(), &json!({}), "codex", T0 + 5)
            .unwrap_err()
            .code(),
        "plan_not_found"
    );

    let detail = db
        .plan_read(PlanReader::Operator, &plan_id, None, T0 + 6)
        .unwrap();
    let original = detail
        .decisions
        .iter()
        .find(|decision| decision.approval_id == first_approval.approval_id)
        .unwrap();
    assert_eq!(original.state, PlanDecisionState::Consumed);
    assert_eq!(
        original.consumed_by_goal_run_id.as_deref(),
        Some(first.run.goal_run_id.as_str())
    );
    let duplicate = detail
        .decisions
        .iter()
        .find(|decision| decision.approval_id == second_approval.approval_id)
        .unwrap();
    assert_eq!(duplicate.state, PlanDecisionState::Consumed);
    assert_eq!(
        duplicate.consumed_by_goal_run_id.as_deref(),
        Some(first.run.goal_run_id.as_str())
    );

    let conn = db.lock_connection(crate::StoreDomain::Goal);
    let run_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wc_goal_runs WHERE plan_id = ?1 AND revision = ?2 AND content_sha256 = ?3",
            rusqlite::params![plan_id, saved.revision.revision, saved.revision.content_sha256],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run_count, 1);
}

#[test]
fn a_new_revision_can_start_a_distinct_goal_run() {
    let (_temp, db) = db();
    let (plan_id, first_revision) = started_plan(&db);
    approve(&db, &plan_id, &first_revision, T0 + 2);
    let first = start_run(&db, &plan_id, T0 + 3).unwrap();

    let second_revision = db
        .plan_save_revision(OWNER, &plan_id, 1, "# Plan v2\nstep two", spec(), T0 + 4)
        .unwrap();
    approve(&db, &plan_id, &second_revision, T0 + 5);
    let second = start_run(&db, &plan_id, T0 + 6).unwrap();

    assert!(first.created && second.created);
    assert_ne!(first.run.goal_run_id, second.run.goal_run_id);
    assert_eq!(second.run.revision, 2);
    assert_eq!(second.run.content_sha256, second_revision.revision.content_sha256);
}

#[test]
fn direct_goal_reapproval_replaces_an_expired_active_approval() {
    let (_temp, db) = db();
    let saved = db
        .goal_request_create(OWNER, PROJECT, "Goal expiry", "Fix add", spec(), &json!({}), T0)
        .unwrap();
    let plan_id = saved.plan.plan_id.clone();
    let first = approve(&db, &plan_id, &saved, T0 + 1);
    let retry = approve(&db, &plan_id, &saved, first.expires_at_unix_ms.unwrap());
    assert_ne!(first.approval_id, retry.approval_id);
    assert_eq!(retry.state, PlanDecisionState::Active);
    let run = start_run(&db, &plan_id, retry.created_at_unix_ms + 1).unwrap();
    assert!(run.created);
    assert_eq!(run.run.approval_id, retry.approval_id);
}

#[test]
fn concurrent_starts_are_single_flight_for_same_and_mixed_modes() {
    for (left_mode, right_mode) in [("codex", "codex"), ("codex", "chatgpt")] {
        let ((left_approval, left), (right_approval, right)) =
            concurrent_start_pair(left_mode, right_mode);
        assert_eq!(left_approval.approval_id, right_approval.approval_id);
        assert_eq!(usize::from(left.created) + usize::from(right.created), 1);
        assert_eq!(left.run.goal_run_id, right.run.goal_run_id);
        assert_eq!(left.run.provider, right.run.provider);
        assert_eq!(left.run.dispatch_auth, right.run.dispatch_auth);
    }
}

#[test]
fn reopened_reapproval_replays_the_persisted_goal_without_new_approval() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plans.db");
    let db = Database::open(&path).unwrap();
    let (plan_id, saved) = started_plan(&db);
    let approval = approve(&db, &plan_id, &saved, T0 + 2);
    let first = db
        .goal_run_start(
            OWNER,
            &plan_id,
            &env(),
            &json!({"mode": "codex", "session": 1}),
            "codex",
            T0 + 3,
        )
        .unwrap();
    drop(db);

    let db = Database::open(&path).unwrap();
    let replayed_approval = approve(&db, &plan_id, &saved, T0 + 4);
    let replay = db
        .goal_run_start(
            OWNER,
            &plan_id,
            &env(),
            &json!({"mode": "chatgpt", "session": 2}),
            "chatgpt",
            T0 + 5,
        )
        .unwrap();
    assert_eq!(replayed_approval.approval_id, approval.approval_id);
    assert!(!replay.created);
    assert_eq!(replay.run.goal_run_id, first.run.goal_run_id);
    assert_eq!(replay.run.approval_id, approval.approval_id);
    assert_eq!(replay.run.provider, "codex");
    assert_eq!(replay.run.dispatch_auth, json!({"mode": "codex", "session": 1}));
    assert_eq!(db.goal_run_list(PlanReader::Operator, 20).unwrap().len(), 1);
}

#[test]
fn competing_approvals_leave_exactly_one_active() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    let first = approve(&db, &plan_id, &v1, T0 + 2);
    let second = approve(&db, &plan_id, &v1, T0 + 3);
    let detail = db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 4).unwrap();
    let active: Vec<_> = detail
        .decisions
        .iter()
        .filter(|decision| decision.state == PlanDecisionState::Active)
        .collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].approval_id, second.approval_id);
    assert!(detail
        .decisions
        .iter()
        .any(|d| d.approval_id == first.approval_id && d.state == PlanDecisionState::Superseded));
}

#[test]
fn plan_lock_is_per_owner_expires_and_is_released_only_by_operator_paths() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    let locks = db.plan_active_locks(T0 + 2).unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].owner_key, OWNER);
    // Same owner, other project: refused rather than silently switching.
    let other = db
        .plan_start(OWNER, "agent:client-1:other", "x", T0 + 3)
        .unwrap_err();
    assert_eq!(other.code(), "plan_mode_active_for_other_project");
    // Same owner, same project: idempotent.
    let again = db.plan_start(OWNER, PROJECT, "again", T0 + 4).unwrap();
    assert!(!again.created);
    assert_eq!(again.plan.plan_id, plan_id);
    // A different owner is unaffected.
    assert!(db.plan_start("owner-b", PROJECT, "b", T0 + 5).unwrap().created);
    // Rejection keeps the lock.
    db.plan_reject(&plan_id, 1, &v1.revision.content_sha256, "operator", "needs tests", T0 + 6)
        .unwrap();
    assert!(db
        .plan_active_locks(T0 + 7)
        .unwrap()
        .iter()
        .any(|lock| lock.owner_key == OWNER));
    // Closing releases it.
    db.plan_close(&plan_id, "exit", T0 + 8).unwrap();
    assert!(!db
        .plan_active_locks(T0 + 9)
        .unwrap()
        .iter()
        .any(|lock| lock.owner_key == OWNER));
    // Expiry releases it.
    let next = db.plan_start(OWNER, PROJECT, "next", T0 + 10).unwrap();
    let expired_at = next.plan.plan_mode_expires_at_unix_ms.unwrap();
    assert!(db
        .plan_active_locks(expired_at)
        .unwrap()
        .iter()
        .all(|lock| lock.owner_key != OWNER));
}

#[test]
fn lease_fencing_rejects_stale_supervisor_and_op_journal_blocks_redispatch() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let claimed = db.goal_run_claim("sup-a", T0 + 4, 30_000, 2).unwrap();
    assert_eq!(claimed.len(), 1);
    let lease_a = claimed[0].1.clone();
    // A concurrent supervisor gets nothing while the lease is live.
    assert!(db.goal_run_claim("sup-b", T0 + 5, 30_000, 2).unwrap().is_empty());
    db.goal_op_intent(&run.goal_run_id, &lease_a, "agent:1", "agent", 1, T0 + 6)
        .unwrap();
    // Re-recording the same op is refused: the caller must reconcile.
    assert_eq!(
        db.goal_op_intent(&run.goal_run_id, &lease_a, "agent:1", "agent", 1, T0 + 7)
            .unwrap_err()
            .code(),
        "goal_op_exists"
    );
    // Lease expires; another supervisor takes over with a higher fence.
    let claimed_b = db.goal_run_claim("sup-b", T0 + 40_000, 30_000, 2).unwrap();
    assert_eq!(claimed_b.len(), 1);
    let lease_b = claimed_b[0].1.clone();
    assert!(lease_b.generation > lease_a.generation);
    // The stale holder can no longer write anything.
    for err in [
        db.goal_op_dispatched(&run.goal_run_id, &lease_a, "agent:1", "job-1"),
        db.goal_run_renew_lease(&run.goal_run_id, &lease_a, T0 + 40_001, 30_000),
        db.goal_run_progress(&run.goal_run_id, &lease_a, 1, 0, None, None, T0 + 40_002),
        db.goal_run_transition(&run.goal_run_id, &lease_a, GoalRunState::Succeeded, "x", T0 + 40_003)
            .map(|_| ()),
    ] {
        assert_eq!(err.unwrap_err().code(), "goal_lease_lost");
    }
    // The new holder sees the unresolved intent from the previous holder.
    let op = db.goal_op_get(&run.goal_run_id, "agent:1").unwrap().unwrap();
    assert_eq!(op.state, GoalOpState::Intent);
    db.goal_op_finish(&run.goal_run_id, &lease_b, "agent:1", GoalOpState::Unknown, &json!({}), T0 + 40_004)
        .unwrap();
    let parked = db
        .goal_run_transition(
            &run.goal_run_id,
            &lease_b,
            GoalRunState::NeedsAttention,
            "outcome_unknown",
            T0 + 40_005,
        )
        .unwrap();
    assert_eq!(parked.state, GoalRunState::NeedsAttention);
    assert!(parked.lease_owner.is_none());
    // Owner may not resume an unknown outcome; the operator may.
    assert_eq!(
        db.goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Resume, T0 + 40_006)
            .unwrap_err()
            .code(),
        "goal_resume_not_allowed"
    );
    let resumed = db
        .goal_run_control(PlanReader::Operator, &run.goal_run_id, GoalControlAction::Resume, T0 + 40_007)
        .unwrap();
    assert_eq!(resumed.state, GoalRunState::Queued);
}

#[test]
fn pause_cancel_and_emergency_stop() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    // Queued run pauses directly.
    let paused = db
        .goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Pause, T0 + 4)
        .unwrap();
    assert_eq!(paused.state, GoalRunState::Paused);
    assert!(db.goal_run_claim("sup", T0 + 5, 30_000, 2).unwrap().is_empty());
    db.goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Resume, T0 + 6)
        .unwrap();
    let (_, lease) = db.goal_run_claim("sup", T0 + 7, 30_000, 2).unwrap().remove(0);
    // Running run: cancel becomes a request the supervisor must honour.
    let requested = db
        .goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Cancel, T0 + 8)
        .unwrap();
    assert_eq!(requested.control_request.as_deref(), Some("cancel"));
    assert_eq!(requested.state, GoalRunState::Running);
    // Emergency stop: blocks claims and new starts until released.
    assert_eq!(db.goal_kill_switch_set(true, "panic", T0 + 9).unwrap(), 1);
    assert!(db.goal_kill_switch_engaged().unwrap());
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::Stopped, "emergency_stop", T0 + 10)
        .unwrap();
    let (plan2, v2) = {
        let start = db.plan_start("owner-z", PROJECT, "z", T0).unwrap();
        let saved = db
            .plan_save_revision("owner-z", &start.plan.plan_id, 0, "z", spec(), T0 + 1)
            .unwrap();
        (start.plan.plan_id, saved)
    };
    approve(&db, &plan2, &v2, T0 + 11);
    let blocked = db
        .goal_run_start("owner-z", &plan2, &env(), &json!({}), "codex", T0 + 12)
        .unwrap_err();
    assert_eq!(blocked.code(), "goal_emergency_stop_engaged");
    db.goal_kill_switch_set(false, "", T0 + 13).unwrap();
    assert!(db
        .goal_run_start("owner-z", &plan2, &env(), &json!({}), "codex", T0 + 14)
        .unwrap()
        .created);
    let detail = db.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap();
    assert_eq!(detail.run.state, GoalRunState::Stopped);
    assert!(detail.events.iter().any(|event| event.kind == "emergency_stop"));
}

#[test]
fn review_h1_owner_cannot_launder_needs_attention_through_pause() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::NeedsAttention, "outcome_unknown", T0 + 5)
        .unwrap();
    for reader in [PlanReader::Owner(OWNER), PlanReader::Operator] {
        assert_eq!(
            db.goal_run_control(reader, &run.goal_run_id, GoalControlAction::Pause, T0 + 6)
                .unwrap_err()
                .code(),
            "goal_pause_not_allowed"
        );
    }
    assert_eq!(
        db.goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Resume, T0 + 7)
            .unwrap_err()
            .code(),
        "goal_resume_not_allowed"
    );
    assert!(db.goal_run_claim("sup", T0 + 8, 30_000, 1).unwrap().is_empty());
}

#[test]
fn review_m1_requesting_changes_withdraws_approval_and_relocks() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    db.plan_reject(&plan_id, 1, &v1.revision.content_sha256, "operator", "not yet", T0 + 3)
        .unwrap();
    let detail = db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 4).unwrap();
    assert_eq!(detail.plan.state, PlanState::Drafting);
    assert!(detail.plan.plan_mode_active);
    assert!(detail.decisions.iter().all(|d| d.state != PlanDecisionState::Active));
    assert_eq!(start_run(&db, &plan_id, T0 + 5).unwrap_err().code(), "plan_not_approved");
}

#[test]
fn review_l1_pending_cancel_wins_over_parking() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    db.goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Cancel, T0 + 5)
        .unwrap();
    let parked = db
        .goal_run_transition(&run.goal_run_id, &lease, GoalRunState::NeedsAttention, "no_progress", T0 + 6)
        .unwrap();
    assert_eq!(parked.state, GoalRunState::Cancelled);
}

#[test]
fn review_m5_claim_skips_runs_this_process_still_drives() {
    let (_temp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 10, 1).unwrap().remove(0);
    // Lease expired (sleep) but this process still drives it: not re-claimed.
    let again = db
        .goal_run_claim_excluding("sup", T0 + 100, 10, 1, &[run.goal_run_id.clone()])
        .unwrap();
    assert!(again.is_empty());
    // The original driver can still renew after waking up.
    db.goal_run_renew_lease(&run.goal_run_id, &lease, T0 + 101, 30_000).unwrap();
}

#[test]
fn direct_goal_request_never_locks_needs_acceptance_and_starts_only_after_operator_approval() {
    let (_temp, db) = db();
    let dispatch = json!({"kind": "test"});
    let no_checks = PlanSpec {
        acceptance: vec![],
        budget: GoalBudget::default(),
        workspace_requirements: vec![],
        dependencies: vec![],
    };
    let missing = db
        .goal_request_create(OWNER, PROJECT, "Fix add", "make add add", no_checks, &dispatch, T0)
        .unwrap_err();
    assert_eq!(missing.code(), "plan_missing_acceptance");

    let first = db
        .goal_request_create(OWNER, PROJECT, "Fix add", "make add add", spec(), &dispatch, T0)
        .unwrap();
    assert_eq!(first.plan.origin, "goal");
    assert_eq!(first.revision.revision, 1);
    // A direct Goal request never makes the connection read-only.
    assert!(!first.plan.plan_mode_active);
    assert!(db.plan_active_locks(T0 + 1).unwrap().is_empty());
    // The model cannot start it, and cannot revise it into something else.
    assert_eq!(
        start_run(&db, &first.plan.plan_id, T0 + 1).unwrap_err().code(),
        "plan_not_approved"
    );
    assert_eq!(
        db.plan_save_revision(OWNER, &first.plan.plan_id, 1, "other", spec(), T0 + 1)
            .unwrap_err()
            .code(),
        "goal_request_immutable"
    );

    // Sending it back for changes would lock the owner: refused.
    assert_eq!(
        db.plan_reject(&first.plan.plan_id, 1, &first.revision.content_sha256, "operator", "no", T0 + 1)
            .unwrap_err()
            .code(),
        "goal_request_immutable"
    );
    assert!(db.plan_active_locks(T0 + 1).unwrap().is_empty());
    // A newer request replaces the unstarted one.
    let second = db
        .goal_request_create(OWNER, PROJECT, "Fix add", "make add add, keep API", spec(), &dispatch, T0 + 2)
        .unwrap();
    let old = db
        .plan_read(PlanReader::Owner(OWNER), &first.plan.plan_id, None, T0 + 3)
        .unwrap();
    assert_eq!(old.plan.state, PlanState::Closed);

    let context = db.plan_start_context(&second.plan.plan_id, T0 + 3).unwrap();
    assert_eq!(context.owner_key, OWNER);
    assert_eq!(context.dispatch, Some(dispatch));
    assert_eq!(context.dispatch_recorded_at_unix_ms, Some(T0 + 2));
    assert_eq!(context.owner_locked_plan, None);
    approve(&db, &second.plan.plan_id, &second, T0 + 4);
    let run = start_run(&db, &second.plan.plan_id, T0 + 5).unwrap();
    assert!(run.created);
    assert_eq!(run.run.state, GoalRunState::Queued);
}

#[test]
fn direct_goal_request_is_refused_while_plan_mode_is_active() {
    let (_temp, db) = db();
    let _ = started_plan(&db);
    let error = db
        .goal_request_create(OWNER, PROJECT, "Fix add", "x", spec(), &json!({}), T0 + 2)
        .unwrap_err();
    assert_eq!(error.code(), "plan_mode_active_for_other_plan");
}

#[test]
fn plan_dispatch_is_owner_only_and_never_outlives_its_revision() {
    let (_temp, db) = db();
    let (plan_id, _) = started_plan(&db);
    assert_eq!(db.plan_start_context(&plan_id, T0 + 2).unwrap().dispatch, None);
    assert_eq!(
        db.plan_record_dispatch("someone-else", &plan_id, 1, Some(&json!({"kind": "x"})), T0 + 2)
            .unwrap_err()
            .code(),
        "plan_not_found"
    );
    db.plan_record_dispatch(OWNER, &plan_id, 1, Some(&json!({"kind": "test"})), T0 + 2)
        .unwrap();
    let context = db.plan_start_context(&plan_id, T0 + 3).unwrap();
    assert_eq!(context.dispatch, Some(json!({"kind": "test"})));
    // Its own Plan Mode lock is not "another plan".
    assert_eq!(context.owner_locked_plan, None);
    // A newer revision saved without a fresh principal does not inherit it.
    db.plan_save_revision(OWNER, &plan_id, 1, "v2", spec(), T0 + 4).unwrap();
    assert_eq!(db.plan_start_context(&plan_id, T0 + 5).unwrap().dispatch, None);
    // Recording for a stale revision is a no-op; clearing works.
    db.plan_record_dispatch(OWNER, &plan_id, 1, Some(&json!({"kind": "old"})), T0 + 6).unwrap();
    assert_eq!(db.plan_start_context(&plan_id, T0 + 7).unwrap().dispatch, None);
    db.plan_record_dispatch(OWNER, &plan_id, 2, Some(&json!({"kind": "new"})), T0 + 8).unwrap();
    db.plan_record_dispatch(OWNER, &plan_id, 2, None, T0 + 9).unwrap();
    assert_eq!(db.plan_start_context(&plan_id, T0 + 10).unwrap().dispatch, None);
}

#[test]
fn start_context_reports_the_owner_planning_another_plan() {
    let (_temp, db) = db();
    let request = db
        .goal_request_create(OWNER, PROJECT, "Fix add", "x", spec(), &json!({"kind": "t"}), T0)
        .unwrap();
    let (other, _) = started_plan(&db);
    let context = db.plan_start_context(&request.plan.plan_id, T0 + 3).unwrap();
    assert_eq!(context.owner_locked_plan, Some(other));
}

#[test]
fn a_succeeded_goal_closes_its_plan_but_a_failed_one_keeps_it_open() {
    let (_tmp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 2).unwrap().remove(0);
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::Failed, "budget", T0 + 5)
        .unwrap();
    let plan = db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 6).unwrap().plan;
    assert_ne!(plan.state, PlanState::Closed);

    // Revise, approve again and succeed: the plan is done.
    let v2 = db
        .plan_save_revision(OWNER, &plan_id, 1, "plan v2", spec(), T0 + 7)
        .unwrap();
    approve(&db, &plan_id, &v2, T0 + 8);
    let run = start_run(&db, &plan_id, T0 + 9).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 10, 30_000, 2).unwrap().remove(0);
    record_commit_and_pass(&db, &run.goal_run_id, &lease, 1, T0 + 10);
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::Succeeded, "acceptance_passed", T0 + 11)
        .unwrap();
    let plan = db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 12).unwrap().plan;
    assert_eq!(plan.state, PlanState::Closed);
    assert!(!plan.plan_mode_active);
    let open = db.plan_list(PlanReader::Operator, false, 50, T0 + 12).unwrap();
    assert!(open.iter().all(|summary| summary.plan_id != plan_id));
}

#[test]
fn only_a_finished_goal_run_can_be_removed_from_the_lists() {
    let (_tmp, db) = db();
    let (plan_id, v1) = started_plan(&db);
    approve(&db, &plan_id, &v1, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let refused = db.goal_run_dismiss(&run.goal_run_id, T0 + 4).unwrap_err();
    assert_eq!(refused.code(), "goal_run_not_finished");
    let (_, lease) = db.goal_run_claim("sup", T0 + 5, 30_000, 2).unwrap().remove(0);
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::Failed, "budget", T0 + 6)
        .unwrap();
    db.goal_run_dismiss(&run.goal_run_id, T0 + 7).unwrap();
    assert!(db.goal_run_list(PlanReader::Operator, 30).unwrap().is_empty());
    // The model still finds it, to report the result.
    assert_eq!(db.goal_run_list(PlanReader::Owner(OWNER), 30).unwrap().len(), 1);
    // Still readable by id, with the removal audited.
    let detail = db.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap();
    assert_eq!(detail.run.state, GoalRunState::Failed);
    let missing = db.goal_run_dismiss("wc_grun_missing", T0 + 8).unwrap_err();
    assert_eq!(missing.code(), "goal_run_not_found");
}

#[test]
fn each_project_keeps_only_its_newest_plan_and_run() {
    let (_tmp, db) = db();
    let (old_plan, v1) = started_plan(&db);
    let old_approval = approve(&db, &old_plan, &v1, T0 + 2);
    let run = start_run(&db, &old_plan, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 2).unwrap().remove(0);
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::Failed, "budget", T0 + 5)
        .unwrap();

    // Another principal's plan on the project is not touched by A's.
    let foreign = db.plan_start("owner-b", PROJECT, "foreign", T0 + 6).unwrap();
    assert_ne!(
        db.plan_read(PlanReader::Operator, &old_plan, None, T0 + 6).unwrap().plan.state,
        PlanState::Closed
    );
    db.plan_close(&foreign.plan.plan_id, "test", T0 + 6).unwrap();
    // The same principal's new plan replaces its old one.
    let newer = db.plan_start(OWNER, PROJECT, "newer", T0 + 6).unwrap();
    assert!(newer.created);
    let open: Vec<_> = db
        .plan_list(PlanReader::Operator, false, 50, T0 + 7)
        .unwrap()
        .into_iter()
        .map(|plan| plan.plan_id)
        .collect();
    assert_eq!(open, vec![newer.plan.plan_id.clone()]);
    let old = db.plan_read(PlanReader::Operator, &old_plan, None, T0 + 7).unwrap();
    assert_eq!(old.plan.state, PlanState::Closed);
    assert!(old
        .decisions
        .iter()
        .all(|decision| decision.approval_id != old_approval.approval_id
            || decision.state != PlanDecisionState::Active));

    // Another project is untouched.
    let other = db.plan_start("owner-c", "agent:client-1:other", "other", T0 + 8).unwrap();
    let other_saved = db
        .plan_save_revision("owner-c", &other.plan.plan_id, 0, "other project plan", spec(), T0 + 9)
        .unwrap();
    let mut other_binding = binding();
    other_binding.project_id = other.plan.project_id.clone();
    let other_approval = db
        .plan_approve(
            &other.plan.plan_id,
            other_saved.revision.revision,
            &other_saved.revision.content_sha256,
            other_binding,
            "operator",
            DEFAULT_PLAN_APPROVAL_TTL_MS,
            T0 + 10,
        )
        .unwrap();
    assert_eq!(db.plan_list(PlanReader::Operator, false, 50, T0 + 9).unwrap().len(), 2);

    // Starting a new run hides the project's earlier finished runs.
    let saved = db
        .plan_save_revision(OWNER, &newer.plan.plan_id, 0, "plan", spec(), T0 + 10)
        .unwrap();
    approve(&db, &newer.plan.plan_id, &saved, T0 + 11);
    let started = db
        .goal_run_start(OWNER, &newer.plan.plan_id, &env(), &json!({}), "codex", T0 + 12)
        .unwrap()
        .run;
    let runs: Vec<_> = db
        .goal_run_list(PlanReader::Operator, 30)
        .unwrap()
        .into_iter()
        .map(|run| run.goal_run_id)
        .collect();
    assert_eq!(runs, vec![started.goal_run_id.clone()]);

    // Clearing the board for the project closes its plan, keeps the
    // unfinished run, and leaves the other project alone.
    let (plans, dismissed) = db.plan_goal_clear_board(Some(PROJECT), T0 + 13).unwrap();
    assert_eq!((plans, dismissed), (1, 0));
    let open: Vec<_> = db
        .plan_list(PlanReader::Operator, false, 50, T0 + 14)
        .unwrap()
        .into_iter()
        .map(|plan| plan.plan_id)
        .collect();
    assert_eq!(open, vec![other.plan.plan_id.clone()]);
    assert_eq!(db.goal_run_list(PlanReader::Operator, 30).unwrap().len(), 1);
    assert!(db.plan_active_locks(T0 + 14).unwrap().iter().all(|lock| lock.project_id != PROJECT));
    let other_detail = db
        .plan_read(PlanReader::Operator, &other.plan.plan_id, None, T0 + 14)
        .unwrap();
    assert!(other_detail.decisions.iter().any(|decision| {
        decision.approval_id == other_approval.approval_id
            && decision.state == PlanDecisionState::Active
    }));
    assert!(db
        .goal_run_start("owner-c", &other.plan.plan_id, &env(), &json!({}), "codex", T0 + 14)
        .unwrap()
        .created);

    // Clearing everything closes the rest.
    db.plan_goal_clear_board(None, T0 + 15).unwrap();
    assert!(db.plan_list(PlanReader::Operator, false, 50, T0 + 16).unwrap().is_empty());
}

#[test]
fn database_reopen_preserves_plan_mode_guard_and_exact_active_approval() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plans.db");
    let db = Database::open(&path).unwrap();
    let (plan_id, saved) = started_plan(&db);

    drop(db);
    let db = Database::open(&path).unwrap();
    let locks = db.plan_active_locks(T0 + 2).unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].owner_key, OWNER);
    assert_eq!(locks[0].project_id, PROJECT);
    assert_eq!(locks[0].plan_id, plan_id);

    let approval = approve(&db, &plan_id, &saved, T0 + 3);
    drop(db);

    let db = Database::open(&path).unwrap();
    let detail = db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 4).unwrap();
    let persisted = detail
        .decisions
        .iter()
        .find(|decision| decision.approval_id == approval.approval_id)
        .unwrap();
    assert_eq!(persisted.revision, saved.revision.revision);
    assert_eq!(persisted.content_sha256, saved.revision.content_sha256);
    assert_eq!(persisted.binding, Some(binding()));
    assert_eq!(persisted.state, PlanDecisionState::Active);
    assert!(db.plan_active_locks(T0 + 4).unwrap().is_empty());
    assert!(db
        .goal_run_start(OWNER, &plan_id, &env(), &json!({}), "codex", T0 + 5)
        .unwrap()
        .created);
}

#[test]
fn reopened_revoked_expired_superseded_and_wrong_owner_approvals_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plans.db");
    let db = Database::open(&path).unwrap();

    let revoked_start = db.plan_start("owner-revoked", PROJECT, "revoked", T0).unwrap();
    let revoked_saved = db
        .plan_save_revision("owner-revoked", &revoked_start.plan.plan_id, 0, "revoked", spec(), T0 + 1)
        .unwrap();
    let revoked = db
        .plan_approve(
            &revoked_start.plan.plan_id,
            1,
            &revoked_saved.revision.content_sha256,
            binding(),
            "operator",
            DEFAULT_PLAN_APPROVAL_TTL_MS,
            T0 + 2,
        )
        .unwrap();
    db.plan_revoke_approval(&revoked.approval_id, T0 + 3).unwrap();

    let expired_start = db.plan_start("owner-expired", PROJECT, "expired", T0 + 4).unwrap();
    let expired_saved = db
        .plan_save_revision("owner-expired", &expired_start.plan.plan_id, 0, "expired", spec(), T0 + 5)
        .unwrap();
    let expired = db
        .plan_approve(
            &expired_start.plan.plan_id,
            1,
            &expired_saved.revision.content_sha256,
            binding(),
            "operator",
            60_000,
            T0 + 6,
        )
        .unwrap();

    let superseded_start = db.plan_start("owner-superseded", PROJECT, "superseded", T0 + 7).unwrap();
    let superseded_saved = db
        .plan_save_revision("owner-superseded", &superseded_start.plan.plan_id, 0, "v1", spec(), T0 + 8)
        .unwrap();
    let superseded = db
        .plan_approve(
            &superseded_start.plan.plan_id,
            1,
            &superseded_saved.revision.content_sha256,
            binding(),
            "operator",
            DEFAULT_PLAN_APPROVAL_TTL_MS,
            T0 + 9,
        )
        .unwrap();
    db.plan_save_revision(
        "owner-superseded",
        &superseded_start.plan.plan_id,
        1,
        "v2",
        spec(),
        T0 + 10,
    )
    .unwrap();

    let owner_start = db.plan_start("owner-approved", PROJECT, "owner-bound", T0 + 11).unwrap();
    let owner_saved = db
        .plan_save_revision("owner-approved", &owner_start.plan.plan_id, 0, "owner bound", spec(), T0 + 12)
        .unwrap();
    let owner_approval = db
        .plan_approve(
            &owner_start.plan.plan_id,
            1,
            &owner_saved.revision.content_sha256,
            binding(),
            "operator",
            DEFAULT_PLAN_APPROVAL_TTL_MS,
            T0 + 13,
        )
        .unwrap();
    drop(db);

    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.goal_run_start("owner-revoked", &revoked_start.plan.plan_id, &env(), &json!({}), "codex", T0 + 14)
            .unwrap_err()
            .code(),
        "plan_not_approved"
    );

    let after_expiry = expired.expires_at_unix_ms.unwrap() + 1;
    let expired_detail = db
        .plan_read(PlanReader::Operator, &expired_start.plan.plan_id, None, after_expiry)
        .unwrap();
    assert!(expired_detail.decisions.iter().any(|decision| {
        decision.approval_id == expired.approval_id && decision.state == PlanDecisionState::Expired
    }));
    assert_eq!(
        db.goal_run_start("owner-expired", &expired_start.plan.plan_id, &env(), &json!({}), "codex", after_expiry)
            .unwrap_err()
            .code(),
        "plan_not_approved"
    );

    let superseded_detail = db
        .plan_read(PlanReader::Operator, &superseded_start.plan.plan_id, None, T0 + 14)
        .unwrap();
    assert!(superseded_detail.decisions.iter().any(|decision| {
        decision.approval_id == superseded.approval_id
            && decision.state == PlanDecisionState::Superseded
    }));
    assert_eq!(
        db.goal_run_start("owner-superseded", &superseded_start.plan.plan_id, &env(), &json!({}), "codex", T0 + 14)
            .unwrap_err()
            .code(),
        "plan_not_approved"
    );

    assert_eq!(
        db.plan_read(PlanReader::Owner("other-owner"), &owner_start.plan.plan_id, None, T0 + 14)
            .unwrap_err()
            .code(),
        "plan_not_found"
    );
    assert_eq!(
        db.goal_run_start("other-owner", &owner_start.plan.plan_id, &env(), &json!({}), "codex", T0 + 14)
            .unwrap_err()
            .code(),
        "plan_not_found"
    );
    assert!(db
        .plan_read(PlanReader::Operator, &owner_start.plan.plan_id, None, T0 + 14)
        .unwrap()
        .decisions
        .iter()
        .any(|decision| {
            decision.approval_id == owner_approval.approval_id
                && decision.state == PlanDecisionState::Active
        }));
}

#[test]
fn legacy_plan_spec_defaults_preserve_the_original_serialized_hash_input() {
    use sha2::{Digest, Sha256};

    let legacy_json = r#"{"acceptance":[{"name":"tests","argv":["cargo","test"],"cwd":null,"timeout_secs":600}],"budget":{"max_iterations":5,"max_minutes":120,"agent_timeout_secs":1800}}"#;
    let parsed: PlanSpec = serde_json::from_str(legacy_json).unwrap();
    assert_eq!(parsed.acceptance[0].lane, AcceptanceLane::OfflineSandbox);
    assert_eq!(parsed.acceptance[0].kind, AcceptanceKind::Test);
    assert!(parsed.workspace_requirements.is_empty());
    assert!(parsed.dependencies.is_empty());
    let serialized = serde_json::to_string(&parsed).unwrap();
    assert_eq!(serialized, legacy_json);

    let expected = {
        let mut hasher = Sha256::new();
        hasher.update(b"chadex.plan.revision.v1\0");
        hasher.update(b"legacy plan");
        hasher.update(b"\0");
        hasher.update(legacy_json.as_bytes());
        format!("{:x}", hasher.finalize())
    };
    assert_eq!(plan_content_sha256("legacy plan", &parsed), expected);
}

#[test]
fn acceptance_spec_hash_covers_requirements_and_dependency_provenance() {
    let checks = spec().acceptance;
    let empty = goal_acceptance_spec_sha256(&checks, &[], &[]).unwrap();
    let requirement = WorkspaceRequirement {
        source_path: "fixtures/input.json".to_string(),
        source_sha256: "a".repeat(64),
        destination_path: "testdata/input.json".to_string(),
        permission: WorkspaceRequirementPermission::ReadOnly,
    };
    let with_requirement = goal_acceptance_spec_sha256(&checks, &[requirement.clone()], &[]).unwrap();
    let mut changed_requirement = requirement.clone();
    changed_requirement.source_sha256 = "b".repeat(64);
    let changed_source = goal_acceptance_spec_sha256(&checks, &[changed_requirement], &[]).unwrap();
    let dependency = WorkspaceDependency {
        path: "Cargo.lock".to_string(),
        provenance: "approved lockfile".to_string(),
    };
    let with_dependency = goal_acceptance_spec_sha256(&checks, &[], &[dependency.clone()]).unwrap();
    let mut changed_dependency = dependency;
    changed_dependency.provenance = "different provenance".to_string();
    let changed_provenance = goal_acceptance_spec_sha256(&checks, &[], &[changed_dependency]).unwrap();

    assert_ne!(empty, with_requirement);
    assert_ne!(with_requirement, changed_source);
    assert_ne!(empty, with_dependency);
    assert_ne!(with_dependency, changed_provenance);
    assert_eq!(empty.len(), 64);
}

#[test]
fn acceptance_lanes_gates_dependencies_and_workspace_requirements_validate() {
    let mut invalid_loopback = spec();
    invalid_loopback.acceptance[0].lane = AcceptanceLane::LoopbackIntegration;
    assert_eq!(invalid_loopback.normalized().unwrap_err().code(), "invalid_plan_acceptance");

    let mut valid = spec();
    valid.acceptance[0].lane = AcceptanceLane::LoopbackIntegration;
    valid.acceptance[0].loopback_ports = vec![54317, 54318];
    valid.workspace_requirements.push(WorkspaceRequirement {
        source_path: "fixtures/input.json".to_string(),
        source_sha256: "c".repeat(64),
        destination_path: "testdata/input.json".to_string(),
        permission: WorkspaceRequirementPermission::ReadOnly,
    });
    valid.dependencies.push(WorkspaceDependency {
        path: "Cargo.toml".to_string(),
        provenance: "workspace manifest".to_string(),
    });
    assert!(valid.normalized().is_ok());

    let mut invalid_path = spec();
    invalid_path.workspace_requirements.push(WorkspaceRequirement {
        source_path: "../secret".to_string(),
        source_sha256: "c".repeat(64),
        destination_path: "testdata/secret".to_string(),
        permission: WorkspaceRequirementPermission::ReadOnly,
    });
    assert_eq!(invalid_path.normalized().unwrap_err().code(), "invalid_workspace_requirement");

    let mut bad_gate = external_gate_spec();
    bad_gate.acceptance[0].loopback_ports = vec![8080];
    assert_eq!(bad_gate.normalized().unwrap_err().code(), "invalid_plan_acceptance");

    let mut cyclic = spec();
    cyclic.acceptance.push(AcceptanceCheck {
        name: "integration".to_string(),
        argv: vec!["cargo".to_string(), "test".to_string()],
        cwd: None,
        timeout_secs: 60,
        lane: AcceptanceLane::OfflineSandbox,
        kind: AcceptanceKind::Test,
        loopback_ports: vec![],
        container_image: None,
        depends_on: vec!["tests".to_string()],
        external_gate: None,
    });
    cyclic.acceptance[0].depends_on = vec!["integration".to_string()];
    assert_eq!(cyclic.normalized().unwrap_err().code(), "invalid_plan_acceptance");
}

#[test]
fn legacy_commit_journal_derives_round_and_operator_adopts_only_runner_verified_git_oid() {
    let (temp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    let stage_key = format!("stage:1@g{}", lease.generation);
    db.goal_op_intent(&run.goal_run_id, &lease, &stage_key, "commit", 1, T0 + 4).unwrap();
    db.goal_op_finish(&run.goal_run_id, &lease, &stage_key, GoalOpState::Succeeded, &json!({}), T0 + 5)
        .unwrap();
    let op_key = format!("commit:1@g{}", lease.generation);
    db.goal_op_intent(&run.goal_run_id, &lease, &op_key, "commit", 1, T0 + 6).unwrap();
    db.goal_op_finish(&run.goal_run_id, &lease, &op_key, GoalOpState::Succeeded, &json!({}), T0 + 7)
        .unwrap();
    db.conn_for_tests()
        .execute(
            "UPDATE wc_goal_runs SET committed_rounds = 0, checkpoint_sha = NULL, iteration = 0 WHERE goal_run_id = ?1",
            [&run.goal_run_id],
        )
        .unwrap();
    db.goal_run_transition(
        &run.goal_run_id,
        &lease,
        GoalRunState::NeedsAttention,
        "legacy_commit_uncheckpointed",
        T0 + 8,
    )
    .unwrap();
    db.conn_for_tests()
        .execute(
            "ALTER TABLE wc_goal_runs RENAME COLUMN checkpoint_sha TO checkpoint_sha256",
            [],
        )
        .unwrap();
    drop(db);

    let reopened = Database::open(&temp.path().join("plans.db")).unwrap();
    let migrated = reopened.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap().run;
    assert_eq!(migrated.committed_rounds, 1);
    assert_eq!(migrated.checkpoint_sha, None);
    assert!(is_git_oid(&"d".repeat(40)));
    assert!(is_git_oid(&"d".repeat(64)));
    assert!(!is_git_oid(&"d".repeat(39)));
    assert_eq!(
        reopened.goal_run_checkpoint_adopt(
            PlanReader::Owner(OWNER),
            &run.goal_run_id,
            1,
            &op_key,
            &"d".repeat(40),
            "verified legacy commit",
            &binding(),
            "operator",
            T0 + 9,
        )
            .unwrap_err()
            .code(),
        "goal_checkpoint_adopt_operator_required"
    );
    let verified = "d".repeat(40);
    let adopted = reopened
        .goal_run_checkpoint_adopt(
            PlanReader::Operator,
            &run.goal_run_id,
            1,
            &op_key,
            &verified,
            "verified legacy commit",
            &binding(),
            "operator",
            T0 + 10,
        )
        .unwrap();
    assert_eq!(adopted.checkpoint_sha.as_deref(), Some(verified.as_str()));
    assert_eq!(adopted.committed_rounds, 1);
    assert_eq!(adopted.current_attempt, 0);
    assert_eq!(adopted.state, GoalRunState::NeedsAttention);
    let replay = reopened
        .goal_run_checkpoint_adopt(
            PlanReader::Operator,
            &run.goal_run_id,
            1,
            &op_key,
            &verified,
            "verified legacy commit",
            &binding(),
            "operator",
            T0 + 11,
        )
        .unwrap();
    assert_eq!(replay.checkpoint_sha.as_deref(), Some(verified.as_str()));
    assert_eq!(
        reopened.goal_run_checkpoint_adopt(
            PlanReader::Operator,
            &run.goal_run_id,
            1,
            &stage_key,
            &verified,
            "verified legacy commit",
            &binding(),
            "operator",
            T0 + 12,
        )
            .unwrap_err()
            .code(),
        "goal_checkpoint_commit_missing"
    );
    let retried = reopened
        .goal_run_control(
            PlanReader::Operator,
            &run.goal_run_id,
            GoalControlAction::RetryAcceptance,
            T0 + 13,
        )
        .unwrap();
    assert_eq!(retried.state, GoalRunState::Queued);
    assert_eq!(retried.current_attempt, 1);
    assert_eq!(retried.checkpoint_sha.as_deref(), Some(verified.as_str()));
    assert_eq!(
        reopened.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap().ops
            .iter()
            .filter(|op| op.op_key == op_key)
            .count(),
        1,
        "same-checkpoint recovery must not replay the successful commit"
    );
}

#[test]
fn chatgpt_acceptance_retry_is_claimable_after_ready_iteration_is_caught_up() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let run = start_run_provider(&db, &plan_id, "chatgpt", T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    db.goal_run_set_workspace(&run.goal_run_id, &lease, "workspace-project-1", "/repo", "goal/1", T0 + 5)
        .unwrap();
    record_commit_checkpoint(&db, &run.goal_run_id, &lease, 1, T0 + 6);
    db.goal_run_transition(
        &run.goal_run_id,
        &lease,
        GoalRunState::NeedsAttention,
        "acceptance_failed",
        T0 + 7,
    )
    .unwrap();
    let retried = db
        .goal_run_control(
            PlanReader::Operator,
            &run.goal_run_id,
            GoalControlAction::RetryAcceptance,
            T0 + 8,
        )
        .unwrap();
    assert_eq!(retried.iteration, retried.chatgpt_ready_iteration);
    assert_eq!(retried.state_reason.as_deref(), Some("acceptance_retry"));
    let claims = db.goal_run_claim("sup-retry", T0 + 9, 30_000, 1).unwrap();
    assert_eq!(claims.len(), 1);
    let (claimed, retry_lease) = &claims[0];
    assert_eq!(claimed.goal_run_id, run.goal_run_id);
    assert_eq!(claimed.current_attempt, 2);
    assert_eq!(claimed.state_reason.as_deref(), Some("acceptance_retry"));
    let check = &claimed.spec.acceptance[0];
    db.goal_acceptance_receipt_record(
        &run.goal_run_id,
        retry_lease,
        GoalAcceptanceReceiptObservation {
            acceptance_revision: claimed.acceptance_revision,
            checkpoint_round: claimed.committed_rounds,
            checkpoint_sha: claimed.checkpoint_sha.clone().unwrap(),
            check_name: check.name.clone(),
            argv_sha256: goal_acceptance_argv_sha256(&check.argv).unwrap(),
            exit_code: Some(0),
            timed_out: false,
            output_sha256: "b".repeat(64),
            observed_at_unix_ms: T0 + 10,
            external_attestation: None,
        },
    )
    .unwrap();
    db.goal_run_progress(&run.goal_run_id, retry_lease, claimed.iteration, 0, None, None, T0 + 11)
        .unwrap();
    assert_eq!(
        db.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap().run.state_reason,
        None
    );
}

#[test]
fn stage_only_legacy_journal_never_derives_or_records_a_checkpoint() {
    let (temp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    let stage_key = format!("stage:1@g{}", lease.generation);
    db.goal_op_intent(&run.goal_run_id, &lease, &stage_key, "commit", 1, T0 + 5).unwrap();
    db.goal_op_finish(&run.goal_run_id, &lease, &stage_key, GoalOpState::Succeeded, &json!({}), T0 + 6)
        .unwrap();
    db.conn_for_tests()
        .execute(
            "UPDATE wc_goal_runs SET committed_rounds = 0, checkpoint_sha = NULL WHERE goal_run_id = ?1",
            [&run.goal_run_id],
        )
        .unwrap();
    drop(db);

    let reopened = Database::open(&temp.path().join("plans.db")).unwrap();
    let migrated = reopened.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap().run;
    assert_eq!(migrated.committed_rounds, 0);
    assert_eq!(migrated.checkpoint_sha, None);
    assert_eq!(
        reopened.goal_run_checkpoint_record(
            &run.goal_run_id,
            &lease,
            1,
            &"d".repeat(40),
            T0 + 7,
        )
        .unwrap_err()
        .code(),
        "goal_checkpoint_commit_missing"
    );
}

#[test]
fn retry_acceptance_is_operator_only_same_checkpoint_and_never_replays_commit() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let (parked, _lease, checkpoint) = park_run_at_checkpoint(&db, &plan_id, T0 + 3);
    assert_eq!(parked.current_attempt, 1);
    assert_eq!(
        db.goal_run_control(
            PlanReader::Owner(OWNER),
            &parked.goal_run_id,
            GoalControlAction::RetryAcceptance,
            T0 + 6,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_retry_operator_required"
    );
    let retried = db
        .goal_run_control(
            PlanReader::Operator,
            &parked.goal_run_id,
            GoalControlAction::RetryAcceptance,
            T0 + 7,
        )
        .unwrap();
    assert_eq!(retried.state, GoalRunState::Queued);
    assert_eq!(retried.current_attempt, 2);
    assert_eq!(retried.committed_rounds, 1);
    assert_eq!(retried.checkpoint_sha.as_deref(), Some(checkpoint.as_str()));
    assert_eq!(retried.iteration, parked.iteration);
    let ops = db.goal_run_read(PlanReader::Operator, &parked.goal_run_id).unwrap().ops;
    assert_eq!(ops.iter().filter(|op| op.kind == "commit").count(), 1);
}

#[test]
fn unresolved_agent_outcome_blocks_same_checkpoint_acceptance_retry() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    db.goal_op_intent(&run.goal_run_id, &lease, "agent:1@g3", "agent", 1, T0 + 5).unwrap();
    db.goal_op_finish(&run.goal_run_id, &lease, "agent:1@g3", GoalOpState::Unknown, &json!({}), T0 + 6)
        .unwrap();
    record_commit_checkpoint(&db, &run.goal_run_id, &lease, 1, T0 + 7);
    db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::NeedsAttention, "unknown", T0 + 8)
        .unwrap();
    assert_eq!(
        db.goal_run_control(
            PlanReader::Operator,
            &run.goal_run_id,
            GoalControlAction::RetryAcceptance,
            T0 + 9,
        )
        .unwrap_err()
        .code(),
        "goal_agent_outcome_unknown"
    );
}

#[test]
fn operator_proposal_and_approval_append_acceptance_and_workspace_revision() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let (parked, _, checkpoint) = park_run_at_checkpoint(&db, &plan_id, T0 + 3);
    let immutable_plan_hash = parked.content_sha256.clone();

    let mut changed_check = parked.spec.acceptance[0].clone();
    changed_check.argv = vec!["cargo".to_string(), "test".to_string(), "--workspace".to_string()];
    let proposed_acceptance = vec![changed_check];
    let requirements = vec![WorkspaceRequirement {
        source_path: "private-fixtures/input.json".to_string(),
        source_sha256: "c".repeat(64),
        destination_path: "testdata/input.json".to_string(),
        permission: WorkspaceRequirementPermission::ReadOnly,
    }];
    let dependencies = vec![WorkspaceDependency {
        path: "Cargo.toml".to_string(),
        provenance: "approved repository manifest".to_string(),
    }];
    let hash = goal_acceptance_spec_sha256(&proposed_acceptance, &requirements, &dependencies).unwrap();
    let proposal = db
        .goal_acceptance_revision_propose(
            PlanReader::Operator,
            &parked.goal_run_id,
            &binding(),
            parked.acceptance_revision,
            &checkpoint,
            &hash,
            proposed_acceptance.clone(),
            requirements.clone(),
            dependencies.clone(),
            T0 + 8,
        )
        .unwrap();
    assert!(proposal.proposal_id.starts_with(GOAL_ACCEPTANCE_PROPOSAL_ID_PREFIX));
    assert_eq!(proposal.proposer, GoalAcceptanceProposer::Operator);
    assert_eq!(proposal.binding, binding());
    let detail_before_approval = db.goal_run_read(PlanReader::Operator, &parked.goal_run_id).unwrap();
    assert_eq!(detail_before_approval.acceptance_revisions.len(), 1);
    assert_eq!(detail_before_approval.pending_proposals, vec![proposal.clone()]);
    let before_approval = detail_before_approval.run;
    assert_eq!(before_approval.acceptance_revision, 1);
    assert!(before_approval.spec.workspace_requirements.is_empty());
    assert_eq!(
        db.goal_acceptance_revision_approve(
            PlanReader::Operator,
            &parked.goal_run_id,
            &proposal.proposal_id,
            1,
            &checkpoint,
            &"0".repeat(64),
            &binding(),
            "operator",
            T0 + 9,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_proposal_hash_mismatch"
    );
    assert_eq!(
        db.goal_acceptance_revision_approve(
            PlanReader::Owner(OWNER),
            &parked.goal_run_id,
            &proposal.proposal_id,
            1,
            &checkpoint,
            &proposal.proposed_acceptance_sha256,
            &binding(),
            "operator",
            T0 + 9,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_approval_operator_required"
    );
    let mut wrong_binding = binding();
    wrong_binding.runner_client_id = "other-runner".to_string();
    assert_eq!(
        db.goal_acceptance_revision_approve(
            PlanReader::Operator,
            &parked.goal_run_id,
            &proposal.proposal_id,
            1,
            &checkpoint,
            &proposal.proposed_acceptance_sha256,
            &wrong_binding,
            "operator",
            T0 + 9,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_binding_changed"
    );
    let approved = db
        .goal_acceptance_revision_approve(
            PlanReader::Operator,
            &parked.goal_run_id,
            &proposal.proposal_id,
            1,
            &checkpoint,
            &proposal.proposed_acceptance_sha256,
            &binding(),
            "operator",
            T0 + 10,
        )
        .unwrap();
    assert_eq!(approved.revision, 2);
    assert_eq!(approved.acceptance, proposed_acceptance);
    assert_eq!(approved.workspace_requirements, requirements);
    assert_eq!(approved.dependencies, dependencies);
    let detail_after = db.goal_run_read(PlanReader::Operator, &parked.goal_run_id).unwrap();
    assert_eq!(detail_after.acceptance_revisions.len(), 2);
    assert!(detail_after.pending_proposals.is_empty());
    let after = detail_after.run;
    assert_eq!(after.acceptance_revision, 2);
    assert_eq!(after.content_sha256, immutable_plan_hash);
    assert_eq!(after.plan_id, parked.plan_id);
    assert_eq!(after.revision, parked.revision);
    assert_eq!(after.spec.workspace_requirements.len(), 1);
    assert_eq!(after.spec.dependencies.len(), 1);
    assert_eq!(db.plan_read(PlanReader::Operator, &plan_id, None, T0 + 11).unwrap().revision.unwrap().content_sha256, immutable_plan_hash);
    assert!(db.conn_for_tests().execute(
        "UPDATE wc_goal_acceptance_revisions SET approver = 'tampered' WHERE goal_run_id = ?1 AND revision = 2",
        [&parked.goal_run_id],
    ).is_err());
}

#[test]
fn acceptance_revision_cannot_remove_required_gates_or_downgrade_external_gate() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let (parked, _, checkpoint) = park_run_at_checkpoint(&db, &plan_id, T0 + 3);
    let empty_hash = goal_acceptance_spec_sha256(&[], &[], &[]).unwrap();
    assert_eq!(
        db.goal_acceptance_revision_propose(
            PlanReader::Owner(OWNER),
            &parked.goal_run_id,
            &binding(),
            1,
            &checkpoint,
            &empty_hash,
            vec![],
            vec![],
            vec![],
            T0 + 8,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_gate_removed"
    );

    let (_tmp2, db2) = fresh_db();
    let (plan2, saved2) = started_plan_with_spec(&db2, external_gate_spec());
    approve(&db2, &plan2, &saved2, T0 + 2);
    let (parked2, _, checkpoint2) = park_run_at_checkpoint(&db2, &plan2, T0 + 3);
    let mut weakened = parked2.spec.acceptance[0].clone();
    weakened.kind = AcceptanceKind::Test;
    weakened.lane = AcceptanceLane::OfflineSandbox;
    weakened.external_gate = None;
    let weakened = vec![weakened];
    assert_eq!(
        db2.goal_acceptance_revision_propose(
            PlanReader::Owner(OWNER),
            &parked2.goal_run_id,
            &binding(),
            1,
            &checkpoint2,
            &goal_acceptance_spec_sha256(&weakened, &[], &[]).unwrap(),
            weakened,
            vec![],
            vec![],
            T0 + 8,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_gate_weakened"
    );
}

#[test]
fn external_gate_revision_keeps_command_and_manifest_identity_immutable() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan_with_spec(&db, external_gate_spec());
    approve(&db, &plan_id, &saved, T0 + 2);
    let (parked, _, checkpoint) = park_run_at_checkpoint(&db, &plan_id, T0 + 3);
    let original = parked.spec.acceptance[0].clone();
    let assert_rejected = |replacement: AcceptanceCheck| {
        let acceptance = vec![replacement];
        let hash = goal_acceptance_spec_sha256(&acceptance, &[], &[]).unwrap();
        assert_eq!(
            db.goal_acceptance_revision_propose(
                PlanReader::Owner(OWNER),
                &parked.goal_run_id,
                &binding(),
                1,
                &checkpoint,
                &hash,
                acceptance,
                vec![],
                vec![],
                T0 + 8,
            )
            .unwrap_err()
            .code(),
            "goal_acceptance_gate_weakened"
        );
    };

    let mut changed_argv = original.clone();
    changed_argv.argv = vec!["true".to_string()];
    assert_rejected(changed_argv);

    let mut changed_cwd = original.clone();
    changed_cwd.cwd = Some("testdata".to_string());
    assert_rejected(changed_cwd);

    for field in ["owner", "action", "evidence"] {
        let mut changed_manifest = original.clone();
        let manifest = changed_manifest.external_gate.as_mut().unwrap();
        match field {
            "owner" => manifest.owner = "untrusted operator".to_string(),
            "action" => manifest.action = "skip approval".to_string(),
            "evidence" => manifest.evidence = "always pass".to_string(),
            _ => unreachable!(),
        }
        assert_rejected(changed_manifest);
    }

    let mut next_step_only = original;
    next_step_only.external_gate.as_mut().unwrap().next_step = "collect the signed record".to_string();
    let acceptance = vec![next_step_only];
    let hash = goal_acceptance_spec_sha256(&acceptance, &[], &[]).unwrap();
    assert!(db
        .goal_acceptance_revision_propose(
            PlanReader::Owner(OWNER),
            &parked.goal_run_id,
            &binding(),
            1,
            &checkpoint,
            &hash,
            acceptance,
            vec![],
            vec![],
            T0 + 9,
        )
        .is_ok());

    let (_tmp2, db2) = fresh_db();
    let (legacy_plan, legacy_saved) = started_plan(&db2);
    approve(&db2, &legacy_plan, &legacy_saved, T0 + 2);
    let (legacy_parked, _, legacy_checkpoint) = park_run_at_checkpoint(&db2, &legacy_plan, T0 + 3);
    let mut upgraded = legacy_parked.spec.acceptance[0].clone();
    upgraded.kind = AcceptanceKind::ExternalGate;
    upgraded.lane = AcceptanceLane::ExternalAttestation;
    upgraded.argv = vec!["true".to_string()];
    upgraded.external_gate = Some(ExternalGateManifest {
        owner: "operator".to_string(),
        action: "attest completion".to_string(),
        evidence: "signed record".to_string(),
        next_step: "review evidence".to_string(),
    });
    let acceptance = vec![upgraded];
    let hash = goal_acceptance_spec_sha256(&acceptance, &[], &[]).unwrap();
    assert_eq!(
        db2.goal_acceptance_revision_propose(
            PlanReader::Owner(OWNER),
            &legacy_parked.goal_run_id,
            &binding(),
            1,
            &legacy_checkpoint,
            &hash,
            acceptance,
            vec![],
            vec![],
            T0 + 8,
        )
        .unwrap_err()
        .code(),
        "goal_acceptance_gate_weakened"
    );
}

#[test]
fn external_gate_exit_two_is_blocked_and_store_hmac_receipt_survives_reopen() {
    let (temp, db) = db();
    let (plan_id, saved) = started_plan_with_spec(&db, external_gate_spec());
    approve(&db, &plan_id, &saved, T0 + 2);
    let run = start_run(&db, &plan_id, T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    let checkpoint = record_commit_checkpoint(&db, &run.goal_run_id, &lease, 1, T0 + 5);
    let check = &run.spec.acceptance[0];
    let receipt = db
        .goal_acceptance_receipt_record(
            &run.goal_run_id,
            &lease,
            GoalAcceptanceReceiptObservation {
                acceptance_revision: 1,
                checkpoint_round: 1,
                checkpoint_sha: checkpoint,
                check_name: check.name.clone(),
                argv_sha256: goal_acceptance_argv_sha256(&check.argv).unwrap(),
                exit_code: Some(2),
                timed_out: false,
                output_sha256: "f".repeat(64),
                observed_at_unix_ms: T0 + 6,
                external_attestation: None,
            },
        )
        .unwrap();
    assert_eq!(receipt.outcome, GoalAcceptanceOutcome::BlockedExternal);
    assert_eq!(receipt.proof_hmac_sha256.len(), 64);
    assert_eq!(
        db.goal_run_transition(&run.goal_run_id, &lease, GoalRunState::Succeeded, "passed", T0 + 7)
            .unwrap_err()
            .code(),
        "goal_acceptance_not_passed"
    );
    let before_reopen = db.goal_acceptance_receipts(PlanReader::Owner(OWNER), &run.goal_run_id).unwrap();
    assert_eq!(before_reopen, vec![receipt.clone()]);
    drop(db);

    let reopened = Database::open(&temp.path().join("plans.db")).unwrap();
    assert_eq!(
        reopened.goal_acceptance_receipts(PlanReader::Operator, &run.goal_run_id).unwrap(),
        before_reopen
    );
    {
        let conn = reopened.conn_for_tests();
        conn.execute("DROP TRIGGER trg_wc_goal_acceptance_receipts_immutable_update", []).unwrap();
        conn.execute(
            "UPDATE wc_goal_acceptance_receipts SET output_sha256 = ?2 WHERE receipt_id = ?1",
            [&receipt.receipt_id, &"1".repeat(64)],
        )
        .unwrap();
    }
    assert_eq!(
        reopened.goal_acceptance_receipts(PlanReader::Operator, &run.goal_run_id).unwrap_err().code(),
        "goal_receipt_integrity_failed"
    );
}

#[test]
fn acceptance_receipt_mac_matches_rfc_4231_hmac_sha256_vector() {
    let key = vec![0x0b; 20];
    let actual = hex_encode(&hmac_sha256(&key, b"Hi There"));
    assert_eq!(
        actual,
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

#[test]
fn goal_activity_is_owner_project_scoped_and_supervisor_refresh_is_lease_fenced() {
    let (_tmp, db) = db();
    let (plan_id, saved) = started_plan(&db);
    approve(&db, &plan_id, &saved, T0 + 2);
    let run = start_run_provider(&db, &plan_id, "chatgpt", T0 + 3).unwrap().run;
    let (_, lease) = db.goal_run_claim("sup", T0 + 4, 30_000, 1).unwrap().remove(0);
    db.goal_run_set_workspace(&run.goal_run_id, &lease, "workspace-project-1", "/repo", "goal/1", T0 + 5)
        .unwrap();
    db.goal_run_override_chatgpt_seen(&run.goal_run_id, T0 + 4).unwrap();

    assert_eq!(db.goal_run_note_activity("other-owner", "workspace-project-1", T0 + 10).unwrap(), 0);
    assert_eq!(db.goal_run_note_activity(OWNER, "wrong-project", T0 + 10).unwrap(), 0);
    assert_eq!(db.goal_run_note_activity(OWNER, "workspace-project-1", T0 + 10).unwrap(), 1);
    assert_eq!(db.goal_run_note_activity(OWNER, "workspace-project-1", T0 + 10).unwrap(), 0);
    assert_eq!(db.goal_run_verified_activity(&run.goal_run_id, &lease, T0 + 11).unwrap(), 1);
    assert_eq!(db.goal_run_verified_activity(&run.goal_run_id, &lease, T0 + 11).unwrap(), 0);

    db.goal_run_control(PlanReader::Owner(OWNER), &run.goal_run_id, GoalControlAction::Pause, T0 + 12)
        .unwrap();
    assert_eq!(db.goal_run_verified_activity(&run.goal_run_id, &lease, T0 + 13).unwrap(), 0);
    assert_eq!(db.goal_run_note_activity(OWNER, "workspace-project-1", T0 + 13).unwrap(), 0);
    let events = db.goal_run_read(PlanReader::Operator, &run.goal_run_id).unwrap().events;
    assert_eq!(events.iter().filter(|event| event.kind == "owner_activity").count(), 1);
    assert_eq!(events.iter().filter(|event| event.kind == "verified_job_activity").count(), 1);
}

#[test]
fn external_exit_zero_without_a_real_authority_is_still_blocked() {
    let (_temp,db)=db();let (plan_id,saved)=started_plan_with_spec(&db,external_gate_spec());
    approve(&db,&plan_id,&saved,T0+2);let run=start_run(&db,&plan_id,T0+3).unwrap().run;
    let (_,lease)=db.goal_run_claim("sup",T0+4,30_000,1).unwrap().remove(0);
    let checkpoint=record_commit_checkpoint(&db,&run.goal_run_id,&lease,1,T0+5);
    let observation=GoalAcceptanceReceiptObservation{acceptance_revision:1,checkpoint_round:1,checkpoint_sha:checkpoint,
        check_name:run.spec.acceptance[0].name.clone(),argv_sha256:goal_acceptance_argv_sha256(&run.spec.acceptance[0].argv).unwrap(),
        exit_code:Some(0),timed_out:false,output_sha256:"a".repeat(64),observed_at_unix_ms:T0+6,external_attestation:None};
    let receipt=db.goal_acceptance_receipt_record(&run.goal_run_id,&lease,observation).unwrap();
    assert_eq!(receipt.exit_code,Some(0));
    assert_eq!(receipt.outcome,GoalAcceptanceOutcome::BlockedExternal);
    assert_eq!(db.goal_run_transition(&run.goal_run_id,&lease,GoalRunState::Succeeded,"fake pass",T0+7).unwrap_err().code(),"goal_acceptance_not_passed");
}

#[test]
fn external_authority_signature_is_bound_to_runner_project_checkpoint_command_output_and_artifact() {
    use ring::signature::{Ed25519KeyPair,KeyPair};
    let (_temp,db)=db();let (plan_id,saved)=started_plan_with_spec(&db,external_gate_spec());
    approve(&db,&plan_id,&saved,T0+2);let run=start_run(&db,&plan_id,T0+3).unwrap().run;
    let (_,lease)=db.goal_run_claim("sup",T0+4,30_000,1).unwrap().remove(0);
    let checkpoint=record_commit_checkpoint(&db,&run.goal_run_id,&lease,1,T0+5);
    let run=db.goal_run_read(PlanReader::Operator,&run.goal_run_id).unwrap().run;
    let check=&run.spec.acceptance[0];let manifest=check.external_gate.as_ref().unwrap();
    let pair=Ed25519KeyPair::from_seed_unchecked(&[91;32]).unwrap();
    let hex=|bytes:&[u8]|bytes.iter().map(|b|format!("{b:02x}")).collect::<String>();
    let keys=vec![crate::goal_attestation::TrustedGateKey{owner:manifest.owner.clone(),project_root:run.binding.project_root.clone(),runner_client_id:run.binding.runner_client_id.clone(),public_key_hex:hex(pair.public_key().as_ref())}];
    let argv=goal_acceptance_argv_sha256(&check.argv).unwrap();
    let statement=json!({"goal_run_id":run.goal_run_id,"acceptance_revision":1,"checkpoint_sha":checkpoint,"check_name":check.name,
        "argv_sha256":argv,"output_sha256":"a".repeat(64),"artifact_sha256":"b".repeat(64),"owner":manifest.owner,"action":manifest.action,
        "exit_code":0,"not_before_unix_ms":T0,"expires_at_unix_ms":T0+60_000});
    let signature=hex(pair.sign(&crate::goal_attestation::test_signing_bytes(statement.clone())).as_ref());
    let observation=GoalAcceptanceReceiptObservation{acceptance_revision:1,checkpoint_round:1,checkpoint_sha:checkpoint,
        check_name:check.name.clone(),argv_sha256:argv,exit_code:Some(0),timed_out:false,output_sha256:"a".repeat(64),observed_at_unix_ms:T0+6,
        external_attestation:Some(json!({"statement":statement,"signature_hex":signature,"observed_artifact_sha256":"b".repeat(64)}))};
    assert!(crate::goal_attestation::verify(&run,check,&observation,&keys));
    assert!(!crate::goal_attestation::verify(&run,check,&observation,&[]));
    for field in ["goal_run_id","checkpoint_sha","check_name","argv_sha256","output_sha256","artifact_sha256","owner","action"] {
        let mut changed=observation.clone();changed.external_attestation.as_mut().unwrap()["statement"][field]=json!("forged");
        assert!(!crate::goal_attestation::verify(&run,check,&changed,&keys),"{field}");
    }
    let mut changed=observation.clone();changed.observed_at_unix_ms=T0+60_001;
    assert!(!crate::goal_attestation::verify(&run,check,&changed,&keys));
    let mut changed=observation.clone();changed.external_attestation.as_mut().unwrap()["observed_artifact_sha256"]=json!("c".repeat(64));
    assert!(!crate::goal_attestation::verify(&run,check,&changed,&keys));
    let mut changed=run.clone();changed.binding.runner_client_id="different-runner".into();
    assert!(!crate::goal_attestation::verify(&changed,check,&observation,&keys));
    let mut changed=run.clone();changed.binding.project_root="/different-project".into();
    assert!(!crate::goal_attestation::verify(&changed,check,&observation,&keys));
}

#[test]
fn pending_owned_container_prevents_retry_and_survives_event_pagination() {
    let (_temp,db)=db();let (plan_id,saved)=started_plan_with_spec(&db,spec());approve(&db,&plan_id,&saved,T0+2);
    let run=start_run(&db,&plan_id,T0+3).unwrap().run;
    let (_,lease)=db.goal_run_claim("sup",T0+4,30_000,1).unwrap().remove(0);
    record_commit_checkpoint(&db,&run.goal_run_id,&lease,1,T0+5);
    let identity=json!({"id":"a".repeat(64),"created":"2026-10-15T00:00:00Z","backend":{}});
    db.goal_run_event(&run.goal_run_id,"container_owned",&identity,T0+6).unwrap();
    for i in 0..70 {db.goal_run_event(&run.goal_run_id,"fixture_event",&json!({}),T0+7+i).unwrap();}
    db.goal_run_transition(&run.goal_run_id,&lease,GoalRunState::NeedsAttention,"container_cleanup_pending",T0+100).unwrap();
    assert_eq!(db.goal_container_recovery_candidates(T0+101).unwrap().len(),1);
    assert_eq!(db.goal_run_control(PlanReader::Operator,&run.goal_run_id,GoalControlAction::RetryAcceptance,T0+102).unwrap_err().code(),"goal_container_cleanup_pending");
    db.goal_run_event(&run.goal_run_id,"container_cleaned",&identity,T0+103).unwrap();
    assert!(db.goal_container_recovery_candidates(T0+104).unwrap().is_empty());
    assert_eq!(db.goal_run_control(PlanReader::Operator,&run.goal_run_id,GoalControlAction::RetryAcceptance,T0+105).unwrap().state,GoalRunState::Queued);
}

#[test]
fn chatgpt_preparation_resume_reclaims_queued_workspace_without_reporting_a_turn() {
    let (_tmp,db)=db();let (plan,saved)=started_plan(&db);approve(&db,&plan,&saved,T0+2);
    let run=start_run_provider(&db,&plan,"chatgpt",T0+3).unwrap().run;
    let (_,lease)=db.goal_run_claim("sup",T0+4,30_000,1).unwrap().remove(0);
    db.goal_run_set_workspace(&run.goal_run_id,&lease,"workspace-project-1","/repo","goal/1",T0+5).unwrap();
    db.goal_run_transition(&run.goal_run_id,&lease,GoalRunState::NeedsAttention,"toolchain_preflight_blocked",T0+6).unwrap();
    let resumed=db.goal_run_control(PlanReader::Operator,&run.goal_run_id,GoalControlAction::Resume,T0+7).unwrap();
    assert_eq!(resumed.state,GoalRunState::Queued);
    assert_eq!(resumed.chatgpt_ready_iteration,0);assert_eq!(resumed.committed_rounds,0);
    assert_eq!(resumed.deadline_at_unix_ms,run.deadline_at_unix_ms);
    let claims=db.goal_run_claim("sup-retry",T0+8,30_000,1).unwrap();assert_eq!(claims.len(),1);
    assert_eq!(claims[0].0.goal_run_id,run.goal_run_id);
    db.goal_run_release_lease(&run.goal_run_id,&claims[0].1).unwrap();
    assert!(db.goal_run_claim("sup-retry",T0+9,30_000,1).unwrap().is_empty());
}
