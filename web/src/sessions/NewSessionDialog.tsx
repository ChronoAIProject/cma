import { useEffect, useId, useState } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { Picker } from '../ui/Picker';
import { ServicePicker } from '../environment/ServicePicker';
import { emptyDraft, validate } from '../environment/types';
import type {
  EnvironmentProfile,
  EnvironmentProfileStore,
  ProfileDraft,
  ServiceCatalogue,
} from '../environment/types';
import { kernelLabel, planeRefusal, PLANES } from './kernels';
import './NewSessionDialog.css';
import type { KernelOffer, SessionPlane } from './types';
import { Input } from '../ui/Input';

/**
 * The choice made when a session starts.
 *
 * Two things are decided here and both are fixed for the session's life: which
 * profile describes it. **A session is a conversation**: the plane is no longer
 * asked here, because moving a session between planes is a rotation rather
 * than a toggle (`Web_Style.md` §18.6), and it is
 * chosen at the only moment it can be.
 *
 * Deliberately small at rest. Everything has a working default, so someone who
 * wants what this deployment gives them can press Start; the rest of the form
 * appears only for the person authoring a profile here.
 *
 * A profile is authored **in place**. What a session's credential may reach
 * lives on a profile and stays there — a named, reusable, auditable record —
 * but somebody creating their first service-scoped session should not have to
 * leave, author one, and come back. This composes the same picker the profile
 * editor uses and saves a real profile, so the record is unchanged and only
 * the detour is gone.
 */

/** The option that means "author one here", kept out of the identifier space. */
const AUTHOR = '#new';

interface NewSessionDialogProps {
  readonly open: boolean;
  readonly onCancel: () => void;
  /**
   * Start a session from this profile, or from the deployment's own defaults.
   *
   * One argument because a profile is the whole description of a session:
   * which agent, which plane, how freely it acts, and what it carries. A
   * profile authored here is saved first, so this always names a stored one.
   */
  readonly onStart: (environmentProfile: string | null) => void;
  /**
   * Environment bundles this person can start a session with.
   *
   * Passed in rather than fetched here so the dialog stays a form: it is opened
   * and closed constantly, and fetching on every open would ask the control
   * plane the same question each time.
   */
  readonly profiles?: readonly EnvironmentProfile[];
  /**
   * The runtimes this deployment can run, and what each one can be.
   *
   * Empty means the list could not be read. That refuses nothing that was
   * already stored, but it does withdraw the offer to author a profile here:
   * choosing a runtime blind is how somebody ends up with a session the
   * control plane refuses after they have filled the form in.
   */
  readonly kernels?: readonly KernelOffer[];
  /** Where a profile authored here is stored, and where the catalogue is read. */
  readonly store: EnvironmentProfileStore;
  /** A profile this dialog created, so the list it was given stays current. */
  readonly onProfileCreated?: (profile: EnvironmentProfile) => void;
}

