import { randomUUID } from 'node:crypto';
import { object } from './environment';
import { CallerResponseLost, type TC117Client } from './visitor-abuse-client';
import type { Actor, Visitor } from './visitor-abuse-environment';
import {
  admissionDays,
  blocked,
  assertQuotaRefusal,
  check,
  receipt,
  requireInstances,
  utcBoundary,
  type Counts,
  type Receipt,
  type Result,
} from './visitor-abuse-model';

type Snapshot = Awaited<ReturnType<TC117Client['snapshot']>>;
export function noEffects(before: Snapshot, after: Snapshot, counts: Counts) {
  const changedIds = (left: string[], right: string[]) =>
    new Set([...left, ...right].filter((id) => !left.includes(id) || !right.includes(id))).size;
  let changes = changedIds(before.sandboxes, after.sandboxes);
  for (const actor of Object.keys(before.launches))
    changes += changedIds(before.launches[actor]!, after.launches[actor] ?? []);
  for (const id of Object.keys(before.totals))
    for (const key of ['launches', 'ready'] as const)
      changes += Math.abs(before.totals[id]![key] - after.totals[id]![key]);
  counts.sideEffectChecks++;
  counts.anonymousSideEffects += changes;
  check(changes === 0, 'anonymous/read/navigation/callback produced public work side effects');
}
export async function identityBoundary(
  body: Record<string, unknown>,
  subject: string,
  visitor: boolean,
) {
  check(body.authenticated === true && body.subject === subject, 'fixture identity mismatch');
  check(body.visitor === visitor && Array.isArray(body.capabilities), 'fixture role mismatch');
  const capabilities = body.capabilities as unknown[];
  if (visitor)
    check(
      capabilities.length === 3 &&
        ['launch_triggers', 'use_own_sandboxes', 'delete_own_sandbox'].every((value) =>
          capabilities.includes(value),
        ),
      'visitor must have exactly the three public visitor capabilities',
    );
}
export async function validatePrerequisites(h: TC117Client, visitors: Visitor[]) {
  for (const origin of [...new Set([h.config.baseURL, ...h.fixture.instances])]) {
    for (const actor of ['owner', 'admin', ...visitors] as Exclude<Actor, 'anonymous'>[]) {
      await identityBoundary(
        await h.read(actor, '/auth/session', origin),
        h.fixture.accounts[actor].subject,
        visitors.includes(actor as Visitor),
      );
    }
    const view = await h.read('admin', '/api/v1/admin/settings', origin);
    const settings = object(view.settings, 'platform settings');
    const limits = object(settings.limits, 'platform limits');
    const triggers = object(settings.triggers, 'Trigger settings');
    check(
      limits.visitor_max_sandboxes === 2 && limits.visitor_max_launches_per_day === 5,
      'staging must expose the default 2-home and 5-per-UTC-day visitor limits',
    );
    check(
      triggers.enabled === true && triggers.orchestration_paused === false,
      'Trigger orchestration must be enabled',
    );
    h.observations.platform = {
      revision: view.revision,
      limits: { visitor_max_sandboxes: 2, visitor_max_launches_per_day: 5 },
      triggers: { enabled: true, orchestration_paused: false },
    };
  }
  for (const [name, target] of Object.entries(h.fixture.triggers)) {
    const trigger = await h.read('owner', `/api/v1/triggers/${target.id}`);
    check(
      trigger.public_id === target.publicId && trigger.availability === 'published',
      'fixture Trigger is not published',
    );
    check(
      Array.isArray(trigger.placements) &&
        trigger.placements.some(
          (item) => object(item).id === target.placementId && object(item).retired_at == null,
        ),
      'fixture placement is unavailable',
    );
    const revisions = await h.read('owner', `/api/v1/triggers/${target.id}/revisions`);
    check(
      Array.isArray(revisions.items) && revisions.items.length > 0,
      'published Trigger revision is unavailable',
    );
    const revision = object(revisions.items.at(-1));
    check(
      typeof revision.digest === 'string' && /^[a-f0-9]{64}$/.test(revision.digest),
      'published revision digest is invalid',
    );
    const content = object(revision.content);
    const reuse = object(content.reuse);
    check(
      object(content.consent).always_review === false,
      'fixture must permit the explicit API consent without another review',
    );
    const expected =
      name === 'alwaysNew'
        ? ['always_new', 'always_new', 'always_new']
        : ['reuse_existing', 'reuse_existing', 'always_new'];
    check(
      [reuse.sandbox, reuse.workspace, reuse.agent].every((value, i) => value === expected[i]),
      'fixture Trigger reuse policy mismatch',
    );
    h.observations[`trigger-${name}`] = { revision: revision.revision, digest: revision.digest };
  }
  const initial = await h.snapshot(visitors);
  for (const actor of visitors) {
    const today = new Date().toISOString().slice(0, 10);
    const history = (await h.list(actor, '/api/v1/launches')).map(receipt);
    check(
      history.every((row) => row.created_at.slice(0, 10) !== today),
      'campaign visitor must have no consumed allowance in the current UTC day',
    );
    check(
      initial.liveByVisitor[actor] === (actor === 'boundary' ? 0 : 1),
      'reuse visitors need one compatible pre-provisioned home; boundary visitor needs none',
    );
  }
}
function accepted(result: Result) {
  return result.status === 201 || result.status === 202;
}
async function quotaAttempt(h: TC117Client, result: Result, countAttempt = true) {
  if (countAttempt) h.counts.overquotaAttempts++;
  assertQuotaRefusal(result);
  h.counts.overquotaRejects++;
}
export async function boundaryScenario(h: TC117Client) {
  const target = h.fixture.triggers.alwaysNew;
  const before = await h.snapshot(['boundary']);
  for (const [actor, expected] of [
    ['anonymous', 401],
    ['boundary', 403],
  ] as const) {
    const key = randomUUID();
    for (let replay = 0; replay < 2; replay++) {
      const result =
        actor === 'anonymous'
          ? await h.send(actor, `/api/v1/triggers/public/${target.publicId}/launches`, {
              method: 'POST',
              key,
              data: { placement_id: target.placementId, start_if_ready: true },
            })
          : await h.send(actor, '/api/v1/sandboxes', {
              method: 'POST',
              key,
              data: { sandbox_profile: h.fixture.sandboxProfile, name: 'Scoped refusal probe' },
            });
      h.counts.overquotaAttempts++;
      const refused = result.status === expected && typeof result.body.code === 'string';
      if (refused) h.counts.overquotaRejects++;
      check(refused, 'identity/capability boundary did not return the required typed refusal');
    }
  }
  noEffects(before, await h.snapshot(['boundary']), h.counts);
  const homes: string[] = [];
  for (let index = 1; index <= 2; index++) {
    const result = await h.launch('boundary', target);
    check(accepted(result), 'legitimate always-new Launch admission failed');
    const row = await h.settled('boundary', receipt(result.body).id);
    check(row.resolution?.sandbox?.state === 'created', 'always-new Launch must create a home');
    homes.push(row.resolution.sandbox.id!);
    check(
      (await h.snapshot(['boundary'])).liveByVisitor.boundary === index,
      'unexpected visitor home count',
    );
  }
  check(new Set(homes).size === 2, 'always-new homes must be distinct');
  const beforeThird = await h.snapshot(['boundary']);
  h.counts.overquotaAttempts++;
  const third = await h.launch('boundary', target);
  check(accepted(third), 'third-home refusal must remain an admitted Launch terminal outcome');
  const refused = await h.settled('boundary', receipt(third.body).id, false);
  const typed =
    refused.state === 'failed_terminal' && refused.failure?.code === 'sandbox_limit_reached';
  if (typed) h.counts.overquotaRejects++;
  const afterThird = await h.snapshot(['boundary']);
  h.observations.thirdHome = {
    id: refused.id,
    state: refused.state,
    code: typed ? 'sandbox_limit_reached' : 'unexpected',
    live: afterThird.liveByVisitor.boundary,
  };
  check(
    typed && object(h.observations.thirdHome).live === 2,
    'third home was not refused before allocation',
  );
  check(
    JSON.stringify(beforeThird.sandboxes) === JSON.stringify(afterThird.sandboxes) &&
      ['sandbox', 'workspace', 'agent'].every((kind) => {
        const item = refused.resolution?.[kind];
        return item !== undefined && item.id === undefined && item.state !== 'created';
      }),
    'terminal refusal has unproved allocation effects',
  );
  // Retain the receipt in the append-only ledger. No worker or resource remains to cancel.
  h.journal.record({
    action: 'terminal_refusal_reconciled',
    kind: 'launches',
    id: refused.id,
    subject: h.fixture.accounts.boundary.subject,
    reason: 'sandbox_limit_reached_before_allocation',
  });
  const index = h.journal.resources.findIndex(
    (row) => row.kind === 'launches' && row.id === refused.id,
  );
  check(index >= 0, 'refused Launch is absent from the run ledger');
  h.journal.resources.splice(index, 1);
}
export async function sharedQuota(h: TC117Client) {
  const origins = requireInstances(h.fixture.instances);
  const baseline = (await h.list('a', '/api/v1/launches')).map(receipt);
  const baselineIds = new Set(baseline.map((row) => row.id));
  const today = new Date().toISOString().slice(0, 10);
  check(
    baseline.every((row) => row.created_at.slice(0, 10) !== today),
    'shared-quota visitor already consumed current UTC allowance',
  );
  const targets = [h.fixture.triggers.reuseA, h.fixture.triggers.reuseB];
  // Use independent parity for origins and Triggers so all four combinations run.
  const results = await Promise.allSettled(
    Array.from({ length: 50 }, (_, i) =>
      h.launch('a', targets[Math.floor(i / 2) % 2]!, {
        origin: origins[i % 2]!,
        key: randomUUID(),
      }),
    ),
  );
  const admitted: Receipt[] = [];
  h.counts.overquotaAttempts += 45;
  let invalid = false;
  for (const result of results) {
    if (result.status === 'rejected') {
      invalid = true;
      continue;
    }
    if (accepted(result.value)) admitted.push(receipt(result.value.body));
    else {
      try {
        await quotaAttempt(h, result.value, false);
      } catch {
        invalid = true;
      }
    }
  }
  h.observations.shared = {
    attempts: 50,
    admitted: admitted.length,
    refused: 50 - admitted.length,
    days: admissionDays(admitted),
  };
  check(
    !invalid && admitted.length === 5,
    'fifty shared-quota attempts must yield five admissions, typed refusals and zero transport/5xx errors',
  );
  const rows = (await h.list('a', '/api/v1/launches')).filter(
    (row) => !baselineIds.has(String(row.id)),
  );
  check(
    rows.length === 5 && new Set(rows.map((row) => row.id)).size === 5,
    'public receipts disagree with admitted daily usage',
  );
  const settled = await Promise.all(admitted.map((row) => h.settled('a', row.id)));
  check(
    settled.every((row) => row.resolution?.sandbox?.state === 'reused') &&
      new Set(settled.map((row) => row.resolution?.sandbox?.id)).size === 1,
    'daily allowance must be isolated from home allocation',
  );
}
export async function utcScenario(h: TC117Client) {
  const origins = requireInstances(h.fixture.instances);
  const midnight = utcBoundary();
  const withinWindow = (after: boolean) => {
    const delta = Date.now() - midnight;
    if (after ? delta < 0 || delta > 30000 : delta < -30000 || delta >= 0)
      throw new Error(blocked.utcWindow);
  };
  const baselineIds = new Set(
    (await h.list('utc', '/api/v1/launches')).map((row) => String(row.id)),
  );
  const targets = [h.fixture.triggers.reuseA, h.fixture.triggers.reuseB];
  withinWindow(false);
  const key = randomUUID();
  let responseLost = false;
  try {
    await h.launchAndLoseResponse('utc', targets[0]!, {
      origin: origins[0],
      key,
      data: { placement_id: targets[0]!.placementId, start_if_ready: false },
    });
  } catch (error) {
    if (!(error instanceof CallerResponseLost)) throw error;
    responseLost = true;
  }
  check(responseLost, 'admitted response was delivered instead of lost');
  withinWindow(false);
  const recovered = (await h.list('utc', '/api/v1/launches')).filter(
    (row) => !baselineIds.has(String(row.id)),
  );
  withinWindow(false);
  check(recovered.length === 1, 'lost-response recovery needs exactly one new public receipt');
  const original = receipt(recovered[0]!);
  const same = {
    key,
    origin: origins[1],
    data: { placement_id: targets[0]!.placementId, start_if_ready: false },
  };
  const verifyReplay = async (postMidnight = false) => {
    withinWindow(postMidnight);
    const before = await h.list('utc', '/api/v1/launches');
    withinWindow(postMidnight);
    const replay = await h.launch('utc', targets[0]!, same);
    withinWindow(postMidnight);
    const after = await h.list('utc', '/api/v1/launches');
    withinWindow(postMidnight);
    h.counts.idempotencyChecks++;
    const duplicates =
      Math.max(0, after.length - before.length) + (replay.body.id === original.id ? 0 : 1);
    h.counts.idempotentChargeDuplicates += duplicates;
    check(
      accepted(replay) && duplicates === 0,
      'exact idempotency replay duplicated an admission or receipt',
    );
  };
  await verifyReplay();
  const conflict = await h.launch('utc', targets[0]!, { key, origin: origins[1] });
  check(
    conflict.status === 409 && conflict.body.code === 'idempotency_key_reused',
    'same-scope changed payload must be refused',
  );
  const runBatch = async (remaining: number, after = false) => {
    withinWindow(after);
    h.counts.overquotaAttempts += 50 - remaining;
    const results = await Promise.allSettled(
      Array.from({ length: 50 }, (_, i) =>
        h.launch('utc', targets[Math.floor(i / 2) % 2]!, {
          origin: origins[i % 2]!,
          key: randomUUID(),
        }),
      ),
    );
    withinWindow(after);
    let admitted = 0;
    let invalid = false;
    for (const result of results) {
      if (result.status === 'rejected') {
        invalid = true;
        continue;
      }
      if (accepted(result.value)) admitted++;
      else {
        try {
          await quotaAttempt(h, result.value, false);
        } catch {
          invalid = true;
        }
      }
    }
    check(!invalid, 'UTC batch had a transport, 5xx or untyped refusal');
    return admitted;
  };
  // The same bare key on a different public_id belongs to a distinct idempotency scope.
  const other = await h.launch('utc', targets[1]!, { key, origin: origins[1] });
  check(
    accepted(other) && other.body.id !== original.id,
    'different Trigger scopes must admit independently',
  );
  await runBatch(3);
  const cancel = await h.control('utc', 'cancel', original.id, origins[0]);
  check(cancel.status >= 200 && cancel.status < 300, 'draft cancellation failed');
  await quotaAttempt(h, await h.launch('utc', targets[0]!, { origin: origins[1] }));
  withinWindow(false);
  await new Promise((resolve) => setTimeout(resolve, midnight - Date.now()));
  withinWindow(true);
  await verifyReplay(true);
  await runBatch(5, true);
  await verifyReplay(true);
  withinWindow(true);
  const rows = (await h.list('utc', '/api/v1/launches'))
    .map(receipt)
    .filter((row) => !baselineIds.has(row.id));
  h.counts.idempotentChargeDuplicates += Math.max(0, 10 - rows.length);
  const days = admissionDays(rows);
  h.observations.utc = {
    midnight: new Date(midnight).toISOString(),
    days,
    receipts: rows.length,
    responseLoss: 'caller_delivery_after_admission',
    chargeProof: 'public receipts plus exhausted daily allowance; no private billing ledger',
  };
  check(
    Object.keys(days).length === 2 && Object.values(days).every((used) => used === 5),
    'UTC days must each contain exactly five admissions without replay/cancel refunds',
  );
}
