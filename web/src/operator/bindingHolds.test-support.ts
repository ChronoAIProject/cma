import type { BindingHoldOverview } from '@cma/client';

export const bindingTime = '2026-11-24T09:00:00Z';
export function heldBindings(): BindingHoldOverview {
  return {
    observed_at: bindingTime,
    observed_sandboxes: 1,
    registered_reporters: 3,
    unavailable_reporters: 1,
    truncated: false,
    reports: [
      {
        reporter: 'observation-a',
        sample: {
          observed_at: bindingTime,
          truncated: false,
          holds: [
            {
              sandbox_id: 'sbx_observed',
              owner: 'private-owner-id',
              owner_name: 'Alex Chen',
              reason: 'unavailable',
              phase: 'deferred',
              first_held_at: '2026-11-24T08:59:00Z',
              attempts: 2,
              retry_not_before: '2026-11-24T09:01:00Z',
              adapter_error_class: 'unavailable',
            },
          ],
        },
      },
      {
        reporter: 'observation-b',
        sample: {
          observed_at: '2026-11-24T09:00:01Z',
          truncated: false,
          holds: [
            {
              sandbox_id: 'sbx_observed',
              owner: 'private-owner-id',
              owner_name: null,
              reason: 'raced',
              phase: 'retrying',
              first_held_at: '2026-11-24T08:59:30Z',
              attempts: 1,
              retry_not_before: null,
              adapter_error_class: null,
            },
          ],
        },
      },
    ],
  };
}
