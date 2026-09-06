import type { LaunchStepKey } from '@chronoai/cma-sdk';

export const stepNames = {
  signing_in: 'Signing you in',
  checking_requirements: "Checking what's needed",
  sandbox: 'Preparing your sandbox',
  workspace: 'Setting up the workspace',
  agent: 'Starting your agent',
  first_message: 'Sending your first message',
  ready: 'Ready',
} satisfies Record<LaunchStepKey, string>;

export const stepOrder: readonly LaunchStepKey[] = [
  'signing_in',
  'checking_requirements',
  'sandbox',
  'workspace',
  'agent',
  'first_message',
  'ready',
];

export function progressDetail(detail: string | undefined): string | undefined {
  const known: Readonly<Record<string, string>> = {
    'Launch work will be retried': 'CMA will try this step again.',
    'Launch orchestration is paused': 'CMA has paused preparation.',
    'Launch recovery is required': 'Your sandbox needs attention.',
    'Finishing launch': 'Finishing preparation.',
  };
  if (detail === undefined || detail === '') return undefined;
  if (known[detail] !== undefined) return known[detail];
  return /\b(launch|revision|digest|idempotency|reconcile|retained|capability|ceiling|checkpoint|reach|fence|userservice|mib|millis)\b|\b[a-z]+_[a-z_]+\b/i.test(
    detail,
  )
    ? undefined
    : detail;
}
