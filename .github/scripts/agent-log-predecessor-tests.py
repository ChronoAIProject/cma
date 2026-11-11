#!/usr/bin/env python3
"""Execute the actual pre-segmentation store against the format tripwire.

Run under a shared heavy lock with CARGO_BUILD_JOBS=2 and the worktree's one
CARGO_TARGET_DIR. The canonical runner creates and removes a private replica
set. Only a test module is injected into archived predecessor source; no old
production code is replaced. The permitted release rollback artifact still
requires separate qualification under the release's reader-floor contract.
"""

import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PREDECESSOR = "0932e58cb6c821c4cdc69f2cb5d82fe538b8c2b6"
TEST_NAME = "predecessor_refuses_segmented_reads_and_writes_without_source_changes"
TEST = r'''
use super::*;
use futures_util::TryStreamExt;

#[tokio::test]
#[ignore = "requires the disposable replica-set contract runner"]
async fn predecessor_refuses_segmented_reads_and_writes_without_source_changes() {
    let (client, name) = database("segment_predecessor").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, agent) = worktree_and_agent(&SandboxId("sbx_predecessor".into()));
    store.create_agent(agent.clone()).await.unwrap();
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent.clone()));
    store.append_agent_log(&agent.id, NOW.into(), AgentLogKind::ModeChanged {
        mode: "code".into(),
    }).await.unwrap();
    assert_eq!(store.agent_log(&agent.id, 0, 10).await.unwrap().len(), 2);
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":{
        "document_schema_version":2_i64, "agent_log_format":"segmented_v1",
        "agent_log_incarnation":"a".repeat(64), "agent_log_head_revision":1_i64,
    }}).await.unwrap();
    let before = store.agents.find_one(doc! {"_id":agent.id.as_str()})
        .await.unwrap().unwrap();
    let source = store.agent_log.clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .find(doc! {"agent_id":agent.id.as_str()}).sort(doc! {"seq":1})
        .await.unwrap().try_collect::<Vec<_>>().await.unwrap();
    assert!(store.get_agent(&agent.id).await.is_err());
    assert!(store.agent_log(&agent.id, 0, 10).await.is_err());
    assert!(store.append_agent_log(&agent.id, NOW.into(), AgentLogKind::ModeChanged {
        mode: "different".into(),
    }).await.is_err());
    assert_eq!(store.agents.find_one(doc! {"_id":agent.id.as_str()})
        .await.unwrap().unwrap(), before);
    assert_eq!(store.agent_log.clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .find(doc! {"agent_id":agent.id.as_str()}).sort(doc! {"seq":1})
        .await.unwrap().try_collect::<Vec<_>>().await.unwrap(), source);
    client.database(&name).drop().await.unwrap();
}
'''


def fixture_runner(scratch):
    """Reuse the canonical image, resource limits, startup, and cleanup body."""
    source = (ROOT / ".github/scripts/mongo-contract-tests.sh").read_text()
    marker = "# This entry point always creates its own disposable replica set."
    if source.count(marker) != 1:
        raise RuntimeError("Canonical Mongo runner changed; review the fixture adapter")
    fixture = source[source.index(marker):]
    return "\n".join([
        "#!/usr/bin/env bash", "set -euo pipefail",
        "unset CHRONO_TEST_MONGODB_URI CHRONO_TEST_MONGODB_FAILPOINT_URI CMA_NEXTEST_REUSE",
        f"readonly repo_root={shlex.quote(str(scratch))}",
        "selection=ordinary", "partition=count:1/1",
        "nextest_contracts() {",
        '  cargo nextest "$@" --locked -p cma --bin cma --run-ignored only --no-tests fail \\\n    -E ' + shlex.quote(f"test(={TEST_PATH}::{TEST_NAME})"),
        "}", fixture,
    ])


TEST_PATH = "store::sandbox_mongo_integration_tests::agent_log_predecessor_qualification"


def main():
    target_value = os.environ.get("CARGO_TARGET_DIR")
    if not target_value or os.environ.get("CARGO_BUILD_JOBS") != "2":
        raise RuntimeError("Use the shared heavy lock, one explicit Cargo target and two jobs")
    target = Path(target_value).resolve()
    if not target.is_relative_to(ROOT / "api/target") or target == ROOT / "api/target":
        raise RuntimeError("The shared target must be owned by this worktree")
    if shutil.disk_usage(ROOT).free < 35 * 1024**3:
        raise RuntimeError("Insufficient headroom for predecessor qualification")
    commit = subprocess.check_output(
        ["git", "rev-parse", "--verify", f"{PREDECESSOR}^{{commit}}"],
        cwd=ROOT, text=True,
    ).strip()
    target.mkdir(parents=True, exist_ok=True)
    # This is a source archive inside the existing target, not another worktree
    # or another Cargo target. TemporaryDirectory removes only this archive.
    with tempfile.TemporaryDirectory(prefix="predecessor-source-", dir=target) as temp:
        scratch = Path(temp)
        archive = scratch / "source.tar"
        subprocess.run(
            ["git", "archive", "--format=tar", f"--output={archive}", commit, "api"],
            cwd=ROOT, check=True,
        )
        with tarfile.open(archive) as source:
            source.extractall(scratch, filter="data")
        archive.unlink()
        store = scratch / "api/crates/cma/src/store"
        parent = store / "sandbox_mongo_integration_tests.rs"
        production_before = parent.read_bytes()
        test_path = store / "sandbox_mongo_integration_tests/agent_log_predecessor_qualification.rs"
        if test_path.exists():
            raise RuntimeError("Qualification test unexpectedly exists in predecessor")
        test_path.write_text(TEST)
        parent.write_bytes(production_before + b"\nmod agent_log_predecessor_qualification;\n")
        script = scratch / "mongo-predecessor.sh"
        script.write_text(fixture_runner(scratch))
        print(f"Testing original predecessor store at {commit}", flush=True)
        # Build before allocating Mongo resources. The subsequent test run
        # reuses these artifacts in the same caller-owned target directory.
        subprocess.run(
            ["cargo", "nextest", "list", "--locked", "-p", "cma", "--bin", "cma",
             "--run-ignored", "only", "-E", f"test(={TEST_PATH}::{TEST_NAME})"],
            cwd=scratch / "api", check=True,
        )
        subprocess.run(["bash", str(script)], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
