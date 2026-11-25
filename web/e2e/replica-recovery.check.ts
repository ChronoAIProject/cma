import { test, expect, type Page } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { binding } from './agent-v2.fixture';
import { ReplicaFixture, cursor } from './replica-recovery.fixture';

const enabled = process.env.CMA_RECOVERY_ENABLED === 'true';
const samples = Number(process.env.CMA_RECOVERY_SAMPLES ?? 3);
const stop = (page: Page) => page.getByRole('button', { name: 'Stop response', exact: true });
const recover = (page: Page) => page.getByRole('button', { name: 'Recover pending request' });
const input = (page: Page) => page.getByRole('textbox', { name: 'Message', exact: true });
const provenance = {
  source: process.env.CMA_RECOVERY_SOURCE ?? 'unspecified',
  enabled,
  scope:
    'compiled Chrome, two loopback HTTP ingresses sharing a synthetic ledger; not real Mongo, multi-pod or production qualification',
};
function clean(fixture: ReplicaFixture) {
  expect(fixture.errors).toEqual([]);
  expect(fixture.base.errors).toEqual([]);
  expect(fixture.base.boundary.errors).toEqual([]);
  expect(fixture.base.boundary.unexpected).toEqual([]);
  expect(fixture.base.boundary.writes).toEqual([]);
}

test('serving artifact binds activation to immutable build input', async ({ request }) => {
  const identity = await (await request.get('/build-info.json')).json();
  const recovery = await (await request.get('/recovery-info.json')).json();
  expect(recovery).toEqual({ ...identity, replicaRecoveryEnabled: enabled });
  if (process.env.CMA_RECOVERY_SOURCE)
    expect(identity.revision).toBe(process.env.CMA_RECOVERY_SOURCE);
});

