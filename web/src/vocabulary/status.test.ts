import { expect, it } from 'vitest';
import { statusLabel } from './status';

it.each([
  ['active', 'Live'],
  ['provisioning', 'Provisioning'],
  ['installing', 'Installing'],
  ['reviving', 'Resuming'],
  ['rotating', 'Updating runtime'],
  ['credential_rotating', 'Updating access'],
  ['pausing', 'Pausing'],
  ['paused', 'Paused'],
  ['retiring', 'Deleting'],
  ['retired', 'Deleted'],
  ['ready', 'Ready'],
  ['preparing', 'Starting'],
  ['creating', 'Starting'],
  ['starting', 'Starting'],
  ['working', 'Working'],
  ['stopped', 'Stopped'],
  ['failed', 'Needs attention'],
] as const)('names resource state %s as %s, never Done', (status, label) => {
  expect(statusLabel(status)).toBe(label);
});
