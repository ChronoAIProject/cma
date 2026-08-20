import { useCallback, useEffect, useState } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { InlineConfirm } from '../ui/InlineConfirm';
import { ProfileEditor } from './ProfileEditor';
import { NOTHING_MANDATED } from './types';
import type {
  EnvironmentProfile,
  EnvironmentProfileStore,
  ProfileDraft,
  ServiceCatalogue,
  SkillCatalogue,
} from './types';
import { draftOf, emptyDraft, validate } from './types';
import type { KernelOffer } from '../sessions/types';
import './EnvironmentProfiles.css';

/**
 * Managing the environment bundles a session can be started with.
 *
 * Reached from the profile menu, per docs/Web_Style.md section 6: an account
 * action that opens a dialog, not navigation. The product remains one page.
 *
 * This surface authors a resource the API owns. It renders what the server
 * says cannot be removed and does not try to enforce it — a client-side rule
 * would drift, and the server merges the mandatory entries back in on every
 * write regardless of what is sent.
 */

interface EnvironmentProfilesProps {
  readonly store: EnvironmentProfileStore;
  readonly onClose: () => void;
  /**
   * The runtimes this deployment can run, and what each can be.
   *
   * Passed through to the editor rather than fetched here: the console already
   * asks once, and a second asker is a second answer that can disagree with
   * the first while somebody is choosing between them.
   */
  readonly kernels?: readonly KernelOffer[];
}

/** Until a profile has been read back, this is what the platform mandates. */

type Mode = { kind: 'list' } | { kind: 'new' } | { kind: 'edit'; id: string };

