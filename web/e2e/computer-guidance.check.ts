import { expect, test } from '@playwright/test';
import { LocalAgentV2, cursor } from './agent-v2.fixture';

for (const width of [390, 1600]) {
  test(`LOCAL unavailable Computer explains policy and returns to the retained draft at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    const unavailable = {
      state: 'not_enabled',
      reason: 'browser_disabled',
      scope: 'agent_runtime',
      writable: false,
    };
    const unsupported = {
      state: 'unsupported',
      reason: 'not_supported_by_contract',
      scope: 'none',
      writable: false,
    };
    try {
      await page.setViewportSize({ width, height: 844 });
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/ui/task-experience', (route) =>
        route.fulfill({
          json: {
            object: 'cma.ui.task_experience',
            schema_version: 1,
            account_id: fixture.owner,
            capabilities: {
              task_home_admission: false,
              suggested_followup_admission: false,
              desktop_enable: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(0),
            active_turn: null,
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/environment', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_contract',
            policy_revision: 1,
            runtime_incarnation: 'b'.repeat(64),
            revalidate_at: null,
            files: { ...unavailable, reason: 'file_access_disabled', scope: 'mounted_home' },
            operation_output: unsupported,
            browser: unavailable,
            replay: unsupported,
            control: unsupported,
          },
        }),
      );
      await page.goto('/agents/agt_contract');
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(composer).toBeEnabled();
      await composer.fill('Keep this unsent conversation draft');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      const entry = page.getByRole('button', { name: 'Show Agent computer', exact: true });
      await entry.click();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
      const screen = computer.getByRole('tabpanel', { name: 'Screen', exact: true });
      await expect(screen.getByRole('status')).toHaveCount(1);
      await expect(screen.getByRole('status')).toHaveText('Live screen is not enabled');
      await expect(screen).toContainText(
        'This service does not currently allow you to enable a live screen for this environment.',
      );
      await expect(screen).toContainText('You can continue the conversation');
      await expect(computer.getByRole('button', { name: 'Reconnect', exact: true })).toHaveCount(0);
      await expect(computer.getByRole('button', { name: 'Enable browser' })).toHaveCount(0);
      const help = screen.getByRole('button', { name: 'About Computer tools', exact: true });
      await screen.getByRole('button', { name: 'Back to conversation' }).focus();
      await page.keyboard.press('Tab');
      await expect(help).toBeFocused();
      await page.keyboard.press('Enter');
      const dialog = page.getByRole('dialog', { name: 'About Computer tools', exact: true });
      await expect(dialog).toBeVisible();
      await expect(dialog).toContainText('cannot enable a tool that this service does not allow');
      await expect(dialog).toContainText('Screen and file access are separate capabilities');
      await page.keyboard.press('Escape');
      await expect(dialog).toBeHidden();
      await expect(help).toBeFocused();
      await page.screenshot({ path: info.outputPath(`screen-guidance-${width}.png`) });
      await screen.getByRole('button', { name: 'Back to conversation' }).click();
      await expect(computer).toBeHidden();
      await expect(composer).toBeFocused();
      await expect(composer).toHaveValue('Keep this unsent conversation draft');
      await entry.click();
      await computer.getByRole('tab', { name: 'Files', exact: true }).click();
      const files = computer.getByRole('tabpanel', { name: 'Files', exact: true });
      await expect(files.getByRole('status')).toHaveCount(1);
      await expect(files.getByRole('status')).toHaveText(
        'File browsing is not available for this environment.',
      );
      await expect(files).not.toContainText('Live screen is not enabled');
      await expect(files).not.toContainText('This service does not currently allow');
      await expect(files.getByRole('button', { name: 'About Computer tools' })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`files-guidance-${width}.png`) });
      await files.getByRole('button', { name: 'Back to conversation' }).click();
      await expect(computer).toBeHidden();
      await expect(composer).toBeFocused();
      await expect(composer).toHaveValue('Keep this unsent conversation draft');
      await entry.click();
      await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
      await help.click();
      await dialog.getByRole('link', { name: 'Guide to environments and Computer tools' }).click();
      await expect(
        page.getByRole('heading', { name: 'Environments and capacity', exact: true }),
      ).toBeFocused();
      await expect(page.getByRole('article')).toContainText('Reconnect cannot enable a capability');
      await page.goBack();
      await expect(composer).toHaveValue('Keep this unsent conversation draft');
      await expect(composer).toBeEnabled();
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
    } catch (error) {
      console.log({
        body: await page.locator('body').innerText(),
        errors: fixture.errors,
        unexpected: fixture.boundary.unexpected,
      });
      throw error;
    } finally {
      await fixture.close(page);
    }
  });
}
