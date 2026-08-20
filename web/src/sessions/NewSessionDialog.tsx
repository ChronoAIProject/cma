import { useEffect, useId, useState } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { Picker } from '../ui/Picker';
import { ProfileEditor } from '../environment/ProfileEditor';
import { emptyDraft, NOTHING_MANDATED, validate } from '../environment/types';
import type {
  EnvironmentProfile,
  EnvironmentProfileStore,
  ProfileDraft,
  SkillCatalogue,
  ServiceCatalogue,
} from '../environment/types';
import { kernelLabel, planeRefusal, PLANES } from './kernels';
import './NewSessionDialog.css';
import type { KernelOffer, SessionPlane } from './types';

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
  /*
   * Starts on the profile this owner chose, and on the deployment's own
   * defaults where they have not chosen one — both described starting points,
   * so neither is a bare session.
   *
   * Seeded when the dialog opens rather than once when it mounts. This is
   * mounted with the console, long before the profile list has been read, so
   * an initialiser saw an empty list every time and the preference never
   * applied — which is the whole of what the preference is for.
   *
   * Adjusted during render rather than in an effect, so the dialog is never
   * drawn once on the wrong profile and then corrected.
   */
  const preferred = profiles.find((candidate) => candidate.preferred)?.id ?? '';
  const [profile, setProfile] = useState<string>(preferred);
  const [opened, setOpened] = useState(open);
  if (opened !== open) {
    setOpened(open);
    if (open) {
      // Opening is when the choice starts, so it starts from the preference —
      // and a choice made in a previous opening does not survive into this one.
      setProfile(preferred);
    }
  }
  const [draft, setDraft] = useState<ProfileDraft>(emptyDraft);
  const [catalogue, setCatalogue] = useState<ServiceCatalogue | undefined>(undefined);
  /*
   * The skill registry, read alongside the services for the same reason and
   * failing the same way: a registry that cannot be listed is recoverable, and
   * everything else on the form still works.
   */
  const [skills, setSkills] = useState<SkillCatalogue | undefined>(undefined);
  const [skillsError, setSkillsError] = useState<string | undefined>(undefined);
  const [servicesError, setServicesError] = useState<string | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const profileLabelId = useId();
  const noRuntimesId = useId();

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
      .catalogue()
      .then((available) => {
        if (live) {
          setSkills(available);
          setSkillsError(undefined);
        }
      })
      .catch((cause: unknown) => {
        if (live) {
          setSkillsError(message(cause, 'The skill registry could not be reached.'));
        }
      });
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
      /*
       * A whole profile form needs the room; choosing between profiles needs
       * less, but not the 380px default — that card held a picker, a key and
       * two hints in a column narrower than the hints.
       */
      placement={authoring ? 'modal--widest' : 'modal--wide'}
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
            placement="profilerow__picker"
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

      {/*
       * The real editor, not a smaller copy of it.
       *
       * This carried its own form of three fields — a name, an agent and the
       * services it reaches — while a profile has an agent configuration, a
       * skill set, its NyxID access and its environment variables. Somebody
       * authoring one here got a profile they then had to go and finish
       * somewhere else, and the two forms had to be kept in step by hand.
       *
       * One form, in both places. What it can express is what a profile is.
       *
       * It is given no `error`: this dialog draws the save failure once at its
       * head, above whichever half of it is showing, and the editor works out
       * the agent's own refusal for itself. Handing it either drew the same
       * sentence twice.
       */}
      {!authoring ? null : (
        <ProfileEditor
          draft={draft}
          onChange={setDraft}
          kernels={kernels}
          skills={skills?.skills ?? []}
          skillsError={skillsError}
          services={catalogue?.services ?? []}
          mandatoryServices={catalogue?.mandatory ?? []}
          servicesError={servicesError}
          mandatory={skills?.mandatory ?? NOTHING_MANDATED}
        />
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
