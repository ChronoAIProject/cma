import { useEffect, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import {
  connectionUrl,
  readRepositoryConnections,
  type ConnectionReport,
  type ConnectionTarget,
} from './client';

export function needsManualGitHubRepair(report: ConnectionReport): boolean {
  return report.services.some(
    (row) =>
      row.service === 'api-github' && row.connection_mode === 'manual' && row.state !== 'satisfied',
  );
}

interface Choice {
  readonly id: string;
  readonly label: string;
  readonly granted_scopes: readonly string[] | null;
  readonly manage_url: string;
}

interface Inventory {
  readonly manage_url: string;
  readonly connections?: readonly Choice[];
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function choice(value: unknown, manage: URL): Choice | undefined {
  if (
    !record(value) ||
    typeof value.id !== 'string' ||
    !/^[a-zA-Z0-9_-]{1,128}$/.test(value.id) ||
    typeof value.label !== 'string' ||
    !value.label ||
    typeof value.manage_url !== 'string' ||
    (value.granted_scopes !== null &&
      (!Array.isArray(value.granted_scopes) ||
        !value.granted_scopes.every((scope) => typeof scope === 'string')))
  )
    return undefined;
  const href = connectionUrl(value.manage_url);
  if (href !== new URL(`/keys/${value.id}`, manage).href) return undefined;
  return {
    id: value.id,
    label: value.label,
    granted_scopes: value.granted_scopes as readonly string[] | null,
    manage_url: href,
  };
}

function inventory(value: unknown, owner: string): Inventory | undefined {
  if (!record(value) || value.owner !== owner || !record(value.github)) return undefined;
  const { github } = value;
  const url = typeof github.manage_url === 'string' ? connectionUrl(github.manage_url) : undefined;
  if (!url) return undefined;
  const manage = new URL(url);
  if (url !== new URL('/keys', manage).href) return undefined;
  const entries: unknown = github.connections;
  if (!Array.isArray(entries) || entries.length > 12) return { manage_url: url };
  const rows = entries.map((row) => choice(row, manage));
  if (
    rows.some((row) => row === undefined) ||
    new Set(rows.map((row) => row?.id)).size !== rows.length
  )
    return { manage_url: url };
  return { manage_url: url, connections: rows.filter((row) => row !== undefined) };
}

interface Props {
  readonly target: ConnectionTarget;
  readonly owner: string;
  readonly pending: boolean;
  readonly onRecheck: () => void;
}

export function RepositoryConnectionRepair({ target, owner, pending, onRecheck }: Props) {
  return (
    <Alert
      tone="warning"
      title="GitHub repository access could not be verified"
      actions={
        <Button size="sm" disabled={pending} onClick={onRecheck}>
          Recheck repository access
        </Button>
      }
    >
      <p>
        Your request is saved. The current runtime pauses setup for public and private repositories
        until CMA can verify the connection the Sandbox will use. A scope change in NyxID alone
        cannot verify that connection.
      </p>
      <GitHubConnectionDetails owner={owner} target={target} />
    </Alert>
  );
}

type ConnectionDetailsProps = Pick<Props, 'target' | 'owner'> & {
  readonly selection?: {
    readonly id?: string;
    readonly disabled: boolean;
    readonly onSelect: (id: string) => void;
  };
};

export function GitHubConnectionDetails(props: ConnectionDetailsProps) {
  return <ConnectionDetails key={JSON.stringify([props.owner, props.target])} {...props} />;
}

function ConnectionDetails({ target, owner, selection }: ConnectionDetailsProps) {
  const [result, setResult] = useState<{ readonly inventory?: Inventory }>();
  useEffect(() => {
    const controller = new AbortController();
    void readRepositoryConnections(target, controller.signal).then(
      (value) => {
        if (!controller.signal.aborted) setResult({ inventory: inventory(value, owner) });
      },
      () => {
        if (!controller.signal.aborted) setResult({});
      },
    );
    return () => controller.abort();
  }, [target, owner]);
  return (
    <>
      {result === undefined ? <p role="status">Looking up GitHub connections…</p> : null}
      {result?.inventory ? (
        <p>
          <a href={result.inventory.manage_url} target="_blank" rel="noreferrer">
            Manage permissions in NyxID
          </a>
        </p>
      ) : result !== undefined ? (
        <p>
          In NyxID, open External Services and choose Manage permissions for your GitHub connection.
        </p>
      ) : null}
      {result?.inventory?.connections?.length ? (
        <>
          <p>
            {selection
              ? 'Choose the connection CMA should verify for this Sandbox.'
              : 'NyxID reports these connections. CMA has not verified the Agent’s connection.'}
          </p>
          <ul>
            {result.inventory.connections.map((row) => (
              <li key={row.id}>
                <a href={row.manage_url} target="_blank" rel="noreferrer">
                  {row.label} ({row.id})
                </a>
                <p>
                  {row.granted_scopes === null
                    ? 'Granted scopes unavailable'
                    : `Reported scopes: ${row.granted_scopes.join(', ') || 'None'}`}
                </p>
                {selection && row.granted_scopes?.includes('repo') ? (
                  <Button
                    size="sm"
                    aria-pressed={selection.id === row.id}
                    disabled={selection.disabled}
                    onClick={() => selection.onSelect(row.id)}
                  >
                    Select {row.label} ({row.id})
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </>
  );
}
