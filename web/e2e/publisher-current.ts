import { randomUUID } from 'node:crypto';
import type { TriggerDocument, TriggerEmbed, TriggerRevision } from '@cma/client';
import type { Page } from '@playwright/test';
import { expect, type Harness } from './harness';
import { object, type PublisherRole } from './environment';
import { currentTriggerTarget } from './current-trigger-target';
import { publicApiPath } from './public-api-path';
import {
  assertInlineRequest,
  assertPublishedTarget,
  assertPublisherIdentity,
} from './public-journey-contracts';

export async function publishCurrentTrigger(h: Harness, page: Page, role: PublisherRole) {
  const subject = h.fixture.accounts[role].subject;
  const identity = await h.get<{ subject: string }>(page, '/auth/session');
  assertPublisherIdentity(identity, subject);
  const fixture = h.fixture.publicJourneys.owners[role];
  const target = currentTriggerTarget(
    h.fixture,
    role,
    `Private journey configuration ${role} ${randomUUID()}`,
  );
  const commandsBefore = h.commands.length;
  await page.goto(`${h.config.baseURL}/triggers/manage`);
  await expect(page.getByRole('button', { name: 'Image', exact: true })).toHaveCount(0);
  await page.getByLabel('Model', { exact: true }).fill(fixture.model);
  await page.getByRole('button', { name: 'Reasoning effort', exact: true }).click();
  await page
    .getByRole('menuitemradio', { name: target.agent.reasoning_effort, exact: true })
    .click();
  await page.getByLabel('System prompt', { exact: true }).fill(target.agent.system_prompt);
  await page
    .getByLabel('Agent key scope services (comma separated)', { exact: true })
    .fill(target.agent.services.join(', '));
  await page
    .getByLabel('GitHub repository (optional)', { exact: true })
    .fill(h.fixture.publicJourneys.repository.url);
  await page
    .getByLabel('GitHub repository ref', { exact: true })
    .fill(h.fixture.publicJourneys.repository.reference);
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  for (const label of ['sandbox reuse', 'workspace reuse', 'Agent reuse'])
    await page.locator(`[aria-label="${label}"] input[value="always_new"]`).check();
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.locator('#trigger-title').fill(`Current journey ${role} ${randomUUID().slice(0, 8)}`);
  await page.locator('#trigger-description').fill('Scoped public journey verification');
  await page
    .locator('#trigger-first_message')
    .fill('Reply with your current working directory. Do not modify files.');
  await page.getByLabel('Show a review page first', { exact: true }).uncheck();
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.getByLabel('Source label', { exact: true }).fill(`Current ${role} embed`);
  await page.getByLabel('Source URL', { exact: true }).fill(fixture.embedUrl);
  const creation = page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' &&
      publicApiPath(new URL(response.url()).pathname) === '/api/v1/triggers',
  );
  await page.getByRole('button', { name: 'Add source URL', exact: true }).click();
  const created = await creation;
  expect(created.status()).toBe(201);
  const requestTarget = object(object(created.request().postDataJSON()).content).target;
  assertInlineRequest(requestTarget, target);
  await expect(page.getByLabel('Source label', { exact: true })).toHaveValue('');
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.locator('.trigger-review input[type="checkbox"]').check();
  await page.getByRole('button', { name: 'Publish revision 1', exact: true }).click();
  await page.waitForURL(/\/triggers\/manage\/[^/]+\/embed$/);
  await h.flush();
  const id = new URL(page.url()).pathname.split('/').at(-2)!;
  const document = await h.get<TriggerDocument>(page, `/api/v1/triggers/${id}`);
  const revisions = await h.get<TriggerRevision[]>(page, `/api/v1/triggers/${id}/revisions`);
  expect(revisions).toHaveLength(1);
  const revision = revisions[0]!;
  assertPublishedTarget(document, revision, target);
  expect(document.placements).toHaveLength(1);
  const placement = document.placements[0]!;
  expect(placement.source_url).toBe(fixture.embedUrl);
  expect(h.journal.resources).toContainEqual({ kind: 'triggers', id, subject });
  const commands = h.commands.slice(commandsBefore);
  expect(
    commands.filter((row) => row.method === 'POST' && row.path === '/api/v1/triggers'),
  ).toHaveLength(1);
  expect(
    commands.filter(
      (row) => row.method === 'POST' && row.path === `/api/v1/triggers/${id}/publish`,
    ),
  ).toHaveLength(1);
  expect(commands.filter((row) => /profiles|marketplace/.test(row.path))).toEqual([]);
  const embed = await h.get<TriggerEmbed>(
    page,
    `/api/v1/triggers/${id}/embed?placement_id=${placement.id}`,
  );
  expect(embed.public_url).toBe(`${h.config.baseURL}/t/${document.public_id}/${placement.id}`);
  await expect(page.getByLabel('README or Markdown snippet', { exact: true })).not.toBeEmpty();
  await page.getByRole('tab', { name: 'Web page', exact: true }).click();
  await expect(page.getByLabel('Web page snippet', { exact: true })).not.toBeEmpty();
  await page.getByRole('tab', { name: 'Page with JavaScript', exact: true }).click();
  await expect(page.getByLabel('Subresource integrity', { exact: true })).toContainText('sha384-');
  return {
    requestTarget,
    authoringCommands: commands,
    document,
    revision,
    placement,
    embed,
    target,
    subject,
    observedPublisher: identity.subject,
  };
}

export async function followPublishedEmbed(page: Page, publicUrl: string) {
  const link = page.getByRole('link', { name: 'Open in CMA', exact: true });
  await expect(link).toHaveCount(1);
  expect(new URL((await link.getAttribute('href'))!, page.url()).href).toBe(publicUrl);
  const landing = Promise.race([
    page.waitForURL(publicUrl).then(() => page),
    page
      .context()
      .waitForEvent('page')
      .then(async (popup) => {
        await popup.waitForURL(publicUrl);
        return popup;
      }),
  ]);
  await link.click();
  return landing;
}
