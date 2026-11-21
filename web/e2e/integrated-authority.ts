import { expect, type Page } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function authorityBoundaries(page: Page, directory: string, agentId: string) {
  const baseURL = new URL(page.url()).origin;
  const context = await page.context().browser()!.newContext({ baseURL });
  const checks: { account: string; revoked: boolean; path: string; status: number }[] = [];
  try {
    for (const [account, revoked] of [
      ['other', false],
      ['owner', true],
    ] as const) {
      await context.addCookies([
        { name: 'integrated-account', value: account, url: baseURL },
        { name: 'integrated-revoked', value: String(revoked), url: baseURL },
      ]);
      for (const suffix of [
        '',
        '/transcript',
        '/message-feedback',
        '/environment',
        '/files/content?path=cma%2Freport.txt',
        '/files/download?path=cma%2Freport.txt',
      ]) {
        const path = `/api/v1/agents/${agentId}${suffix}`;
        const response = await context.request.get(path);
        checks.push({ account, revoked, path, status: response.status() });
        expect([403, 404]).toContain(response.status());
        expect(await response.text()).not.toContain('Tool A: simple');
      }
    }
    await context.addCookies([
      { name: 'integrated-account', value: 'owner', url: baseURL },
      { name: 'integrated-revoked', value: 'false', url: baseURL },
    ]);
    const restored = await context.request.get(`/api/v1/agents/${agentId}/transcript`);
    expect(restored.status()).toBe(200);
    const transcript = await restored.json();
    expect(transcript.turns).toHaveLength(3);
    expect(
      transcript.turns.every(
        (turn: { outcome: { type: string } }) => turn.outcome.type === 'completed',
      ),
    ).toBe(true);
    await writeFile(
      join(directory, 'authority-result.json'),
      JSON.stringify(
        {
          passed: true,
          checks,
          restoredTurns: transcript.turns.map(
            (turn: { turn_sequence: number }) => turn.turn_sequence,
          ),
          boundary:
            'Independent current public client with changed account and revoked authority; retained native Agent/history/file. UI late-response fencing is separately covered by the current Web suite.',
        },
        null,
        2,
      ),
    );
  } finally {
    await context.close();
  }
}
