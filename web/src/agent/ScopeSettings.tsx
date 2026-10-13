import { useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentDetail } from '../api/types';
import { Button } from '../ui/Button';
import { ScopePicker } from './ScopePicker';
import { REQUIRED_AGENT_SCOPES } from './SettingsFields';

export function ScopeSettings({ detail }: { readonly detail: AgentDetail }) {
  const client = useCmaClient();
  const [granted, setGranted] = useState([...new Set([...REQUIRED_AGENT_SCOPES, ...detail.reach])]);
  const [selected, setSelected] = useState<readonly string[]>(granted);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const [confirmed, setConfirmed] = useState(false);
  const [intent, setIntent] = useState<{ key: string; services: readonly string[] } | undefined>(
    undefined,
  );
  const paused = detail.sandbox.status === 'paused';
  const save = async () => {
    if (pending || !paused || !client.grantSandboxServiceAccess) return;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      services: selected.filter((service) => !granted.includes(service)),
    };
    if (!retained.services.length) return;
    setIntent(retained);
    setPending(true);
    setError(undefined);
    try {
      await client.grantSandboxServiceAccess(detail.sandbox.id, retained.services, {
        idempotencyKey: retained.key,
      });
      setGranted((current) => [...new Set([...current, ...retained.services])]);
      setIntent(undefined);
      setConfirmed(true);
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : 'The service grant could not be confirmed. Retry the same request.',
      );
    } finally {
      setPending(false);
    }
  };
  return (
    <div className="agent-settings">
      <ScopePicker
        value={selected}
        onChange={(value) => {
          setSelected(value);
          setConfirmed(false);
        }}
        fixed={granted}
        disabled={pending || intent !== undefined || !client.grantSandboxServiceAccess}
      />
      {!paused ? (
        <p>
          Pause this Sandbox before adding services. The updated key takes effect when the Sandbox
          resumes.
        </p>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
      {confirmed ? (
        <p role="status">Services granted. Resume the Sandbox to apply them to its Agent keys.</p>
      ) : null}
      {client.grantSandboxServiceAccess ? (
        <Button
          disabled={pending || !paused || !selected.some((service) => !granted.includes(service))}
          onClick={() => void save()}
        >
          {pending ? 'Saving scope…' : intent ? 'Retry scope grant' : 'Add services'}
        </Button>
      ) : null}
    </div>
  );
}
