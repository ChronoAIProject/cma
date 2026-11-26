import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const standingPermission = 'auth::account_standing::mongo_tests::replicas::fresh_snapshot_cannot_mask_real_database_permission_failure';
const activeProofOperator = 'package(=cma) and test(=store::agent_log_segments::active_proof::prepare::tests::active_proof_operator_cli_uses_generated_isolated_role)';

const fixedPinMachine = 'package(=cma) and test(=store::native_warm_pools::retirement::machine_selection::tests::fixed_pin_machine_selection_works_without_released_runtime_grant)';
const gatewayResetJourneys = 'package(=cma) and (test(=store::bindings::reset_tests::gateway_reset_tests::configured_gateway_replays_real_m1_reset_and_preserves_both_scope_floors) or test(=store::bindings::reset_tests::gateway_reset_tests::profile_reset_receipt_replays_after_a_later_channel_revision))';

// Independent inventory of every invocation, including replica delivery and
// the exact owner-key contract. Check complete arguments in source and archive mode.
const selections = [
  ['package(=cmaeg) and kind(lib) and test(~console::session_backend::)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=console) and test(~shared_)', 'only', true],
  ['package(=cma-service-migrations) and kind(lib)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~migrations::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~optional_forms_storage)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~optional_media_continuation_storage)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~transaction_terminal::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~activity::history::mongo_tests::)', 'only', true],
  ['package(=cmaeg) and test(=worker_credentials::mongo::integration_tests::worker_history_cross_client_visibility_append_only_grants_and_cutover_barrier)', 'only', false],
  ['package(=cmaeg) and kind(lib) and test(=channels::create_cleanup_tests::retirement_at_the_record_limit_preserves_tracked_worker_credentials)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~whatsapp_retention_on_real_mongo)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=mongo_isolation)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(~github_integrations::)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(~organization_channels::)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(=record_management::mongo_legacy_read_is_owned_and_read_only)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=github_source)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~tools::threads::ledger::mongo_tests::)', 'only', true],
  ['package(=cma) and (test(~store::isolation::tests::real_) or test(~store::migrations::tests::segments_runtime::))', 'only', false],
  ['package(=cmaeg) and kind(test) and binary(=management) and test(~binary::compiled_binary_)', 'all', true],
  ['package(=cmaeg) and kind(test) and binary(=relay_ingress) and test(~runner_binary::)', 'only', true],
  ['package(=cmaeg) and kind(test) and binary(=replica_admission_mongo)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~replicas::partition_discovery::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(=records::mongo::replica_schema::tests::replica_migration_recovers_after_second_index_conflict)', 'only', true],
  ['package(=cmaeg) and kind(lib) and (test(~channels::create_fence_tests::) or test(~records::mongo::create_fences::tests::))', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~turns::routed::replica_tool_tests::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~turns::routed::replica_legacy_tool_tests::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~turns::routed::compatibility_mongo_tests::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~turns::routed::tool_relay::mongo_tests::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~turns::routed::tool_effects::mongo_)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~turns::routed::media_effects::tests::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~replicas::membership::capability::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~replicas::unresolved::)', 'only', true],
  ['package(=cmaeg) and kind(lib) and test(~replicas::mongo_tests::)', 'only', true],
  ['package(=cma) and test(~gateway::tests::)', 'only', false],
  ['package(=cma) and test(~sender_context_tests::)', 'only', true],
  [gatewayResetJourneys, 'only', true],
  [`package(=cma) and test(=${standingPermission})`, 'only', true],
  [fixedPinMachine, 'only', true],
  [activeProofOperator, 'only', true],
];

