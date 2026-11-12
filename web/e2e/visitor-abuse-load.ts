import { randomUUID } from 'node:crypto';
import type { APIRequestContext, Browser } from '@playwright/test';
import type { TC117Client } from './visitor-abuse-client';
import { readBearer, type Target, type Visitor } from './visitor-abuse-environment';
import {
  assertControls,
  assertAnonymousBudget,
  assertQuotaRefusal,
  blocked,
  check,
  p99,
  receipt,
  requireInstances,
} from './visitor-abuse-model';
import { noEffects } from './visitor-abuse-scenarios';

export async function runLoadWave(
  h: TC117Client,
  proxies: APIRequestContext[],
  rps: number,
  seconds: number,
  cohort: (second: number) => Promise<void>,
) {
  const origins = requireInstances(h.fixture.instances);
  let invalid = false;
  const started = Date.now();
  const offeredBefore = h.counts.offered;
  const admittedBefore = h.counts.admitted;
  let lastOfferedAt = started;
  const jobs = Array.from({ length: rps * seconds }, async (_, sequence) => {
    await new Promise((resolve) =>
      setTimeout(resolve, Math.max(0, started + (sequence * 1000) / rps - Date.now())),
    );
    h.counts.offered++;
    lastOfferedAt = Date.now();
    const target = h.fixture.triggers.reuseA;
    const path = `/api/v1/triggers/public/${target.publicId}${sequence % 2 ? '/badge.svg' : ''}`;
    try {
      const result = await h.send('anonymous', path, {
        origin: origins[sequence % 2]!,
        api: proxies[Math.floor(sequence / 2) % proxies.length]!,
      });
      if (result.status === 200) h.counts.admitted++;
      else if (
        result.status === 429 &&
        typeof result.body.code === 'string' &&
        Number(result.retryAfter) > 0
      )
        h.counts.refused++;
      else invalid = true;
    } catch {
      invalid = true;
    }
  });
  let cohortChain = Promise.resolve();
  const cohorts = Array.from({ length: seconds }, async (_, second) => {
    await new Promise((resolve) =>
      setTimeout(resolve, Math.max(0, started + second * 1000 - Date.now())),
    );
    cohortChain = cohortChain
      .then(() => cohort(second))
      .catch(() => {
        invalid = true;
      });
    await cohortChain;
  });
  const results = await Promise.allSettled([
    ...jobs,
    ...cohorts,
    new Promise((resolve) =>
      setTimeout(resolve, Math.max(0, started + seconds * 1000 - Date.now())),
    ),
  ]);
  if (results.some((result) => result.status === 'rejected')) invalid = true;
  const dispatch = {
    plannedRps: rps,
    offered: h.counts.offered - offeredBefore,
    elapsedMs: lastOfferedAt - started,
    achievedRps:
      (h.counts.offered - offeredBefore) / Math.max(seconds, (lastOfferedAt - started) / 1000),
  };
  if (h.observations) h.observations[`dispatch-${offeredBefore}`] = dispatch;
  check(
    lastOfferedAt - started <= seconds * 1000 + 1000,
    'offered load missed its absolute dispatch schedule',
  );
  check(!invalid, 'anonymous wave had transport/5xx/untyped failures or a failed cohort operation');
  if (h.fixture.load)
    assertAnonymousBudget(
      h.counts.admitted - admittedBefore,
      Math.max(seconds * 1000, lastOfferedAt - started),
      h.fixture.load.perPodPerMinute,
      h.fixture.load.maxServingPods,
    );
}

