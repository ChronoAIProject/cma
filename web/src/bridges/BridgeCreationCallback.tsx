import { useState } from 'react';
import { Button } from '../ui/Button';
import type { useBridgeCreation } from './useBridgeCreation';

export function BridgeCreationCallback({
  creation,
}: {
  readonly creation: ReturnType<typeof useBridgeCreation>;
}) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  const state = creation.preview.state;
  return (
    <section className="bridges__fields" aria-labelledby="bridge-callback-title">
      <h2 id="bridge-callback-title">Connect your NyxID key</h2>
      <p>
        Set this callback on a dedicated Agent key in NyxID, then add that key to your bot’s route.
        Return here and refresh the connection choices.
      </p>
      {state.status === 'loading' ? <p role="status">Preparing your callback address</p> : null}
      {state.status === 'error' ? (
        <p role="alert">
          {state.message} <Button onClick={creation.preview.reload}>Check callback again</Button>
        </p>
      ) : null}
      {state.status === 'ready' ? (
        <>
          <code className="bridges__callback">{state.value.callback_url}</code>
          <Button
            onClick={async () => {
              try {
                await navigator.clipboard.writeText(state.value.callback_url);
                setCopied(true);
                setCopyError(false);
              } catch {
                setCopyError(true);
              }
            }}
          >
            {copied ? 'Callback copied' : 'Copy callback'}
          </Button>
          {copyError ? <p role="status">Select and copy the callback address above.</p> : null}
          <p>This address belongs to this setup. Your bridge starts disabled when you save it.</p>
        </>
      ) : null}
    </section>
  );
}
