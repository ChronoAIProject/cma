import { connectionUrl, type ConnectionTarget } from '../connections/client';
import { signInPath } from './nyxid';

type AccountGrant = {
  readonly status:
    | 'authorized'
    | 'partial'
    | 'expired'
    | 'not_granted'
    | 'revoked'
    | 'unknown'
    | 'sign_in_required';
  readonly managementUrl?: string;
  readonly managementAccess?: 'unsupported_credential';
};

export async function accountGrant(owner: string, signal: AbortSignal): Promise<AccountGrant> {
  const response = await fetch('/auth/service-authorization', {
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
  });
  if (response.status === 401) return { status: 'sign_in_required' };
  if (!response.ok) return { status: 'unknown' };
  const body = await response.json();
  if (body?.owner !== owner) return { status: 'unknown' };
  const status =
    body.status === 'authorized' ||
    body.status === 'partial' ||
    body.status === 'expired' ||
    body.status === 'not_granted' ||
    body.status === 'revoked'
      ? body.status
      : 'unknown';
  const managementUrl =
    typeof body.management_url === 'string' ? connectionUrl(body.management_url) : undefined;
  return {
    status,
    managementUrl,
    ...(body.management_access === 'unsupported_credential'
      ? { managementAccess: 'unsupported_credential' as const }
      : {}),
  };
}

export async function startServiceAuthorization(
  target: ConnectionTarget,
  owner: string,
  returnTo: string,
  signal: AbortSignal,
  purpose?: 'skill_catalog',
): Promise<string> {
  const response = await fetch('/auth/service-authorization/start', {
    method: 'POST',
    credentials: 'same-origin',
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ target, return_to: returnTo, purpose }),
    signal,
  });
  if (response.status === 401) return new URL(signInPath(returnTo), window.location.origin).href;
  if (response.ok) {
    const body = await response.json();
    if (body?.owner === owner && typeof body.hosted_url === 'string') {
      const destination = connectionUrl(body.hosted_url);
      if (destination !== undefined) return destination;
    }
  }
  throw new Error('Service authorization could not start. Your draft is still here.');
}

export function navigateToServiceAuthorization(popup: Window, destination: string): void {
  popup.location.replace(destination);
}
