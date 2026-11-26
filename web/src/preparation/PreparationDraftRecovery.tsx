import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { AgentDraft } from '../agent/draft';
import type { AgentDetail } from '../api/types';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { retainPreparationDraftOwner } from './preparationDraftIdentity';
import {
  preparationDraftsForAgent,
  preparationDraftReadError,
  type PreparationDraftRecord,
} from './preparationDraftStorage';
import { transferPreparationDraft } from './preparationDraftHandoff';

function restore(owner: string, agentId: string) {
  try {
    retainPreparationDraftOwner(owner);
    return { records: preparationDraftsForAgent(owner, agentId).filter((row) => row.text) };
  } catch {
    return { records: [] as PreparationDraftRecord[], error: preparationDraftReadError };
  }
}

function matchesLineage(record: PreparationDraftRecord, detail: AgentDetail) {
  const binding = record.binding;
  return (
    binding?.agentId === detail.id &&
    binding.worktreeId === detail.worktree.id &&
    binding.workspaceId === detail.workspace.id &&
    binding.sandboxId === detail.sandbox.id &&
    detail.worktree.agentId === detail.id &&
    detail.worktree.workspaceId === detail.workspace.id &&
    detail.worktree.sandboxId === detail.sandbox.id
  );
}

/** The original request may be cleared; bound drafts remain independently discoverable. */
export function PreparationDraftRecovery({
  client,
  viewerId,
  detail,
  draft,
  authorized,
  revision,
  epoch,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly detail: AgentDetail;
  readonly draft?: AgentDraft;
  readonly authorized: boolean;
  readonly revision?: string;
  readonly epoch?: number;
}) {
  const identity = useMemo(
    () => ({ client, viewerId, id: detail.id, revision, epoch }),
    [client, viewerId, detail.id, revision, epoch],
  );
  const [state, setState] = useState(() => ({ identity, ...restore(viewerId, detail.id) }));
  if (state.identity !== identity) setState({ identity, ...restore(viewerId, detail.id) });
  const authority = useMemo(
    () => ({ identity, authorized, detail, draft }),
    [identity, authorized, detail, draft],
  );
  const current = useRef<typeof authority | undefined>(undefined);
  useLayoutEffect(() => {
    current.current = authority;
    return () => {
      current.current = undefined;
    };
  }, [authority]);
  const attempted = useRef<string | undefined>(undefined);
  const transfer = (record: PreparationDraftRecord) => {
    const binding = record.binding;
    const permitted = () =>
      current.current === authority && authorized && matchesLineage(record, detail);
    if (!draft || !binding || !permitted()) return;
    if (transferPreparationDraft(record, viewerId, binding, draft, permitted) === 'transferred')
      setState((previous) =>
        previous.identity !== identity
          ? previous
          : {
              ...previous,
              records: previous.records.filter((row) => row.revision !== record.revision),
            },
      );
  };
  // Only an empty destination can accept one source automatically. Conflicts stay explicit.
  useEffect(() => {
    const record =
      state.identity === identity && state.records.length === 1 ? state.records[0] : undefined;
    if (!record || !authorized || !draft || attempted.current === record.revision) return;
    attempted.current = record.revision;
    if (draft.content || draft.attachments.length || draft.error || draft.recoveryBlocked) return;
    transfer(record);
  });
  if (state.identity !== identity || !authorized || (!state.records.length && !state.error))
    return null;
  return (
    <section className="preparation-draft" aria-label="Saved preparation drafts">
      {state.error ? (
        <>
          <p role="alert">{state.error}</p>
          <Button
            onClick={() => {
              if (current.current === authority && authorized)
                setState({ identity, ...restore(viewerId, detail.id) });
            }}
          >
            Retry reading saved drafts
          </Button>
        </>
      ) : null}
      {state.records.some((record) => !matchesLineage(record, detail)) ? (
        <p role="status">A saved draft belongs to a different environment. It is kept unchanged.</p>
      ) : null}
      {state.records
        .filter((record) => matchesLineage(record, detail))
        .map((record) => (
          <div key={record.revision}>
            <label>
              Saved follow-up draft
              <textarea readOnly value={record.text} />
            </label>
            <p>
              Your current message is unchanged. Keep or copy this draft, or move it to an empty
              message.
            </p>
            <Button
              onClick={() => transfer(record)}
              disabled={
                !draft?.importText ||
                Boolean(
                  draft.content || draft.attachments.length || draft.error || draft.recoveryBlocked,
                )
              }
            >
              Move draft to message
            </Button>
          </div>
        ))}
    </section>
  );
}
