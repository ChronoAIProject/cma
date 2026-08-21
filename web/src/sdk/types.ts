/**
 * The session model, as the control plane reports it — `docs/API_Overview.md`.
 *
 * A projection of the Layer 2 model, and never more than one: nothing here
 * invents state the core does not have. It lives in the SDK because it is what
 * the wire says rather than how anything paints it — a consumer that never
 * renders a pixel still needs every one of these, and the tone and label a tab
 * is drawn with stay in `sessions/` where the drawing is.
 */

import type { SessionCapabilities } from '../agui';

/**
 * Lifecycle: provisioning -> ready -> running <-> idle -> terminated.
 *
 * `rotating` sits across the top of that: a running session being moved onto
 * an updated runtime. It is a state of its own because the session is neither
 * usable nor stopped while it happens, and calling it either would be a lie
 * the owner acts on.
 */
export type SessionLifecycle =
  'provisioning' | 'ready' | 'running' | 'rotating' | 'idle' | 'terminated';

/**
 * Why a session stopped, where the control plane says.
 *
 * Only `error` is distinguished here, because it is the only one that changes
 * what the interface should do: a session that failed stays visible so its
 * owner can find out why.
 */
export type StopReason = 'error' | 'other';

/**
 * The two planes a session can be reached on.
 *
 * Exclusive by construction: `capabilities.rs` fixes the granted set at
 * creation and a session holds one or the other. Moving between them is a
 * rotation and never a toggle — the session is captured, its runtime is
 * replaced, and it resumes from the capture (§18.6).
 */
export type SessionPlane = 'terminal' | 'conversation';

/**
 * The identifier a session names its runtime by.
 *
 * A string rather than a union of the ones this build happens to know. Which
 * runtimes a deployment offers is a fact about that deployment — `kernels.ts`
 * already reads them from `GET /api/v1/kernels` and labels an unfamiliar one
 * under its own identifier. A closed union here contradicted that and did
 * worse than mislabel: the client refused to parse the session at all, and
 * one such session made the whole list unreadable.
 */
export type Kernel = string;

/**
 * A runtime this deployment can start a session on, and what it can be.
 *
 * Read from `GET /api/v1/kernels` rather than listed here, because which
 * runtimes exist and what each can serve are facts about a deployment. A list
 * kept in the client is wrong the first time one changes, and being wrong here
 * means offering somebody a session the control plane will refuse.
 */
export interface KernelOffer {
  readonly id: string;
  /** The planes this runtime can serve. Empty means it can serve none. */
  readonly modes: readonly SessionPlane[];
}

/**
 * One service a session's credential may reach.
 *
 * Identifiers always; a name and a description only when the caller's own
 * service catalogue offered them. A catalogue that could not be asked costs
 * the names and not the answer, so a service with no name is still a service
 * this session reaches.
 */
export interface SessionService {
  readonly id: string;
  /** What the owner recognises it by, when the catalogue said. */
  readonly name?: string;
  readonly description?: string;
  /**
   * Whether the platform mandates it.
   *
   * Drawn as fixed rather than as something the owner chose, matching how the
   * picker already treats the entries that cannot be unticked — for the same
   * stated reason: a person deciding what a session may touch should see the
   * whole answer.
   */
  readonly mandated: boolean;
}

