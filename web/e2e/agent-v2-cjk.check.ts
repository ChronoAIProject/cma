import { test, expect } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';

for (const theme of ['light', 'dark']) {
  test(`LOCAL CJK emphasis survives streaming, completion, copy and reload in ${theme}`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    const partial = '**检查日期：';
    const content = `${partial}**2026 年\n\n**检查范围：**仅对当前本地工作区\n\n**QA 结论：证据不足，暂不具备通过验收的依据。**上述为结果`;
    const labels = ['检查日期：', '检查范围：', 'QA 结论：证据不足，暂不具备通过验收的依据。'];
    const paragraphs = [
      '检查日期：2026 年',
      '检查范围：仅对当前本地工作区',
      `${labels[2]}上述为结果`,
    ];
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    try {
      await fixture.route(page, baseURL!);
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      await page.goto('/agents/agt_contract');
      await expect.poll(() => fixture.streams.size).toBe(1);
      const answer = page.locator('.message--assistant');
      const body = answer.locator('.agent-content');
      fixture.live('text_delta', partial);
      await expect(body).toBeEmpty();
      await expect(body.locator('strong')).toHaveCount(0);
      fixture.live('text_delta', content.slice(partial.length));
      await expect(body.locator('strong')).toHaveText(labels);
      expect(await body.locator('p').allTextContents()).toEqual(paragraphs);
      fixture.append({ type: 'message', role: 'assistant', content });
      fixture.append({ type: 'turn_completed', turn_sequence: 1 });
      await expect(answer).toHaveCount(1);
      await expect(body).not.toHaveAttribute('data-streaming');
      await expect(body.locator('strong')).toHaveText(labels);
      const completed = await body.innerHTML();
      await answer.getByRole('button', { name: 'Copy content', exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(content);
      await page.reload();
      await expect(answer).toHaveCount(1);
      await expect(body.locator('strong')).toHaveText(labels);
      expect(await body.innerHTML()).toBe(completed);
      expect(await body.locator('p').allTextContents()).toEqual(paragraphs);
      await page.getByRole('link', { name: 'CMA home', exact: true }).click();
      await expect(page).toHaveURL(new URL('/agent-manager', baseURL!).href);
      await page.goBack();
      await expect(body.locator('strong')).toHaveText(labels);
      expect(await body.innerHTML()).toBe(completed);
      await answer.getByRole('button', { name: 'Copy content', exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(content);
      expect(fixture.items.find((item) => item.kind.type === 'message')?.kind.content).toBe(
        content,
      );
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
