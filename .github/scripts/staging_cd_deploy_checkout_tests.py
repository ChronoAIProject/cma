"""Exercise the deploy job's private checkout offline against the controller guard."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

from staging_cd_checkout import checkout_path
from staging_cd_common import Refusal
from staging_cd_invoke import private_script


WORKFLOW = Path(__file__).resolve().parents[1] / "workflows/staging-deploy.yml"


def git(repository, *args):
    return subprocess.run(
        ["git", "-C", str(repository), *args], check=True, capture_output=True,
        text=True, timeout=15, env={"PATH": os.environ["PATH"], "GIT_CONFIG_NOSYSTEM": "1",
                                   "GIT_CONFIG_GLOBAL": os.devnull, "GIT_TERMINAL_PROMPT": "0"},
    ).stdout.strip()


def fetch_checkout(root, source, revision, inputs):
    """Apply the pinned checkout action's fetch and sparse inputs to a local remote."""
    root.mkdir(parents=True)
    git(root, "init")
    git(root, "remote", "add", "origin", source.as_uri())
    depth = int(inputs.get("fetch-depth", 1))
    patterns = [line.strip() for line in inputs.get("sparse-checkout", "").splitlines() if line.strip()]
    # actions/checkout implicitly requests a partial clone for a sparse checkout.
    fetch_filter = inputs.get("filter") or ("blob:none" if patterns else None)
    options = [f"--depth={depth}"] if depth else []
    if fetch_filter:
        options.append("--filter=" + fetch_filter)
    git(root, "fetch", *options, "origin", revision)
    if patterns:
        if str(inputs.get("sparse-checkout-cone-mode", True)).lower() == "true":
            git(root, "sparse-checkout", "set", *patterns)
        else:
            git(root, "config", "core.sparseCheckout", "true")
            (root / ".git/info/sparse-checkout").write_text("\n".join(patterns) + "\n")
    git(root, "checkout", "--force", revision)


class DeployCheckoutTests(unittest.TestCase):
    def test_workflow_checkout_passes_the_real_guard_without_credentials(self):
        steps = yaml.safe_load(WORKFLOW.read_text())["jobs"]["deploy"]["steps"]
        checkouts = [step for step in steps if step.get("uses", "").startswith("actions/checkout@")
                     and "repository" in step.get("with", {})]
        # A transient failure repeats the identical checkout once; only the repeat can fail the job.
        first, retry = checkouts
        self.assertEqual(first["with"], retry["with"])
        self.assertIs(first["continue-on-error"], True)
        # The repeat keeps the first attempt's own gate (an overtaken set skips both).
        self.assertEqual(first["if"], "steps.release.outputs.disposition == ''")
        self.assertEqual(retry["if"], f"{first['if']} && steps.{first['id']}.outcome == 'failure'")
        self.assertNotIn("continue-on-error", retry)
        inputs = first["with"]
        self.assertEqual(str(inputs["persist-credentials"]).lower(), "false")
        self.assertEqual(inputs["ref"], "${{ steps.private.outputs.revision }}")
        coordinate = next(step for step in steps if step.get("id") == "coordinate")
        self.assertEqual(coordinate["env"]["CD_INFRA_REF"], inputs["ref"])

        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            source = workspace / "source"
            source.mkdir()
            git(source, "init")
            git(source, "config", "uploadpack.allowFilter", "true")
            script = Path("staging/scripts/deploy_set.py")
            (source / script).parent.mkdir(parents=True)
            (source / script).write_text("# synthetic controller\n")
            # Status may need the root ignore rules even when inspecting only staging.
            (source / ".gitignore").write_text("*.pyc\n")
            git(source, "add", ".")
            git(source, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "-m", "Seed the controller fixture")
            revision = git(source, "rev-parse", "HEAD")
            env = {"GITHUB_WORKSPACE": str(workspace), "GITHUB_RUN_ID": "123",
                   "GITHUB_RUN_ATTEMPT": "2", "CD_INFRA_REF": revision, "PATH": os.environ["PATH"]}
            path = inputs["path"].replace("${{ github.run_id }}", env["GITHUB_RUN_ID"])
            root = workspace / path.replace("${{ github.run_attempt }}", env["GITHUB_RUN_ATTEMPT"])
            self.assertEqual(root, checkout_path(env))
            fetch_checkout(root, source, revision, inputs)
            # Removing the local remote models losing the installation token after checkout.
            source.rename(workspace / "offline")
            self.assertEqual(private_script(env), root / script)
            (root / script).write_text("# changed after checkout\n")
            with self.assertRaises(Refusal):
                private_script(env)
