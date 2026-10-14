import { Picker } from '../ui/Picker';
import { Presence } from '../ui/Presence';
import { useCallback, useRef, useState } from 'react';
import type { AdminProfileSummary, ProfileKind, ProfileVisibility } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { IconArchiveOutline20, IconSearchOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import {
  FailureNotice,
  RecordControls,
  Records,
  Timestamp,
  stateWord,
  useRecords,
} from './records';
import { operatorFailure, type OperatorFailure } from './failure';
import { ReferenceLink } from '../profiles/ReferenceLink';

const defaultIds = new Set(['sbp_default', 'wkp_default', 'agp_default']);

export function Profiles({
  client,
  canControl,
}: {
  readonly client: OperatorClient;
  readonly canControl: boolean;
}) {
  const [kind, setKind] = useState<ProfileKind | ''>('');
  const [visibility, setVisibility] = useState<ProfileVisibility | ''>('');
  const [owner, setOwner] = useState('');
  const [filters, setFilters] = useState({ kind, visibility, owner });
  return (
    <section aria-label="Profile inventory">
      <h2>Profiles</h2>
      <form
        className="operator-filters"
        onSubmit={(event) => {
          event.preventDefault();
          setFilters({ kind, visibility, owner });
        }}
      >
        <label className="operator-field">
          <span>Kind</span>
          <Picker
            label="Profile kind"
            value={kind}
            onChange={(choice) => setKind(choice as ProfileKind | '')}
            options={[
              { value: '', label: 'All kinds' },
              { value: 'sandbox', label: 'Sandbox' },
              { value: 'workspace', label: 'Workspace' },
              { value: 'agent', label: 'Agent' },
            ]}
          />
        </label>
        <label className="operator-field">
          <span>Visibility</span>
          <Picker
            label="Profile visibility"
            value={visibility}
            onChange={(choice) => setVisibility(choice as ProfileVisibility | '')}
            options={[
              { value: '', label: 'All visibility' },
              { value: 'public', label: 'Public' },
              { value: 'private', label: 'Private' },
            ]}
          />
        </label>
        <label className="operator-field">
          <span>Owner</span>
          <Input
            aria-label="Profile owner"
            value={owner}
            maxLength={255}
            onChange={(event) => setOwner(event.target.value)}
          />
        </label>
        <Button type="submit" icon={<IconSearchOutline16 />}>
          Apply filters
        </Button>
      </form>
      <ProfileRecords
        key={JSON.stringify(filters)}
        client={client}
        canControl={canControl}
        filters={filters}
      />
    </section>
  );
}

function ProfileRecords({
  client,
  canControl,
  filters,
}: {
  readonly client: OperatorClient;
  readonly canControl: boolean;
  readonly filters: {
    readonly kind: ProfileKind | '';
    readonly visibility: ProfileVisibility | '';
    readonly owner: string;
  };
}) {
  const load = useCallback(
    (cursor: string | undefined, signal: AbortSignal) =>
      client.admin.listProfiles({
        cursor,
        signal,
        limit: 50,
        kind: filters.kind || undefined,
        visibility: filters.visibility || undefined,
        owner: filters.owner || undefined,
      }),
    [client, filters],
  );
  const records = useRecords(load);
  const [target, setTarget] = useState<AdminProfileSummary>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<OperatorFailure>();
  const [uncertain, setUncertain] = useState(false);
  const submitting = useRef(false);
  const retire = async () => {
    if (target === undefined || !canControl || submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setFailure(undefined);
    try {
      if (uncertain) {
        let cursor: string | undefined;
        const seen = new Set<string>();
        do {
          const page = await client.admin.listProfiles({
            kind: target.kind,
            owner: target.owner,
            limit: 200,
            cursor,
          });
          const current = page.items.find((row) => row.id === target.id);
          if (current !== undefined) {
            setTarget(current.retired_at === null ? current : undefined);
            setUncertain(false);
            records.refresh();
            return;
          }
          if (page.next === null || seen.has(page.next)) break;
          cursor = page.next;
          seen.add(cursor);
        } while (cursor !== undefined);
        records.refresh();
        setFailure({
          message: 'This profile was not found in the current inventory.',
          conflict: false,
          uncertain: true,
        });
      } else {
        await client.admin.retireProfile(target.kind, target.id);
        setTarget(undefined);
        records.refresh();
      }
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      setUncertain((previous) => previous || problem.uncertain);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  return (
    <>
      <RecordControls {...records} />
      <FailureNotice failure={records.error} />
      <Records loading={records.loading} empty={records.page?.items.length === 0}>
        {records.page?.items.map((row) => {
          const reason = !canControl
            ? 'Cross-owner control is required.'
            : defaultIds.has(row.id)
              ? 'Deployment-owned default.'
              : row.retired_at !== null
                ? 'Already retired.'
                : undefined;
          return (
            <article className="operator-record" key={`${row.kind}:${row.id}`}>
              <Timestamp value={row.updated_at} />
              <div>
                <p>
                  <ReferenceLink kind={row.kind} id={row.id} name={row.name} />{' '}
                  {row.retired_at === null ? null : <span>Retired</span>}
                </p>
                <p className="operator-meta">
                  <span>{stateWord(row.kind)}</span>
                  <span>{row.visibility}</span>
                  <span>Owner {row.owner_name?.trim() || 'Name unavailable'}</span>
                </p>
              </div>
              <div>
                <Button
                  size="sm"
                  variant="danger"
                  icon={<IconArchiveOutline20 />}
                  disabled={reason !== undefined}
                  onClick={() => {
                    setFailure(undefined);
                    setTarget(row);
                  }}
                >
                  Retire
                </Button>
                {reason === undefined ? null : <p className="operator-notice">{reason}</p>}
              </div>
            </article>
          );
        })}
      </Records>
      <Presence>
        {target === undefined || !canControl ? null : (
          <RiskConfirmation
            open
            title={`Retire ${target.name}`}
            description={`Retire ${target.kind} profile ${target.name}. New creation from this profile will be refused; existing resources and published history remain. The operator action is audited.`}
            acknowledgeLabel="I understand this profile will no longer be available for new resources."
            confirmLabel={uncertain ? 'Check current state' : 'Retire profile'}
            busy={busy}
            dismissible={!busy}
            failure={
              failure === undefined
                ? undefined
                : `${failure.message}${failure.code === undefined ? '' : ` (${failure.code})`}`
            }
            status={
              uncertain
                ? 'The previous outcome is unknown. Only a read will be sent next.'
                : busy
                  ? 'Submitting'
                  : undefined
            }
            onCancel={() => {
              setTarget(undefined);
              if (uncertain) records.refresh();
            }}
            onConfirm={() => void retire()}
          />
        )}
      </Presence>
    </>
  );
}
