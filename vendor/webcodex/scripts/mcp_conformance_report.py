#!/usr/bin/env python3
"""Coverage-aware gate for pinned MCP conformance reports.

The upstream conformance runner intentionally answers a different question from
this gate: it executes scenarios and reports their checks.  WebCodex keeps the
raw upstream output, then this script verifies that the expected scenario set was
actually exercised and that every non-success result has a narrow, reviewed
classification.  A zero upstream process exit is never treated as proof of
coverage or compliance.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

CLASSIFICATIONS = {
    "genuine_protocol_failure",
    "missing_harness_fixture",
    "optional_capability_not_implemented",
    "harness_limitation_pending",
    "inconclusive_infrastructure",
}
VALID_STATUSES = {"SUCCESS", "FAILURE", "WARNING", "SKIPPED", "INFO"}
NON_SUCCESS_STATUSES = {"FAILURE", "WARNING", "SKIPPED"}
APPLICABLE_STATUSES = {"SUCCESS", "FAILURE", "WARNING"}
VERDICT_STATUSES = {"SUCCESS", "FAILURE", "WARNING", "SKIPPED"}
SCENARIO_DIR_RE = re.compile(r"^server-(.+)-\d{4}-\d{2}-\d{2}T.*Z$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")

# The pinned referee uses Date.now() only for these two stateless diagnostic
# probes. Keep this deliberately narrow: a large integer in any other check is
# semantic evidence, not generic timestamp noise.
DYNAMIC_JSONRPC_ID_CHECKS = {
    "sep-2575-http-server-no-independent-requests-on-stream",
    "sep-2575-server-no-log-without-loglevel",
}
DYNAMIC_JSONRPC_ID_PATH = ("details", "response", "id")
DYNAMIC_JSONRPC_ID_SENTINEL = {
    "$webcodexEvidence": "dynamic-millisecond-jsonrpc-id",
    "sourceType": "integer",
}
WIRE_SCHEMA_CHECK_ID = "wire-schema-valid"
WIRE_SCHEMA_MESSAGE_SENTINEL = "<wire-schema-offending-message>"
WIRE_SCHEMA_TOOL_INDEX_RE = re.compile(r"ListToolsResult/tools/(?P<index>\d+)")

# Structural or top-level scenario failures emitted by the immutable referee pin.
# These IDs mean the scenario/harness aborted outside its intended protocol
# assertions. Per-assertion task failures and setup checks are deliberately not
# blanket-listed; transport exceptions from those paths are recognized separately.
PINNED_REFEREE_INFRA_CHECK_IDS = {
    "scenario-timeout",
    "wire-schema-harness-error",
    "json-schema-2020-12-error",
    "server-session-lifecycle-error",
    "server-sse-multiple-streams-error",
    "server-sse-polling-error",
}
PINNED_TRANSPORT_ERROR_RE = re.compile(
    r"^(?:(?:Failed(?::| to initialize:| to set up tests:)|Error:)\s*)?"
    r"(?:fetch failed|connect E(?:CONNREFUSED|CONNRESET|HOSTUNREACH|NETUNREACH)\b.*)$"
)


class GateError(RuntimeError):
    """Raised when report or baseline input is malformed."""


@dataclass(frozen=True)
class Classification:
    scenario: str
    check_id: str
    expected_status: str
    evidence_sha256: str
    classification: str
    reason: str
    evidence: str

    @property
    def key(self) -> tuple[str, str]:
        return (self.scenario, self.check_id)


def _normalize_evidence_value(
    value: Any,
    *,
    path: tuple[str, ...] = (),
    normalize_dynamic_probe_id: bool = False,
) -> Any:
    """Normalize only pin-specific noise at an exact known evidence path."""

    if isinstance(value, list):
        return [
            _normalize_evidence_value(
                item,
                path=path,
                normalize_dynamic_probe_id=normalize_dynamic_probe_id,
            )
            for item in value
        ]
    if not isinstance(value, dict):
        return value

    normalized: dict[str, Any] = {}
    for field, item in value.items():
        item_path = path + (field,)
        if (
            normalize_dynamic_probe_id
            and item_path == DYNAMIC_JSONRPC_ID_PATH
            and value.get("jsonrpc") == "2.0"
            and isinstance(item, int)
            and not isinstance(item, bool)
            and abs(item) >= 1_000_000_000_000
        ):
            # Preserve field presence/type semantics while ignoring only the
            # Date.now()-style numeric value used by the pinned probe.
            normalized[field] = DYNAMIC_JSONRPC_ID_SENTINEL
            continue
        normalized[field] = _normalize_evidence_value(
            item,
            path=item_path,
            normalize_dynamic_probe_id=normalize_dynamic_probe_id,
        )
    return normalized


def _wire_schema_errors_with_tool_names(errors: Any, message: Any) -> Any:
    """Replace volatile tools/list array ordinals with the referenced tool name."""

    if not isinstance(errors, list) or not isinstance(message, dict):
        return errors
    result = message.get("result")
    tools = result.get("tools") if isinstance(result, dict) else None
    if not isinstance(tools, list):
        return errors

    def replace_tool_index(match: re.Match[str]) -> str:
        tool_index = int(match.group("index"))
        if tool_index >= len(tools):
            return match.group(0)
        tool = tools[tool_index]
        if not isinstance(tool, dict):
            return match.group(0)
        tool_name = tool.get("name")
        if not isinstance(tool_name, str) or not tool_name:
            return match.group(0)
        return "ListToolsResult/tools[" + json.dumps(tool_name, ensure_ascii=False) + "]"

    return [
        WIRE_SCHEMA_TOOL_INDEX_RE.sub(replace_tool_index, error)
        if isinstance(error, str)
        else error
        for error in errors
    ]


def _wire_schema_semantic_details(check: dict[str, Any]) -> Any | None:
    """Return structured wire-schema evidence with only the duplicated message normalized."""

    if check.get("id") != WIRE_SCHEMA_CHECK_ID:
        return None
    details = check.get("details")
    if not isinstance(details, dict) or not isinstance(details.get("messagesValidated"), int):
        return None
    violations = details.get("violations")
    if not isinstance(violations, list) or not violations:
        return None

    normalized = _normalize_evidence_value(details, path=("details",))
    normalized_violations = normalized.get("violations")
    if not isinstance(normalized_violations, list):
        return None

    recognized = False
    for index, violation in enumerate(violations):
        if not isinstance(violation, dict):
            continue
        if (
            violation.get("origin") not in {"harness", "implementation"}
            or not isinstance(violation.get("context"), str)
            or not isinstance(violation.get("errors"), list)
            or not isinstance(violation.get("specVersion"), str)
            or "message" not in violation
        ):
            continue
        normalized_violation = normalized_violations[index]
        if not isinstance(normalized_violation, dict):
            continue
        # Tool insertion/reordering changes the numeric array position in the
        # referee diagnostic even when the same named tool has the same schema
        # defect. Resolve that ordinal through the offending response before the
        # complete message is replaced. If resolution fails, keep the raw index
        # so the gate fails closed.
        normalized_violation["errors"] = _wire_schema_errors_with_tool_names(
            normalized_violation.get("errors"), violation.get("message")
        )
        # The same complete JSON-RPC object is also rendered into the referee's
        # errorMessage. Retain message *presence* with a sentinel and hash the
        # authoritative origin/context/errors/specVersion fields instead.
        normalized_violation["message"] = WIRE_SCHEMA_MESSAGE_SENTINEL
        recognized = True
    return normalized if recognized else None


def _has_substantive_evidence(value: Any) -> bool:
    if value is None or value == "":
        return False
    if isinstance(value, dict):
        return any(_has_substantive_evidence(item) for item in value.values())
    if isinstance(value, list):
        return any(_has_substantive_evidence(item) for item in value)
    return True


def check_evidence_sha256(check: dict[str, Any]) -> str:
    """Hash stable failure evidence while excluding only proven referee noise."""

    payload: dict[str, Any] = {}
    wire_schema_details = _wire_schema_semantic_details(check)
    if wire_schema_details is not None:
        # The structured violations are the authoritative evidence. The pinned
        # referee's errorMessage redundantly serializes the entire offending
        # message, so including it would reintroduce unrelated tool/schema drift.
        payload["details"] = wire_schema_details
        if "metadata" in check:
            payload["metadata"] = _normalize_evidence_value(
                check.get("metadata"), path=("metadata",)
            )
    else:
        normalize_dynamic_probe_id = check.get("id") in DYNAMIC_JSONRPC_ID_CHECKS
        for field in ("errorMessage", "details", "metadata"):
            if field in check:
                payload[field] = _normalize_evidence_value(
                    check.get(field),
                    path=(field,),
                    normalize_dynamic_probe_id=normalize_dynamic_probe_id,
                )

    if not payload or not any(_has_substantive_evidence(value) for value in payload.values()):
        payload = {
            "name": check.get("name"),
            "description": check.get("description"),
        }
    encoded = json.dumps(
        payload,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _infrastructure_reason(check: dict[str, Any]) -> str | None:
    # These are structural/top-level failure shapes emitted by the immutable referee
    # pin. Do not scan arbitrary assertion text for words such as "timeout": a
    # valid protocol assertion may contain those words.
    check_id = check.get("id")
    if check_id in PINNED_REFEREE_INFRA_CHECK_IDS:
        if check_id == "scenario-timeout":
            return "referee scenario timeout"
        if check_id == "wire-schema-harness-error":
            return "referee wire-schema harness error"
        return f"referee top-level scenario failure ({check_id})"
    if check.get("description") == "Failed to run scenario":
        return "referee failed to run scenario"
    error_message = check.get("errorMessage")
    if isinstance(error_message, str) and PINNED_TRANSPORT_ERROR_RE.fullmatch(error_message.strip()):
        return "referee transport exception"
    return None


def _load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise GateError(f"cannot read JSON {path}: {exc}") from exc


def _scenario_from_result_dir(path: Path) -> str:
    match = SCENARIO_DIR_RE.match(path.parent.name)
    if not match:
        raise GateError(
            f"unexpected upstream result directory {path.parent.name!r}; "
            "expected server-<scenario>-<timestamp>"
        )
    return match.group(1)


def load_reports(report_root: Path) -> dict[str, list[dict[str, Any]]]:
    reports: dict[str, list[dict[str, Any]]] = {}
    for checks_path in sorted(report_root.glob("server-*/checks.json")):
        scenario = _scenario_from_result_dir(checks_path)
        if scenario in reports:
            raise GateError(
                f"multiple raw reports found for scenario {scenario!r}; clean the output directory"
            )
        value = _load_json(checks_path)
        if not isinstance(value, list):
            raise GateError(f"{checks_path} must contain a JSON array")
        checks: list[dict[str, Any]] = []
        for index, check in enumerate(value):
            if not isinstance(check, dict):
                raise GateError(f"{checks_path}[{index}] is not an object")
            check_id = check.get("id")
            status = check.get("status")
            if not isinstance(check_id, str) or not check_id:
                raise GateError(f"{checks_path}[{index}] has no non-empty check id")
            if not isinstance(status, str) or not status:
                raise GateError(f"{checks_path}[{index}] has no non-empty status")
            if status not in VALID_STATUSES:
                raise GateError(f"{checks_path}[{index}] has unknown status {status!r}")
            checks.append(check)
        reports[scenario] = checks
    return reports


def load_classifications(baseline_path: Path, profile: str) -> dict[tuple[str, str], Classification]:
    baseline = _load_json(baseline_path)
    if not isinstance(baseline, dict) or baseline.get("schema_version") != 2:
        raise GateError("baseline must be an object with schema_version=2")
    profiles = baseline.get("profiles")
    if not isinstance(profiles, dict) or profile not in profiles:
        raise GateError(f"baseline has no profile {profile!r}")
    profile_config = profiles[profile]
    if not isinstance(profile_config, dict):
        raise GateError(f"baseline profile {profile!r} must be an object")
    raw_entries = profile_config.get("classifications", [])
    if not isinstance(raw_entries, list):
        raise GateError(f"baseline profile {profile!r} classifications must be an array")

    entries: dict[tuple[str, str], Classification] = {}
    for index, raw in enumerate(raw_entries):
        if not isinstance(raw, dict):
            raise GateError(f"classification #{index + 1} must be an object")
        scenario = raw.get("scenario")
        check_id = raw.get("check_id")
        expected_status = raw.get("expected_status")
        evidence_sha256 = raw.get("evidence_sha256")
        kind = raw.get("classification")
        reason = raw.get("reason")
        evidence = raw.get("evidence")
        if not isinstance(scenario, str) or not scenario:
            raise GateError(f"classification #{index + 1} needs a non-empty scenario")
        # Whole-scenario masks are deliberately unsupported. They can hide a newly
        # failing check when an older, unrelated check is already expected to fail.
        if not isinstance(check_id, str) or not check_id or check_id == "*":
            raise GateError(
                f"classification {scenario!r} must name one exact check_id; broad scenario masks are forbidden"
            )
        if expected_status not in NON_SUCCESS_STATUSES:
            raise GateError(
                f"classification {scenario}:{check_id} needs expected_status in "
                f"{sorted(NON_SUCCESS_STATUSES)}, got {expected_status!r}"
            )
        if not isinstance(evidence_sha256, str) or not SHA256_RE.fullmatch(evidence_sha256):
            raise GateError(
                f"classification {scenario}:{check_id} needs a lowercase 64-hex evidence_sha256"
            )
        if evidence_sha256 == "0" * 64:
            raise GateError(
                f"classification {scenario}:{check_id} has placeholder evidence_sha256"
            )
        if kind not in CLASSIFICATIONS:
            raise GateError(
                f"classification {scenario}:{check_id} has unsupported kind {kind!r}"
            )
        if not isinstance(reason, str) or not reason.strip():
            raise GateError(f"classification {scenario}:{check_id} needs a reason")
        if not isinstance(evidence, str) or not evidence.strip():
            raise GateError(f"classification {scenario}:{check_id} needs evidence")
        entry = Classification(
            scenario,
            check_id,
            expected_status,
            evidence_sha256,
            kind,
            reason.strip(),
            evidence.strip(),
        )
        if entry.key in entries:
            raise GateError(f"duplicate classification for {scenario}:{check_id}")
        entries[entry.key] = entry
    return entries


def evaluate_reports(
    report_root: Path,
    metadata_path: Path,
    baseline_path: Path,
    profile: str,
) -> dict[str, Any]:
    metadata = _load_json(metadata_path)
    if not isinstance(metadata, dict):
        raise GateError("metadata must be a JSON object")
    if metadata.get("schema_version") != 1:
        raise GateError("metadata must have schema_version=1")
    if metadata.get("profile") != profile:
        raise GateError(
            f"metadata profile {metadata.get('profile')!r} does not match requested {profile!r}"
        )
    server_sha = metadata.get("server_sha")
    harness_sha = metadata.get("harness_sha")
    if not isinstance(server_sha, str) or not GIT_SHA_RE.fullmatch(server_sha):
        raise GateError("metadata server_sha must be a full lowercase Git SHA")
    if not isinstance(harness_sha, str) or not GIT_SHA_RE.fullmatch(harness_sha):
        raise GateError("metadata harness_sha must be a full lowercase Git SHA")
    baseline_metadata = _load_json(baseline_path)
    if not isinstance(baseline_metadata, dict) or baseline_metadata.get("schema_version") != 2:
        raise GateError("baseline must be an object with schema_version=2")
    pinned_harness = baseline_metadata.get("harness_commit")
    if not isinstance(pinned_harness, str) or not GIT_SHA_RE.fullmatch(pinned_harness):
        raise GateError("baseline harness_commit must be a full lowercase Git SHA")
    if harness_sha != pinned_harness:
        raise GateError(
            f"metadata harness_sha {harness_sha} does not match pinned baseline {pinned_harness}"
        )
    harness_exit_code = metadata.get("harness_exit_code")
    if isinstance(harness_exit_code, bool) or not isinstance(harness_exit_code, int):
        raise GateError("metadata harness_exit_code must be an integer")
    required = metadata.get("required_scenarios")
    if not isinstance(required, list) or not required or not all(isinstance(item, str) and item for item in required):
        raise GateError("metadata required_scenarios must be a non-empty string array")
    if len(required) != len(set(required)):
        raise GateError("metadata required_scenarios contains duplicates")

    raw_not_scored = metadata.get("not_scored_scenarios", [])
    if not isinstance(raw_not_scored, list):
        raise GateError("metadata not_scored_scenarios must be an array")
    not_scored: dict[str, str] = {}
    for index, raw in enumerate(raw_not_scored):
        if not isinstance(raw, dict):
            raise GateError(f"metadata not_scored_scenarios[{index}] must be an object")
        scenario = raw.get("scenario")
        reason = raw.get("reason")
        if not isinstance(scenario, str) or not scenario or not isinstance(reason, str) or not reason:
            raise GateError(
                f"metadata not_scored_scenarios[{index}] requires non-empty scenario and reason"
            )
        if scenario in not_scored:
            raise GateError(f"metadata not_scored_scenarios duplicates {scenario!r}")
        not_scored[scenario] = reason

    required_set = set(required)
    if required_set & set(not_scored):
        raise GateError("metadata scenario cannot be both required and not_scored")

    reports = load_reports(report_root)
    classifications = load_classifications(baseline_path, profile)
    problems: list[str] = []
    optional_classifications = [
        entry
        for entry in classifications.values()
        if entry.classification == "optional_capability_not_implemented"
    ]
    server_capabilities = metadata.get("server_capabilities")
    if optional_classifications:
        if not isinstance(server_capabilities, dict):
            raise GateError(
                "metadata server_capabilities must be an object when optional-capability classifications exist"
            )
        profiles = baseline_metadata.get("profiles")
        profile_config = profiles.get(profile) if isinstance(profiles, dict) else None
        expected_capabilities = (
            profile_config.get("expected_server_capabilities")
            if isinstance(profile_config, dict)
            else None
        )
        if not isinstance(expected_capabilities, dict):
            raise GateError(
                f"baseline profile {profile!r} needs expected_server_capabilities for optional classifications"
            )
        if server_capabilities != expected_capabilities:
            problems.append(
                "server capability advertisement changed; optional-capability classifications "
                "require review"
            )
    elif server_capabilities is not None and not isinstance(server_capabilities, dict):
        raise GateError("metadata server_capabilities must be an object when present")
    missing_scenarios = sorted(required_set - set(reports))
    missing_not_scored_scenarios = sorted(set(not_scored) - set(reports))
    expected_scenarios = required_set | set(not_scored)
    unexpected_scenarios = sorted(set(reports) - expected_scenarios)
    if missing_scenarios:
        problems.append("missing required scenario reports: " + ", ".join(missing_scenarios))
    if missing_not_scored_scenarios:
        problems.append(
            "missing not-scored scenario reports: " + ", ".join(missing_not_scored_scenarios)
        )
    if unexpected_scenarios:
        problems.append("unexpected scenario reports: " + ", ".join(unexpected_scenarios))

    invalid_classification_scenarios = sorted(
        {entry.scenario for entry in classifications.values() if entry.scenario not in required_set}
    )
    if invalid_classification_scenarios:
        problems.append(
            "classifications may target scored required scenarios only: "
            + ", ".join(invalid_classification_scenarios)
        )

    statuses = Counter()
    informational_statuses = Counter()
    emitted: dict[tuple[str, str], set[str]] = {}
    applicable_checks = 0
    unclassified: list[str] = []
    expected: list[dict[str, str]] = []
    informational_non_success: list[dict[str, str]] = []
    inconclusive: list[str] = []
    evidence_mismatches: list[str] = []
    infrastructure_failures: list[str] = []
    baseline_observations: list[dict[str, str]] = []

    for scenario, checks in sorted(reports.items()):
        scored = scenario in required_set
        if scored:
            checks_by_id: dict[str, list[dict[str, Any]]] = {}
            for check in checks:
                checks_by_id.setdefault(check["id"], []).append(check)
            ambiguous_duplicates = sorted(
                check_id
                for check_id, repeated in checks_by_id.items()
                if len(repeated) > 1
                and any(item["status"] in NON_SUCCESS_STATUSES for item in repeated)
            )
            if ambiguous_duplicates:
                problems.append(
                    f"required scenario {scenario!r} emitted duplicate non-success check IDs "
                    "that cannot be matched to one baseline entry: "
                    + ", ".join(ambiguous_duplicates)
                )
        if scored and not any(check["status"] in VERDICT_STATUSES for check in checks):
            problems.append(f"required scenario {scenario!r} emitted no verdict checks")
        if not scored and not checks:
            problems.append(f"not-scored scenario {scenario!r} emitted an empty report")
        for check in checks:
            check_id = check["id"]
            status = check["status"]
            key = (scenario, check_id)
            infra_reason = _infrastructure_reason(check)
            if infra_reason is not None and status in NON_SUCCESS_STATUSES:
                infrastructure_failures.append(
                    f"{scenario}:{check_id} [{status}] ({infra_reason})"
                )
            if not scored:
                informational_statuses[status] += 1
                if scenario in not_scored and status in NON_SUCCESS_STATUSES:
                    informational_non_success.append(
                        {
                            "scenario": scenario,
                            "check_id": check_id,
                            "status": status,
                            "reason": not_scored[scenario],
                        }
                    )
                continue

            statuses[status] += 1
            emitted.setdefault(key, set()).add(status)
            if status in APPLICABLE_STATUSES:
                applicable_checks += 1
            if status in NON_SUCCESS_STATUSES:
                entry = classifications.get(key)
                label = f"{scenario}:{check_id} [{status}]"
                observed_evidence = check_evidence_sha256(check)
                baseline_observations.append(
                    {
                        "scenario": scenario,
                        "check_id": check_id,
                        "expected_status": status,
                        "evidence_sha256": observed_evidence,
                    }
                )
                if entry is None:
                    unclassified.append(label)
                    continue
                if entry.expected_status != status:
                    evidence_mismatches.append(
                        f"{label} expected status {entry.expected_status}"
                    )
                    continue
                if entry.evidence_sha256 != observed_evidence:
                    evidence_mismatches.append(
                        f"{label} evidence changed: expected {entry.evidence_sha256}, "
                        f"observed {observed_evidence}"
                    )
                    continue
                expected.append(
                    {
                        "scenario": scenario,
                        "check_id": check_id,
                        "status": status,
                        "evidence_sha256": observed_evidence,
                        "classification": entry.classification,
                        "reason": entry.reason,
                        "evidence": entry.evidence,
                    }
                )
                if entry.classification == "inconclusive_infrastructure":
                    inconclusive.append(label)

    if applicable_checks == 0:
        problems.append("zero applicable SUCCESS/FAILURE/WARNING checks were emitted")
    if harness_exit_code not in {0, 1}:
        problems.append(
            f"conformance referee exited abnormally with code {harness_exit_code}; only 0/1 are normal"
        )
    scored_failures = statuses.get("FAILURE", 0)
    if harness_exit_code == 0 and scored_failures > 0:
        problems.append(
            f"referee exit code 0 disagrees with {scored_failures} scored FAILURE check(s)"
        )
    if harness_exit_code == 1 and scored_failures == 0:
        problems.append("referee exit code 1 had no scored FAILURE checks")
    if infrastructure_failures:
        problems.append(
            "infrastructure failures cannot satisfy the gate: "
            + ", ".join(sorted(infrastructure_failures))
        )
    if unclassified:
        problems.append("unclassified non-success checks: " + ", ".join(sorted(unclassified)))
    if evidence_mismatches:
        problems.append(
            "classified check status/evidence changed and requires review: "
            + ", ".join(sorted(evidence_mismatches))
        )
    if inconclusive:
        problems.append(
            "inconclusive infrastructure results cannot satisfy the gate: "
            + ", ".join(sorted(inconclusive))
        )

    stale: list[str] = []
    for key, entry in sorted(classifications.items()):
        if entry.scenario not in required_set:
            continue
        states = emitted.get(key)
        label = f"{entry.scenario}:{entry.check_id}"
        if not states:
            stale.append(label + " (check not emitted)")
        elif states <= {"INFO"}:
            stale.append(label + " (no verdict emitted)")
        elif states <= {"SUCCESS", "INFO"} and "SUCCESS" in states:
            stale.append(label + " (now passing)")
    if stale:
        problems.append("stale or uncovered classifications: " + ", ".join(stale))

    return {
        "schema_version": 2,
        "profile": profile,
        "server_sha": server_sha,
        "harness_sha": harness_sha,
        "harness_exit_code": harness_exit_code,
        "server_capabilities": server_capabilities,
        "required_scenario_count": len(required),
        "not_scored_scenario_count": len(not_scored),
        "reported_scenario_count": len(reports),
        "applicable_check_count": applicable_checks,
        "status_counts": dict(sorted(statuses.items())),
        "informational_status_counts": dict(sorted(informational_statuses.items())),
        "classified_non_success": expected,
        "informational_non_success": informational_non_success,
        "baseline_observations": baseline_observations,
        "infrastructure_failures": infrastructure_failures,
        "missing_scenarios": missing_scenarios,
        "missing_not_scored_scenarios": missing_not_scored_scenarios,
        "unexpected_scenarios": unexpected_scenarios,
        "problems": problems,
        "gate_passed": not problems,
    }


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", required=True, type=Path)
    parser.add_argument("--metadata", required=True, type=Path)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--summary", type=Path)
    return parser.parse_args()


def main() -> int:
    args = _parse_args()
    try:
        summary = evaluate_reports(args.reports, args.metadata, args.baseline, args.profile)
    except GateError as exc:
        print(f"MCP conformance report gate input error: {exc}", file=sys.stderr)
        return 2

    rendered = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if args.summary:
        args.summary.parent.mkdir(parents=True, exist_ok=True)
        args.summary.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0 if summary["gate_passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
