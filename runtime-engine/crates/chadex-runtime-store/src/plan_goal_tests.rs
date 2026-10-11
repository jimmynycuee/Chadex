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

fn spec() -> PlanSpec {
    PlanSpec {
        acceptance: vec![AcceptanceCheck {
            name: "tests".to_string(),
            argv: vec!["cargo".to_string(), "test".to_string()],
            cwd: None,
            timeout_secs: 600,
        }],
        budget: GoalBudget::default(),
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
    let start = db.plan_start(OWNER, PROJECT, "Add feature", T0).unwrap();
    assert!(start.created);
    assert!(start.plan.plan_mode_active);
    let saved = db
        .plan_save_revision(OWNER, &start.plan.plan_id, 0, "# Plan v1\nstep one", spec(), T0 + 1)
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
    db.goal_run_start(OWNER, plan_id, &env(), &json!({"kind": "test"}), "codex", now)
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
    assert!(db.goal_run_list(PlanReader::Owner(OWNER), 30).unwrap().is_empty());
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

    // Clearing everything closes the rest.
    db.plan_goal_clear_board(None, T0 + 15).unwrap();
    assert!(db.plan_list(PlanReader::Operator, false, 50, T0 + 16).unwrap().is_empty());
}
