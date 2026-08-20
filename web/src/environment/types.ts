/**
 * Environment profiles, as the console models them.
 *
 * A projection of the Layer 2 resource for display and editing. It must not
 * invent state the core does not have, and it must not enforce a rule the
 * server does not: the non-revocable baseline is rendered here so a user is
 * not invited to attempt something that will be undone, but the server applies
 * it whatever this code does.
 */
import type { CredentialMode, PermissionMode, ReasoningEffort } from '../sessions/options';
import type { SessionPlane } from '../sessions/types';

/** One environment variable in a profile. */
export interface ProfileVariable {
  readonly key: string;
  readonly value: string;
}

/** A piece of software a session should carry. */
export interface ProfilePackage {
  readonly name: string;
  /** Absent means whatever version the runtime provides. */
  readonly version?: string;
}

/** What the platform mandates, which no profile may remove. */
export interface MandatoryEntries {
  readonly packages: readonly string[];
  readonly skills: readonly string[];
}

/** A stored environment profile. */
export interface EnvironmentProfile {
  readonly id: string;
  readonly name: string;
  readonly agent: AgentConfiguration;
  readonly nyxid: NyxIdAccess;
  readonly variables: readonly ProfileVariable[];
  readonly packages: readonly ProfilePackage[];
  readonly mandatory: MandatoryEntries;
  readonly createdAt: string;
  readonly updatedAt: string;
}

/**
 * How the agent behaves in a session started from this profile.
 *
 * These were once chosen per session, which meant answering the same five
 * questions on every start and left a session describable only by repeating
 * them. Skills sit here rather than beside software because a skill is
 * something the agent knows how to do, not something installed in the box.
 */
export interface AgentConfiguration {
  readonly kernel: string;
  /**
   * Which plane sessions from this profile are driven on.
   *
   * On the profile rather than on the request, exactly as the core has it:
   * two sessions started from one profile must be the same session twice, and
   * the plane is the largest difference there could be between them. It is
   * also why the choice is offered where a profile is authored — a session
   * cannot be moved onto the other plane without being rotated (§18.6).
   */
  readonly mode: SessionPlane;
  readonly model?: string;
  readonly reasoningEffort: ReasoningEffort;
  readonly permission: PermissionMode;
  readonly credential: CredentialMode;
  readonly skills: readonly string[];
}

/**
 * Which of the owner's platform services a session may reach.
 *
 * A session is given a credential minted for it alone, limited to what is
 * named here. Selecting nothing is not a bare session: what the deployment
 * mandates is added whatever this says, because one credential serves the
 * agent's own model calls as well as everything else.
 */
export interface NyxIdAccess {
  readonly services: readonly string[];
}

/** A service the owner may allow, as the picker lists it. */
export interface PlatformService {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
}

/** The picker's answer: what may be chosen, and what cannot be unchosen. */
export interface ServiceCatalogue {
  readonly services: readonly PlatformService[];
  readonly mandatory: readonly string[];
}

/**
 * What a profile is when nobody has said otherwise.
 *
 * Mirrors what the deployment answers for a session that names no profile, so
 * a new profile starts where an unqualified session would and a form is never
 * drawn with a control at no value.
 */
export const DEFAULT_AGENT: AgentConfiguration = {
  kernel: 'codex',
  // The conversation plane is the default surface: it is where approvals are
  // answered, where tool calls read as structure, and where a failure can be
  // explained rather than printed. A terminal is the escape hatch. This
  // mirrors the deployment's own default, so a new profile starts where an
  // unqualified session would.
  //
  // Only a *new* draft starts here. A stored profile is read back exactly as
  // it was written (`draftOf`), so nothing anybody already saved changes.
  mode: 'conversation',
  model: undefined,
  reasoningEffort: 'high',
  permission: 'guarded',
  credential: 'delegated',
  skills: [],
};

/**
 * What an unstated plane means on a profile that was already stored.
 *
 * Deliberately NOT `DEFAULT_AGENT.mode`. That is what a *new* draft starts
 * as and it moved to conversation; this is what a record written before the
 * field existed already meant, and it is terminal — the same answer the
 * core's own wire format gives. Reading an old profile through the new
 * default would silently change what it starts.
 */