function fixture(reuse, scenario = 'success') {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-authenticated-mongo-runner-')));
  const root = join(directory, 'repo');
  const scripts = join(root, '.github/scripts');
  const metadata = join(root, 'api/target/nextest');
  mkdirSync(scripts, {recursive: true});
  mkdirSync(metadata, {recursive: true});
  for (const name of ['authenticated-mongo-contract.sh', 'authenticated-mongo-readiness.js', 'backend-nextest.sh',
    'pull-pinned-image.sh']) {
    copyFileSync(new URL(name, import.meta.url), join(scripts, name));
  }
  writeFileSync(join(metadata, 'cargo-metadata.json'), JSON.stringify({workspace_root: join(root, 'api'), target_directory: join(root, 'api/target')}));
  writeFileSync(join(metadata, 'binaries-metadata.json'), JSON.stringify({'rust-build-meta': {'target-directory': join(root, 'api/target')}}));
  const log = join(directory, 'calls.ndjson');
  const cli = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const command = path.basename(process.argv[1]);
const log = process.env.CMA_TEST_LOG;
const scenario = process.env.CMA_TEST_SCENARIO;
if (command === 'openssl') { console.log('ab'.repeat(Number(args[2]))); process.exit(0); }
fs.appendFileSync(log, JSON.stringify({command,args,uri:process.env.CMA_AUTHENTICATED_TEST_MONGODB_URI,
  caller:process.env.CHRONO_TEST_MONGODB_URI,failpoint:process.env.CHRONO_TEST_MONGODB_FAILPOINT_URI,
  cma:process.env.CMA_TEST_CMA_EXECUTABLE})+'\\n');
if (command === 'cargo') {
  if (scenario === 'test-error') process.exit(8);
} else if (args[0] === 'run') {
  if (scenario === 'foreign-container') process.exit(9);
  console.log('fixture-container');
} else if (args[0] === 'port') console.log('127.0.0.1:32778');
else if (args[0] === 'inspect') console.log(scenario === 'foreign-container' ? 'foreign' : 'ab'.repeat(12));
else if (args[0] === 'rm' && scenario === 'cleanup-error') process.exit(10);
`;
  for (const command of ['cargo', 'docker', 'openssl']) writeFileSync(join(directory, command), cli, {mode: 0o755});
  const env = {...process.env, CI: reuse ? 'true' : 'false', PATH: `${directory}:${process.env.PATH}`,
    CMA_TEST_LOG: log, CMA_TEST_SCENARIO: scenario, CMA_AUTHENTICATED_TEST_MONGODB_URI: 'caller-uri',
    CHRONO_TEST_MONGODB_URI: 'caller-uri', CHRONO_TEST_MONGODB_FAILPOINT_URI: 'caller-uri'};
  if (reuse) env.CMA_NEXTEST_REUSE = '1'; else delete env.CMA_NEXTEST_REUSE;
  return {
    metadata, env, log,
    run: (track = 'all') => execFileSync('bash', [join(scripts, 'authenticated-mongo-contract.sh'), track], {env, stdio: 'pipe', timeout: 15000}),
    calls: () => existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').map(row => JSON.parse(row)) : [],
    close: () => rmSync(directory, {recursive: true}),
  };
}

for (const reuse of [false, true]) {
  test(`${reuse ? 'archive' : 'source'}: every authenticated Mongo selection is serial and uses its owned authenticated fixture`, () => {
    const f = fixture(reuse);
    try {
      f.run();
      const calls = f.calls();
      const cargo = calls.filter(row => row.command === 'cargo');
      assert.equal(selections.length, 39);
      assert.equal(cargo.length, selections.length, 'no cargo build/test or hidden recompilation in archive mode');
      for (const [index, row] of cargo.entries()) {
        const [selection, ignored, converted] = selections[index];
        const build = reuse ? ['--binaries-metadata', join(f.metadata, 'binaries-metadata.json'), '--cargo-metadata', join(f.metadata, 'cargo-metadata.json')] : ['--workspace', '--locked'];
        const args = ['nextest', 'run', ...build, '--no-tests', 'fail'];
        if (converted) args.push('--run-ignored', ignored, '--test-threads', '1', '--no-fail-fast', '-E', selection);
        else args.push('-E', selection, '--run-ignored', ignored, '--test-threads', '1');
        assert.deepEqual(row.args, args);
        assert.match(row.uri, /^mongodb:\/\/contract-admin:[a-f0-9]+@127\.0\.0\.1:32778\/admin\?directConnection=true$/);
      }
      for (const row of calls) {
        const isActiveProofOperator = row.command === 'cargo' && row.args.includes(activeProofOperator);
        const needsStore = row.command === 'cargo' && (isActiveProofOperator || row.args.includes('package(=cma) and test(~sender_context_tests::)') || row.args.includes(gatewayResetJourneys) || row.args.includes(fixedPinMachine));
        assert.equal(row.caller, needsStore ? row.uri : undefined, 'only M1 store contracts receive the owned fixture URI');
        assert.equal(row.cma, isActiveProofOperator ? join(f.metadata, '..', 'debug', 'cma') : undefined);
        assert.equal(row.failpoint, undefined);
      }
      assert.ok(calls.filter(row => row.command === 'docker' && row.args[0] !== 'rm' && row.args[0] !== 'container' && row.args[0] !== 'inspect').every(row => row.uri === undefined));
      const created = calls.find(row => row.command === 'docker' && row.args[0] === 'run');
      const name = `cma-im-mongo-${'ab'.repeat(12)}`;
      assert.equal(created.args[created.args.indexOf('--name')+1], name);
      assert.equal(created.args[created.args.indexOf('--label')+1], `dev.chrono.cma.im-contract=${'ab'.repeat(12)}`);
      assert.ok(created.args.includes('mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c'));
      assert.match(created.args.at(-1), /--setParameter enableTestCommands=1$/);
      assert.deepEqual(calls.at(-1).args, ['rm', '--force', '--volumes', name]);
      assert.deepEqual(calls.at(-2).args, ['inspect', '--format', '{{index .Config.Labels "dev.chrono.cma.im-contract"}}', name]);
    } finally { f.close(); }
  });
  for (const scenario of ['test-error', 'cleanup-error', 'foreign-container']) test(`${reuse ? 'archive' : 'source'}: ${scenario} fails without deleting foreign containers`, () => {
    const f = fixture(reuse, scenario);
    try {
      assert.throws(f.run, error => error.status === (scenario === 'test-error' ? 8 : 1));
      const removals = f.calls().filter(row => row.args[0] === 'rm');
      assert.equal(removals.length, scenario === 'foreign-container' ? 0 : 1);
      assert.equal(f.calls().filter(row => row.command === 'cargo').length, scenario === 'foreign-container' ? 0 : scenario === 'test-error' ? 1 : selections.length);
    } finally { f.close(); }
  });
}

for (const scenario of ['ci-source', 'empty-mode', 'bad-mode', 'sentinel-mode', 'missing-cargo', 'missing-binaries', 'malformed', 'moved-workspace', 'moved-target', 'moved-build']) test(`reject ${scenario} before fixture allocation`, () => {
  const f = fixture(true);
  try {
    if (scenario === 'ci-source') delete f.env.CMA_NEXTEST_REUSE;
    if (scenario === 'empty-mode') f.env.CMA_NEXTEST_REUSE = '';
    if (scenario === 'sentinel-mode') f.env.CMA_NEXTEST_REUSE = 'unset';
    if (scenario === 'bad-mode') f.env.CMA_NEXTEST_REUSE = 'true';
    if (scenario === 'missing-cargo') rmSync(join(f.metadata, 'cargo-metadata.json'));
    if (scenario === 'missing-binaries') rmSync(join(f.metadata, 'binaries-metadata.json'));
    if (scenario === 'malformed') writeFileSync(join(f.metadata, 'cargo-metadata.json'), 'invalid');
    if (scenario === 'moved-workspace' || scenario === 'moved-target' || scenario === 'moved-build') {
      const path = join(f.metadata, scenario === 'moved-workspace' ? 'cargo-metadata.json' : 'binaries-metadata.json');
      const value = JSON.parse(readFileSync(path));
      if (scenario === 'moved-workspace') value.workspace_root = '/elsewhere';
      else value['rust-build-meta'][scenario === 'moved-build' ? 'build-directory' : 'target-directory'] = '/elsewhere';
      writeFileSync(path, JSON.stringify(value));
    }
    assert.throws(f.run);
    assert.deepEqual(f.calls(), []);
  } finally { f.close(); }
});

for (const track of ['am', 'eg']) {
  test(`${track}: authenticated fixture executes only the owning product selections`, () => {
    const f = fixture(true);
    try {
      f.run(track);
      const calls = f.calls().filter(row => row.command === 'cargo');
      const expected = selections.filter(([selection]) => selection.includes('package(=cmaeg)') === (track === 'eg'));
      assert.deepEqual(calls.map(row => row.args[row.args.indexOf('-E') + 1]), expected.map(([selection]) => selection));
      assert.ok(calls.length > 0);
    } finally { f.close(); }
  });
}

test('an unknown track is refused before creating a Mongo fixture', () => {
  const f = fixture(true);
  try {
    assert.throws(() => f.run('unknown'));
    assert.equal(f.calls().filter(row => row.command === 'docker').length, 0);
  } finally { f.close(); }
});
