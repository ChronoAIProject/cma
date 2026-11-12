import { requireInstances } from './visitor-abuse-model';
import { test } from './visitor-abuse-harness';
import { boundaryScenario, sharedQuota } from './visitor-abuse-scenarios';
import { loadAndControls } from './visitor-abuse-load';

test('TC117-boundary step 1 refuses unauthorized creation and a third home', async ({ abuse }) => {
  await boundaryScenario(abuse);
});
test('TC117-shared step 2 shares five daily admissions across two instances and Triggers', async ({
  abuse,
}) => {
  await sharedQuota(abuse);
});
test('TC117-load-controls steps 4 and 5 bound anonymous traffic and preserve controls without work on reads', async ({
  abuse,
  playwright,
}) => {
  requireInstances(abuse.fixture.instances);
  const browser = await playwright.chromium.launch({
    executablePath: process.env.CMA_E2E_BROWSER_PATH,
  });
  try {
    await loadAndControls(abuse, browser);
  } finally {
    await browser.close();
  }
});
