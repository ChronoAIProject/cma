import type { TriggerStatistics } from '@cma/client';
import { test, expect } from './harness';
import { followPublishedEmbed, publishCurrentTrigger } from './publisher-current';
import { publicationProof, workCommands } from './journey-proof';
import { firstAnswer } from './journey-answer';
import { assertPublishedLaunch, assertSingleLaunchDelta } from './public-journey-contracts';

test('J5 independent image and inline publishers bind visitor embeds, revisions and exact counts', async ({
  harness: h,
}) => {
  const publications = [];
  for (const role of ['owner', 'secondOwner'] as const) {
    const page = await h.open(role);
    const published = await publishCurrentTrigger(h, page, role);
    await h.capture(page, `j5-${role}-published`);
    publications.push({ role, page, published });
  }
  expect(publications[0]!.published.document.id).not.toBe(publications[1]!.published.document.id);
  expect(publications[0]!.published.target.agent.system_prompt).not.toBe(
    publications[1]!.published.target.agent.system_prompt,
  );
  for (const [index, publication] of publications.entries()) {
    const foreign = publications[1 - index]!;
    const response = await publication.page.request.get(
      `${h.config.baseURL}/api/v1/triggers/${foreign.published.document.id}`,
    );
    expect([403, 404]).toContain(response.status());
  }
  const proofs = [];
  for (const { role, page: owner, published } of publications) {
    const { document, revision, placement, embed } = published;
    const page = await h.open('visitor');
    const before = await h.get<TriggerStatistics>(owner, `/api/v1/triggers/${document.id}/stats`);
    const commandsBefore = h.commands.length;
    const input = {
      run: h.journal.run,
      subject: published.subject,
      sourceUrl: placement.source_url,
      triggerId: document.id,
      publicId: document.public_id!,
      placementId: placement.id,
      revision: revision.revision,
      publicUrl: embed.public_url,
      html: embed.snippets.html,
    };
    try {
      await h.operators.publishEmbed({ ...input, phase: 'install' });
      await page.goto(placement.source_url);
      const landing = await followPublishedEmbed(page, embed.public_url);
      await expect(landing.locator('.public-actions button')).toBeEnabled();
      const preview = await h.get(landing, `/api/v1/triggers/public/${document.public_id}`);
      for (const entry of publications)
        expect(JSON.stringify(preview)).not.toContain(entry.published.target.agent.system_prompt);
      await h.flush();
      expect(workCommands(h.commands.slice(commandsBefore))).toEqual([]);
      expect(await h.get(owner, `/api/v1/triggers/${document.id}/stats`)).toEqual(before);
      await landing.locator('.public-actions button').click();
      const launch = await h.ready(landing, (await h.currentLaunch(landing)).id);
      await firstAnswer(h, landing, launch, h.fixture.accounts.visitor.subject);
      const sandbox = await h.get(landing, `/api/v1/sandboxes/${launch.resolution.sandbox.id}`);
      assertPublishedLaunch(launch, sandbox, {
        triggerId: document.id,
        publicId: document.public_id!,
        placementId: placement.id,
        digest: revision.digest,
        subject: h.fixture.accounts.visitor.subject,
      });
      expect(launch.referrer_origin).toBe(new URL(placement.source_url).origin);
      let after = before;
      await expect(async () => {
        after = await h.get<TriggerStatistics>(owner, `/api/v1/triggers/${document.id}/stats`);
        assertSingleLaunchDelta(before, after, placement.id);
      }).toPass({ timeout: 20_000 });
      const commands = workCommands(h.commands.slice(commandsBefore));
      expect(commands).toHaveLength(1);
      expect(commands[0]?.path).toBe(`/api/v2/triggers/public/${document.public_id}/launches`);
      proofs.push(publicationProof(published, launch, sandbox, commands, before, after));
      await h.capture(landing, `j5-${role}-visitor-ready`);
    } finally {
      await h.operators.publishEmbed({ ...input, phase: 'restore' });
    }
  }
  h.proveJourney({ visitor: h.fixture.accounts.visitor.subject, publications: proofs });
});

for (const kind of ['markdown', 'html', 'javascript', 'react'] as const) {
  test(`G-T1-${kind} real embed click-through preserves placement attribution`, async ({
    harness: h,
  }) => {
    const page = await h.open('visitor');
    const fixture = h.fixture.embeds[kind];
    const owner = await h.open('owner');
    const before = await h.get<TriggerStatistics>(
      owner,
      `/api/v1/triggers/${fixture.triggerId}/stats`,
    );
    const commandsBefore = h.commands.length;
    const response = await page.goto(fixture.url);
    if (kind === 'javascript') {
      expect(response?.headers()['content-security-policy']).toBeTruthy();
      expect(response?.headers()['content-security-policy']).not.toContain("'unsafe-inline'");
    }
    if (kind === 'markdown') {
      const image = page.locator('a[href*="/t/"] img').first();
      await expect(image).toBeVisible();
      expect(await image.evaluate((element: HTMLImageElement) => element.naturalWidth > 0)).toBe(
        true,
      );
      expect(new URL((await image.getAttribute('src')) ?? '').origin).not.toBe(h.config.baseURL);
    }
    const link = page.getByRole('link', { name: 'Open in CMA', exact: true });
    const publicUrl = new URL((await link.getAttribute('href'))!, page.url());
    expect(publicUrl.origin).toBe(h.config.baseURL);
    const landing = await followPublishedEmbed(page, publicUrl.href);
    expect(new URL(landing.url()).pathname.endsWith(`/${fixture.placementId}`)).toBe(true);
    expect(workCommands(h.commands.slice(commandsBefore))).toEqual([]);
    await landing.locator('.public-actions button').click();
    const launch = await h.currentLaunch(landing);
    expect(launch.trigger_id).toBe(fixture.triggerId);
    expect(launch.placement_id).toBe(fixture.placementId);
    expect(launch.referrer_origin).toBe(new URL(fixture.url).origin);
    const ready = await h.ready(landing, launch.id);
    await firstAnswer(h, landing, ready, h.fixture.accounts.visitor.subject);
    await expect(async () => {
      const after = await h.get<TriggerStatistics>(
        owner,
        `/api/v1/triggers/${launch.trigger_id}/stats`,
      );
      assertSingleLaunchDelta(before, after, fixture.placementId);
    }).toPass({ timeout: 20_000 });
    expect(workCommands(h.commands.slice(commandsBefore))).toHaveLength(1);
    await h.capture(landing, `g-t1-${kind}-ready`);
  });
}