export async function navigationProbes(h: TC117Client, browser: Browser) {
  const before = await h.snapshot(['b', 'c']);
  const context = await browser.newContext({ serviceWorkers: 'block' });
  let attemptedWrites = 0;
  try {
    await context.route('**/*', async (route) => {
      const request = route.request();
      if (new URL(request.url()).origin !== h.config.baseURL) return route.abort();
      if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method())) {
        attemptedWrites++;
        return route.abort();
      }
      await route.continue({
        headers: {
          ...request.headers(),
          Authorization: `Bearer ${readBearer(h.fixture.accounts.c)}`,
        },
      });
    });
    const page = await context.newPage();
    const target = h.fixture.triggers.reuseA;
    for (let index = 0; index < 3; index++) {
      // Real document navigation executes application code, without an explicit launch action.
      await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`, {
        waitUntil: 'networkidle',
      });
      const open = await h.send(
        'anonymous',
        `/api/v1/triggers/public/${target.publicId}/open/${target.placementId}`,
      );
      check(open.status < 500, 'public open navigation failed');
      const callback = await h.send('anonymous', '/auth/callback');
      check(callback.status < 500, 'callback refusal failed');
      await h.read('c', '/api/v1/launches');
    }
    check(attemptedWrites === 0, 'GET/navigation/callback attempted a browser write');
    h.observations.navigation = { iterations: 3, attemptedWrites };
  } finally {
    await context.close();
  }
  noEffects(before, await h.snapshot(['b', 'c']), h.counts);
}
export async function loadAndControls(h: TC117Client, browser: Browser) {
  const origins = requireInstances(h.fixture.instances);
  const budget = h.fixture.load;
  if (!budget || budget.durationSeconds < 5) throw new Error(blocked.budget);
  const proxies = await h.proxyContexts();
  if (proxies.length < 2) throw new Error(blocked.budget);
  const actors = ['b', 'c'] as const;
  const targetFor = (actor: Visitor): Target =>
    actor === 'b' ? h.fixture.triggers.reuseA : h.fixture.triggers.reuseB;
  const active: Partial<Record<Visitor, string>> = {};
  const drafts: Partial<Record<Visitor, string[]>> = {};
  try {
    for (const actor of actors) {
      const first = await h.launch(actor, targetFor(actor));
      check(first.status === 201 || first.status === 202, 'control-group Launch was not admitted');
      const ready = await h.settled(actor, receipt(first.body).id);
      // Stopping a reused Agent would mutate a pre-existing fixture; require a run-owned Agent.
      const agent = ready.resolution?.agent;
      check(
        agent?.id &&
          h.journal.resources.some((row) => row.kind === 'agents' && row.id === agent.id),
        'control fixture must yield a run-created active Agent',
      );
      active[actor] = agent.id;
      drafts[actor] = [];
    }
    const expected = await h.snapshot([...actors]);
    const cohort = (phase: 'below' | 'above') => async (second: number) => {
      for (const actor of actors) {
        h.counts.collateralAttempts++;
        try {
          await h.read(actor, '/api/v1/launches', origins[second % 2]);
        } catch {
          h.counts.collateralErrors++;
        }
        if (second < 2) {
          h.counts.collateralAttempts++;
          const target = targetFor(actor);
          try {
            const result = await h.launch(actor, target, {
              origin: origins[second % 2],
              data: { placement_id: target.placementId, start_if_ready: false },
              key: randomUUID(),
            });
            if (![201, 202].includes(result.status)) {
              h.counts.collateralErrors++;
              continue;
            }
            const id = receipt(result.body).id;
            drafts[actor]!.push(id);
            expected.launches[actor]!.push(id);
            expected.totals[target.id]!.launches++;
          } catch {
            h.counts.collateralErrors++;
          }
        }
        if (phase === 'above' && second === 2) {
          h.counts.overquotaAttempts++;
          const result = await h.launch(actor, targetFor(actor), { origin: origins[1] });
          assertQuotaRefusal(result);
          h.counts.overquotaRejects++;
        }
        if (phase === 'above' && second === 3) {
          check(
            Date.now() < deadlines[phase]!,
            'control samples must start during the offered attack',
          );
          await h.control(actor, 'stop', active[actor]!, origins[0]);
          for (const id of drafts[actor]!) {
            check(
              Date.now() < deadlines[phase]!,
              'control samples must start during the offered attack',
            );
            await h.control(actor, 'cancel', id, origins[1]);
          }
        }
      }
    };
    const waves: Record<string, unknown> = {};
    const deadlines: Record<string, number> = {};
    for (const phase of ['below', 'above'] as const) {
      const before = {
        offered: h.counts.offered,
        admitted: h.counts.admitted,
        refused: h.counts.refused,
      };
      const started = Date.now();
      deadlines[phase] = started + budget.durationSeconds * 1000;
      await runLoadWave(
        h,
        proxies,
        phase === 'below' ? budget.belowRps : budget.aboveRps,
        budget.durationSeconds,
        cohort(phase),
      );
      waves[phase] = {
        offered: h.counts.offered - before.offered,
        admitted: h.counts.admitted - before.admitted,
        refused: h.counts.refused - before.refused,
        durationMs: Date.now() - started,
      };
    }
    h.observations.load = waves;
    h.observations.controls = {
      successRate:
        h.controls.filter((row) => row.status >= 200 && row.status < 300).length /
        h.controls.length,
      p99Ms: p99(h.controls.map((row) => row.ms)),
    };
    // Do not special-case pending control-tier work: report the measured target directly.
    assertControls(h.controls);
    check(
      h.counts.collateralAttempts > 0 &&
        h.counts.collateralErrors / h.counts.collateralAttempts < 0.001,
      'control-group collateral error rate must remain below 0.1 percent',
    );
    const below = waves.below as { admitted: number; refused: number };
    const above = waves.above as { refused: number };
    check(
      below.admitted > 0 && below.refused === 0 && above.refused > 0,
      'frozen load must demonstrate admission below and refusal above the budget',
    );
    const after = await h.snapshot([...actors]);
    h.observations.fleet = {
      before: expected.fleetLive,
      after: after.fleetLive,
      ceiling: budget.fleetSandboxLimit,
    };
    check(
      after.fleetLive <= budget.fleetSandboxLimit,
      'fleet Sandbox count exceeded frozen capacity',
    );
    noEffects(expected, after, h.counts);
    await navigationProbes(h, browser);
  } finally {
    for (const proxy of proxies) await proxy.dispose();
  }
}
