#!/usr/bin/env python3
"""Compile the pinned predecessor's real store code against a segment tripwire.

Run under the caller's normal build/resource controls. The canonical disposable
Mongo runner owns the fixture and ignores any caller-provided database URI.
The archive receives only a test module; predecessor production code is intact.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[2]
# #1342 merged, before any log-segment reader/writer implementation.
PREDECESSOR = "0932e58cb6c821c4cdc69f2cb5d82fe538b8c2b6"
SCRATCH = ROOT / "api" / "target" / "agent-log-predecessor"

TEST = r'''
#[tokio::test]
#[ignore = "requires the disposable replica-set contract runner"]
async fn predecessor_segment_tripwire_rejects_read_and_append_without_changing_source() {
    let (client, name) = database("segment_predecessor").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, agent) = worktree_and_agent(&SandboxId("sbx_predecessor".into()));
    store.create_agent(agent.clone()).await.unwrap();
    // The predecessor still reads and writes the unchanged version-1 shape.
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent.clone()));
    store.append_agent_log(&agent.id, NOW.into(), AgentLogKind::ModeChanged { mode: "code".into() }).await.unwrap();
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":{
        "document_schema_version":2_i64, "agent_log_format":"segmented_v1",
        "agent_log_incarnation":"a".repeat(64), "agent_log_head_revision":1_i64,
    }}).await.unwrap();
    let before = store.agents.find_one(doc! {"_id":agent.id.as_str()}).await.unwrap().unwrap();
    let rows = store.agent_log.count_documents(doc! {"agent_id":agent.id.as_str()}).await.unwrap();
    assert!(store.get_agent(&agent.id).await.is_err());
    assert!(store.append_agent_log(&agent.id, NOW.into(), AgentLogKind::ModeChanged { mode: "different".into() }).await.is_err());
    assert_eq!(store.agents.find_one(doc! {"_id":agent.id.as_str()}).await.unwrap().unwrap(), before);
    assert_eq!(store.agent_log.count_documents(doc! {"agent_id":agent.id.as_str()}).await.unwrap(), rows);
    client.database(&name).drop().await.unwrap();
}
'''


def main():
    assert not SCRATCH.exists(), "Refusing to overwrite an existing predecessor gate"
    commit = subprocess.check_output(
        ["git", "rev-parse", "--verify", f"{PREDECESSOR}^{{commit}}"], cwd=ROOT, text=True
    ).strip()
    SCRATCH.mkdir(parents=True)
    try:
        archive = SCRATCH / "source.tar"
        subprocess.run(["git", "archive", "--format=tar", f"--output={archive}", commit, "api"], cwd=ROOT, check=True)
        with tarfile.open(archive) as source:
            source.extractall(SCRATCH, filter="data")
        archive.unlink()
        module = SCRATCH / "api/crates/cma/src/store/sandbox_mongo_integration_tests.rs"
        module.write_text(module.read_text() + TEST)
        runner = (ROOT / ".github/scripts/mongo-contract-tests.sh").read_text()
        start = runner.index("script_directory=")
        end = runner.index("unset CHRONO_TEST_MONGODB_URI", start)
        runner = runner[:start] + f"repo_root='{SCRATCH}'\nreadonly repo_root\n" + runner[end:]
        start = runner.index("nextest_contracts() {")
        end = runner.index("\nif [[ $# == 0 ]]", start)
        runner = runner[:start] + '''nextest_contracts() {
  cargo nextest "$@" -p cma --locked --run-ignored only \\
    -E 'test(predecessor_segment_tripwire_rejects_read_and_append_without_changing_source)'
}
''' + runner[end:]
        script = SCRATCH / "mongo-predecessor.sh"
        script.write_text(runner)
        env = {**os.environ, "CARGO_TARGET_DIR": str(SCRATCH / "build")}
        print(f"Testing predecessor store executable from {commit}", flush=True)
        subprocess.run(["bash", str(script), "count:1/1"], cwd=ROOT, env=env, check=True)
    finally:
        # Only the exact owned archive/build directory is removed.
        assert SCRATCH.parent == ROOT / "api" / "target"
        shutil.rmtree(SCRATCH)


if __name__ == "__main__":
    main()
