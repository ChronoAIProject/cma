import { test } from './visitor-abuse-harness';
import { utcScenario } from './visitor-abuse-scenarios';

test('TC117-utc step 3 preserves scoped idempotency and consumed allowance across real UTC midnight', async ({
  abuse,
}) => {
  await utcScenario(abuse);
});
