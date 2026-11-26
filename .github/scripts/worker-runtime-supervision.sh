#!/usr/bin/env bash
# Opt-in real runtime + pinned native Codex/ACP + disposable Mongo continuity proof.
set -euo pipefail
umask 077
repository=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
: "${CHRONO_TEST_MONGODB_URI:?Point this at a disposable local replica set}"
proof=$(mktemp -d "${TMPDIR:-/tmp}/cma-worker-runtime.XXXXXX")
trap 'printf "Retained native continuity evidence: %s\n" "$proof"' EXIT
export CMA_SUPERVISION_EVIDENCE_DIR="$proof/evidence"
mkdir -p "$CMA_SUPERVISION_EVIDENCE_DIR"
export RUST_MIN_STACK=33554432
npm install --prefix "$proof/tools" --ignore-scripts --no-audit --no-fund \
  @openai/codex@0.147.0 @agentclientprotocol/codex-acp@1.4.0 > "$proof/install.log" 2>&1
export CMA_CODEX_TEST_BINARY="$proof/tools/node_modules/.bin/codex"
export CMA_ACP_TEST_BRIDGE="$proof/tools/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
node - "$repository" "$CMA_ACP_TEST_BRIDGE" <<'JS'
const fs = require('node:fs');
const path = require('node:path');
const root = path.join(process.argv[2], 'api/crates/layer3-adapter/src/codex');
const dest = process.argv[3];
let source = fs.readFileSync(dest, 'utf8');
for (const [file, name] of [
  ['persist_conversations', 'persistConversations'], ['subscription_auth', 'subscriptionAuth'],
  ['managed_model_catalog', 'managedModelCatalog'], ['runtime_observations', 'runtimeObservations'],
  ['required_access_tool', 'requiredAccessTool'], ['session_commands', 'sessionCommands'],
  ['agent_instructions', 'agentInstructions'], ['turn_steering', 'turnSteering'],
  ['open_completion', 'openCompletion'],
]) source = require(path.join(root, file + '.cjs'))[name](source);
require('node:assert/strict').equal(require('node:crypto').createHash('sha256').update(source).digest('hex'),
  'ed2ce5c01e53b0a991bab1fd6ccbd44d0182967b3eda44ab21538881be27dc4c');
fs.writeFileSync(dest, source);
JS
export CMA_TEST_RUNTIME_BINARY="$repository/api/target/debug/layer4-runtime"
cargo +1.96.1 build --locked --manifest-path "$repository/api/Cargo.toml" -p layer4-runtime \
  > "$proof/runtime-build.log" 2>&1
node - "$repository" "$proof" <<'JS'
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const root = process.argv[2];
const hash = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const files = [
  '.github/scripts/worker-runtime-supervision.sh', '.github/scripts/worker-runtime-fixture.mjs',
  'api/crates/cma/src/store/mod.rs', 'api/crates/cma/src/store/worker_runtime_supervision_tests.rs',
  'api/crates/cma/src/store/worker_runtime_supervision_tests/native.rs',
  'api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests.rs',
  'api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/work_tests.rs',
  'api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/execution_tests.rs',
];
fs.writeFileSync(path.join(process.argv[3], 'identity.json'), JSON.stringify({
  source_head: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
  source_files: Object.fromEntries(files.map(file => [file, hash(path.join(root, file))])),
  runtime_sha256: hash(process.env.CMA_TEST_RUNTIME_BINARY),
  bridge_sha256: hash(process.env.CMA_ACP_TEST_BRIDGE),
  codex: execFileSync(process.env.CMA_CODEX_TEST_BINARY, ['--version'], { encoding: 'utf8' }).trim(),
  node: process.version, platform: process.platform, arch: process.arch,
  rust: execFileSync('rustc', ['+1.96.1', '--version'], { encoding: 'utf8' }).trim(),
  boundary: 'Host native binaries; controlled loopback model/provider discovery; disposable Mongo replica set.',
}, null, 2));
JS
cargo +1.96.1 test --locked --manifest-path "$repository/api/Cargo.toml" -p cma \
  store::worker_runtime_supervision_tests:: -- --ignored --nocapture --test-threads=1 \
  2>&1 | tee "$proof/tests.log"
