import {readFile, writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {assertOwner, check, hash, poll} from './cma/core.mjs';
import {runJourney} from './cma/journey.mjs';
import {readConfig} from './config.mjs';
import {retainFailure, withRestoration} from './failure.mjs';
import {openPreparedBrowser} from './prepared_browser.mjs';
import {writerCoverage} from './writer_coverage.mjs';

const isDigest = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);

async function servingMember(adapter, am) {
  const topology = await adapter.topology();
  const member = topology.ams?.[am];
  check(topology.shared_database === true && topology.coordination_enabled === true
    && member?.ready === true && typeof member.instance_id === 'string' && member.instance_id
    && typeof member.pod_uid === 'string' && member.pod_uid, 'prepared_browser_ingress_unbound');
  return member;
}

export async function preparedJourney({adapter, create, openBoth, cleanup, step, browser}) {
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
    await step('prepared_create_on_' + ingress, () => browser.forIngress(ingress, async page => {
      const ready = await poll(() => adapter.preparedReady(),
        rows => Array.isArray(rows) && rows.length > 0,
        {timeout: 180000, code: 'no_ready_prepared_reservation'});
      const ingressMember = await servingMember(adapter, ingress);
      let opening;
      const record = await create(ingress, false, {}, ({agentId}) => {
        opening = page.openStarting(agentId).then(started => ({started}), error => ({error}));
      });
      const durable = await evidenceFor(record);
      check(ready.includes(record.sandboxId) && durable.consumed === true &&
        durable.installation_bound === true && durable.agent_ready === true && !consumed.has(record.sandboxId),
      'prepared_admission_consumption_not_observed');
      consumed.add(record.sandboxId);
      const [sandbox, workspace, worktree] = durable.schemas ?? [];
      check(durable.schemas?.length === 3 && [1, 3, 4].includes(sandbox) &&
        [1, 3, 4].includes(workspace) && worktree === 1, 'prepared_parent_schemas_unbound');
      check(durable.ready_fences?.length === 4 && durable.ready_fences.every(count => count === 1),
        'prepared_open_ready_atomic_publication_unproven');
      check(durable.startup_receipts?.length === 3 && durable.startup_receipts.every(count => count === 1),
        'prepared_startup_receipt_count_wrong');
      const owner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
      const browserOpen = await opening;
      if (browserOpen?.error) throw browserOpen.error;
      check(isDigest(browserOpen?.started), 'prepared_browser_starting_read_unproven');
      check(record.am === ingress && typeof record.admissionRequest === 'string'
        && record.admissionRequest.trim().length > 0, 'prepared_admission_ingress_unbound');
      await openBoth(record);
      const browserReply = await page.sendAndRead(record.agentId, adapter);
      check(browserReply?.effects === 1 && isDigest(browserReply.ready_request_sha256)
        && isDigest(browserReply.send_request_sha256) && isDigest(browserReply.answer_sha256),
        'prepared_browser_reply_unproven');
      const requests = {
        admission: hash(record.admissionRequest),
        browser_starting: browserOpen.started,
        browser_ready: browserReply.ready_request_sha256,
        browser_send: browserReply.send_request_sha256,
      };
      check(new Set(Object.values(requests)).size === Object.keys(requests).length, 'prepared_request_attribution_reused');
      const finalOwner = assertOwner(await adapter.ownerOf({agentId: record.agentId}), record.agentId);
      check(owner.instance_id && owner.instance_id === finalOwner.instance_id && owner.pod_uid === finalOwner.pod_uid
        && owner.epoch === finalOwner.epoch && owner.am === finalOwner.am, 'prepared_browser_owner_changed');
      const finalIngress = await servingMember(adapter, ingress);
      check(finalIngress.instance_id === ingressMember.instance_id && finalIngress.pod_uid === ingressMember.pod_uid,
        'prepared_browser_ingress_changed');
      await cleanup(record);
      return {ingress, owner: owner.am, owner_epoch: owner.epoch, parent_schema_versions: durable.schemas,
        ingress_instance_sha256: hash(ingressMember.instance_id), owner_instance_sha256: hash(owner.instance_id),
        request_sha256: requests,
        atomic_publication_fence_count: durable.ready_fences.filter(count => count === 1).length,
        startup_receipt_count: durable.startup_receipts.length,
        browser_starting_read: true, browser_ready_read: true, browser_reply_sha256: browserReply.answer_sha256,
        prepared_reservation_sha256: hash(record.sandboxId), agent_sha256: hash(record.agentId),
        lifecycle_operation_sha256: hash(record.operationId)};
    }));
  }
}

if (process.argv[1]?.endsWith('/prepared_suite.mjs')) {
  const config = await readConfig(process.stdin);
  const registry = JSON.parse(await readFile(new URL('./writer-regressions.json', import.meta.url)));
  const journey = await runJourney(config, {preparedHook: async context => {
    let browser;
    await withRestoration(async () => {
      await context.step('prepared_browser_available', async () => {
        browser = await openPreparedBrowser(config);
      });
      await preparedJourney({...context, browser});
    }, () => browser?.close(), (stage, error) => retainFailure(config.output_dir, stage, error,
      {case: 'prepared_browser'}));
  }});
  const coverage = writerCoverage(registry, [], journey, 'prepared');
  const verdict = journey.verdict === 'LOCAL_PREPARED_TWO_AM_PASS' && coverage.verdict === 'PASS'
    ? 'PASS' : 'FAIL';
  await writeFile(join(config.summary_dir, 'coverage.json'), JSON.stringify({
    schema: 'multi-am-prepared-coverage-v1', suite_verdict: verdict, source: config.expected_api_source,
    writer_coverage: coverage,
    steps: journey.steps.map(({name, verdict, elapsed_ms, parent_schema_versions, owner_epoch, ingress, owner,
      ingress_instance_sha256, owner_instance_sha256, request_sha256,
      atomic_publication_fence_count, startup_receipt_count, browser_starting_read, browser_ready_read, browser_reply_sha256,
      prepared_reservation_sha256, agent_sha256, lifecycle_operation_sha256}) => ({
      name, verdict, elapsed_ms, parent_schema_versions, owner_epoch, ingress, owner,
      ingress_instance_sha256, owner_instance_sha256, request_sha256,
      atomic_publication_fence_count, startup_receipt_count, browser_starting_read, browser_ready_read, browser_reply_sha256,
      prepared_reservation_sha256, agent_sha256, lifecycle_operation_sha256,
    })),
  }, null, 2) + '\n', {flag: 'wx', mode: 0o600});
  process.exitCode = verdict === 'PASS' ? 0 : 1;
}
