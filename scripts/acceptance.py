#!/usr/bin/env python3
"""Run repository acceptance gates without silently skipping unfinished work."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
GATES_FILE = ROOT / "acceptance" / "gates.json"
EVIDENCE_FILE = ROOT / "acceptance" / "evidence.json"
BASELINE_FILE = ROOT / "contracts" / "postman-baseline.json"
VALID_STATUSES = {"active", "pending"}
VALID_KINDS = {"automated", "manual"}
REQUIRED_DOCUMENTS = {
    "README.md": ("# SMSPool Rust SDK", "## Current status"),
    "docs/api-contract.md": ("# API contract", "## Verified collection facts", "## Unverified assumptions"),
    "docs/architecture.md": ("# SDK architecture", "## Reliability model", "## Axum boundary"),
    "docs/production-acceptance.md": ("# Production acceptance standard", "## Acceptance profiles", "## Gate catalogue"),
    "docs/generated/endpoint-matrix.md": ("# Postman endpoint matrix",),
    "contracts/postman-baseline.json": (),
    "acceptance/gates.json": (),
}


class DefinitionError(RuntimeError):
    """Raised when acceptance/gates.json is inconsistent."""


def load_definition() -> dict[str, Any]:
    try:
        value = json.loads(GATES_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise DefinitionError(f"missing gate definition: {GATES_FILE}") from exc
    except json.JSONDecodeError as exc:
        raise DefinitionError(f"invalid JSON in {GATES_FILE}: {exc}") from exc
    if not isinstance(value, dict):
        raise DefinitionError("gate definition root must be an object")
    return value


def validate_definition(definition: dict[str, Any]) -> dict[str, dict[str, Any]]:
    if definition.get("format_version") != 1:
        raise DefinitionError("format_version must be 1")
    gates = definition.get("gates")
    profiles = definition.get("profiles")
    if not isinstance(gates, list) or not isinstance(profiles, dict):
        raise DefinitionError("gates must be an array and profiles must be an object")

    by_id: dict[str, dict[str, Any]] = {}
    for index, gate in enumerate(gates):
        if not isinstance(gate, dict):
            raise DefinitionError(f"gates[{index}] must be an object")
        gate_id = gate.get("id")
        if not isinstance(gate_id, str) or not gate_id:
            raise DefinitionError(f"gates[{index}] has no id")
        if gate_id in by_id:
            raise DefinitionError(f"duplicate gate id: {gate_id}")
        if not isinstance(gate.get("title"), str) or not gate["title"].strip():
            raise DefinitionError(f"{gate_id}: title must be a non-empty string")
        if gate.get("status") not in VALID_STATUSES:
            raise DefinitionError(f"{gate_id}: invalid status {gate.get('status')!r}")
        if gate.get("kind") not in VALID_KINDS:
            raise DefinitionError(f"{gate_id}: invalid kind {gate.get('kind')!r}")
        command = gate.get("command")
        if command is not None and (
            not isinstance(command, list)
            or not command
            or any(not isinstance(part, str) or not part for part in command)
        ):
            raise DefinitionError(f"{gate_id}: command must be a non-empty string array")
        if gate["status"] == "active" and gate["kind"] == "automated" and command is None:
            raise DefinitionError(f"{gate_id}: active automated gate requires a command")
        if gate["status"] == "pending" and (
            not isinstance(gate.get("activation"), str) or not gate["activation"].strip()
        ):
            raise DefinitionError(f"{gate_id}: pending gate requires activation criteria")
        if gate["kind"] == "manual" and (
            not isinstance(gate.get("evidence_max_age_days"), int)
            or gate["evidence_max_age_days"] <= 0
        ):
            raise DefinitionError(f"{gate_id}: manual gate requires positive evidence_max_age_days")
        by_id[gate_id] = gate

    for profile_name, gate_ids in profiles.items():
        if not isinstance(profile_name, str) or not profile_name or not isinstance(gate_ids, list):
            raise DefinitionError("non-empty profile names must map to arrays")
        if any(not isinstance(gate_id, str) or not gate_id for gate_id in gate_ids):
            raise DefinitionError(f"profile {profile_name!r} gate ids must be non-empty strings")
        if len(gate_ids) != len(set(gate_ids)):
            raise DefinitionError(f"profile {profile_name!r} contains duplicate gate ids")
        unknown = [gate_id for gate_id in gate_ids if gate_id not in by_id]
        if unknown:
            raise DefinitionError(f"profile {profile_name!r} references unknown gates: {unknown}")
    return by_id


def command_validate(_: argparse.Namespace) -> int:
    definition = load_definition()
    gates = validate_definition(definition)
    print(f"gate definition is valid: {len(gates)} gates, {len(definition['profiles'])} profiles")
    return 0


def command_check_docs(_: argparse.Namespace) -> int:
    failures: list[str] = []
    for relative_path, headings in REQUIRED_DOCUMENTS.items():
        path = ROOT / relative_path
        try:
            content = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            failures.append(f"missing required document/artifact: {relative_path}")
            continue
        if not content.strip():
            failures.append(f"required document/artifact is empty: {relative_path}")
            continue
        for heading in headings:
            if heading not in content:
                failures.append(f"{relative_path}: missing required heading {heading!r}")
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    print(f"documentation check passed: {len(REQUIRED_DOCUMENTS)} required files")
    return 0


def expanded_command(parts: list[str]) -> list[str]:
    replacements = {
        "{python}": sys.executable,
        "{root}": str(ROOT),
    }
    return [replacements.get(part, part.replace("{root}", str(ROOT))) for part in parts]


def sha256_file(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except FileNotFoundError as exc:
        raise DefinitionError(f"required evidence input is missing: {path}") from exc


def current_revision() -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise DefinitionError("manual evidence requires a committed Git revision")
    dirty = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if dirty.returncode != 0 or dirty.stdout.strip():
        raise DefinitionError("manual evidence requires a clean Git checkout")
    return result.stdout.strip()


def contract_fingerprint() -> str:
    try:
        baseline = json.loads(BASELINE_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise DefinitionError(f"missing contract baseline: {BASELINE_FILE}") from exc
    except json.JSONDecodeError as exc:
        raise DefinitionError(f"invalid contract baseline: {exc}") from exc
    fingerprint = baseline.get("contract_sha256") if isinstance(baseline, dict) else None
    if not isinstance(fingerprint, str) or not fingerprint:
        raise DefinitionError("contract baseline has no contract_sha256")
    return fingerprint


def parse_timestamp(value: Any, field: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise DefinitionError(f"manual evidence field {field!r} must be an ISO-8601 string")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise DefinitionError(f"manual evidence field {field!r} is not ISO-8601") from exc
    if parsed.tzinfo is None:
        raise DefinitionError(f"manual evidence field {field!r} must include a timezone")
    return parsed.astimezone(timezone.utc)


def verify_manual_evidence(gate: dict[str, Any]) -> None:
    try:
        document = json.loads(EVIDENCE_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise DefinitionError(f"manual gate {gate['id']} requires {EVIDENCE_FILE}") from exc
    except json.JSONDecodeError as exc:
        raise DefinitionError(f"invalid manual evidence JSON: {exc}") from exc
    if not isinstance(document, dict) or document.get("format_version") != 1:
        raise DefinitionError("manual evidence format_version must be 1")
    attestations = document.get("attestations")
    attestation = attestations.get(gate["id"]) if isinstance(attestations, dict) else None
    if not isinstance(attestation, dict):
        raise DefinitionError(f"manual evidence has no attestation for {gate['id']}")

    required_matches = {
        "result": "passed",
        "revision": current_revision(),
        "contract_sha256": contract_fingerprint(),
        "gate_definition_sha256": sha256_file(GATES_FILE),
    }
    for field, expected in required_matches.items():
        if attestation.get(field) != expected:
            raise DefinitionError(
                f"manual evidence {gate['id']} field {field!r} does not match current repository"
            )
    if not isinstance(attestation.get("approved_by"), str) or not attestation["approved_by"].strip():
        raise DefinitionError(f"manual evidence {gate['id']} requires approved_by")
    evidence = attestation.get("evidence")
    if not isinstance(evidence, list) or not evidence or any(
        not isinstance(item, str) or not item.strip() for item in evidence
    ):
        raise DefinitionError(f"manual evidence {gate['id']} requires non-empty evidence references")

    recorded_at = parse_timestamp(attestation.get("recorded_at"), "recorded_at")
    expires_at = parse_timestamp(attestation.get("expires_at"), "expires_at")
    now = datetime.now(timezone.utc)
    max_age = timedelta(days=gate["evidence_max_age_days"])
    if recorded_at > now + timedelta(minutes=5):
        raise DefinitionError(f"manual evidence {gate['id']} is dated in the future")
    if now - recorded_at > max_age:
        raise DefinitionError(f"manual evidence {gate['id']} is older than its allowed maximum age")
    if expires_at <= now or expires_at > recorded_at + max_age:
        raise DefinitionError(f"manual evidence {gate['id']} has an invalid or expired expires_at")


def command_evidence_template(args: argparse.Namespace) -> int:
    definition = load_definition()
    by_id = validate_definition(definition)
    gate = by_id.get(args.gate_id)
    if gate is None:
        raise DefinitionError(f"unknown gate: {args.gate_id}")
    if gate["kind"] != "manual":
        raise DefinitionError(f"{args.gate_id} is not a manual gate")
    now = datetime.now(timezone.utc).replace(microsecond=0)
    expires = now + timedelta(days=gate["evidence_max_age_days"])
    template = {
        "format_version": 1,
        "attestations": {
            args.gate_id: {
                "result": "passed",
                "revision": current_revision(),
                "contract_sha256": contract_fingerprint(),
                "gate_definition_sha256": sha256_file(GATES_FILE),
                "recorded_at": now.isoformat().replace("+00:00", "Z"),
                "expires_at": expires.isoformat().replace("+00:00", "Z"),
                "approved_by": "REPLACE_WITH_APPROVER",
                "evidence": ["REPLACE_WITH_IMMUTABLE_EVIDENCE_REFERENCE"],
            }
        },
    }
    print(json.dumps(template, ensure_ascii=False, indent=2, sort_keys=True))
    return 0


def command_list(_: argparse.Namespace) -> int:
    definition = load_definition()
    by_id = validate_definition(definition)
    for profile, gate_ids in definition["profiles"].items():
        print(f"{profile}:")
        for gate_id in gate_ids:
            gate = by_id[gate_id]
            print(f"  {gate_id:<12} {gate['status']:<7} {gate['kind']:<9} {gate['title']}")
    return 0


def command_run(args: argparse.Namespace) -> int:
    definition = load_definition()
    by_id = validate_definition(definition)
    profiles = definition["profiles"]
    if args.profile not in profiles:
        raise DefinitionError(
            f"unknown profile {args.profile!r}; choose one of {', '.join(sorted(profiles))}"
        )

    failures = 0
    passed = 0
    blocked = 0
    for gate_id in profiles[args.profile]:
        gate = by_id[gate_id]
        title = gate["title"]
        if gate["status"] != "active":
            blocked += 1
            failures += 1
            print(f"BLOCKED {gate_id}: {title}")
            print(f"        activation: {gate.get('activation', 'not specified')}")
            continue
        if gate["kind"] == "manual":
            print(f"VERIFY  {gate_id}: {title}", flush=True)
            try:
                verify_manual_evidence(gate)
            except DefinitionError as exc:
                failures += 1
                print(f"FAIL    {gate_id}: {exc}")
                if args.fail_fast:
                    break
            else:
                passed += 1
                print(f"PASS    {gate_id}: current attestation verified")
            continue

        command = expanded_command(gate["command"])
        print(f"RUN     {gate_id}: {title}", flush=True)
        try:
            result = subprocess.run(command, cwd=ROOT, check=False)
        except OSError as exc:
            failures += 1
            print(f"FAIL    {gate_id}: could not execute command: {exc}")
            if args.fail_fast:
                break
            continue
        if result.returncode == 0:
            passed += 1
            print(f"PASS    {gate_id}")
        else:
            failures += 1
            print(f"FAIL    {gate_id}: exit {result.returncode}")
            if args.fail_fast:
                break

    print(
        f"profile {args.profile}: {passed} passed, {failures} failed/blocked, {blocked} pending/manual"
    )
    return 1 if failures else 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate", help="validate acceptance/gates.json")
    validate.set_defaults(handler=command_validate)

    docs = subparsers.add_parser("check-docs", help="check required documents and headings")
    docs.set_defaults(handler=command_check_docs)

    listing = subparsers.add_parser("list", help="list profiles and their gates")
    listing.set_defaults(handler=command_list)

    evidence = subparsers.add_parser(
        "evidence-template", help="print a current attestation template for a manual gate"
    )
    evidence.add_argument("gate_id")
    evidence.set_defaults(handler=command_evidence_template)

    run = subparsers.add_parser("run", help="execute a named acceptance profile")
    run.add_argument("profile", nargs="?", default="foundation")
    run.add_argument("--fail-fast", action="store_true")
    run.set_defaults(handler=command_run)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        return int(args.handler(args))
    except DefinitionError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
