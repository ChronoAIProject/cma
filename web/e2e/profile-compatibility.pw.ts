import { test, expect } from './harness';

test('COMPAT-TC025 historical public profile requires explicit trust before Sandbox allocation', async ({
  harness: h,
}) => {
  const page = await h.open('owner');
  const profile = h.fixture.profiles.sandbox;
  const before = h.commands.length;
  await page.goto(`${h.config.baseURL}/marketplace/sandbox/${profile.id}`);
  await page.getByRole('button', { name: 'Use', exact: true }).click();
  const create = page.getByRole('button', { name: 'Create sandbox', exact: true });
  await expect(create).toBeDisabled();
  await h.flush();
  expect(h.commands.slice(before)).toEqual([]);
  expect(h.journal.resources).toEqual([]);
  await page.getByLabel('I trust this profile', { exact: true }).check();
  await expect(create).toBeEnabled();
  await h.capture(page, 'compat-tc025-trust');
  await create.click();
  await page.waitForURL(/\/sandboxes\/[^/]+$/);
  await h.flush();
  const allocations = h.commands.slice(before).filter((row) => row.path === '/api/v1/sandboxes');
  expect(allocations).toHaveLength(1);
  expect(allocations[0]?.method).toBe('POST');
  const sandboxes = h.journal.resources.filter((row) => row.kind === 'sandboxes');
  expect(sandboxes).toHaveLength(1);
  expect(sandboxes[0]?.subject).toBe(h.fixture.accounts.owner.subject);
});
