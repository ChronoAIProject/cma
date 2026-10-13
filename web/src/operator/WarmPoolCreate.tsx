import { useRef, useState } from 'react';
import { ApiError, type ConfigureWarmPoolBody, type WarmPoolView } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { validPoolNumber, WarmPoolNumber } from './WarmPoolFields';

export function WarmPoolCreate({
  client,
  image,
  cpuMillis,
  memoryMib,
  onChanged,
}: {
  readonly client: OperatorClient;
  readonly image: string;
  readonly cpuMillis: number;
  readonly memoryMib: number;
  readonly onChanged: (pool: WarmPoolView) => void;
}) {
  const [cpu, setCpu] = useState(String(cpuMillis));
  const [memory, setMemory] = useState(String(memoryMib));
  const [target, setTarget] = useState('2');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<OperatorFailure>();
  const [saved, setSaved] = useState(false);
  const [pending, setPending] = useState<{
    body: ConfigureWarmPoolBody;
    key: string;
    image: string;
  }>();
  const submitting = useRef(false);
  const valid =
    validPoolNumber(cpu, 1, 1_048_576) &&
    validPoolNumber(memory, 1, 1_048_576) &&
    validPoolNumber(target);
  async function save() {
    if (submitting.current || (!pending && (!valid || !image))) return;
    const request = pending ?? {
      body: {
        cpu_millis: Number(cpu),
        memory_mib: Number(memory),
        target_replicas: Number(target),
      },
      key: crypto.randomUUID(),
      image,
    };
    submitting.current = true;
    setPending(request);
    setBusy(true);
    setFailure(undefined);
    setSaved(false);
    try {
      const pool = await client.admin.configureWarmPool(request.body, {
        idempotencyKey: request.key,
      });
      setPending(undefined);
      setSaved(true);
      onChanged(pool);
    } catch (error) {
      if (
        error instanceof ApiError &&
        error.status >= 400 &&
        error.status < 500 &&
        error.status !== 408
      )
        setPending(undefined);
      setFailure(operatorFailure(error));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  }
  return (
    <details className="warm-pool-create">
      <summary>Configure a warm pool</summary>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <p className="operator-notice">
          Keep ready Sandboxes for new Agents using the current system runtime image. CPU and memory
          must match the Agent’s requested resources.
        </p>
        <div className="warm-pool-image">
          <span>System runtime image</span>
          <code>{pending?.image || image || 'No runtime image configured'}</code>
        </div>
        <div className="operator-settings-grid">
          <WarmPoolNumber
            label="CPU (millicores)"
            value={cpu}
            onChange={setCpu}
            minimum={1}
            maximum={1_048_576}
            disabled={busy || pending !== undefined}
          />
          <WarmPoolNumber
            label="Memory (MiB)"
            value={memory}
            onChange={setMemory}
            minimum={1}
            maximum={1_048_576}
            disabled={busy || pending !== undefined}
          />
          <WarmPoolNumber
            label="Ready Sandbox target"
            value={target}
            onChange={setTarget}
            disabled={busy || pending !== undefined}
          />
        </div>
        <FailureNotice failure={failure} />
        {pending && !busy ? (
          <p role="status">
            The result is unconfirmed. Retry keeps the original resources and request identity.
          </p>
        ) : null}
        <div className="operator-actions">
          <Button
            type="submit"
            variant="primary"
            disabled={busy || (!pending && (!valid || !image))}
          >
            {busy ? 'Saving target…' : pending ? 'Retry same request' : 'Save pool target'}
          </Button>
          {saved ? (
            <p role="status">
              Pool target saved. Ready capacity updates after the controller applies it.
            </p>
          ) : null}
        </div>
      </form>
    </details>
  );
}