export interface Session {
  readonly id: string;
  readonly kernel: Kernel;
  readonly lifecycle: SessionLifecycle;
  /** Epoch ms the session was created, for the tab's age readout. */
  readonly createdAt: number;
  /** Why it stopped, when it has. */
  readonly stopReason?: StopReason;
  /** Which runtime version is serving it. */
  readonly runtimeVersion?: string;
  /** How many times it has been moved onto a newer runtime. */
  readonly rotations?: number;
  /**
   * Epoch ms of the snapshot the current runtime resumed from.
   *
   * The honest part of a rotation: it says how much of the session was carried
   * across, so anything the agent did in the seconds after it can be
   * recognised as missing rather than silently absent.
   */
  readonly rotatedAt?: number;
  /**
   * Who started the session, as they should be shown.
   *
   * Absent when the control plane would not name them: naming somebody is its
   * own grant, and a caller who does not hold it is told nothing rather than
   * handed an identifier. Never fall back to the subject — it is not a name
   * and reads as a rendering fault when drawn as one.
   */
  readonly creator?: SessionCreator;
  /**
   * What this session's credential may reach.
   *
   * `undefined` and `[]` are different facts and must not be drawn alike.
   * Undefined means the control plane did not report it — an older build, or a
   * projection this one does not carry — and the honest answer is that we do
   * not know. Empty means this session reaches nothing, which is a real answer
   * somebody can act on.
   */
  readonly reach?: readonly SessionService[];
  /**
   * Which plane this session is drawn on — `Web_Style.md` §1 and §18.
   *
   * A property of the session, decided when it was created, and not a view and
   * not a preference. The client reads it and draws what it implies; there is
   * nothing here to toggle, because the two planes are exclusive by
   * construction in the core.
   *
   * Absent when the control plane did not say. A session whose plane is not
   * reported falls back to the terminal, which is what every session was
   * before the conversation plane existed — the safe direction, since a
   * terminal drawn for a conversation session shows an empty screen and a
   * conversation drawn for a terminal session shows nothing at all.
   */
  readonly plane?: SessionPlane;
  /**
   * What this session was granted, read once at the boundary.
   *
   * #116: components ask "may this session steer" rather than trying and
   * handling a refusal. Held on the session because it is a fact about the
   * session, decided at creation and changed only by an explicit, recorded act
   * of its owner — moving it between planes, which is a rotation and arrives
   * as a new record rather than as an edit to this one.
   */
  readonly capabilities: SessionCapabilities;
  /**
   * How this session's agent was configured, as the control plane reported it.
   *
   * Absent from a control plane that does not project it. Every field inside is
   * separately optional for the same reason, so a build that learns one of them
   * later does not have to be matched here first.
   */
  readonly agent?: SessionAgent;
}

/** How much the agent may do before it asks. */
export type PermissionMode = 'guarded' | 'autonomous' | 'unrestricted';

/** How hard the agent thinks. The wire spells the top step `x_high`. */
export type ReasoningEffort = 'minimal' | 'low' | 'medium' | 'high' | 'x_high';

/**
 * Whose account the model calls are spent against.
 *
 * `delegated` is the platform's, on the owner's behalf; `own` is the owner's
 * own account with the model vendor. Never a credential — this says which kind
 * pays, which is a fact somebody needs, and not the value, which is not.
 */
export type CredentialMode = 'delegated' | 'own';

/**
 * What a session's agent was configured with, settled when it was created.
 *
 * The session's answer and never its profile's, for the same reason `reach` is:
 * a profile can be edited after a session started from it, and the session
 * keeps what it was provisioned with.
 */
export interface SessionAgent {
  /** The model the agent selects, where the session names one. */
  readonly model?: string;
  readonly permission?: PermissionMode;
  readonly reasoningEffort?: ReasoningEffort;
  readonly credential?: CredentialMode;
  /**
   * The skills this session was given.
   *
   * `undefined` and `[]` are different facts, as with `reach`: undefined means
   * nothing was recorded — a session created before the grant was kept — and
   * empty means this session was given no skills at all.
   */
  readonly skills?: readonly SessionSkill[];
}

/**
 * One skill a session was given.
 *
 * The same shape as `SessionService` and for the same reason: the identifier
 * is always true, and a catalogue that could not be asked costs the name.
 */
export interface SessionSkill {
  readonly id: string;
  readonly name?: string;
  readonly description?: string;
}

/** How the person who started a session is drawn. */
export interface SessionCreator {
  readonly displayName?: string;
  readonly avatarUrl?: string;
}
