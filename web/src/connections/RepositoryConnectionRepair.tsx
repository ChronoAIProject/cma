import { useEffect, useState } from 'react';
import type {
  RepositoryAccessRefusal,
  RepositoryScopeRepair,
} from '../api-client/agents/lifecycle';
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
  readonly account?: {
    readonly login: string;
    readonly observed_scopes: readonly string[] | null;
  };
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
  const account = value.account;
  const availableAccount =
    record(account) &&
    typeof account.login === 'string' &&
    /^[A-Za-z0-9_-]{1,100}$/.test(account.login) &&
    (account.observed_scopes === null ||
      (Array.isArray(account.observed_scopes) &&
        account.observed_scopes.length <= 100 &&
        account.observed_scopes.every((scope) => typeof scope === 'string')));
  return {
    id: value.id,
    label: value.label,
    granted_scopes: value.granted_scopes as readonly string[] | null,
    manage_url: href,
    account: availableAccount
      ? {
          login: account.login as string,
          observed_scopes: account.observed_scopes as string[] | null,
        }
      : undefined,
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
  readonly repository_access?: RepositoryAccessRefusal;
  readonly repository_scope_repair?: RepositoryScopeRepair;
  readonly pending: boolean;
  readonly onRecheck: () => void;
}

const refusalText: Record<RepositoryAccessRefusal, string> = {
  verification_unavailable: 'GitHub repository access could not be verified',
  insufficient_scope: 'The selected GitHub connection needs additional permissions',
  missing_connection: 'Connect GitHub before preparing this repository',
  repository_unavailable:
    'The selected GitHub connection cannot access this repository for the requested operation',
  reference_unavailable: 'The selected GitHub connection cannot read this ref',
  runtime_unsupported: 'This runtime does not support verified GitHub repository access',
};

export function RepositoryConnectionRepair({
  target,
  owner,
  repository_access: reason,
  repository_scope_repair: repair,
  pending,
  onRecheck,
}: Props) {
  const candidate = repair && connectionUrl(repair.manage_url);
  const exact =
    candidate &&
    /^[a-zA-Z0-9_-]{1,128}$/.test(repair.connection_id) &&
    candidate === new URL(`/keys/${repair.connection_id}`, candidate).href;
  return (
    <Alert
      tone="warning"
      title={refusalText[reason ?? 'verification_unavailable']}
      actions={
        <Button size="sm" disabled={pending} onClick={onRecheck}>
          Recheck repository access
        </Button>
      }
    >
      <p>Your request is saved.</p>
      {reason === 'insufficient_scope' && exact ? (
        <p>
          Grant <code>{repair.required_scope}</code> under{' '}
          <a href={candidate} target="_blank" rel="noreferrer">
            Manage permissions in NyxID
          </a>
          .
        </p>
      ) : null}
      {reason !== 'runtime_unsupported' && !exact ? (
        <GitHubConnectionDetails owner={owner} target={target} />
      ) : null}
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
                {row.account ? <p>GitHub account @{row.account.login}</p> : null}
                <p>
                  {row.account?.observed_scopes
                    ? `Live scopes — ${row.account.observed_scopes.join(', ') || 'None'}`
                    : row.granted_scopes === null
                      ? 'Granted scopes unavailable'
                      : `Reported scopes: ${row.granted_scopes.join(', ') || 'None'}`}
                </p>
                {selection &&
                row.granted_scopes?.includes('repo') &&
                (row.account?.observed_scopes?.includes('repo') ?? true) ? (
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
