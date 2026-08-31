#!/usr/bin/env python3
"""Regression tests for foundation-stage contract and acceptance tooling."""

from __future__ import annotations

import copy
import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

import acceptance as acceptance_tool
import postman_contract


class ShapeTests(unittest.TestCase):
    def test_nested_type_changes_change_shape(self) -> None:
        original = {"success": 1, "data": {"price": "0.24"}}
        changed = {"success": "1", "data": []}
        self.assertNotEqual(postman_contract.json_shape(original), postman_contract.json_shape(changed))

    def test_array_shape_merges_optional_fields_and_types(self) -> None:
        shape = postman_contract.json_shape(
            [
                {"id": 1, "label": "one"},
                {"id": "2"},
            ]
        )
        items = shape["items"]
        self.assertEqual(items["kind"], "object")
        self.assertFalse(items["fields"]["label"]["required"])
        self.assertEqual(items["fields"]["id"]["shape"]["kind"], "union")

    def test_double_encoded_json_shape_is_visible(self) -> None:
        shape = postman_contract.json_shape('[{"country":"US"}]')
        self.assertEqual(shape["kind"], "string")
        self.assertEqual(shape["encoding"], "json")
        self.assertEqual(shape["decoded"]["kind"], "array")

    def test_unsafe_host_and_plaintext_http_are_errors(self) -> None:
        collection = {
            "info": {
                "name": "test",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
            },
            "item": [
                {
                    "name": "unsafe",
                    "request": {
                        "method": "GET",
                        "url": {
                            "raw": "http://evil.example/test",
                            "protocol": "http",
                            "host": ["evil", "example"],
                            "path": ["test"],
                        },
                    },
                    "response": [],
                }
            ],
        }
        _, errors, _ = postman_contract.analyze(collection, "test.json")
        self.assertTrue(any("unexpected host" in error for error in errors))
        self.assertTrue(any("plaintext HTTP" in error for error in errors))

    def test_nested_contract_change_updates_fingerprint(self) -> None:
        collection = {
            "info": {
                "name": "test",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
            },
            "item": [
                {
                    "name": "read",
                    "request": {
                        "method": "GET",
                        "url": {
                            "raw": "https://api.smspool.net/test",
                            "protocol": "https",
                            "host": ["api", "smspool", "net"],
                            "path": ["test"],
                        },
                    },
                    "response": [{"name": "200", "code": 200, "body": '{"data":{"price":"0.24"}}'}],
                }
            ],
        }
        changed = copy.deepcopy(collection)
        changed["item"][0]["response"][0]["body"] = '{"data":{"price":0.24}}'
        before, errors_before, _ = postman_contract.analyze(collection, "test.json")
        after, errors_after, _ = postman_contract.analyze(changed, "test.json")
        self.assertFalse(errors_before)
        self.assertFalse(errors_after)
        self.assertNotEqual(before["contract_sha256"], after["contract_sha256"])


