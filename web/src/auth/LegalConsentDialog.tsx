import { useEffect, useState } from 'react';
import { ApiError } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import type { LegalTerms } from './types';

type LegalConsentClient = NonNullable<CmaWebClient['legalConsent']>;

/** Refusals after which no consent can or need be recorded for this session. */
const NOT_REQUIRED = new Set([
  'legal_terms_not_configured',
  'legal_consent_requires_owner_session',
  'account_deletion_in_progress',
]);

const ACCEPT_REFUSALS: Readonly<Record<string, string>> = {
  legal_terms_version_mismatch:
    'These terms have been updated. Reload the page to review the current version.',
  legal_consent_sign_in_required:
    'Your sign-in could not be confirmed. Sign out and sign in again to accept the terms.',
};

type Phase =
  | { readonly kind: 'checking' }
  | { readonly kind: 'required'; readonly accepting: boolean; readonly error?: string }
  | { readonly kind: 'unavailable' }
  | { readonly kind: 'done' };

function notRequired(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    (error.status === 401 || (error.code !== undefined && NOT_REQUIRED.has(error.code)))
  );
}

/**
 * Ask a signed-in owner to accept the published terms before continuing.
 *
 * The dialog cannot be dismissed while acceptance is outstanding: the owner
 * either accepts this exact version or signs out. Nothing is shown while the
 * record is being checked, once it is accepted, or when this deployment or
 * this kind of session records no consent.
 */
export function LegalConsentDialog({
  terms,
  client,
  onSignOut,
}: {
  readonly terms: LegalTerms;
  readonly client: LegalConsentClient;
  readonly onSignOut: () => void;
}) {
  const [phase, setPhase] = useState<Phase>({ kind: 'checking' });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    client.status({ signal: controller.signal }).then(
      (status) => {
        if (controller.signal.aborted) return;
        setPhase(
          status.version === terms.version && status.accepted_at !== null
            ? { kind: 'done' }
            : { kind: 'required', accepting: false },
        );
      },
      (error: unknown) => {
        if (controller.signal.aborted) return;
        setPhase(notRequired(error) ? { kind: 'done' } : { kind: 'unavailable' });
      },
    );
    return () => controller.abort();
  }, [client, terms.version, attempt]);

  if (phase.kind === 'checking' || phase.kind === 'done') return null;
  const accept = () => {
    setPhase({ kind: 'required', accepting: true });
    client.accept({ version: terms.version }).then(
      () => setPhase({ kind: 'done' }),
      (error: unknown) => {
        if (notRequired(error)) return setPhase({ kind: 'done' });
        setPhase({
          kind: 'required',
          accepting: false,
          error:
            (error instanceof ApiError && error.code !== undefined
              ? ACCEPT_REFUSALS[error.code]
              : undefined) ?? 'Your acceptance could not be recorded. Try again.',
        });
      },
    );
  };
  const busy = phase.kind === 'required' && phase.accepting;
  return (
    <Modal
      open
      dismissible={false}
      onClose={() => {}}
      title="Review the terms"
      description={`Before you continue, review and accept version ${terms.version} of the Terms of Service and Privacy Notice.`}
      footer={
        <>
          <Button variant="outline" onClick={onSignOut} disabled={busy}>
            Sign out
          </Button>
          {phase.kind === 'unavailable' ? (
            <Button variant="primary" onClick={() => setAttempt((value) => value + 1)}>
              Try again
            </Button>
          ) : (
            <Button variant="primary" onClick={accept} disabled={busy}>
              {busy ? 'Accepting…' : 'Accept and continue'}
            </Button>
          )}
        </>
      }
    >
      <ul>
        <li>
          <a href={terms.termsUrl} target="_blank" rel="noopener noreferrer">
            Terms of Service
          </a>
        </li>
        <li>
          <a href={terms.privacyUrl} target="_blank" rel="noopener noreferrer">
            Privacy Notice
          </a>
        </li>
      </ul>
      {phase.kind === 'unavailable' ? (
        <p role="alert">We could not check whether you have accepted these terms.</p>
      ) : phase.error === undefined ? null : (
        <p role="alert">{phase.error}</p>
      )}
    </Modal>
  );
}
