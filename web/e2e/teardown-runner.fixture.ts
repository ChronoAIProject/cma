import { writeFileSync } from 'node:fs';
import { test, expect } from './harness';
import { teardownFixture } from './teardown.fixture';

let f: Awaited<ReturnType<typeof teardownFixture>>;
let closed = false;
let remaining = 0;
const held = process.env.CMA_TEARDOWN_CONTROL_MODE === 'held';

test.beforeAll(async () => {
  f = await teardownFixture();
  Object.assign(process.env, f.fixture.env);
});
test.afterEach(async () => {
  // Consume the real shared after-hooks slot before the exported fixture runs.
  await new Promise((resolve) => setTimeout(resolve, 800));
});
test.afterAll(async () => {
  try {
    writeFileSync(
      process.env.CMA_TEARDOWN_CONTROL_RECEIPT!,
      JSON.stringify({
        closed,
        remaining,
        requests: f.requests,
        rows: f.rows(),
      }),
    );
  } finally {
    await f.close();
  }
});
test('J4-oauth LOCAL real fixture teardown allocation', async ({ harness }) => {
  harness.config.baseURL = f.origin;
  const page = await harness.open('visitor');
  page.context().on('close', () => {
    closed = true;
  });
  await f.own(page, harness);
  expect(harness.journal.resources).toHaveLength(1);
  if (held) await f.hold(page);
  const dispose = harness.dispose.bind(harness);
  harness.dispose = (allocation) => {
    remaining = allocation ?? 0;
    return dispose(allocation);
  };
});