class FixtureCleanupTests(unittest.TestCase):
    def test_clean_rejects_path_outside_fixture_root(self) -> None:
        with tempfile.TemporaryDirectory() as allowed_directory, tempfile.TemporaryDirectory() as outside:
            allowed = Path(allowed_directory)
            target = Path(outside)
            (target / postman_contract.FIXTURE_MARKER).write_text("owned", encoding="utf-8")
            with patch.object(postman_contract, "DEFAULT_FIXTURES", allowed):
                with self.assertRaises(postman_contract.ContractError):
                    postman_contract.clean_fixture_output(target)

    def test_clean_requires_marker(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            allowed = Path(directory) / "allowed"
            target = allowed / "child"
            target.mkdir(parents=True)
            with patch.object(postman_contract, "DEFAULT_FIXTURES", allowed):
                with self.assertRaises(postman_contract.ContractError):
                    postman_contract.clean_fixture_output(target)

    def test_clean_accepts_marked_descendant(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            allowed = Path(directory) / "allowed"
            target = allowed / "child"
            target.mkdir(parents=True)
            (target / postman_contract.FIXTURE_MARKER).write_text("owned", encoding="utf-8")
            with patch.object(postman_contract, "DEFAULT_FIXTURES", allowed):
                postman_contract.clean_fixture_output(target)
            self.assertFalse(target.exists())


class AcceptanceDefinitionTests(unittest.TestCase):
    def valid_definition(self) -> dict:
        return {
            "format_version": 1,
            "profiles": {"foundation": ["DOC-001"]},
            "gates": [
                {
                    "id": "DOC-001",
                    "title": "documents",
                    "status": "active",
                    "kind": "automated",
                    "command": ["true"],
                }
            ],
        }

    def test_title_is_required(self) -> None:
        definition = self.valid_definition()
        del definition["gates"][0]["title"]
        with self.assertRaises(acceptance_tool.DefinitionError):
            acceptance_tool.validate_definition(definition)

    def test_profile_ids_must_be_strings(self) -> None:
        definition = self.valid_definition()
        definition["profiles"]["foundation"] = [["not", "hashable"]]
        with self.assertRaises(acceptance_tool.DefinitionError):
            acceptance_tool.validate_definition(definition)

    def test_sanitized_live_observation_is_valid_but_not_gate_evidence(self) -> None:
        source = acceptance_tool.LIVE_OBSERVATIONS_FILE.read_text(encoding="utf-8")
        with tempfile.TemporaryDirectory() as directory:
            observation_file = Path(directory) / "live-observations.json"
            observation_file.write_text(source, encoding="utf-8")
            with patch.object(acceptance_tool, "LIVE_OBSERVATIONS_FILE", observation_file):
                acceptance_tool.validate_live_observations()
            document = json.loads(source)
            self.assertFalse(document["observations"][0]["gate_eligible"])
            self.assertNotIn("attestations", document)

    def test_validate_command_checks_sanitized_observations(self) -> None:
        with patch.object(
            acceptance_tool,
            "validate_live_observations",
            side_effect=acceptance_tool.DefinitionError("malformed observation"),
        ):
            with self.assertRaises(acceptance_tool.DefinitionError):
                acceptance_tool.command_validate(None)

    def test_sanitized_live_observation_rejects_gate_eligibility(self) -> None:
        source = json.loads(acceptance_tool.LIVE_OBSERVATIONS_FILE.read_text(encoding="utf-8"))
        source["observations"][0]["gate_eligible"] = True
        with tempfile.TemporaryDirectory() as directory:
            observation_file = Path(directory) / "live-observations.json"
            observation_file.write_text(json.dumps(source), encoding="utf-8")
            with patch.object(acceptance_tool, "LIVE_OBSERVATIONS_FILE", observation_file):
                with self.assertRaises(acceptance_tool.DefinitionError):
                    acceptance_tool.validate_live_observations()

    def test_sanitized_live_observation_rejects_sensitive_fields(self) -> None:
        source = json.loads(acceptance_tool.LIVE_OBSERVATIONS_FILE.read_text(encoding="utf-8"))
        source["observations"][0]["facts"]["api_key"] = "must-not-be-recorded"
        with tempfile.TemporaryDirectory() as directory:
            observation_file = Path(directory) / "live-observations.json"
            observation_file.write_text(json.dumps(source), encoding="utf-8")
            with patch.object(acceptance_tool, "LIVE_OBSERVATIONS_FILE", observation_file):
                with self.assertRaises(acceptance_tool.DefinitionError):
                    acceptance_tool.validate_live_observations()

    def _reject_mutated_observation(self, mutate) -> None:
        """The generalized schema must still reject malformed records."""
        source = json.loads(acceptance_tool.LIVE_OBSERVATIONS_FILE.read_text(encoding="utf-8"))
        mutate(source)
        with tempfile.TemporaryDirectory() as directory:
            observation_file = Path(directory) / "live-observations.json"
            observation_file.write_text(json.dumps(source), encoding="utf-8")
            with patch.object(acceptance_tool, "LIVE_OBSERVATIONS_FILE", observation_file):
                with self.assertRaises(acceptance_tool.DefinitionError):
                    acceptance_tool.validate_live_observations()

    def test_sanitized_live_observation_rejects_blank_route_fields(self) -> None:
        self._reject_mutated_observation(
            lambda doc: doc["observations"][0]["facts"].update({"country": "   "})
        )

    def test_sanitized_live_observation_rejects_non_integer_pool(self) -> None:
        self._reject_mutated_observation(
            lambda doc: doc["observations"][0]["facts"].update({"pool": "3"})
        )

    def test_sanitized_live_observation_rejects_boolean_pool(self) -> None:
        self._reject_mutated_observation(
            lambda doc: doc["observations"][0]["facts"].update({"pool": True})
        )

    def test_sanitized_live_observation_rejects_missing_required_fact(self) -> None:
        self._reject_mutated_observation(
            lambda doc: doc["observations"][0]["facts"].pop("purchased_price_usd")
        )

    def test_sanitized_live_observation_rejects_duplicate_record_ids(self) -> None:
        def mutate(doc):
            first = doc["observations"][0]
            doc["observations"].append(json.loads(json.dumps(first)))

        self._reject_mutated_observation(mutate)

    def test_sanitized_live_observation_rejects_altered_all_stock_evidence(self) -> None:
        self._reject_mutated_observation(
            lambda doc: doc["observations"][0]["facts"].update(
                {"all_stock_exceeded_bytes": [1, 2]}
            )
        )

    def test_sanitized_live_observation_scans_added_fact_keys_for_secrets(self) -> None:
        """Extra fact keys are allowed, so the secret scan must still reach them."""
        self._reject_mutated_observation(
            lambda doc: doc["observations"][-1]["facts"].update(
                {"provider_full_order_identifier": "ABC12345"}
            )
        )

    def test_active_manual_gate_can_verify_fresh_bound_evidence(self) -> None:
        now = datetime.now(timezone.utc).replace(microsecond=0)
        attestation = {
            "format_version": 1,
            "attestations": {
                "LIVE-001": {
                    "result": "passed",
                    "revision": "abc123",
                    "contract_sha256": "contract",
                    "gate_definition_sha256": "gates",
                    "recorded_at": now.isoformat(),
                    "expires_at": (now + timedelta(days=1)).isoformat(),
                    "approved_by": "operator@example.test",
                    "evidence": ["artifact://live-smoke/123"],
                }
            },
        }
        gate = {
            "id": "LIVE-001",
            "kind": "manual",
            "evidence_max_age_days": 7,
        }
        with tempfile.TemporaryDirectory() as directory:
            evidence_file = Path(directory) / "evidence.json"
            evidence_file.write_text(json.dumps(attestation), encoding="utf-8")
            with (
                patch.object(acceptance_tool, "EVIDENCE_FILE", evidence_file),
                patch.object(acceptance_tool, "current_revision", return_value="abc123"),
                patch.object(acceptance_tool, "contract_fingerprint", return_value="contract"),
                patch.object(acceptance_tool, "sha256_file", return_value="gates"),
            ):
                acceptance_tool.verify_manual_evidence(gate)

    def test_manual_gate_requires_evidence_freshness(self) -> None:
        definition = self.valid_definition()
        definition["gates"][0] = {
            "id": "DOC-001",
            "title": "manual check",
            "status": "pending",
            "kind": "manual",
            "activation": "when evidence exists",
        }
        with self.assertRaises(acceptance_tool.DefinitionError):
            acceptance_tool.validate_definition(definition)


if __name__ == "__main__":
    unittest.main()
