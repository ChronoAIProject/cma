"""Expose observed cache usage, projected relief and the outcome of maintenance."""
import json
import sys
from pathlib import Path

from actions_cache_retention import BUDGET


def retained_family(key):
    for prefix, name in (("buildkit-blob-", "shared_buildkit_blobs"), ("index-", "buildkit_indexes"),
                         ("cma-cd-cargo-v2-", "historical_cargo"), ("cma-cd-cargo-v3-", "historical_cargo"),
                         ("cma-cd-cargo-v4-", "v4_cargo"), ("codeql-overlay-base-", "codeql_bases"),
                         ("v0-rust-build-Linux-x64-", "rust_ci_build"), ("v0-rust-lint-Linux-x64-", "rust_ci_lint")):
        if key.startswith(prefix):
            return name
    return "other"


def publish_result(result, env):
    status = result["status"]
    if status == "execution-error":
        text = f"Retention failed during {result['phase']} ({result['error_class']}). Actual recovery is unverified."
    else:
        text = (f"Observed usage is {result['observed_bytes']} bytes. Projected usage is "
                f"{result['projected_bytes']} bytes. Admission requires usage below {BUDGET} bytes. "
                f"Observed headroom is {BUDGET - result['observed_bytes']} bytes and projected headroom is "
                f"{BUDGET - result['projected_bytes']} bytes. Confirmed deletions totaled "
                f"{result['deleted_bytes']} bytes.")
        if result["observed_bytes_to_admit"]:
            text += f" Another {result['observed_bytes_to_admit']} bytes must be freed for admission."
    print(json.dumps(result), flush=True)
    if status in {"execution-error", "proof-incomplete"}:
        print("::error::" + text, file=sys.stderr)
    elif result["observed_bytes_to_admit"]:
        print("::warning::" + text, file=sys.stderr)
    if env.get("GITHUB_STEP_SUMMARY"):
        with Path(env["GITHUB_STEP_SUMMARY"]).open("a") as summary:
            summary.write(f"### Cache retention — {status}\n\n{text}\n")
            if "observed_bytes" in result:
                post = result["after_bytes"]
                summary.write(f"\nUsage before the sweep was {result['before_bytes']} bytes. ")
                summary.write(f"Usage after recount was {post} bytes.\n" if post is not None else
                              "This preview made no deletions.\n")
                for heading, counts in (("Held candidates", result["held_by_reason"]),
                                        ("Retained bytes by family", result["retained_by_family"])):
                    if counts:
                        summary.write(f"\n#### {heading}\n\n")
                        for reason, size in sorted(counts.items()):
                            summary.write(f"- {reason} ({size} bytes)\n")
    if env.get("GITHUB_OUTPUT"):
        with Path(env["GITHUB_OUTPUT"]).open("a") as output:
            output.write(f"status={status}\n")
            for field in ("before_bytes", "after_bytes", "observed_bytes_to_admit", "deleted_bytes"):
                if result.get(field) is not None:
                    output.write(f"{field}={result[field]}\n")
    if status in {"execution-error", "proof-incomplete"}:
        return 1
    return 3 if status == "target-unmet" else 0
