import { useEffect, useRef, useState } from 'react';
import { PageHeader } from '../ui/PageHeader';
import type { AgentProfileContent, ProfileEditorClient } from '../profiles/client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { validateFatherDraft, type FatherDraft } from './fatherDraft';
import { fatherProfileDraft, retainFatherReceipt, type FatherReceipt } from './fatherReceipt';
import { problem } from './model';
import '../profiles/Profiles.css';

export function BotFatherProfile({
  client,
  viewer,
  viewerLabel,
  draft,
  receipt,
  onRetained,
}: {
  readonly client: ProfileEditorClient<AgentProfileContent>;
  readonly viewer: string;
  readonly viewerLabel: string;
  readonly draft: FatherDraft;
  readonly receipt?: FatherReceipt;
  readonly onRetained: (value: FatherReceipt) => void;
}) {
  const [name, setName] = useState(receipt?.draft.name ?? draft.name);
  const [instructions, setInstructions] = useState(
    receipt?.draft.instructions ?? draft.instructions,
  );
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();
  const request = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => request.current?.abort(), []);
  const submit = async () => {
    if (request.current !== undefined || !confirmed) return;
    let reviewed: FatherDraft;
    try {
      reviewed = validateFatherDraft(receipt?.draft ?? { v: 1, name, instructions });
    } catch (error) {
      setFailure(error instanceof Error ? error.message : 'Check the name and instructions.');
      return;
    }
    const abort = new AbortController();
    request.current = abort;
    setBusy(true);
    setFailure(undefined);
    const retain = (value: FatherReceipt) => {
      retainFatherReceipt(value);
      onRetained(value);
    };
    try {
      let current: FatherReceipt = receipt ?? {
        version: 1,
        owner: viewer,
        draft: reviewed,
        createKey: crypto.randomUUID(),
        publishKey: crypto.randomUUID(),
      };
      // Retain exact input and both keys before any mutation; reload never auto-submits.
      retain(current);
      if (current.document === undefined) {
        const document = await client.createProfile(fatherProfileDraft(current.draft), {
          idempotencyKey: current.createKey,
          signal: abort.signal,
        });
        if (abort.signal.aborted) return;
        if (
          document.owner !== viewer ||
          !document.id ||
          !Number.isSafeInteger(document.draftVersion) ||
          document.draftVersion < 1 ||
          document.visibility !== 'private'
        )
          throw new Error('The profile response did not match your account.');
        current = {
          ...current,
          document: { id: document.id, draftVersion: document.draftVersion },
        };
        retain(current);
      }
      const basis = current.document;
      if (basis === undefined) return;
      const revision = await client.publishProfile(
        basis.id,
        { draftVersion: basis.draftVersion },
        { idempotencyKey: current.publishKey, signal: abort.signal },
      );
      if (abort.signal.aborted) return;
      if (
        revision.profileId !== basis.id ||
        !Number.isSafeInteger(revision.revision) ||
        revision.revision < 1
      )
        throw new Error('The published profile did not match the reviewed request.');
      retain({
        ...current,
        published: {
          revision: revision.revision,
          digest: revision.digest,
          at: revision.publishedAt,
        },
      });
    } catch (error) {
      if (!abort.signal.aborted) setFailure(problem(error));
    } finally {
      if (!abort.signal.aborted) {
        request.current = undefined;
        setBusy(false);
      }
    }
  };
  const locked = busy || receipt !== undefined;
  return (
    <section aria-labelledby="father-setup-title">
      <div className="bots__header">
        <PageHeader
          breadcrumb={[{ label: 'Event Gateway' }, { label: 'Consumers' }, { label: 'New Bot' }]}
          title="Create your Bot with CMA Bot Father"
          headingId="father-setup-title"
        />
      </div>
      <div className="bots__body">
        <div className="bots__form">
          <p>
            Signed in as <strong>{viewerLabel}</strong>. Your profile and Bot will belong to this
            CMA account. Review the suggested name and instructions before continuing.
          </p>
          <label className="bots__field">
            Bot name
            <Input
              value={name}
              disabled={locked}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <label className="bots__field">
            Agent instructions
            <textarea
              className="profile-textarea"
              rows={8}
              value={instructions}
              disabled={locked}
              onChange={(event) => setInstructions(event.target.value)}
              aria-describedby="father-instructions-help"
            />
          </label>
          <p id="father-instructions-help">
            These instructions were suggested in a setup link. You can edit them before creating
            your private Agent Profile. It uses the default model, medium thinking effort, guarded
            permissions and no additional skills.
          </p>
          <p>
            Next, choose or register your own Telegram bot in NyxID, choose who can message it, and
            review the connection before creating the Bot. Never paste a bot token into chat or
            these instructions.
          </p>
          {receipt === undefined ? null : (
            <p role="status">
              Profile creation is retained for this account. Continue the same request to avoid
              creating another profile. The reviewed name and instructions are locked.
            </p>
          )}
          <label className="bots__check">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={busy}
              onChange={(event) => setConfirmed(event.target.checked)}
            />
            I reviewed these instructions and want to create this profile in my CMA account.
          </label>
          {failure === undefined ? null : <p role="alert">{failure}</p>}
          <div className="bots__actions">
            <Button variant="primary" disabled={busy || !confirmed} onClick={() => void submit()}>
              {busy
                ? 'Preparing your profile'
                : receipt === undefined
                  ? 'Create my agent profile'
                  : 'Recover profile creation'}
            </Button>
          </div>
        </div>
      </div>
    </section>
  );
}
