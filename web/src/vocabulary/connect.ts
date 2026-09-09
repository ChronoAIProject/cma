import type { ServiceConnectionState } from '@chronoai/cma-sdk';

export const connectionStates = {
  pending: 'Waiting for NyxID',
  satisfied: 'Connected',
  expired: 'That link expired. Try again.',
  cancelled: 'The connection was cancelled. Try again.',
  failed: 'The account could not be connected. Try again.',
} satisfies Record<ServiceConnectionState, string>;
export const connectionCopy = {
  required: 'Connect required accounts',
  verifyAll: 'Verify connections',
  title: (name: string) => `Connect ${name}`,
  connect: 'Connect',
  unknown: 'This service',
  metadataUnavailable: 'Account setup details are unavailable. Check again in a moment.',
  checklist: [
    'A NyxID window opened. Follow the steps there.',
    'Return here and select Verify after connecting.',
    "We'll continue automatically.",
  ],
  blocked: 'Your browser blocked the NyxID window. Continue in this tab, then return here.',
  sameTab: 'Continue in NyxID',
  verify: 'Verify',
  verifying: 'Verifying',
  notConnected:
    'This account is not connected yet. Select Connect to finish in NyxID, then Verify again.',
  manual: 'Set up this account in NyxID first.',
  open: 'Open NyxID',
  expired: 'That link expired. Try again.',
  timeout:
    'We stopped checking after five minutes. Check again when you have connected the account.',
  invalidUrl: 'NyxID did not provide a secure account setup page. Try again later.',
} as const;
