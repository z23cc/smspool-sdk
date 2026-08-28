#!/usr/bin/env python3
"""Audit SMSPool's Postman collection and generate deterministic contract artifacts.

The script intentionally uses only Python's standard library so it can run in a
minimal CI environment before the Rust crate exists.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_COLLECTION = ROOT / "postman.json"
DEFAULT_BASELINE = ROOT / "contracts" / "postman-baseline.json"
DEFAULT_MATRIX = ROOT / "docs" / "generated" / "endpoint-matrix.md"
DEFAULT_FIXTURES = ROOT / "tests" / "fixtures" / "postman"
FINGERPRINT_DOCUMENTS = (ROOT / "README.md", ROOT / "docs" / "api-contract.md")
FIXTURE_MARKER = ".smspool-fixtures-generated"
SUPPORTED_BODY_MODES = {"none", "formdata", "urlencoded", "raw"}


class ContractError(RuntimeError):
    """Raised when the collection cannot be interpreted safely."""


@dataclass(frozen=True)
class Leaf:
    folders: tuple[str, ...]
    name: str
    item: dict[str, Any]


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ContractError(f"file not found: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ContractError(f"invalid JSON in {path}: {exc}") from exc


def walk_items(items: Any, folders: tuple[str, ...] = ()) -> Iterable[Leaf]:
    if not isinstance(items, list):
        raise ContractError("collection item must be an array")
    for index, item in enumerate(items):
        if not isinstance(item, dict):
            raise ContractError(f"item at {'/'.join(folders) or '<root>'}[{index}] is not an object")
        name = str(item.get("name") or f"unnamed-{index + 1}")
        children = item.get("item")
        if children is not None:
            yield from walk_items(children, folders + (name,))
            continue
        if "request" not in item:
            raise ContractError(f"leaf item {'/'.join(folders + (name,))} has no request")
        yield Leaf(folders=folders, name=name, item=item)


def normalized_path(url: Any) -> str:
    if isinstance(url, dict):
        path = url.get("path")
        if isinstance(path, list) and path:
            return "/" + "/".join(str(part).strip("/") for part in path)
        if isinstance(path, str) and path:
            return "/" + path.strip("/")
        raw = url.get("raw")
    else:
        raw = url

    if not isinstance(raw, str) or not raw.strip():
        raise ContractError("request URL has neither a structured path nor a raw URL")
    candidate = raw.strip().replace("{{baseUrl}}", "https://api.smspool.net")
    parsed = urlsplit(candidate if "://" in candidate else "//" + candidate)
    if not parsed.path:
        raise ContractError(f"request URL has no path: {raw!r}")
    return "/" + parsed.path.strip("/")


def url_metadata(url: Any) -> tuple[str | None, str | None, str | None]:
    if isinstance(url, dict):
        raw = url.get("raw")
        protocol = url.get("protocol")
        host_value = url.get("host")
        if isinstance(host_value, list):
            host = ".".join(str(part) for part in host_value)
        elif isinstance(host_value, str):
            host = host_value
        else:
            host = None
    else:
        raw = url
        protocol = None
        host = None

    if isinstance(raw, str) and raw:
        candidate = raw.replace("{{baseUrl}}", "https://api.smspool.net")
        parsed = urlsplit(candidate if "://" in candidate else "//" + candidate)
        protocol = protocol or parsed.scheme or None
        host = host or parsed.hostname
    return (str(raw) if raw is not None else None, str(protocol) if protocol else None, host)


def fields_from(container: Any) -> list[dict[str, Any]]:
    if not isinstance(container, list):
        return []
    fields: list[dict[str, Any]] = []
    for entry in container:
        if not isinstance(entry, dict):
            continue
        fields.append(
            {
                "key": str(entry.get("key") or ""),
                "type": str(entry.get("type") or "text"),
                "disabled": bool(entry.get("disabled", False)),
            }
        )
    return fields


def value_kind(value: Any) -> str:
    if isinstance(value, dict):
        return "object"
    if isinstance(value, list):
        return "array"
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number"
    return "string"


def infer_shape(values: list[Any]) -> dict[str, Any]:
    """Infer a compact recursive shape from one or more JSON values."""
    by_kind: dict[str, list[Any]] = {}
    for value in values:
        by_kind.setdefault(value_kind(value), []).append(value)
    if len(by_kind) > 1:
        return {
            "kind": "union",
            "variants": [infer_shape(by_kind[kind]) for kind in sorted(by_kind)],
        }

    kind, same_kind_values = next(iter(by_kind.items()))
    if kind == "object":
        objects = same_kind_values
        all_keys = sorted({str(key) for value in objects for key in value})
        fields: dict[str, Any] = {}
        for key in all_keys:
            present = [value[key] for value in objects if key in value]
            fields[key] = {
                "required": len(present) == len(objects),
                "shape": infer_shape(present),
            }
        return {"kind": "object", "fields": fields}
    if kind == "array":
        items = [item for value in same_kind_values for item in value]
        return {
            "kind": "array",
            "items": infer_shape(items) if items else {"kind": "unknown"},
        }
    if kind == "string":
        decoded: list[Any] = []
        all_encoded_json = True
        for value in same_kind_values:
            stripped = value.strip()
            if not stripped or stripped[0] not in "[{":
                all_encoded_json = False
                break
            try:
                decoded.append(json.loads(stripped))
            except json.JSONDecodeError:
                all_encoded_json = False
                break
        if all_encoded_json and decoded:
            return {"kind": "string", "encoding": "json", "decoded": infer_shape(decoded)}
    return {"kind": kind}


def json_shape(value: Any) -> dict[str, Any]:
    return infer_shape([value])


def request_auth_type(request: dict[str, Any], collection: dict[str, Any]) -> str:
    auth = request.get("auth")
    if auth is None:
        auth = collection.get("auth")
    if not isinstance(auth, dict):
        return "inherit-or-none"
    return str(auth.get("type") or "unspecified")


def analyze(collection: dict[str, Any], source_name: str) -> tuple[dict[str, Any], list[str], list[str]]:
    info = collection.get("info")
    if not isinstance(info, dict):
        raise ContractError("collection.info must be an object")

    schema = str(info.get("schema") or "")
    errors: list[str] = []
    warnings: list[str] = []
    if "collection/v2.1.0" not in schema:
        errors.append(f"unsupported Postman schema: {schema or '<missing>'}")

    endpoints: list[dict[str, Any]] = []
    seen: dict[tuple[str, str], str] = {}
    for position, leaf in enumerate(walk_items(collection.get("item")), start=1):
        request = leaf.item.get("request")
        if not isinstance(request, dict):
            errors.append(f"{' / '.join(leaf.folders + (leaf.name,))}: request is not an object")
            continue

        method = str(request.get("method") or "").upper()
        if not method:
            errors.append(f"{' / '.join(leaf.folders + (leaf.name,))}: missing HTTP method")
            method = "UNKNOWN"
        try:
            path = normalized_path(request.get("url"))
        except ContractError as exc:
            errors.append(f"{' / '.join(leaf.folders + (leaf.name,))}: {exc}")
            path = "/<invalid>"

        key = (method, path)
        display_name = " / ".join(leaf.folders + (leaf.name,))
        if key in seen:
            warnings.append(
                f"{method} {path}: multiple operations share one route: "
                f"{seen[key]} and {display_name}; request fields must disambiguate them"
            )
        else:
            seen[key] = display_name

        body = request.get("body")
        if isinstance(body, dict):
            body_mode = str(body.get("mode") or "none")
            body_fields = fields_from(body.get(body_mode)) if body_mode in {"formdata", "urlencoded"} else []
        else:
            body_mode = "none"
            body_fields = []
        if body_mode not in SUPPORTED_BODY_MODES:
            errors.append(f"{method} {path}: unsupported body mode {body_mode!r}")
        for field in body_fields:
            if not field["disabled"] and not field["key"]:
                errors.append(f"{method} {path}: active body field has an empty key")

        url = request.get("url")
        query_fields = fields_from(url.get("query")) if isinstance(url, dict) else []
        raw_url, protocol, host = url_metadata(url)
        if host and host != "api.smspool.net":
            errors.append(f"{method} {path}: unexpected host {host!r}")
        if protocol == "http":
            errors.append(f"{method} {path}: explicit plaintext HTTP is forbidden")
        elif protocol != "https":
            known_missing_scheme = method == "GET" and path == "/business/users" and protocol is None
            finding = f"{method} {path}: URL protocol is {protocol or 'missing'}, expected https"
            if known_missing_scheme:
                warnings.append(finding)
            else:
                errors.append(finding)
        if method == "GET" and body_mode != "none":
            warnings.append(f"{method} {path}: Postman defines a {body_mode} body; SDK should verify query semantics")

        response_entries: list[dict[str, Any]] = []
        invalid_json = 0
        responses = leaf.item.get("response") or []
        if not isinstance(responses, list):
            errors.append(f"{method} {path}: response must be an array")
            responses = []
        for response_index, response in enumerate(responses, start=1):
            if not isinstance(response, dict):
                errors.append(f"{method} {path}: response[{response_index}] is not an object")
                continue
            body_text = response.get("body")
            shape: dict[str, Any] | None = None
            if isinstance(body_text, str) and body_text.strip():
                try:
                    shape = json_shape(json.loads(body_text))
                except json.JSONDecodeError as exc:
                    invalid_json += 1
                    errors.append(
                        f"{method} {path}: response[{response_index}] is not valid JSON: {exc.msg}"
                    )
            response_entries.append(
                {
                    "name": str(response.get("name") or response_index),
                    "code": response.get("code"),
                    "shape": shape,
                }
            )

        if not response_entries:
            warnings.append(f"{method} {path}: no response examples; typed response remains unverified")

        endpoint = {
            "index": position,
            "group": " / ".join(leaf.folders) or "Ungrouped",
            "name": leaf.name,
            "method": method,
            "path": path,
            "raw_url": raw_url,
            "body_mode": body_mode,
            "body_fields": body_fields,
            "query_fields": query_fields,
            "auth": request_auth_type(request, collection),
            "responses": response_entries,
            "invalid_json_responses": invalid_json,
        }
        endpoints.append(endpoint)

    canonical = json.dumps(endpoints, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    method_counts = Counter(endpoint["method"] for endpoint in endpoints)
    body_counts = Counter(endpoint["body_mode"] for endpoint in endpoints)
    group_counts = Counter(endpoint["group"] for endpoint in endpoints)
    response_example_count = sum(len(endpoint["responses"]) for endpoint in endpoints)
    no_example_count = sum(not endpoint["responses"] for endpoint in endpoints)

    baseline = {
        "format_version": 1,
        "source": source_name,
        "collection": {
            "id": info.get("_postman_id"),
            "name": info.get("name"),
            "schema": info.get("schema"),
        },
        "summary": {
            "endpoint_count": len(endpoints),
            "response_example_count": response_example_count,
            "endpoints_without_examples": no_example_count,
            "methods": dict(sorted(method_counts.items())),
            "body_modes": dict(sorted(body_counts.items())),
            "groups": dict(sorted(group_counts.items())),
            "warning_count": len(warnings),
            "error_count": len(errors),
        },
        "contract_sha256": hashlib.sha256(canonical.encode("utf-8")).hexdigest(),
        "endpoints": endpoints,
        "known_warnings": warnings,
    }
    return baseline, errors, warnings


def json_text(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def markdown_cell(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def matrix_text(baseline: dict[str, Any]) -> str:
    summary = baseline["summary"]
    lines = [
        "<!-- Generated by scripts/postman_contract.py; do not edit manually. -->",
        "# Postman endpoint matrix",
        "",
        f"- Contract fingerprint: `{baseline['contract_sha256']}`",
        f"- Endpoints: **{summary['endpoint_count']}**",
        f"- Response examples: **{summary['response_example_count']}**",
        f"- Endpoints without examples: **{summary['endpoints_without_examples']}**",
        "",
        "| # | Group | Method | Path | Body | Active request fields | Example codes |",
        "|---:|---|---|---|---|---|---|",
    ]
    for endpoint in baseline["endpoints"]:
        body_fields = [field["key"] for field in endpoint["body_fields"] if not field["disabled"]]
        query_fields = [field["key"] for field in endpoint["query_fields"] if not field["disabled"]]
        fields = body_fields + [f"query:{field}" for field in query_fields]
        codes = ["?" if response["code"] is None else str(response["code"]) for response in endpoint["responses"]]
        lines.append(
            "| {index} | {group} | `{method}` | `{path}` | `{body}` | {fields} | {codes} |".format(
                index=endpoint["index"],
                group=markdown_cell(endpoint["group"]),
                method=endpoint["method"],
                path=endpoint["path"],
                body=endpoint["body_mode"],
                fields=markdown_cell(", ".join(fields) or "—"),
                codes=markdown_cell(", ".join(codes) or "—"),
            )
        )
    lines.extend(
        [
            "",
            "## Known collection warnings",
            "",
        ]
    )
    warnings = baseline.get("known_warnings") or []
    lines.extend(f"- {warning}" for warning in warnings)
    if not warnings:
        lines.append("- None.")
    lines.append("")
    return "\n".join(lines)


def load_analysis(collection_path: Path) -> tuple[dict[str, Any], list[str], list[str], dict[str, Any]]:
    collection = read_json(collection_path)
    if not isinstance(collection, dict):
        raise ContractError("collection root must be an object")
    try:
        source_name = str(collection_path.relative_to(ROOT))
    except ValueError:
        source_name = str(collection_path)
    baseline, errors, warnings = analyze(collection, source_name)
    return baseline, errors, warnings, collection


def print_findings(baseline: dict[str, Any], errors: list[str], warnings: list[str]) -> None:
    summary = baseline["summary"]
    print(
        "contract: "
        f"{summary['endpoint_count']} endpoints, "
        f"{summary['response_example_count']} response examples, "
        f"fingerprint {baseline['contract_sha256']}"
    )
    for warning in warnings:
        print(f"warning: {warning}", file=sys.stderr)
    for error in errors:
        print(f"error: {error}", file=sys.stderr)


def command_audit(args: argparse.Namespace) -> int:
    baseline, errors, warnings, _ = load_analysis(args.collection)
    print_findings(baseline, errors, warnings)
    return 1 if errors else 0


def command_generate(args: argparse.Namespace) -> int:
    baseline, errors, warnings, _ = load_analysis(args.collection)
    print_findings(baseline, errors, warnings)
    if errors:
        print("refusing to generate artifacts while contract errors exist", file=sys.stderr)
        return 1
    args.baseline.parent.mkdir(parents=True, exist_ok=True)
    args.matrix.parent.mkdir(parents=True, exist_ok=True)
    args.baseline.write_text(json_text(baseline), encoding="utf-8")
    args.matrix.write_text(matrix_text(baseline), encoding="utf-8")
    print(f"wrote {args.baseline}")
    print(f"wrote {args.matrix}")
    return 0


def command_check(args: argparse.Namespace) -> int:
    baseline, errors, warnings, _ = load_analysis(args.collection)
    print_findings(baseline, errors, warnings)
    if errors:
        return 1

    expected = {
        args.baseline: json_text(baseline),
        args.matrix: matrix_text(baseline),
    }
    stale: list[Path] = []
    for path, content in expected.items():
        try:
            actual = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            stale.append(path)
            print(f"error: generated artifact is missing: {path}", file=sys.stderr)
            continue
        if actual != content:
            stale.append(path)
            print(f"error: generated artifact is stale: {path}", file=sys.stderr)
    fingerprint = baseline["contract_sha256"]
    for path in FINGERPRINT_DOCUMENTS:
        try:
            content = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            stale.append(path)
            print(f"error: fingerprint document is missing: {path}", file=sys.stderr)
            continue
        if fingerprint not in content:
            stale.append(path)
            print(f"error: current contract fingerprint is missing from {path}", file=sys.stderr)
    if stale:
        print(
            "run `python3 scripts/postman_contract.py generate`, update manual fingerprint references, "
            "and review the diff",
            file=sys.stderr,
        )
        return 1
    print("generated contract artifacts and documented fingerprints are current")
    return 0


def slug(value: str) -> str:
    normalized = re.sub(r"[^a-zA-Z0-9._-]+", "-", value.strip()).strip("-.").lower()
    return normalized or "unnamed"


def clean_fixture_output(output: Path) -> None:
    """Delete only generator-owned output beneath the dedicated fixture root."""
    if output.is_symlink():
        raise ContractError(f"refusing to clean symlinked fixture path: {output}")
    resolved = output.resolve()
    allowed_root = DEFAULT_FIXTURES.resolve()
    if resolved != allowed_root and allowed_root not in resolved.parents:
        raise ContractError(
            f"--clean is restricted to {allowed_root} or its descendants; got {resolved}"
        )
    marker = resolved / FIXTURE_MARKER
    if not marker.is_file():
        raise ContractError(
            f"refusing to clean fixture directory without generator marker {marker}"
        )
    shutil.rmtree(resolved)


def command_extract_fixtures(args: argparse.Namespace) -> int:
    baseline, errors, warnings, collection = load_analysis(args.collection)
    print_findings(baseline, errors, warnings)
    if errors:
        return 1
    if args.clean and args.output.exists():
        clean_fixture_output(args.output)
    args.output.mkdir(parents=True, exist_ok=True)

    manifest: list[dict[str, Any]] = []
    leaves = list(walk_items(collection.get("item")))
    for endpoint_index, leaf in enumerate(leaves, start=1):
        request = leaf.item["request"]
        method = str(request.get("method") or "unknown").lower()
        path = normalized_path(request.get("url"))
        endpoint_dir = args.output / f"{endpoint_index:03d}-{method}-{slug(path)}"
        responses = leaf.item.get("response") or []
        for response_index, response in enumerate(responses, start=1):
            if not isinstance(response, dict):
                continue
            body = response.get("body")
            if not isinstance(body, str) or not body.strip():
                continue
            try:
                value = json.loads(body)
            except json.JSONDecodeError:
                continue
            code = response.get("code") or "unknown"
            name = slug(str(response.get("name") or response_index))
            filename = f"{response_index:03d}-{code}-{name}.json"
            endpoint_dir.mkdir(parents=True, exist_ok=True)
            fixture_path = endpoint_dir / filename
            fixture_path.write_text(json_text(value), encoding="utf-8")
            manifest.append(
                {
                    "endpoint": f"{request.get('method')} {path}",
                    "group": " / ".join(leaf.folders),
                    "name": leaf.name,
                    "response_name": response.get("name"),
                    "status": response.get("code"),
                    "file": str(fixture_path.relative_to(args.output)),
                }
            )
    (args.output / "manifest.json").write_text(json_text({"fixtures": manifest}), encoding="utf-8")
    (args.output / FIXTURE_MARKER).write_text(
        "Generated by scripts/postman_contract.py; safe for guarded --clean.\n",
        encoding="utf-8",
    )
    print(f"wrote {len(manifest)} fixtures under {args.output}")
    return 0


def path_arg(value: str) -> Path:
    return Path(value).expanduser().resolve()


def add_common_paths(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--collection", type=path_arg, default=DEFAULT_COLLECTION)
    parser.add_argument("--baseline", type=path_arg, default=DEFAULT_BASELINE)
    parser.add_argument("--matrix", type=path_arg, default=DEFAULT_MATRIX)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    audit = subparsers.add_parser("audit", help="validate and summarize the collection")
    audit.add_argument("--collection", type=path_arg, default=DEFAULT_COLLECTION)
    audit.set_defaults(handler=command_audit)

    generate = subparsers.add_parser("generate", help="regenerate committed contract artifacts")
    add_common_paths(generate)
    generate.set_defaults(handler=command_generate)

    check = subparsers.add_parser("check", help="verify committed artifacts match the collection")
    add_common_paths(check)
    check.set_defaults(handler=command_check)

    fixtures = subparsers.add_parser("extract-fixtures", help="extract JSON examples for Rust tests")
    fixtures.add_argument("--collection", type=path_arg, default=DEFAULT_COLLECTION)
    fixtures.add_argument("--output", type=path_arg, default=DEFAULT_FIXTURES)
    fixtures.add_argument("--clean", action="store_true", help="remove the output directory first")
    fixtures.set_defaults(handler=command_extract_fixtures)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        return int(args.handler(args))
    except ContractError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