export const PLANE_BEFORE_PLANES_EXISTED: SessionPlane = 'terminal';

/** What a caller supplies when creating or replacing a profile. */
export interface ProfileDraft {
  readonly name: string;
  readonly agent: AgentConfiguration;
  readonly nyxid: NyxIdAccess;
  readonly variables: readonly ProfileVariable[];
  readonly packages: readonly ProfilePackage[];
}

/** A skill the registry offers, as the picker renders it. */
export interface CatalogSkill {
  readonly id: string;
  readonly name: string;
  readonly description: string;
  /** Why this user can see it — owner, public, shared. Display only. */
  readonly access?: string;
}

/**
 * What the editor needs before it has a profile to read from.
 *
 * The mandated entries ride with the catalogue rather than only with a stored
 * profile, so a user authoring their first one is told what it will carry
 * instead of discovering it after saving.
 */
export interface SkillCatalogue {
  readonly skills: readonly CatalogSkill[];
  readonly mandatory: MandatoryEntries;
}

/**
 * Where profiles come from.
 *
 * A port, mirroring the auth seam: components consume this and never a
 * concrete implementation, so the in-memory store the console is developed
 * against and a REST-backed one are interchangeable without touching a single
 * consumer.
 *
 * Implementations must not throw. A failure is reported by rejecting with an
 * Error whose message is fit to show a user.
 */
export interface EnvironmentProfileStore {
  /** Every profile the signed-in user owns, newest first. */
  list(): Promise<readonly EnvironmentProfile[]>;
  /** Store a new profile. */
  create(draft: ProfileDraft): Promise<EnvironmentProfile>;
  /** Replace a profile's contents. */
  update(id: string, draft: ProfileDraft): Promise<EnvironmentProfile>;
  /** Remove a profile. */
  remove(id: string): Promise<void>;
  /** Skills this user may select, and what the platform mandates. */
  catalogue(): Promise<SkillCatalogue>;
  /** Services this user may allow a session to reach, and which are fixed. */
  services(): Promise<ServiceCatalogue>;
}

/**
 * An empty draft, for the "new profile" form.
 *
 * Empty of the owner's own choices, not of behaviour: the agent section starts
 * where an unqualified session would, so a profile someone names and saves
 * without touching anything behaves the way this deployment already does.
 */
export function emptyDraft(): ProfileDraft {
  return {
    name: '',
    variables: [],
    packages: [],
    agent: { ...DEFAULT_AGENT },
    nyxid: { services: [] },
  };
}

/** The draft that would replace an existing profile unchanged. */
export function draftOf(profile: EnvironmentProfile): ProfileDraft {
  return {
    name: profile.name,
    variables: [...profile.variables],
    packages: [...profile.packages],
    agent: { ...profile.agent, skills: [...profile.agent.skills] },
    nyxid: { services: [...profile.nyxid.services] },
  };
}

/**
 * Whether a variable name is one the session will actually see.
 *
 * The same POSIX shape the core enforces. Checked here too so a user is told
 * at the field rather than after a round trip — never *instead* of the server
 * checking, which is the rule that matters.
 */
export function isValidKey(key: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(key);
}

/**
 * What is wrong with a draft, or null when nothing is.
 *
 * One message at a time: a form that reports six problems at once is read as
 * one problem by most people, and the first is the one they can act on.
 */
export function validate(draft: ProfileDraft): string | null {
  if (draft.name.trim().length === 0) {
    return 'Give the profile a name.';
  }
  const seen = new Set<string>();
  for (const variable of draft.variables) {
    if (!isValidKey(variable.key)) {
      return `"${variable.key}" is not a usable variable name. Use letters, digits and underscores, starting with a letter or underscore.`;
    }
    if (seen.has(variable.key)) {
      return `${variable.key} is set twice. Remove one of them.`;
    }
    seen.add(variable.key);
  }
  return null;
}

/**
 * What a deployment mandates when nothing has said.
 *
 * Shared rather than declared per form: two copies of "nothing" that drift
 * apart is a form claiming a package is required and another saying it is not.
 */
export const NOTHING_MANDATED: MandatoryEntries = { packages: [], skills: [] };
