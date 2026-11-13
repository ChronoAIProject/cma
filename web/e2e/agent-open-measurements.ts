import type { Page } from '@playwright/test';

/** DOM publication plus a rendering opportunity, independent of assertion polling. */
export async function observeAgentOpening(page: Page) {
  await page.addInitScript(() => {
    const prefix = 'cma:agent-open:';
    let scheduled = false;
    const mark = (name: string) => {
      if (performance.getEntriesByName(prefix + name).length === 0) performance.mark(prefix + name);
    };
    const observe = () => {
      if (scheduled) return;
      scheduled = true;
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          scheduled = false;
          const surface = document.querySelector('.agent-surface');
          const editor = surface?.querySelector<HTMLTextAreaElement>('textarea');
          if (!surface || !editor) return;
          mark('meaningfulPaint');
          if (!editor.disabled) mark('editorEnabled');
          const send = surface.querySelector<HTMLButtonElement>(
            'button[aria-label="Send message"]',
          );
          if (send && !send.disabled) mark('inputReady');
          if (surface.textContent?.includes('Previous recorded answer')) {
            mark('content');
            const notice = surface.querySelector('.agent-surface__history-notice');
            // Keep animation-inclusive completion comparable with the production
            // predicate while exposing when actual history publication finished.
            if (!notice || notice.getAttribute('data-presence') === 'exiting')
              mark('historyPublished');
            if (!notice) mark('historyComplete');
          }
        }),
      );
    };
    new MutationObserver(observe).observe(document, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
    // Cold navigation starts at navigationStart. List navigation starts on the
    // user's click, while any earlier intent work remains visible in the trace.
    document.addEventListener(
      'click',
      (event) => {
        if (
          event.target instanceof Element &&
          event.target.closest('a[href="/agents/agt_contract"]')
        ) {
          for (const entry of performance.getEntriesByType('mark'))
            if (entry.name.startsWith(prefix)) performance.clearMarks(entry.name);
          performance.mark(prefix + 'start');
        }
      },
      true,
    );
  });
}

export async function agentOpeningTimings(page: Page) {
  return page.evaluate(() => {
    const prefix = 'cma:agent-open:';
    const entries = performance
      .getEntriesByType('mark')
      .filter((entry) => entry.name.startsWith(prefix));
    const start = entries.find((entry) => entry.name === prefix + 'start')?.startTime ?? 0;
    return Object.fromEntries(
      entries.map((entry) => [entry.name.slice(prefix.length), entry.startTime - start]),
    );
  });
}

/** Include initial scripts, styles, fonts and lazy chunks alongside the API waterfall. */
export async function agentOpeningResources(page: Page) {
  return page.evaluate(() => ({
    navigation: performance.getEntriesByType('navigation').map((entry) => {
      const navigation = entry as PerformanceNavigationTiming;
      return {
        domContentLoadedMs: navigation.domContentLoadedEventEnd,
        loadMs: navigation.loadEventEnd,
      };
    }),
    resources: performance.getEntriesByType('resource').map((entry) => {
      const resource = entry as PerformanceResourceTiming;
      return {
        path: new URL(resource.name).pathname,
        type: resource.initiatorType,
        startMs: resource.startTime,
        endMs: resource.responseEnd,
        bytes: resource.encodedBodySize,
      };
    }),
  }));
}