for (let sample = 1; sample <= samples; sample += 1) {
  for (const { refusal, cut } of [
    { refusal: undefined, cut: 'frame' },
    { refusal: 429, cut: 'frame' },
    { refusal: 503, cut: 'frame' },
    { refusal: undefined, cut: 'comment' },
  ] as const) {
    test(`LOCAL stream mid-${cut} loss sample ${sample} refusal ${refusal ?? 'none'}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new ReplicaFixture();
      fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      fixture.append({ type: 'message', role: 'user', content: 'Retained request' });
      try {
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        await expect(stop(page)).toBeEnabled();
        await expect.poll(() => fixture.streams.size).toBe(1);
        await expect(page.locator('.message--user')).toContainText('Retained request');
        if (refusal !== undefined) fixture.tailRefusals.push(refusal);
        await fixture.cutMidFrame(cut);
        fixture.append({ type: 'message', role: 'assistant', content: 'Final durable message' });
        fixture.append({ type: 'turn_completed', turn_sequence: 1 });
        await expect(page.locator('.message--assistant')).toContainText([
          'Recovered complete message',
          'Final durable message',
        ]);
        await expect(stop(page)).toHaveCount(0);
        const recoveredAt = performance.now();
        expect(recoveredAt - fixture.faultAt).toBeLessThan(10_000);
        expect(fixture.reads[0]!.after).toBe(cursor(2));
        for (const read of fixture.reads.slice(1)) {
          expect(read.ingress).toBe(1);
          expect(read.after).toBe(cursor(2));
        }
        if (refusal !== undefined) {
          const refused = fixture.reads.findIndex((read) => read.status === refusal);
          expect(refused).toBeGreaterThan(0);
          expect(
            fixture.reads[refused + 1]!.at - fixture.reads[refused]!.at,
          ).toBeGreaterThanOrEqual(1000);
        }
        await expect(page.locator('.message--user')).toHaveCount(1);
        await expect(page.locator('.message__delivery')).toHaveCount(0);
        await expect(page.getByText('Disposable live fragment', { exact: true })).toHaveCount(0);
        // A second connection starts from the terminal cursor, not live fragment progress.
        for (const stream of fixture.streams) stream.destroy();
        await expect.poll(() => fixture.reads.at(-1)?.after).toBe(cursor(5));
        await expect(page.locator('.message--assistant')).toHaveCount(2);
        await expect(stop(page)).toHaveCount(0);
        clean(fixture);
        await writeFile(
          info.outputPath('recovery.json'),
          JSON.stringify(
            {
              ...provenance,
              sample,
              refusal: refusal ?? null,
              cut,
              recoveryMs: recoveredAt - fixture.faultAt,
              completeDurableEvents: 5,
              userEchoes: 1,
              assistantMessages: 2,
              duplicates: 0,
              reads: fixture.reads.map(({ ingress, after, at, status }) => ({
                ingress,
                afterSequence: Number(after?.split('.').at(-1)),
                sinceFaultMs: at - fixture.faultAt,
                status,
              })),
            },
            null,
            2,
          ),
        );
      } finally {
        await fixture.close(page);
      }
    });
  }
  for (const action of ['send', 'cancel'] as const) {
    test(`LOCAL lost ${action} admission receipt sample ${sample} automatic ${enabled}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new ReplicaFixture();
      fixture.lose = action;
      if (action === 'send') fixture.revealInputReceipt = enabled;
      if (action === 'cancel') fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      try {
        await fixture.route(page, baseURL!);
        // An OFF bundle must ignore every browser-supplied activation lookalike.
        await page.addInitScript(() => {
          localStorage.setItem('CHRONO_WEB_REPLICA_RECOVERY_ENABLED', 'true');
          sessionStorage.setItem('CHRONO_WEB_REPLICA_RECOVERY_ENABLED', 'true');
        });
        await page.goto('/agents/agt_contract?CHRONO_WEB_REPLICA_RECOVERY_ENABLED=true');
        await expect(input(page)).toBeVisible();
        await expect.poll(() => fixture.streams.size).toBe(1);
        if (action === 'send') {
          await input(page).fill('Retained exact input');
          await page.getByRole('button', { name: 'Send message', exact: true }).click();
        } else await stop(page).click();
        if (!enabled) {
          await expect(recover(page)).toBeEnabled();
          expect(fixture.writes).toHaveLength(1);
          fixture.revealInputReceipt = true;
          await recover(page).click();
        }
        await expect
          .poll(() => (action === 'send' ? fixture.receiptReads.length : fixture.writes.length))
          .toBeGreaterThanOrEqual(action === 'send' ? 1 : 2);
        await expect(recover(page)).toHaveCount(0);
        const recoveredAt = performance.now();
        expect(recoveredAt - fixture.faultAt).toBeLessThan(10_000);
        const [first] = fixture.writes;
        const recovered = action === 'send' ? fixture.receiptReads.at(-1) : fixture.writes[1];
        expect(fixture.writes).toHaveLength(action === 'send' ? 1 : 2);
        expect(recovered).toMatchObject({
          path: action === 'send' ? `${first!.path}/receipt` : first!.path,
          key: first!.key,
          body: first!.body,
          ingress: 1,
        });
        expect(first!.ingress).toBe(0);
        expect(fixture.effects.size).toBe(1);
        if (action === 'send') {
          await expect(page.locator('.message--user')).toContainText('Sent');
          fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
          fixture.append({ type: 'message', role: 'user', content: 'Retained exact input' });
          await expect(page.locator('.message--user')).toHaveCount(1);
        }
        await expect(stop(page)).toBeVisible();
        fixture.append({
          type: action === 'send' ? 'turn_completed' : 'turn_cancelled',
          turn_sequence: 1,
        });
        await expect(stop(page)).toHaveCount(0);
        expect(fixture.effects.size).toBe(1);
        clean(fixture);
        await writeFile(
          info.outputPath('recovery.json'),
          JSON.stringify(
            {
              ...provenance,
              sample,
              action,
              recoveryMs: recoveredAt - fixture.faultAt,
              automatic: enabled,
              posts: fixture.writes.length,
              receiptReads: fixture.receiptReads.length,
              effects: fixture.effects.size,
              exactIntent: recovered?.key === first!.key && recovered?.body === first!.body,
              durableTerminalConfirmed: true,
            },
            null,
            2,
          ),
        );
      } finally {
        await fixture.close(page);
      }
    });
  }
}
