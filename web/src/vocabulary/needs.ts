import type { LaunchReviewNeed, TriggerPreview } from '@cma/client';
import type { SignupOptions } from '../auth/signup';

export const needNames = {
  trust_confirmation: 'Install software',
  connection: 'Connect your account',
  owner_requires_review: 'Review before continuing',
  repeat_choice: 'Choose your agent',
} satisfies Record<LaunchReviewNeed['type'], string>;
export const needOrder = {
  trust_confirmation: 0,
  connection: 1,
  owner_requires_review: 2,
  repeat_choice: 3,
} satisfies Record<LaunchReviewNeed['type'], number>;
export const buttonNames = {
  open_in_cma: 'Open in CMA',
  chat_with_this: 'Chat with this',
  run_with_cma: 'Run with CMA',
  try_it: 'Try it',
} satisfies Record<TriggerPreview['button_label'], string>;
export const publicCopy = {
  loading: 'Loading',
  pageUnavailable: 'Page unavailable',
  pageTitle: (title: string) => `${title} | CMA`,
  updatesPaused: 'Automatic updates have paused. Check again for the latest progress.',
  from: (publisher: string) => `From ${publisher}`,
  next: 'What happens next',
  details: 'Technical details',
  model: 'Model',
  effort: 'Thinking',
  size: 'Size',
  services: 'Services',
  defaults: 'Default',
  none: 'None',
  software: 'Software will be installed in your sandbox.',
  fullSoftware: 'Installation commands',
  environment: 'Environment values',
  confirmation: 'I understand this software will be installed in my sandbox',
  review: 'Review and continue',
  continue: 'Continue',
  back: 'Back',
  signIn: 'Sign in',
  createAccount: 'Create account',
  unavailable: 'This agent is no longer available from this page.',
  title: 'Preparing your agent',
  reviewDetails: 'The publisher asks you to review these details before continuing.',
  firstMessage: 'Your first message',
  retry: 'Try again',
  checkAgain: 'Check again',
  startOver: 'Start over',
  copySupport: 'Copy support code',
  copied: 'Support code copied',
  copyFailed: 'The support code could not be copied.',
  repeat: 'Choose your agent',
  previous: 'Continue where you left off',
  fresh: 'Start fresh',
  untitled: 'Your previous agent',
  signupUnavailable:
    'Account options are unavailable. You can still sign in to an existing account.',
  signupClosed: 'New accounts are not available. Sign in to an existing account.',
  storageUnavailable:
    'Your browser could not save this request. Allow session storage and try again.',
  storageChanged: 'Your saved request changed. Read it again before continuing.',
  storageReadUnavailable:
    'Your saved request could not be read. Allow session storage and try again.',
  expired: 'Your saved request expired. Select the CMA button to start again.',
  readOnly: 'Your account cannot start this agent. Contact your CMA administrator.',
  offline: (at: string) => `Updates are unavailable. Last checked ${at}.`,
  observation: (at: string) => `Last checked ${at}`,
  cancelled: 'This request was cancelled.',
} as const;

export function signupSentence(options: SignupOptions): string {
  if (!options.registerAvailable) return publicCopy.signupClosed;
  if (options.inviteCodeRequired)
    return "New here? Create a NyxID account with an invitation code. You'll come straight back.";
  const names: Readonly<Record<string, string>> = {
    github: 'GitHub',
    google: 'Google',
    apple: 'Apple',
  };
  const providers = options.socialProviders.flatMap((id) =>
    names[id] === undefined ? [] : [names[id]!],
  );
  if (options.emailSignup) providers.push('email');
  const methods =
    providers.length === 0
      ? ''
      : ` with ${new Intl.ListFormat('en', { type: 'disjunction' }).format(providers)}`;
  return `New here? Create a free NyxID account${methods}. You'll come straight back.`;
}
