import { useEffect, useRef, useState } from 'react';
import { signInPath } from '../auth/nyxid';
import {
  navigateToServiceAuthorization,
  startServiceAuthorization,
} from '../auth/serviceAuthorization';
import { routePath } from '../routing/routes';
import { Button } from '../ui/Button';

export function SkillCatalogAuthorization({
  agentId,
  owner,
}: {
  readonly agentId: string;
  readonly owner: string;
}) {
  const pending = useRef<AbortController | undefined>(undefined);
  const [starting, setStarting] = useState(false);
  const [failed, setFailed] = useState(false);
  const returnTo = routePath({ kind: 'agent', agentId });
  useEffect(() => () => pending.current?.abort(), []);

  async function authorize() {
    if (pending.current) return;
    const request = new AbortController();
    pending.current = request;
    setStarting(true);
    setFailed(false);
    try {
      const destination = await startServiceAuthorization(
        { type: 'agent', id: agentId },
        owner,
        returnTo,
        request.signal,
        'skill_catalog',
      );
      if (request.signal.aborted) return;
      navigateToServiceAuthorization(window, destination);
    } catch {
      if (request.signal.aborted) return;
      pending.current = undefined;
      setStarting(false);
      setFailed(true);
    }
  }

  return (
    <>
      {failed ? (
        <p>
          Ornn authorization could not start. Try again or{' '}
          <a href={signInPath(returnTo)}>sign in again</a>.
        </p>
      ) : null}
      <Button variant="primary" disabled={starting} onClick={() => void authorize()}>
        {starting ? 'Opening authorization…' : 'Authorize Ornn access'}
      </Button>
    </>
  );
}
