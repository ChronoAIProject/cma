import { expect, type Page } from '@playwright/test';
import { redesignHomeFixture, redesignTime, type HomeState } from './redesign-home.fixture';

export async function selectHomeTemplate(page: Page, name: string) {
  const button = page.getByRole('button', { name, exact: true });
  if (await button.isVisible()) return button.click();
  await page.getByRole('button', { name: 'Templates', exact: true }).click();
  await page.getByRole('menuitem', { name, exact: true }).click();
}

export async function homeSimplificationFixture(
  page: Page,
  baseURL: string,
  state: HomeState = 'crew',
) {
  const local = await redesignHomeFixture(page, baseURL, state);
  const previews: string[] = [];
  await page.route('**/api/v1/agents/*/reply-preview', (route) => {
    expect(route.request().method()).toBe('GET');
    const id = new URL(route.request().url()).pathname.split('/').at(-2)!;
    previews.push(id);
    return route.fulfill({
      json: {
        status: id === local.boundary.agent.id ? 'ready' : 'none',
        agent_id: id,
        source_through_seq: 9,
        latest_assistant_reply:
          id === local.boundary.agent.id
            ? {
                text: 'A real recorded reply: 中文 <sample> — synthetic findings.',
                truncated: false,
                message_sequence: 3,
                turn_sequence: 1,
                appended_at: '2026-11-22T05:55:00Z',
              }
            : null,
      },
    });
  });
  return { ...local, previews, now: redesignTime };
}