export function EnvironmentProfiles({ store, onClose, kernels = [] }: EnvironmentProfilesProps) {
  const [profiles, setProfiles] = useState<readonly EnvironmentProfile[] | null>(null);
  const [catalogue, setCatalogue] = useState<SkillCatalogue | null>(null);
  const [skillsError, setSkillsError] = useState<string | undefined>(undefined);
  const [servicesCatalogue, setServicesCatalogue] = useState<ServiceCatalogue | undefined>(
    undefined,
  );
  const [servicesError, setServicesError] = useState<string | undefined>(undefined);
  const [error, setError] = useState<string | undefined>(undefined);
  const [mode, setMode] = useState<Mode>({ kind: 'list' });
  const [draft, setDraft] = useState<ProfileDraft>(emptyDraft);
  const [saving, setSaving] = useState(false);
  const [confirmingRemoval, setConfirmingRemoval] = useState<string | null>(null);

  /*
   * Promise-chained rather than awaited so nothing sets state synchronously
   * inside the effect below, which is the cascading-render shape React's lint
   * rule exists to prevent.
   */
  const reload = useCallback(
    () =>
      store
        .list()
        .then((available) => {
          setProfiles(available);
          setError(undefined);
        })
        .catch((cause: unknown) => {
          // An empty list rather than none: the surface renders, and the
          // reason it is empty is stated rather than implied.
          setProfiles([]);
          setError(message(cause, 'Your profiles could not be loaded.'));
        }),
    [store],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    let live = true;
    store
      .catalogue()
      .then((available) => {
        if (live) {
          setCatalogue(available);
          setSkillsError(undefined);
        }
      })
      .catch((cause: unknown) => {
        // A registry that cannot be listed is recoverable: everything else on
        // this form still works, so it must not take the dialog down with it.
        if (live) {
          setSkillsError(message(cause, 'The skill registry could not be reached.'));
        }
      });
    return () => {
      live = false;
    };
  }, [store]);

  useEffect(() => {
    let live = true;
    store
      .services()
      .then((available) => {
        if (live) {
          setServicesCatalogue(available);
          setServicesError(undefined);
        }
      })
      .catch((cause: unknown) => {
        // Same rule as the registry: a list that cannot be read costs the
        // choice, not the form. A session still reaches what the deployment
        // mandates, so a profile saved now is a working profile.
        if (live) {
          setServicesError(message(cause, 'Your services could not be listed.'));
        }
      });
    return () => {
      live = false;
    };
  }, [store]);

  /*
   * Whatever the server last said it mandates, never a constant here: a
   * deployment that changes what every session carries changes this with no
   * release of the frontend. A stored profile carries the same answer, and is
   * the fallback for a registry that could not be reached.
   */
  const mandatory = catalogue?.mandatory ?? profiles?.[0]?.mandatory ?? NOTHING_MANDATED;

  async function save(): Promise<void> {
    const problem = validate(draft);
    if (problem !== null) {
      setError(problem);
      return;
    }
    setSaving(true);
    try {
      if (mode.kind === 'edit') {
        await store.update(mode.id, draft);
      } else {
        await store.create(draft);
      }
      await reload();
      setMode({ kind: 'list' });
      setError(undefined);
    } catch (cause: unknown) {
      setError(message(cause, 'The profile could not be saved.'));
    } finally {
      setSaving(false);
    }
  }

  async function remove(id: string): Promise<void> {
    try {
      await store.remove(id);
      setConfirmingRemoval(null);
      await reload();
    } catch (cause: unknown) {
      setError(message(cause, 'The profile could not be removed.'));
    }
  }

  if (mode.kind !== 'list') {
    return (
      <Modal
        open
        title={mode.kind === 'new' ? 'New environment profile' : 'Edit environment profile'}
        /*
         * A form with pickers beside it. At the card's own width its rows
         * truncated, which is the same reason §14 took operations out of a
         * dialog altogether.
         */
        placement="modal--widest"
        onClose={onClose}
        footer={
          <>
            <Button
              onClick={() => {
                setMode({ kind: 'list' });
                setError(undefined);
              }}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={saving}
              onClick={() => {
                void save();
              }}
            >
              {saving ? 'Saving' : 'Save profile'}
            </Button>
          </>
        }
      >
        <ProfileEditor
          draft={draft}
          onChange={setDraft}
          kernels={kernels}
          skills={catalogue?.skills ?? []}
          skillsError={skillsError}
          services={servicesCatalogue?.services ?? []}
          mandatoryServices={servicesCatalogue?.mandatory ?? []}
          servicesError={servicesError}
          mandatory={mandatory}
          error={error}
        />
      </Modal>
    );
  }

  return (
    <Modal
      open
      title="Environment profiles"
      /* A list of rows with controls on each. */
      placement="modal--wide"
      onClose={onClose}
      footer={
        <Button
          variant="primary"
          onClick={() => {
            setDraft(emptyDraft());
            setError(undefined);
            setMode({ kind: 'new' });
          }}
        >
          New profile
        </Button>
      }
    >
      {error === undefined ? null : (
        <p className="field__error" role="alert">
          {error}
        </p>
      )}

      {profiles === null ? (
        <p className="empty">Loading your profiles.</p>
      ) : profiles.length === 0 ? (
        <p className="empty">
          No profiles yet. A profile bundles environment variables, software and skills so a new
          session already has them.
        </p>
      ) : (
        <ul className="profiles">
          {profiles.map((profile) => (
            <li className="profiles__row" key={profile.id}>
              <span className="profiles__text">
                <span className="profiles__name">{profile.name}</span>
                <span className="profiles__meta">{summarise(profile)}</span>
              </span>
              {confirmingRemoval === profile.id ? (
                /*
                 * Inline, in place of the row's own controls (§7). A second
                 * dialog over this one is what the rule exists to forbid, and
                 * a confirmation is exactly the thing that reaches for one.
                 */
                <InlineConfirm
                  question="Remove this profile?"
                  confirmLabel="Remove"
                  onConfirm={() => {
                    void remove(profile.id);
                  }}
                  onCancel={() => {
                    setConfirmingRemoval(null);
                  }}
                />
              ) : (
                <>
                  <Button
                    size="sm"
                    onClick={() => {
                      setDraft(draftOf(profile));
                      setError(undefined);
                      setMode({ kind: 'edit', id: profile.id });
                    }}
                  >
                    Edit
                  </Button>
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={() => {
                      setConfirmingRemoval(profile.id);
                    }}
                  >
                    Remove
                  </Button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </Modal>
  );
}

/** One line describing what a profile carries. */
function summarise(profile: EnvironmentProfile): string {
  const parts = [
    `${profile.variables.length} variable${profile.variables.length === 1 ? '' : 's'}`,
    `${profile.packages.length} package${profile.packages.length === 1 ? '' : 's'}`,
    `${profile.agent.skills.length} skill${profile.agent.skills.length === 1 ? '' : 's'}`,
  ];
  return parts.join(' · ');
}

/**
 * A sentence fit to show a user.
 *
 * Web_Style.md section 7: errors say what went wrong, never a raw code. A
 * store is injected and may reject with anything, so a fallback sentence is
 * what stops "[object Object]" reaching the screen.
 */
function message(cause: unknown, fallback: string): string {
  return cause instanceof Error && cause.message.length > 0 ? cause.message : fallback;
}
