import { expect, test, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

type Mode = 'settings' | 'rename' | 'prompt';
interface Write {
  readonly key?: string;
  readonly body: Record<string, unknown>;
  readonly bytes: string | null;
}

async function openEditor(page: Page, mode: Mode) {
  if (mode === 'settings') {
    await page.getByRole('button', { name: /^Agent settings:/ }).click();
    return page.getByRole('form', { name: 'Message Agent' });
  }
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page
    .getByRole('menuitem', {
      name:
        mode === 'rename'
          ? 'Rename'
          : /^Update system prompt(?:$| Review the pending system prompt change\.)/,
      exact: true,
    })
    .click();
  return page.getByRole('dialog', {
    name: mode === 'rename' ? 'Rename Agent' : 'Update system prompt',
  });
}

for (const mode of ['settings', 'rename', 'prompt'] as const) {
  for (const failure of [
    'none',
    'conflict',
    'unavailable',
    'lost',
    'throttled',
    'lost-then-denied',
    '503-then-denied',
  ] as const) {
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
        mode === 'rename'
          ? 'Proposed title'
          : '  Synthetic prompt storage marker\nKeep these spaces.  ';
      const writes: Write[] = [];
      const storageWrites: boolean[] = [];
      if (mode === 'prompt') {
        await page.exposeBinding('recordPromptStorageWrite', (_, containsPrompt: boolean) => {
          storageWrites.push(containsPrompt);
        });
        await page.addInitScript(() => {
          const write = Storage.prototype.setItem;
          const { recordPromptStorageWrite } = window as Window & {
            recordPromptStorageWrite?: (containsPrompt: boolean) => Promise<void>;
          };
          Storage.prototype.setItem = function (key, value) {
            void recordPromptStorageWrite?.(value.includes('Synthetic prompt storage marker'));
            return write.call(this, key, value);
          };
        });
      }
      const receipts = new Map<string, { write: Write; result: Record<string, unknown> }>();
      let committed = 0;
      const deniedReplay = failure === 'lost-then-denied' || failure === '503-then-denied';
      const lostResponse = failure === 'lost' || failure === 'lost-then-denied';
      const unavailable = failure === 'unavailable' || failure === '503-then-denied';
      const acceptedEffort = failure === 'conflict' ? 'low' : 'high';
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
        expect(route.request().headers()['prefer']).toBe('cma-settings-reasons');
        const body = route.request().postDataJSON() as Record<string, unknown>;
        const key = route.request().headers()['idempotency-key'];
        const write = { key, body, bytes: route.request().postData() };
        writes.push(write);
        if (writes.length === 2 && deniedReplay) {
          preferences.revision = 9;
          settings.reasoning_effort = 'low';
          return route.fulfill({
            status: lostResponse ? 401 : 403,
            json: { code: 'forbidden', detail: 'Private access details' },
          });
        }
        if (writes.length === 1 && failure === 'conflict') {
          preferences.revision = 4;
          preferences.title = 'Server title';
          preferences.system_prompt = 'Server prompt';
          settings.reasoning_effort = 'low';
          return route.fulfill({
            status: 409,
            json: {
              code: 'agent_operation_in_flight',
              settings_update_reason:
                mode === 'settings' ? 'dispatch_in_progress' : 'stale_revision',
              detail: 'Private upstream operation details',
            },
          });
        }
        if (writes.length === 1 && (unavailable || failure === 'throttled'))
          return route.fulfill({
            status: failure === 'throttled' ? 429 : 503,
            headers: { 'Retry-After': '1' },
            json: { code: 'unknown_code', detail: '<html>Private upstream details</html>' },
          });
        const receipt = receipts.get(key!);
        if (receipt) {
          expect(write).toEqual(receipt.write);
          return route.fulfill({ json: receipt.result });
        }
        if (mode === 'settings') {
          expect(body).toEqual({
            model: 'gpt-6',
            reasoning_effort: acceptedEffort,
            permission_mode: 'guarded',
          });
          Object.assign(settings, body);
        } else {
          expect(body).toEqual({
            expected_revision: failure === 'conflict' ? 4 : 3,
            ...(mode === 'rename' ? { title: proposal } : { system_prompt: proposal }),
          });
          preferences.revision = failure === 'conflict' ? 5 : Math.max(preferences.revision, 4);
          if (mode === 'rename') preferences.title = proposal;
          else preferences.system_prompt = proposal;
        }
        expect(key).toBeTruthy();
        committed += 1;
        const result = { ...(mode === 'settings' ? settings : preferences) };
        receipts.set(key!, { write, result });
        // The lost response can follow a server acceptance; retry still uses that intent.
        if (writes.length === 1 && lostResponse) return route.abort('connectionfailed');
        return route.fulfill({ json: result });
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
            mode === 'settings'
              ? 'A dispatch was in progress when this update was refused.'
              : 'Preferences changed before this update.',
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
          if (mode === 'settings') {
            await expect(
              editor.getByRole('button', { name: /Agent settings:.*Low.*Autonomous/ }),
            ).toBeVisible();
            await expect(
              editor.getByRole('button', { name: /Model and reasoning effort:.*Low/ }),
            ).toBeVisible();
          } else {
            await expect(field()).toHaveValue(proposal);
            await expect(editor).toContainText('Current preferences revision: 4.');
          }
          expect(writes).toHaveLength(1);
          await apply.click();
        } else {
          if (failure === 'lost') {
            expect(committed).toBe(1);
            await page.reload();
            editor = await openEditor(page, mode);
            expect(writes).toHaveLength(1);
          }
          const retry = editor.getByRole('button', {
            name: mode === 'settings' ? 'Retry settings update' : 'Retry change',
          });
          if (mode === 'prompt' && failure === 'lost') {
            await expect(field()).toHaveValue('');
            await field().fill('A different proposed prompt');
            await retry.click();
            await expect(editor.getByRole('alert')).toContainText(
              'Enter the original system prompt exactly',
            );
            expect(writes).toHaveLength(1);
            await field().fill(proposal);
          }
          if (unavailable || failure === 'throttled') await expect(retry).toBeDisabled();
          await expect(retry).toBeEnabled();
          expect(writes).toHaveLength(1);
          await retry.click();
          if (deniedReplay) {
            await expect(editor.getByRole('alert')).toContainText(
              /refresh to check access before saving/i,
            );
            if (mode === 'prompt') {
              await expect(field()).toHaveCount(0);
              await expect(retry).toHaveCount(0);
              await expect(editor).not.toContainText(
                /pending change|Synthetic prompt storage marker/,
              );
            } else {
              await expect(retry).toBeDisabled();
              await expect(editor).toContainText('reopen this editor in the same tab');
            }
            expect(writes).toHaveLength(2);
            await editor
              .getByRole('button', {
                name: mode === 'settings' ? 'Refresh current settings' : 'Reload preferences',
              })
              .click();
            await expect(retry).toBeEnabled();
            if (mode === 'prompt') {
              await expect(field()).toHaveValue('');
              await expect(editor).toContainText('Re-enter the exact original system prompt');
              await field().fill(proposal);
            } else if (mode === 'rename') {
              await expect(field()).toHaveValue(proposal);
              await expect(editor).toContainText('Current preferences revision: 9.');
            }
            if (mode !== 'prompt')
              await expect(editor).toContainText('reopen this editor in the same tab');
            await expect(editor).not.toContainText('applied to this Agent');
            expect(writes).toHaveLength(2);
            await retry.click();
          }
        }
      }
      if (mode === 'settings')
        await expect(editor.getByText('Settings applied to this Agent.')).toBeVisible();
      else await expect(editor).toHaveCount(0);
      const writeCount = failure === 'none' ? 1 : deniedReplay ? 3 : 2;
      expect(writes).toHaveLength(writeCount);
      expect(committed).toBe(1);
      expect(writes[0]?.key).toBeTruthy();
      if (failure === 'conflict') expect(writes[1]?.key).not.toBe(writes[0]?.key);
      else if (failure !== 'none') expect(writes[1]).toEqual(writes[0]);
      if (deniedReplay) expect(writes[2]).toEqual(writes[0]);
      await page.reload();
      editor = await openEditor(page, mode);
      if (mode === 'settings') {
        await expect(editor.getByRole('button', { name: 'Permission mode' })).toHaveText('Guarded');
        if (failure === 'conflict')
          await expect(
            editor.getByRole('button', { name: /Agent settings:.*Low.*Guarded/ }),
          ).toBeVisible();
      } else await expect(field()).toHaveValue(proposal);
      expect(writes).toHaveLength(writeCount);
      if (mode === 'prompt') {
        expect(storageWrites.length).toBeGreaterThan(0);
        expect(storageWrites).not.toContain(true);
      }
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
