import { expect, it } from 'vitest';
import { ProtocolError } from '@cma/client';
import { errorMessage } from '../controllerError';
import { ApprovalBaselineRequired } from './approvalBaseline';

it.each(['policy', 'overflow'] as const)(
  'shows fixed reload guidance through the consumer wrapper for %s',
  (reason) => {
    const refusal = new ApprovalBaselineRequired(reason);
    const wrapped = new ProtocolError('Agent consumer did not acknowledge a complete frame', {
      cause: refusal,
    });
    expect(errorMessage(wrapped)).toBe(refusal.message);
    expect(errorMessage(wrapped)).toContain('Reload this Agent');
    expect(errorMessage(wrapped)).toContain('Your original request is retained');
  },
);
it('keeps arbitrary protocol or nested service details sanitized', () => {
  const error = new ProtocolError('Synthetic private wire detail', {
    cause: new Error('Synthetic nested detail'),
  });
  expect(errorMessage(error)).toBe('The Agent operation did not complete.');
});
