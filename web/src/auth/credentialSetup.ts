import { ApiError, createClient } from '@chronoai/cma-sdk';

export const credentialSetupMessage = 'Sign in to CMA with this same account before trying again.';

export class CredentialAccountError extends Error {}

export async function checkCredentialRecoveryOwner(owner?: string, signal?: AbortSignal) {
  if (owner === undefined) return;
  let session;
  try {
    session = await createClient().auth.readSession({ signal });
  } catch {
    throw new CredentialAccountError('Your CMA account could not be checked. Try again.');
  }
  if (!session.authenticated || session.subject !== owner)
    throw new CredentialAccountError('Sign in with the same CMA account that made this request.');
}

export function needsSandboxCredentialSetup(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 409 &&
    error.code === 'sandbox_credential_setup_required'
  );
}
