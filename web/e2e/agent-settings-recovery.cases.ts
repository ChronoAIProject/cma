import { expect, test, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

type Mode = 'settings' | 'rename' | 'prompt';
interface Write {
  readonly key?: string;
  readonly body: Record<string, unknown>;
}

async function openEditor(page: Page, mode: Mode) {
  if (mode === 'settings') {
    await page.getByRole('button', { name: /^Agent settings:/ }).click();
    return page.getByRole('form', { name: 'Message Agent' });
  }
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page
    .getByRole('menuitem', {
      name: mode === 'rename' ? 'Rename' : 'Update system prompt',
      exact: true,
    })
    .click();
  return page.getByRole('dialog', {
    name: mode === 'rename' ? 'Rename Agent' : 'Update system prompt',
  });
}

for (const mode of ['settings', 'rename', 'prompt'] as const) {
  for (const failure of ['none', 'conflict', 'unavailable', 'lost', 'throttled'] as const) {
    test(`LOCAL ${mode} ${failure} preserves proposals and request identity through confirmation and reload`, async ({
      page,
      baseURL,
    }) => {
      const boundary = new LocalStartup();
      await boundary.route(page, baseURL!);
      const settings = {
        model: 'gpt-6',
        reasoning_effort: 'high',
        permission_mode: 'autonomous',
        pending: false,
      };
      const preferences = {
        title: 'Original title',
        system_prompt: 'Original prompt',
        revision: 3,
        prompt_pending: false,
        prompt_supported: true,
      };
      const proposal =
        mode === 'rename' ? 'Proposed title' : '  Proposed prompt\nKeep these spaces.  ';
      const writes: Write[] = [];
      await page.route('**/api/v1/agents/agt_contract/models', (route) =>
        route.fulfill({
          json: { models: [{ id: 'gpt-6', label: 'GPT-6', available: true }] },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/preferences', (route) =>
        route.fulfill({ json: preferences }),
      );
      const path =
        mode === 'settings'
          ? '**/api/agent-manager/v1/agents/agt_contract/settings'
          : '**/api/v1/agents/agt_contract/preferences';
      await page.route(path, async (route) => {
        if (route.request().method() === 'GET')
          return route.fulfill({ json: mode === 'settings' ? settings : preferences });
        expect(route.request().method()).toBe('PATCH');
        const body = route.request().postDataJSON() as Record<string, unknown>;
        writes.push({ key: route.request().headers()['idempotency-key'], body });
        if (writes.length === 1 && failure === 'conflict') {
          preferences.revision = 4;
          preferences.title = 'Server title';
          preferences.system_prompt = 'Server prompt';
          settings.reasoning_effort = 'low';
          return route.fulfill({
            status: 409,
            json: {
              code: 'agent_operation_in_flight',
              detail: 'Private upstream operation details',
            },
          });
        }
        if (writes.length === 1 && (failure === 'unavailable' || failure === 'throttled'))
          return route.fulfill({
            status: failure === 'throttled' ? 429 : 503,
            headers: { 'Retry-After': '1' },
            json: { code: 'unknown_code', detail: '<html>Private upstream details</html>' },
          });
        if (mode === 'settings') {
          expect(body).toEqual({
            model: 'gpt-6',
            reasoning_effort: 'high',
            permission_mode: 'guarded',
          });
          settings.reasoning_effort = 'high';
          settings.permission_mode = 'guarded';
        } else {
          expect(body).toEqual({
            expected_revision: failure === 'conflict' ? 4 : 3,
            ...(mode === 'rename' ? { title: proposal } : { system_prompt: proposal }),
          });
          preferences.revision = failure === 'conflict' ? 5 : 4;
          if (mode === 'rename') preferences.title = proposal;
          else preferences.system_prompt = proposal;
        }
        // The lost response can follow a server acceptance; retry still uses that intent.
        if (writes.length === 1 && failure === 'lost') return route.abort('connectionfailed');
        return route.fulfill({ json: mode === 'settings' ? settings : preferences });
      });
      await page.goto('/agents/agt_contract');
      let editor = await openEditor(page, mode);
      const field = () =>
        editor.getByRole('textbox', {
          name: mode === 'rename' ? 'Agent name' : 'Agent system prompt',
        });
      if (mode === 'settings') {
        await editor.getByRole('button', { name: 'Permission mode' }).click();
        await page.getByRole('menuitemradio', { name: 'Guarded', exact: true }).click();
      } else await field().fill(proposal);
      await editor
        .getByRole('button', { name: mode === 'settings' ? 'Apply settings' : 'Save', exact: true })
        .click();
      if (failure !== 'none') {
        await expect(editor.getByRole('alert')).toBeVisible();
        await expect(editor).not.toContainText('Private upstream');
        expect(writes).toHaveLength(1);
        if (mode === 'settings')
          await expect(editor.getByRole('button', { name: 'Permission mode' })).toHaveText(
            'Guarded',
          );
        else await expect(field()).toHaveValue(proposal);
        if (failure === 'conflict') {
          await expect(editor.getByRole('alert')).toContainText(
            'The Agent state changed or an update is in progress.',
          );
          await editor
            .getByRole('button', {
              name: mode === 'settings' ? 'Refresh current settings' : 'Reload preferences',
            })
            .click();
          const apply = editor.getByRole('button', {
            name: mode === 'settings' ? 'Apply settings' : 'Save',
            exact: true,
          });
          await expect(apply).toBeEnabled();
          if (mode !== 'settings') {
            await expect(field()).toHaveValue(proposal);
            await expect(editor).toContainText('Current preferences revision: 4.');
          }
          expect(writes).toHaveLength(1);
          await apply.click();
        } else {
          const retry = editor.getByRole('button', {
            name: mode === 'settings' ? 'Retry settings update' : 'Retry change',
          });
          if (failure === 'unavailable' || failure === 'throttled')
            await expect(retry).toBeDisabled();
          await expect(retry).toBeEnabled();
          expect(writes).toHaveLength(1);
          await retry.click();
        }
      }
      if (mode === 'settings')
        await expect(editor.getByText('Settings applied to this Agent.')).toBeVisible();
      else await expect(editor).toHaveCount(0);
      expect(writes).toHaveLength(failure === 'none' ? 1 : 2);
      expect(writes[0]?.key).toBeTruthy();
      if (failure === 'conflict') expect(writes[1]?.key).not.toBe(writes[0]?.key);
      else if (failure !== 'none') expect(writes[1]).toEqual(writes[0]);
      await page.reload();
      editor = await openEditor(page, mode);
      if (mode === 'settings')
        await expect(editor.getByRole('button', { name: 'Permission mode' })).toHaveText('Guarded');
      else await expect(field()).toHaveValue(proposal);
      expect(writes).toHaveLength(failure === 'none' ? 1 : 2);
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
