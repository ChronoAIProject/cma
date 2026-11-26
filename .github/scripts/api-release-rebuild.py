#!/usr/bin/env python3
"""Compare Cargo compiler artifacts from two locked release builds.

Capture metadata via cargo metadata --locked --no-deps --format-version 1.
Build once to warm the target directory. Capture cargo build --release --locked
--bin NAME --message-format=json before and after the implementation edit.
Use the same workspace, target directory, command and features for both runs.
Cargo package IDs can contain local paths. Keep the input and output private.

Exit 0 for a passing comparison with or without a pending extraction, 1 for a
failed comparison, or 2 for invalid evidence or arguments.
"""

import argparse
import json
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Optional


@dataclass(frozen=True)
class Target:
    package: str
    kind: str
    name: str

    @classmethod
    def parse(cls, value):
        parts = value.split(":")
        if len(parts) != 3 or not all(parts):
            raise argparse.ArgumentTypeError("expected PACKAGE:KIND:TARGET")
        return cls(*parts)

    def __str__(self):
        return f"{self.package}:{self.kind}:{self.name}"


@dataclass(frozen=True)
class Artifact:
    package_id: str
    name: str
    kinds: tuple
    features: tuple
    executable: Optional[str]
    profile: dict
    fresh: bool


def read_workspace(path):
    data = json.loads(path.read_text(encoding="utf-8"))
    members = set(data["workspace_members"])
    workspace = {package["id"]: package["name"] for package in data["packages"] if package["id"] in members}
    if not workspace or workspace.keys() != members or len(set(workspace.values())) != len(workspace):
        raise ValueError("workspace metadata lacks unambiguous package IDs")
    return workspace


def read_artifacts(path, workspace):
    completions = []
    artifacts = []
    with path.open(encoding="utf-8") as stream:
        for line in stream:
            event = json.loads(line)
            reason = event["reason"]
            if reason == "build-finished":
                completions.append(event["success"])
            if reason != "compiler-artifact" or event["package_id"] not in workspace:
                continue
            if type(event["fresh"]) is not bool:
                raise ValueError("compiler artifact lacks a boolean fresh flag")
            profile = event["profile"]
            executable = event["executable"]
            if not isinstance(profile, dict) or type(profile["test"]) is not bool:
                raise ValueError("compiler artifact lacks a Cargo profile with a boolean test flag")
            if executable is not None and not isinstance(executable, str):
                raise ValueError("compiler artifact has an invalid executable path")
            target = event["target"]
            artifacts.append(Artifact(
                event["package_id"], target["name"], tuple(target["kind"]), tuple(sorted(event["features"])),
                executable, profile, event["fresh"],
            ))
    if len(completions) != 1 or completions[0] is not True:
        raise ValueError(f"{path.name} lacks one successful Cargo build completion")
    return artifacts


def is_release_executable(artifact):
    if not artifact.executable or artifact.profile["test"]:
        return False
    path = Path(artifact.executable)
    return (
        path.is_absolute()
        and path.parent.name == "release"
        and path.name in (artifact.name, f"{artifact.name}.exe")
    )


def check_target(target, fresh, package_id, baseline, edited):
    def matches(artifact):
        return (
            artifact.package_id == package_id
            and artifact.name == target.name
            and target.kind in artifact.kinds
        )

    def compile_units(artifacts):
        return {
            (artifact.kinds, artifact.features, json.dumps(artifact.profile, sort_keys=True))
            for artifact in artifacts
        }

    before = [artifact for artifact in baseline if matches(artifact)]
    after = [artifact for artifact in edited if matches(artifact)]
    if not before or not after:
        return f"missing artifact for {target}"
    if target.kind == "bin":
        if not all(is_release_executable(artifact) for artifact in before + after):
            return f"expected an ordinary release executable for {target}"
        before_paths = {artifact.executable for artifact in before}
        after_paths = {artifact.executable for artifact in after}
        if len(before_paths) != 1 or before_paths != after_paths:
            return f"release executable path changed for {target}"
    if compile_units(before) != compile_units(after):
        return f"target, features or Cargo profile changed for {target}"
    if any(artifact.fresh != fresh for artifact in after):
        return f"expected {target} to be {'fresh' if fresh else 'rebuilt'}"
    return None


def compare(workspace, baseline, edited, current, extraction):
    package_by_name = {name: package_id for package_id, name in workspace.items()}
    current_errors = []
    if not baseline or not all(artifact.fresh for artifact in baseline):
        current_errors.append("baseline workspace artifacts are not all fresh")
    for target, fresh in current:
        error = check_target(target, fresh, package_by_name.get(target.package), baseline, edited)
        if error:
            current_errors.append(error)

    missing_packages = sorted({target.package for target, _ in extraction} - package_by_name.keys())
    extraction_errors = []
    for target, fresh in extraction:
        if target.package in package_by_name:
            error = check_target(target, fresh, package_by_name[target.package], baseline, edited)
            if error:
                extraction_errors.append(error)
    extraction_status = "pass"
    if not extraction:
        extraction_status = "unrequested"
    elif extraction_errors:
        extraction_status = "fail"
    elif missing_packages:
        extraction_status = "pending"

    return {
        "current": {"status": "fail" if current_errors else "pass", "errors": current_errors},
        "extraction": {
            "status": extraction_status, "missing_packages": missing_packages, "errors": extraction_errors,
        },
        "artifacts": {
            phase: [dict(asdict(row), package=workspace[row.package_id]) for row in artifacts]
            for phase, artifacts in (("baseline", baseline), ("edited", edited))
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("metadata", type=Path, help="Cargo workspace metadata JSON")
    parser.add_argument("baseline", type=Path, help="Warm Cargo build JSON stream")
    parser.add_argument("edited", type=Path, help="Cargo build JSON stream after the edit")
    target_option = {"type": Target.parse, "metavar": "PACKAGE:KIND:TARGET"}
    parser.add_argument("--implementation", **target_option, required=True, help="Current target that must rebuild")
    parser.add_argument("--binary", **target_option, required=True, help="Service executable that must relink")
    parser.add_argument("--fresh", **target_option, action="append", default=[],
                        help="Current sibling target that must stay fresh (repeatable)")
    parser.add_argument("--extracted-implementation", **target_option, action="append", default=[],
                        help="Extracted target that must rebuild if present (repeatable)")
    parser.add_argument("--extracted-fresh", **target_option, action="append", default=[],
                        help="Extracted sibling target that must stay fresh if present (repeatable)")
    args = parser.parse_args()
    if args.binary.kind != "bin":
        parser.error("--binary must select a bin target")
    current = [(target, False) for target in dict.fromkeys((args.implementation, args.binary))]
    current.extend((target, True) for target in args.fresh)
    extraction = [(target, False) for target in args.extracted_implementation]
    extraction.extend((target, True) for target in args.extracted_fresh)
    try:
        workspace = read_workspace(args.metadata)
        report = compare(
            workspace, read_artifacts(args.baseline, workspace), read_artifacts(args.edited, workspace),
            current, extraction,
        )
    except (OSError, KeyError, TypeError, ValueError) as error:
        parser.exit(2, f"Invalid Cargo evidence: {error}\n")
    print(json.dumps(report, indent=2))
    return int(any(report[comparison]["status"] == "fail" for comparison in ("current", "extraction")))


if __name__ == "__main__":
    raise SystemExit(main())