export function NewSessionDialog({
  open,
  onCancel,
  onStart,
  profiles = [],
  kernels = [],
  store,
  onProfileCreated,
}: NewSessionDialogProps) {
  const [profile, setProfile] = useState<string>('');
  const [draft, setDraft] = useState<ProfileDraft>(emptyDraft);
  const [catalogue, setCatalogue] = useState<ServiceCatalogue | undefined>(undefined);
  const [servicesError, setServicesError] = useState<string | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const profileLabelId = useId();
  const noRuntimesId = useId();
  const nameId = useId();
  const kernelId = useId();

  const authoring = profile === AUTHOR;

  useEffect(() => {
    if (!authoring) {
      return;
    }
    let live = true;
    /*
     * Asked when somebody starts authoring rather than when the dialog opens:
     * most sessions are started from a profile that already exists, and that
     * path has no use for the catalogue.
     *
     * A deployment with no catalogue is drawn, not failed. `available_services`
     * answers as the caller, so a deployment that cannot answer leaves the
     * picker out and the profile still saves — what this deployment mandates
     * is added to it either way.
     */
    store
      .services()
      .then((available) => {
        if (live) {
          setCatalogue(available);
          setServicesError(undefined);
        }
      })
      .catch((cause: unknown) => {
        if (live) {
          setCatalogue(undefined);
          setServicesError(message(cause, 'The services you can reach could not be read.'));
        }
      });
    return () => {
      live = false;
    };
  }, [authoring, store]);

  if (!open) {
    return null;
  }

  const chosen = profiles.find((p) => p.id === profile);
  const refusal = authoring
    ? planeRefusal(kernels, draft.agent.kernel, draft.agent.mode)
    : undefined;

  function start(): void {
    if (!authoring) {
      onStart(profile === '' ? null : profile);
      return;
    }
    const problem = validate(draft);
    if (problem !== null) {
      setError(problem);
      return;
    }
    setSaving(true);
    setError(undefined);
    /*
     * Saved before the session is started, and the dialog stays open if that
     * fails. Closing on a failure would lose the name and the services the
     * person just chose, which is the detour this form exists to remove.
     */
    store
      .create(draft)
      .then((created) => {
        onProfileCreated?.(created);
        onStart(created.id);
      })
      .catch((cause: unknown) => {
        setError(message(cause, 'The profile could not be saved.'));
      })
      .finally(() => {
        setSaving(false);
      });
  }

  return (
    <Modal
      open
      title="New session"
      /* A form with a picker in it needs the room; a question does not. */
      placement={authoring ? 'modal--wide' : undefined}
      onClose={onCancel}
      footer={
        <>
          {/*
           * Short here and named in full beside the control that caused it:
           * §7 states a disabled key's reason once per row, as a sibling of
           * the keys it explains, and the same sentence in both places reads
           * as two problems.
           */}
          {refusal === undefined ? null : (
            <span className="control-reason">The agent cannot serve this plane</span>
          )}
          <Button onClick={onCancel}>Cancel</Button>
          <Button variant="primary" disabled={saving || refusal !== undefined} onClick={start}>
            {saving ? 'Starting' : 'Start session'}
          </Button>
        </>
      }
    >
      {error === undefined ? null : (
        <p className="field__error" role="alert">
          {error}
        </p>
      )}

      <div className="field">
        <span className="field__label" id={profileLabelId}>
          Environment profile
        </span>
        {/*
         * "None" is not "bare". A session that names no profile is started the
         * way this deployment says an unqualified session should be — which
         * agent, which model, how freely it acts — so this chooses between
         * described sessions rather than between something and nothing.
         */}
        {/*
         * The picker lists only what a session can be started from, and the
         * authoring entry is a button beside it rather than a last option
         * inside it. A picker's options are alternatives to each other;
         * authoring is not an alternative to a profile — choosing it opened a
         * form while choosing anything else started a session, which is why
         * it alone needed a disabled state and a reason of its own.
         */}
        <div className="profilerow">
          <Picker
            label="Environment profile"
            value={profile}
            options={[
              { value: '', label: "This deployment's defaults" },
              ...profiles.map((p) => ({ value: p.id, label: p.name })),
            ]}
            onChange={setProfile}
          />
          {authoring ? null : (
            <Button
              disabled={kernels.length === 0}
              aria-describedby={kernels.length === 0 ? noRuntimesId : undefined}
              onClick={() => {
                setProfile(AUTHOR);
              }}
            >
              New environment profile
            </Button>
          )}
        </div>
        {/*
         * §7: a disabled key states its reason beside it. Choosing a runtime
         * blind is how somebody fills a form in and then learns the deployment
         * could not tell them what it runs.
         */}
        {authoring || kernels.length > 0 ? null : (
          <span className="control-reason" id={noRuntimesId}>
            This deployment&rsquo;s runtimes could not be read
          </span>
        )}
        <span className="field__hint">
          The whole description of the session: the agent and how it behaves, the software it
          carries, and the variables it is started with.
        </span>
        {chosen === undefined ? null : (
          <span className="field__hint">
            Starts a {planeWord(chosen.agent.mode)} session on {kernelLabel(chosen.agent.kernel)}.
          </span>
        )}
      </div>

      {!authoring ? null : (
        <>
          <div className="field">
            <label className="field__label" htmlFor={nameId}>
              Name
            </label>
            <Input
              id={nameId}
              placement="field__input"
              value={draft.name}
              onChange={(event) => {
                setDraft({ ...draft, name: event.target.value });
              }}
            />
            <span className="field__hint">
              Saved as a profile you can start again, and edit under Environment profiles.
            </span>
          </div>

          <div className="field">
            <label className="field__label" htmlFor={kernelId}>
              Agent
            </label>
            <Picker
              id={kernelId}
              label="Agent"
              value={draft.agent.kernel}
              options={kernels.map((kernel) => ({
                value: kernel.id,
                label: kernelLabel(kernel.id),
                unavailable: kernel.modes.length === 0 ? 'Runs no sessions' : undefined,
              }))}
              onChange={(kernel) => {
                setDraft({ ...draft, agent: { ...draft.agent, kernel } });
              }}
            />
            {/*
             * The refusal sits under the control that causes it. It used to
             * live with the plane picker, which is where somebody chose the
             * thing being refused — with the plane no longer asked, the agent
             * is the only choice that can produce it, and §9 wants it named
             * here rather than reported as a disabled key with no reason.
             */}
            {refusal === undefined ? null : (
              <p className="field__error" role="alert">
                {refusal}
              </p>
            )}
          </div>

          <div className="field">
            <span className="field__label">Reaches</span>
            {catalogue === undefined && servicesError === undefined ? (
              <p className="empty">Reading what you can reach…</p>
            ) : (
              <ServicePicker
                services={catalogue?.services ?? []}
                selected={draft.nyxid.services}
                mandatory={catalogue?.mandatory ?? []}
                error={servicesError}
                onChange={(services) => {
                  setDraft({ ...draft, nyxid: { services } });
                }}
              />
            )}
            <span className="field__hint">
              What this session&rsquo;s own credential may reach. Nothing else is delegated to it.
            </span>
          </div>
        </>
      )}
    </Modal>
  );
}

/**
 * The plane in the words a sentence needs.
 *
 * Still needed although the form no longer asks: a profile stored before this
 * may name the terminal, and the hint describing it must say so rather than
 * claim every profile is a conversation.
 */
function planeWord(plane: SessionPlane): string {
  return PLANES.find((p) => p.id === plane)?.label.toLowerCase() ?? plane;
}

function message(cause: unknown, fallback: string): string {
  return cause instanceof Error && cause.message.length > 0 ? cause.message : fallback;
}
