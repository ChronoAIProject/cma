import { test, expect, type Page } from '@playwright/test';
import { placementBoundary } from './bridge-placement.fixture';
import { organizationTarget } from '../src/bridges/organization.fixture';

const modes = [
  { kind: 'new_sandbox', label: 'A separate Sandbox for each conversation' },
  { kind: 'workspace_per_partition', label: 'A new Workspace in an existing Sandbox' },
  { kind: 'worktree_agent_per_partition', label: 'A new Agent in an existing Workspace' },
  { kind: 'existing_agent', label: 'One retained Agent' },
] as const;

async function choose(page: Page, label: string, option: string) {
  await page.getByRole('button', { name: label, exact: true }).click();
  await page.getByRole('menuitemradio', { name: option, exact: true }).click();
}

for (const mode of modes)
  test(`LOCAL ${mode.kind} editor keeps exact consent and fits both themes and narrow screens`, async ({
    page,
    baseURL,
  }, info) => {
    const { state, boundary } = await placementBoundary(page, baseURL!);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/organizations/org-one/bridges/new');
    await page.getByLabel('Bridge name', { exact: true }).fill('Organization research');
    await choose(page, 'Existing channel bot', 'telegram · bot-one');
    await choose(page, 'Existing channel key', 'key-one');
    await page.getByRole('checkbox', { name: 'route-one' }).check();
    await choose(page, 'Resource placement', mode.label);
    const retained = mode.kind === 'existing_agent';
    const shared =
      mode.kind === 'workspace_per_partition' || mode.kind === 'worktree_agent_per_partition';
    if (mode.kind !== 'new_sandbox') {
      await choose(page, retained ? 'Retained Sandbox' : 'Shared Sandbox', 'Organization Sandbox');
      if (mode.kind !== 'workspace_per_partition')
        await choose(
          page,
          retained ? 'Retained Workspace' : 'Shared Workspace',
          'Organization project',
        );
    }
    if (retained) {
      await choose(page, 'Retained Agent', 'Organization Agent');
      await expect(page.getByText('Linked to a channel', { exact: true })).toBeVisible();
      await expect(page.getByText(/Its output remains in CMA/)).toBeVisible();
      await expect(
        page.getByText('Unavailable in the current resource view', { exact: true }),
      ).toHaveCount(2);
    } else {
      if (mode.kind === 'new_sandbox')
        await choose(page, 'sandbox profile', 'Research sandbox · Revision 1');
      if (mode.kind !== 'worktree_agent_per_partition')
        await choose(page, 'workspace profile', 'Research workspace · Revision 1');
      await choose(page, 'agent profile', 'Research agent · Revision 1');
    }
    if (shared) {
      await expect(page.getByRole('checkbox', { name: /I acknowledge/ })).not.toBeChecked();
      await page.getByRole('checkbox', { name: /I reviewed who owns/ }).check();
      await expect(page.getByRole('button', { name: 'Save disabled bridge' })).toBeDisabled();
      await page.getByRole('checkbox', { name: /I acknowledge/ }).check();
      await page.getByLabel('Agent slots per Workspace reserved for other work').fill('0');
      await expect(page.getByRole('button', { name: 'Save disabled bridge' })).toBeDisabled();
      await page.getByLabel('Agent slots per Workspace reserved for other work').fill('2');
      await page.getByRole('checkbox', { name: /I acknowledge/ }).check();
    }
    await page.getByRole('checkbox', { name: /I reviewed who owns/ }).check();
    const save = page.getByRole('button', { name: 'Save disabled bridge', exact: true });
    await expect(save).toBeEnabled();
    const picker = page.getByRole('button', { name: 'Resource placement', exact: true });
    await picker.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('menuitemradio', { name: mode.label, exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    for (const theme of ['light', 'dark']) {
      await page.evaluate(
        (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
        theme,
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 1000 });
        const fields = page.getByRole('group', { name: 'Conversation and resources', exact: true });
        await fields.scrollIntoViewIfNeeded();
        await expect
          .poll(() =>
            fields.evaluate((element) =>
              [...element.querySelectorAll('button,input,dd')].every((item) => {
                const rect = item.getBoundingClientRect();
                return rect.width === 0 || (rect.left >= 0 && rect.right <= innerWidth + 1);
              }),
            ),
          )
          .toBe(true);
        await page.screenshot({
          path: info.outputPath(`${mode.kind}-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
    }
    if (shared || retained) state.loseFirstSave = true;
    await save.click();
    if (shared || retained) {
      const retry = page.getByRole('button', { name: 'Retry create bridge' });
      await expect(retry).toBeVisible();
      state.available = false;
      state.authorized = false;
      await retry.click();
      await expect.poll(() => state.writes.length).toBe(2);
      expect(state.writes[1]).toEqual(state.writes[0]);
    } else await expect.poll(() => state.writes.length).toBe(1);
    const config = state.writes[0]!.body.config as { placement: Record<string, unknown> };
    expect(config.placement.kind).toBe(mode.kind);
    if (retained) expect(config.placement.target).toEqual(organizationTarget);
    if (shared)
      expect(config.placement).toMatchObject({
        sandbox_id: 'sandbox-1',
        shared_trust: 'all_participants_share_filesystem_processes_and_credentials',
        agent_profile: { id: 'agent-1', revision: 1 },
        capacity: { keep_other_workspaces: 1, keep_other_agents_per_workspace: 2 },
      });
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
