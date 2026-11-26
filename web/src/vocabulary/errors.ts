import { ApiError, ProtocolError, TransportError } from '@cma/client';
import { CredentialAccountError, credentialSetupMessage } from '../auth/credentialSetup';
import { visitorAllowanceGuidanceEnabled } from '../public/visitorAllowanceFeature';

export const refusalSentences = {
  sandbox_credential_setup_required: credentialSetupMessage,
  forbidden: 'Your account cannot start this agent. Contact your CMA administrator.',
  triggers_disabled: 'CMA has paused new requests. Try again later.',
  trigger_not_found: 'This page is not available. Ask the publisher for a current link.',
  placement_not_found: 'This link is no longer available. Ask the publisher for a current link.',
  trigger_retired: 'This agent is no longer available from this page.',
  trigger_not_published: 'This page is not available yet.',
  trigger_repeat_choice_required: 'Choose whether to continue your previous agent or start fresh.',
  launch_not_found: 'This request is not available to your account.',
  launch_not_startable: 'This request has already moved on. Check its latest progress.',
  launch_requirements_not_met: 'Connect the accounts below to continue.',
  trust_confirmation_mismatch: 'Review the software below and confirm it before continuing.',
  invalid_launch_request: 'This request could not be accepted. Start over from the original page.',
  invalid_idempotency_key: 'This request could not be accepted. Start over from the original page.',
  idempotency_in_flight:
    'CMA is still processing this request. Your choices are saved; check again in a moment.',
  idempotency_recovery_required:
    'This request needs review. Your choices are saved. Contact your CMA administrator to check its result.',
  idempotency_key_reused:
    'This request has different saved choices. Start over to use the new choices.',
  invalid_launch_cursor: 'Progress could not be updated. Check again in a moment.',
  cursor_beyond_end: 'Progress could not be updated. Check again in a moment.',
  launch_changed: 'This request changed while you were reviewing it. Check its latest progress.',
  launch_service_unavailable: 'CMA could not complete this request. Try again in a moment.',
  trigger_service_unavailable: 'This page could not be loaded. Try again in a moment.',
  service_connection_not_found: 'This account connection is no longer available. Connect again.',
  invalid_service_connection: 'This account could not be connected. Check its setup in NyxID.',
  service_connection_intent_reused:
    'This connection attempt has different saved choices. Connect again.',
  service_connection_changed: 'This account connection changed. Check again in a moment.',
  service_connection_unavailable: 'NyxID could not complete the connection. Try again in a moment.',
  trigger_reference_unavailable:
    'The software for this page is no longer available. Contact its publisher.',
  trigger_reference_not_public: 'The software for this page is not shared. Contact its publisher.',
} as const;

export function settledAllowanceReset(error: unknown): Date | undefined {
  if (
    !(error instanceof ApiError) ||
    error.status !== 429 ||
    error.code !== 'resource_quota' ||
    error.extensions.resource !== 'visitor_launches' ||
    typeof error.extensions.reset_at !== 'string'
  )
    return undefined;
  const resetAt = new Date(error.extensions.reset_at);
  return Number.isNaN(resetAt.getTime()) ? undefined : resetAt;
}

export function refusalSentence(error: unknown): string {
  if (error instanceof CredentialAccountError) return error.message;
  const resetAt = visitorAllowanceGuidanceEnabled() ? settledAllowanceReset(error) : undefined;
  if (resetAt !== undefined) {
    const resetTime = new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    }).format(resetAt);
    return `You have used your daily allowance for starting agents. Try again after ${resetTime}.`;
  }
  if (
    error instanceof ApiError &&
    error.code !== undefined &&
    Object.hasOwn(refusalSentences, error.code)
  )
    return refusalSentences[error.code as keyof typeof refusalSentences];
  return 'CMA could not confirm the result. Your previous choices are saved; try again in a moment.';
}

export function failureRemediation(code: string): string {
  if (visitorAllowanceGuidanceEnabled() && code === 'sandbox_limit_reached')
    return 'Your sandbox allowance is full. Delete a sandbox you no longer need, wait for deletion to complete, then start again.';
  switch (code) {
    case 'first_message_failed':
      return 'Your first message could not be sent. Open the agent to check its conversation before sending it again.';
    case 'recovery_required':
      return 'Your sandbox needs attention. Open it in CMA to review the recovery options.';
    case 'runtime_unavailable':
      return 'Your sandbox could not be prepared. CMA will check again when service is available.';
    default:
      return 'CMA could not finish preparing your agent. Check again, or share the support code with your administrator.';
  }
}

const NO_DETAIL = /^HTTP (\d{3}) problem document carried no detail$/;

function statusSentence(status: number): string | undefined {
  if (status === 401) return 'Your sign-in has expired. Reload the page to sign in again.';
  if (status === 403) return 'Your account does not have access to this.';
  if (status === 404) return 'This could not be found. It may have been deleted or moved.';
  if (status === 429) return 'CMA is receiving too many requests. Wait a moment, then try again.';
  if (status >= 500) return 'CMA had a problem on its side. Try again in a moment.';
  return undefined;
}

/**
 * The sentence a person sees when a read or command fails. A service detail
 * written for people is kept; a protocol fault, a server-side failure or a
 * lost connection becomes a plain sentence that says what to do next.
 */
export function readableError(error: unknown, fallback: string): string {
  if (typeof error === 'object' && error !== null && 'name' in error && error.name === 'AbortError')
    return 'The request was interrupted. Check the current status and try again.';
  if (
    error instanceof TransportError ||
    (error instanceof TypeError && /fetch|network|load failed/i.test(error.message))
  )
    return 'CMA could not be reached. Check your connection, then try again.';
  if (error instanceof ApiError)
    return error.status >= 500 ? (statusSentence(error.status) ?? fallback) : error.message;
  if (error instanceof ProtocolError) {
    const status = NO_DETAIL.exec(error.message)?.[1];
    return (status === undefined ? undefined : statusSentence(Number(status))) ?? fallback;
  }
  return error instanceof Error && error.message !== '' ? error.message : fallback;
}
