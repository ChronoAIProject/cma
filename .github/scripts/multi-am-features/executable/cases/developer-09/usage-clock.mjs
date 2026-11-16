// Observe the real rendered completion inside the Page so the harness's 2-second
// observation cadence does not inflate the reported UI latency.
export async function startClock(page, range) {
  await page.evaluate(range => {
    window.__c275UsageClock?.observer?.disconnect();
    const state = {range, started: null, completed: null};
    const check = () => {
      if (state.started === null || state.completed !== null) return;
      const dashboard = document.querySelector('[aria-label="Operations resource dashboard"]');
      const table = dashboard?.querySelector('.usage-inventory table');
      if (dashboard?.getAttribute('data-usage-state') === 'committed'
        && dashboard.getAttribute('data-usage-requested-range') === range
        && table && table.getAttribute('aria-busy') !== 'true' && !table.querySelector('.table__placeholder')
        && !dashboard.querySelector('.usage-report-time')?.textContent.includes('refreshing')) {
        state.completed = performance.now(); state.observer.disconnect();
      }
    };
    state.observer = new MutationObserver(check);
    state.observer.observe(document.body, {attributes: true, childList: true, subtree: true, characterData: true});
    state.click = event => {
      if (!event.target.closest('[aria-label="Apply usage filters"]')) return;
      state.started = performance.now();
      document.removeEventListener('click', state.click, true);
      requestAnimationFrame(() => requestAnimationFrame(check));
    };
    document.addEventListener('click', state.click, true); window.__c275UsageClock = state;
  }, range);
}
export async function finishClock(page) {
  return page.evaluate(() => {
    const state = window.__c275UsageClock;
    if (!state || state.started === null || state.completed === null) throw Error('usage_dom_timing_missing');
    state.observer.disconnect(); document.removeEventListener('click', state.click, true);
    const result = {range: state.range, start_ms: state.started, complete_ms: state.completed,
      elapsed_ms: state.completed - state.started, clock: 'browser_performance', boundary: 'apply_click_to_committed_inventory'};
    delete window.__c275UsageClock; return result;
  });
}
