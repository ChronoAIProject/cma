"""Generate an inert diagnostic patch; never apply it or invoke a workload."""

import argparse
import difflib
import hashlib
import json
from pathlib import Path
import re
import subprocess
import uuid

HERE = Path(__file__).resolve().parent
RECORDS = "api/crates/cmaeg/src/records/"
ACTIONS = {
    "pre_submit_refusal": 0,
    "pre_submit_stall": 1,
    "post_commit_unknown": 2,
}


def digest(value):
    return hashlib.sha256(value).hexdigest()


def canonical_uuid(value):
    if not isinstance(value, str):
        raise ValueError("fixture ID must be a canonical UUIDv4")
    parsed = uuid.UUID(value)
    if parsed.version != 4 or str(parsed) != value:
        raise ValueError("fixture ID must be a canonical UUIDv4")
    return parsed


def validate(config):
    if (
        not isinstance(config, dict)
        or set(config)
        != {"schema", "database_name", "admission_root", "case_id", "action"}
        or config["schema"] != "eg_records_fault_v1"
    ):
        raise ValueError("invalid diagnostic configuration fields")
    database = config["database_name"]
    if not isinstance(database, str) or not re.fullmatch(
        r"eg_diag_[0-9a-f]{32}", database
    ):
        raise ValueError("dedicated generated fixture database required")
    if uuid.UUID(hex=database[8:]).version != 4:
        raise ValueError("fixture database must contain a UUIDv4")
    canonical_uuid(config["admission_root"])
    canonical_uuid(config["case_id"])
    if not isinstance(config["action"], str) or config["action"] not in ACTIONS:
        raise ValueError("unsupported diagnostic action")


def unique_replace(value, before, after):
    if value.count(before) != 1:
        raise ValueError("source boundary does not match reviewed template")
    return value.replace(before, after, 1)


def render(repository, revision, config):
    validate(config)
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("exact immutable revision required")
    baseline = json.loads((HERE / "source-base.json").read_text())
    originals = {}
    for path, expected in baseline["source_sha256"].items():
        data = subprocess.check_output(
            ["git", "show", f"{revision}:{path}"], cwd=repository
        )
        if digest(data) != expected:
            raise ValueError(
                "relevant source differs from reviewed patch base: " + path
            )
        originals[path] = data.decode()
    source_path = RECORDS + "mongo_replicas.rs"
    source = unique_replace(
        originals[source_path],
        "use serde::{Deserialize, Serialize};\n",
        "use serde::{Deserialize, Serialize};\n\n"
        '#[path = "mongo_diagnostic_fault.rs"]\nmod diagnostic_fault;\n',
    )
    source = unique_replace(
        source,
        "        observation.phase(Phase::Session);\n",
        "        let diagnostic_fault = diagnostic_fault::select(self.db.name(), writes);\n"
        "        if let Some(fault) = &diagnostic_fault {\n"
        "            fault.before_submission(observation).await?;\n        }\n"
        "        observation.phase(Phase::Session);\n",
    )
    source = unique_replace(
        source,
        "        Ok(versions)\n",
        "        if let Some(fault) = &diagnostic_fault {\n"
        "            fault.after_commit(observation)?;\n        }\n"
        "        Ok(versions)\n",
    )
    encoded = json.dumps(config, sort_keys=True, separators=(",", ":")).encode()
    values = {
        "fixture_sha256": digest(encoded),
        "database_sha256": digest(config["database_name"].encode()),
        "admission_sha256": digest(config["admission_root"].encode()),
        "case_sha256": digest(canonical_uuid(config["case_id"]).bytes),
    }
    fields = "\n".join(f'    {name}: "{value}",' for name, value in values.items())
    generated_config = (
        "//! Generated compile-time selectors; no runtime control interface.\n"
        "use super::{Action, Fixture};\nuse std::sync::atomic::AtomicBool;\n\n"
        "pub(super) static FIXTURE: Fixture = Fixture {\n" + fields + "\n"
        f"    action: Action::from_id({ACTIONS[config['action']]}),\n"
        "    fired: AtomicBool::new(false),\n};\n"
    )
    patched = {
        source_path: source,
        RECORDS + "mongo_diagnostic_fault.rs": (
            HERE / "mongo_diagnostic_fault.rs.in"
        ).read_text(),
        RECORDS + "mongo_diagnostic_fault_tests.rs": (
            HERE / "mongo_diagnostic_fault_tests.rs.in"
        ).read_text(),
        RECORDS + "mongo_diagnostic_config.rs": generated_config,
    }
    chunks = []
    for path, value in patched.items():
        chunks.extend(
            difflib.unified_diff(
                originals.get(path, "").splitlines(keepends=True),
                value.splitlines(keepends=True),
                fromfile="a/" + path if path in originals else "/dev/null",
                tofile="b/" + path,
            )
        )
    patch = "".join(chunks)
    binding = {
        "schema": "eg_records_fault_patch_v1",
        "base_revision": revision,
        "base_tree": subprocess.check_output(
            ["git", "rev-parse", revision + "^{tree}"], cwd=repository, text=True
        ).strip(),
        "config": config,
        "selectors": values,
        "base_source_sha256": baseline["source_sha256"],
        "patched_source_sha256": {
            path: digest(value.encode()) for path, value in patched.items()
        },
        "template_sha256": {
            name: digest((HERE / name).read_bytes())
            for name in [
                "build_patch.py",
                "source-base.json",
                "mongo_diagnostic_fault.rs.in",
                "mongo_diagnostic_fault_tests.rs.in",
            ]
        },
        "patch_sha256": digest(patch.encode()),
        "runtime_or_execution_authorization": False,
        "fault_scope": "Once per process for one exact database/root; process restart resets the latch.",
        "known_abort": "Not injected; requires separately reviewed owner Mongo fault and observed real transaction provenance.",
    }
    return patch, patched, binding


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    patch, patched, binding = render(
        args.repository, args.revision, json.loads(args.config.read_text())
    )
    # Refuse replacement of an existing packet; cases and failed history are immutable.
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "diagnostic.patch").write_text(patch)
    (args.output / "binding.json").write_text(json.dumps(binding, indent=2) + "\n")
    for path, value in patched.items():
        destination = args.output / "source" / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(value)
    print(
        json.dumps(
            {"patch_sha256": binding["patch_sha256"], "selectors": binding["selectors"]}
        )
    )


if __name__ == "__main__":
    main()
