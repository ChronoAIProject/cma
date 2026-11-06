import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { useAuth } from './context';
import type { AuthState } from './types';

export function IdentityNotice() {
  const { state, outage, retry } = useAuth();
  if (outage === undefined || state.status !== 'authenticated') return null;
  return (
    <Alert
      tone="warning"
      title={outage.message}
      actions={<Button onClick={retry}>Retry now</Button>}
    >
      Your drafts are still here. You can keep editing.
    </Alert>
  );
}

export function IdentityError({
  state,
}: {
  readonly state: Extract<AuthState, { status: 'error' }>;
}) {
  const { retry } = useAuth();
  return (
    <main className="app-error" role="alert">
      <h1 className="app-error__title">
        {state.reason === 'identity_unavailable'
          ? 'The identity service is temporarily unavailable'
          : 'CMA could not verify your sign-in'}
      </h1>
      <p className="app-error__next">
        {state.reason === undefined
          ? 'Try checking your sign-in again.'
          : 'Retrying automatically. You can also try again now.'}
      </p>
      <Button variant="primary" onClick={retry}>
        Retry now
      </Button>
      <p className="app-error__what">{state.message}</p>
    </main>
  );
}
