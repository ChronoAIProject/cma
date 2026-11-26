#!/usr/bin/env python3
"""Run the staging publication secret scan on the image this CI job tested.

Staging CD refuses to publish an image whose secret scan leaves a blocking
finding, but it runs only after the merge. This runs the same gate, with the
same pinned scanner and reviewed content policy, on the exported CI image, so
the pull request or merge group fails first and names what to fix. CD still
scans the bytes it publishes and stays the authority.

Output follows the scanner contract: no matched value, tool message or image
path is printed. A path is named only when its digest matches a path the
committed content policy already names.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "image-secret-scanner"))

from image_scan_evidence import project  # noqa: E402
from image_secret_gate import ScanRefusal, verified_staging_scan  # noqa: E402

# CI matrix names and the publication component names the gate and policy use.
COMPONENTS = {"web": "web", "api": "am", "cmaeg": "eg", "runtime": "runtime", "runtime-omarchy": "runtime-omarchy"}
POLICY = HERE / "image-content-policy.json"
FIRST_PARTY_BINARIES = {"am": "usr/local/bin/cma", "eg": "usr/local/bin/cmaeg"}
CONVERT_SECONDS = 600
SHOWN_FINDINGS = 20

ADVICE = {
    "scanner_findings": [
        "Staging CD runs this same scan before publishing and would refuse this image after the merge.",
        "If the matched text is a real credential, remove it from the source or build inputs.",
        "If it is public text the rule misreads (for example a Rust panic location laid out next to other",
        "strings), record a reviewed row in .github/scripts/image-content-policy.json: `binary_candidates`",
        "for the am/eg executable, an exact `entries` row otherwise. Then refresh",
        "image-content-manifest.json and pin APPROVED_CONTENT_POLICY in image_scanner_policy.py with",
        "an SA-01 review verdict (see the README in .github/scripts/image-secret-scanner).",
    ],
    "content_refused": [
        "The reviewed content or package evidence did not verify against this image.",
        "Check that image-content-manifest.json matches its files and APPROVED_CONTENT_POLICY pins it.",
    ],
    "receipt_invalid": [
        "The scanner finished but its receipt did not verify, so the result cannot be trusted.",
        "This points at scanner bundle or pin drift, not at the image; rerun once, then report it.",
    ],
    "scanner_timeout": [
        "The scan exceeded its fixed budget. Rerun once; if it repeats, the image grew past the",
        "budget and the scanner limits need a deliberate change.",
    ],
    "scanner_error": [
        "The scanner stopped before completing (exit 2). This is a scanner or classification error,",
        "not a finding; rerun once, then report it with this job link.",
    ],
    "scanner_unavailable": [
        "The scanner could not start. Check the pinned gitleaks install step above; rerun the job.",
    ],
    "scan_refused": [
        "The scan was refused before it produced a verified result; see the stage above.",
    ],
}


def emit(line=""):
    print(line, flush=True)


def summary(lines):
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as stream:
            stream.write("\n".join(lines) + "\n")


def named_paths():
    """Digest -> path for paths the committed policy already names in clear."""
    names = set(FIRST_PARTY_BINARIES.values())
    try:
        catalog = json.loads(POLICY.read_text())
        for section in ("entries", "binary_candidates"):
            names.update(row["path"] for row in catalog.get(section, [])
                         if isinstance(row, dict) and isinstance(row.get("path"), str))
    except (OSError, ValueError, TypeError):
        pass
    return {"path-sha256:" + hashlib.sha256(name.encode("utf-8", "surrogatepass")).hexdigest(): name
            for name in names}


def bundle(directory):
    metadata = json.loads((directory / "metadata.json").read_text())
    archive = directory / metadata["archive"]
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if digest != metadata["archiveSha256"] or archive.stat().st_size != metadata["archiveBytes"]:
        raise ValueError("Exported CI image differs from its metadata")
    return metadata, archive


def convert(archive, output):
    # The same transport conversion staging CD applies before scanning a reused
    # CI image; it changes the archive format, never the image.
    subprocess.run(["skopeo", "copy", "--format", "oci", "docker-archive:" + str(archive),
                    "oci-archive:" + str(output)], check=True, timeout=CONVERT_SECONDS,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def source_snapshot(revision, destination):
    # CD scans against a clean archive of the commit, not a working checkout.
    archive = destination.parent / "source.tar"
    subprocess.run(["git", "-C", str(ROOT), "archive", "--format=tar", "--output=" + str(archive), revision],
                   check=True, timeout=120, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    destination.mkdir()
    subprocess.run(["tar", "-x", "-f", str(archive), "-C", str(destination)], check=True, timeout=120,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    archive.unlink()


def report_refusal(component, error):
    evidence = project(error.evidence if isinstance(error.evidence, dict) else {}, error.outcome)
    outcome = evidence.get("outcome", "scan_refused")
    counts = evidence.get("counts", {})
    findings = evidence.get("findings", [])
    head = f"FAIL ci-image-scan component={component} outcome={outcome}"
    if evidence.get("complete") is True:
        head += " " + " ".join(f"{key}={counts[key]}" for key in ("blocking", "detected", "reviewed") if key in counts)
    emit(head)
    lines = []
    if evidence.get("complete") is not True:
        lines.append(f"  Scan incomplete; partial detections={len(findings) + evidence.get('omitted_findings', 0)}")
        diagnostic = evidence.get("diagnostic")
        if isinstance(diagnostic, dict):
            # The gate projects fixed diagnostic tokens and a content-free digest.
            lines.append(f"  diagnostic={diagnostic['kind']}" +
                         (f" diagnostic_sha256={diagnostic['sha256']}" if "sha256" in diagnostic else ""))
    else:
        names = named_paths()
        for number, finding in enumerate(findings[:SHOWN_FINDINGS], 1):
            path = finding.get("path")
            where = names.get(path, path or "unknown")
            lines.append(f"  finding {number}: rule={finding.get('rule')} scope={finding.get('scope', '?')} "
                         f"layer={finding.get('layer', '?')} path={where} "
                         f"candidate_sha256={finding.get('candidate_sha256', 'none')}")
        hidden = len(findings) - SHOWN_FINDINGS + evidence.get("omitted_findings", 0)
        if hidden > 0:
            lines.append(f"  ... {hidden} more blocking findings")
        if any(f.get("path") not in names for f in findings):
            lines.append("  A path shown as path-sha256 is the SHA256 of the image path without a leading '/'.")
    advice = ADVICE.get(outcome, ADVICE["scan_refused"])
    for line in lines + ["What to do:"] + ["  " + line for line in advice]:
        emit(line)
    emit(f"::error title=Image secret scan refused {component}::{outcome}: staging CD would refuse to "
         "publish this image. See the diagnostic and next steps in the job log.")
    summary([f"### Image secret scan refused `{component}`", "", f"`{head}`", "", "```", *lines, "```", "",
             *advice])


def scan(ci_component, export_dir, revision, work):
    component = COMPONENTS[ci_component]
    metadata, archive = bundle(export_dir)
    if metadata.get("component") != ci_component or metadata.get("sourceRevision") != revision:
        raise ValueError("Exported CI image belongs to another component or revision")
    oci = work / "image.oci.tar"
    convert(archive, oci)
    source = work / "source"
    source_snapshot(revision, source)
    verified_staging_scan(oci, work / "scan", component=component, source=source)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("component", choices=sorted(COMPONENTS))
    parser.add_argument("export", type=Path, help="directory written by export-ci-native-image.py")
    parser.add_argument("revision")
    args = parser.parse_args(argv)
    component = COMPONENTS[args.component]
    temp = Path(os.environ.get("RUNNER_TEMP") or tempfile.gettempdir())
    work = Path(tempfile.mkdtemp(prefix="ci-image-scan-", dir=temp))
    try:
        scan(args.component, args.export, args.revision, work)
    except ScanRefusal as error:
        report_refusal(component, error)
        return 1
    except subprocess.TimeoutExpired as error:
        stage = "convert" if error.cmd[0] == "skopeo" else "source"
        emit(f"FAIL ci-image-scan component={component} stage={stage} reason=command_timeout")
        emit(f"::error title=Image secret scan did not run for {component}::{stage} timed out; rerun the job.")
        return 1
    except (subprocess.CalledProcessError, OSError, ValueError, KeyError) as error:
        stage = "prepare"
        if isinstance(error, subprocess.CalledProcessError):
            stage = "convert" if error.cmd[0] == "skopeo" else "source"
        emit(f"FAIL ci-image-scan component={component} stage={stage} reason={type(error).__name__}")
        emit(f"::error title=Image secret scan did not run for {component}::{stage} failed "
             f"({type(error).__name__}); the exported image or its conversion is unusable.")
        return 1
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
