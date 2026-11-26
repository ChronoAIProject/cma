import {readFile, writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {assertOwner, check, hash, other, poll} from './cma/core.mjs';
import {runJourney} from './cma/journey.mjs';
import {readConfig} from './config.mjs';
import {writerCoverage} from './writer_coverage.mjs';

export async function preparedJourney({adapter, create, openBoth, cleanup, step, complete}) {
  const evidenceFor = record => adapter.preparedEvidence({
    agentId: record.agentId, sandboxId: record.sandboxId, operationId: record.operationId,
  });
  const consumed = new Set();
  for (const ingress of ['a', 'b']) {
    await step('cold_comparison_on_' + ingress, async () => {
      const record = await create(ingress, false, {profile: {system_prompt: 'CI cold comparison with custom instructions'}});
      const observed = await evidenceFor(record);
      check(observed.agent_ready === true && observed.consumed === false,
        'cold_comparison_consumed_prepared_reservation');
      await cleanup(record);
      return {ingress, cold_admission_confirmed: true};
    });
    await step('prepared_create_on_' + ingress, async () => {
      const ready = await poll(() => adapter.preparedReady(),
        rows => Array.isArray(rows) && rows.length > 0,
        {timeout: 180000, code: 'no_ready_prepared_reservation'});
      const record = await create(ingress, false);
      const durable = await evidenceFor(record);
      check(ready.includes(record.sandboxId) && durable.consumed === true &&
        durable.installation_bound === true && durable.agent_ready === true && !consumed.has(record.sandboxId),
      'prepared_admission_consumption_not_observed');
      consumed.add(record.sandboxId);
      check(durable.schemas?.length === 3 && durable.schemas.every(version =>
        Number.isSafeInteger(version) && version > 0), 'prepared_parent_schemas_unbound');
      const owner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
      await openBoth(record);
      await complete(record, other(owner.am));
      await cleanup(record);
      return {ingress, owner: owner.am, owner_epoch: owner.epoch, parent_schema_versions: durable.schemas,
        prepared_reservation_sha256: hash(record.sandboxId), agent_sha256: hash(record.agentId)};
    });
  }
}

if (process.argv[1]?.endsWith('/prepared_suite.mjs')) {
  const config = await readConfig(process.stdin);
  const registry = JSON.parse(await readFile(new URL('./writer-regressions.json', import.meta.url)));
  const journey = await runJourney(config, {preparedHook: preparedJourney});
  const coverage = writerCoverage(registry, [], journey, 'prepared');
  const verdict = journey.verdict === 'LOCAL_PREPARED_TWO_AM_PASS' && coverage.verdict === 'PASS'
    ? 'PASS' : 'FAIL';
  await writeFile(join(config.summary_dir, 'coverage.json'), JSON.stringify({
    schema: 'multi-am-prepared-coverage-v1', suite_verdict: verdict, source: config.expected_api_source,
    writer_coverage: coverage,
    steps: journey.steps.map(({name, verdict, elapsed_ms, parent_schema_versions, owner_epoch, ingress, owner,
      prepared_reservation_sha256, agent_sha256}) => ({
      name, verdict, elapsed_ms, parent_schema_versions, owner_epoch, ingress, owner,
      prepared_reservation_sha256, agent_sha256,
    })),
  }, null, 2) + '\n', {flag: 'wx', mode: 0o600});
  process.exitCode = verdict === 'PASS' ? 0 : 1;
}
