import { Presence } from '../ui/Presence';
import { useState } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { InlineConfirm } from '../ui/InlineConfirm';
import { IconShareOutline16, IconArchiveOutline20 } from '../ui/icons';
import type { ProfileSummary } from './client';
import type { ProfileCommand } from './editor';
import type { ProfileFailure } from './failure';

export function EditorActions({
  profile,
  dirty,
  locked,
  pending,
  readingCurrent,
  failure,
  success,
  errorCount,
  onPublish,
  onRetire,
  onRetry,
  onLoadCurrent,
}: {
  readonly profile?: ProfileSummary;
  readonly dirty: boolean;
  readonly locked: boolean;
  readonly pending?: ProfileCommand;
  readonly readingCurrent: boolean;
  readonly failure?: ProfileFailure;
  readonly success?: string;
  readonly errorCount: number;
  readonly onPublish: () => void;
  readonly onRetire: () => void;
  readonly onRetry: () => Promise<void>;
  readonly onLoadCurrent: () => Promise<void>;
}) {
  const [publishOpen, setPublishOpen] = useState(false);
  const [retireOpen, setRetireOpen] = useState(false);
  const saved = profile !== undefined && !dirty;
  const nextRevision = (profile?.latestRevision?.revision ?? 0) + 1;
  return (
    <>
      <Presence>
        {errorCount > 0 ? (
          <p className="profile-editor__failure" role="alert">
            Review {errorCount} {errorCount === 1 ? 'field' : 'fields'} before saving.
          </p>
        ) : null}
      </Presence>
      <Presence>
        {failure === undefined ? null : (
          <div className="profile-editor__notice" role={failure.conflict ? 'status' : 'alert'}>
            <p>{failure.message}</p>
            {failure.conflict ? (
              <div className="profile-editor__actions">
                <Button
                  type="button"
                  disabled={pending !== undefined || readingCurrent}
                  onClick={() => void onLoadCurrent()}
                >
                  Load current
                </Button>
                <Button
                  type="button"
                  disabled={pending !== undefined || readingCurrent}
                  onClick={() => void onRetry()}
                >
                  Retry save
                </Button>
              </div>
            ) : failure.uncertain ? (
              <Button type="button" disabled={pending !== undefined} onClick={() => void onRetry()}>
                Retry
              </Button>
            ) : null}
          </div>
        )}
      </Presence>
      <Presence>{success === undefined ? null : <p role="status">{success}</p>}</Presence>
      <footer className="profile-editor__actions">
        <Button type="submit" variant="primary" disabled={locked || failure?.conflict || saved}>
          {pending === 'save' ? 'Saving' : 'Save draft'}
        </Button>
        <Button
          type="button"
          icon={<IconShareOutline16 />}
          disabled={!saved || locked || failure?.conflict}
          onClick={() => setPublishOpen(true)}
        >
          Publish revision
        </Button>
        {profile === undefined || profile.retiredAt !== undefined || retireOpen ? null : (
          <Button
            type="button"
            icon={<IconArchiveOutline20 />}
            disabled={!saved || locked || failure?.conflict}
            onClick={() => setRetireOpen(true)}
          >
            Retire
          </Button>
        )}
      </footer>
      {profile !== undefined && dirty ? (
        <p className="profile-field__help">
          Save your draft before publishing or changing sharing.
        </p>
      ) : null}
      {retireOpen ? (
        <InlineConfirm
          question="Retire this profile? Existing agents keep their settings; new agents cannot use it."
          confirmLabel="Retire"
          busy={pending === 'retire'}
          onCancel={() => setRetireOpen(false)}
          onConfirm={() => {
            setRetireOpen(false);
            onRetire();
          }}
        />
      ) : null}
      <Modal
        open={publishOpen}
        onClose={() => setPublishOpen(false)}
        title={`Publish revision ${String(nextRevision)}`}
        description="This creates an immutable version of the saved profile. Existing agents keep their current settings."
        footer={
          <>
            <Button type="button" onClick={() => setPublishOpen(false)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="primary"
              disabled={locked || !saved}
              onClick={() => {
                setPublishOpen(false);
                onPublish();
              }}
            >
              Publish revision {nextRevision}
            </Button>
          </>
        }
      />
    </>
  );
}
