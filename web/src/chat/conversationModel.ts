import type { Interrupt } from '../agui';

/**
 * What a conversation *is* — `Web_Style.md` §18.1.
 *
 * The shape only. How events become one of these is `conversation.ts`, and
 * how one is drawn is the components'. Separated when this file crossed the
 * 500-line ceiling: the vocabulary is read by every surface on this plane,
 * while the fold is read by whoever is changing the fold.
 *
 * No React and no DOM.
 */

/** One thing in the stream, in the order it happened. */
export type Entry = MessageEntry | ToolCallEntry | GapEntry | FailureEntry | UnknownEntry;

export interface MessageEntry {
  readonly kind: 'message';
  readonly id: string;
  readonly role: 'assistant' | 'user';
  readonly text: string;
  /** False while the turn is still in flight. Carries the caret; nothing else. */
  readonly committed: boolean;
  readonly seq: number;
  /**
   * When the message entered the durable log, RFC 3339. Absent for an event a
   * control plane predating timestamps wrote; §18.1 renders no time then.
   */
  readonly at?: string;
}

/** A tool call's arguments or result, as the log carried it. */
export interface ToolBody {
  readonly text: string;
  /** Whether the body was cut to fit the log's bound. */
  readonly truncated: boolean;
}

export interface ToolCallEntry {
  readonly kind: 'tool';
  readonly id: string;
  /** The kernel's own word. Never mapped onto a set this client invented. */
  readonly toolKind: string;
  readonly target?: string;
  /** Absent while it is still running, which is how §18.2 tells them apart. */
  readonly status?: string;
  readonly output?: string;
  /** What it was asked to do, when the log carried it. */
  readonly arguments?: ToolBody;
  /** What it produced, when the log carried it. */
  readonly result?: ToolBody;
  readonly seq: number;
  /** When the call started, RFC 3339. */
  readonly startedAt?: string;
  /** When the call finished, RFC 3339. Absent while running. */
  readonly finishedAt?: string;
}

/**
 * A boundary where conversation was lost.
 *
 * §18.6 and `CLAUDE.md` §5: anything the agent did after the snapshot did not
 * travel, and an owner who can see the boundary can recognise a gap rather
 * than puzzle over one.
 */
export interface GapEntry {
  readonly kind: 'gap';
  readonly id: string;
  readonly rotatedAt?: string;
  readonly reason?: string;
  /**
   * Where the session was moved to, when the boundary is a plane change.
   *
   * The same boundary either way — history stopped being continuous at
   * `rotatedAt` — and worth naming, because an owner who asked for the move is
   * looking for confirmation that this gap is the one they caused rather than
   * a second thing that went wrong.
   */
  readonly movedTo?: string;
  readonly seq: number;
}

/**
 * A run that stopped badly — §18.9.
 *
 * In the stream rather than in a band above it, because a failure has a
 * position: it happened after the last tool call and before nothing. §18.6
 * already settles this for the other kind of boundary, and a failure hoisted
 * out of the stream loses the one thing that says what the agent was doing
 * when it stopped.
 *
 * A rotation never becomes one of these. It ends the run and reaches this
 * client on the same frame, but nothing has gone wrong and §11 forbids it
 * taking the failure ink.
 */
export interface FailureEntry {
  readonly kind: 'failure';
  readonly id: string;
  /** What failed, in the kernel's own words. */
  readonly message: string;
  /**
   * The ability the session lacked, when the failure was a refusal.
   *
   * Named rather than described. An interface that renders this as "something
   * went wrong" has thrown away the only part a person could act on.
   */
  readonly capability?: string;
  /** The stop reason, as the API spells it. */
  readonly code?: string;
  readonly seq: number;
}

/** An event kind this build does not know. Shown, never dropped. */
export interface UnknownEntry {
  readonly kind: 'unknown';
  readonly id: string;
  readonly eventKind: string;
  readonly seq: number;
}

/**
 * What the owner has to do somewhere else before the agent can continue.
 *
 * Never a credential. The address and the code are worth nothing without the
 * person's own account, and what the sign-in produces is written by the agent
 * inside its sandbox.
 */
export interface SignIn {
  /** The page to open, on their own device. */
  readonly url: string;
  /** The one-time code to enter there, where the kernel gave one. */
  readonly code?: string;
  /** The kernel's own wording, rendered verbatim where it gave any. */
  readonly message?: string;
}

/** An approval, and when the run that raised it stopped. */
export interface PendingApproval extends Interrupt {
  /** RFC 3339, from the frame that carried it. Absent on an older log. */
  readonly askedAt?: string;
}

/** Everything a surface needs to draw the conversation. */
export interface Conversation {
  readonly entries: readonly Entry[];
  /** True between `RUN_STARTED` and the `RUN_FINISHED` that closes it. */
  readonly turnInFlight: boolean;
  /**
   * The approval the session is parked on, when it is parked on one.
   *
   * Carries the time it was asked alongside the kernel's own question, which
   * §18.3 requires the eyebrow to show. The kernel does not put a time on the
   * question; the frame that delivered it does, so it is taken from there
   * rather than read off a clock here — a session reopened tomorrow must say
   * when the agent stopped, not when somebody looked.
   */
  readonly pending?: PendingApproval;
  /**
   * What the session has cost, when it reports it.
   *
   * `undefined` is not zero (§18.5): a session with no `usage` grant reports
   * nothing, and has often spent a great deal.
   */
  readonly usage?: Usage;
  /**
   * A sign-in the agent is waiting on — `Web_Style.md` §18.
   *
   * Present only on a session spending its owner's own account. It is the one
   * thing on this plane that has to be completed somewhere else, so it is
   * durable rather than live: a prompt lost to a reload leaves a session
   * waiting on a code its owner can no longer read.
   */
  readonly signIn?: SignIn;
  /** Why the last run failed, when one did. */
  readonly error?: { readonly message: string; readonly capability?: string };
  /**
   * The session's permission mode, when the kernel reported a change to it.
   *
   * The kernel's own word, carried verbatim. A safety fact about the session,
   * not agent content, so it is drawn as state rather than as a message.
   */
  readonly mode?: string;
  /**
   * The agent's plan, when it set one.
   *
   * A full revision, replaced on each set — the agent sends the complete list
   * each time, so the latest is the whole plan rather than an accumulation.
   */
  readonly plan?: readonly PlanStep[];
  /**
   * The settings the kernel says are settable, when it reported them.
   *
   * Drawn as a selection rather than a free-text box, from the kernel's own
   * names and kinds.
   */
  readonly configOptions?: readonly ConfigOption[];
  /**
   * The actions the owner may invoke right now — liveness, not history.
   *
   * Absent on reload, deliberately: a stale list replayed from the log would
   * offer actions that no longer exist, and none is better than dead ones.
   */
  readonly availableCommands?: readonly AvailableCommand[];
  /**
   * What the agent is saying right now, before it has committed to it.
   *
   * **Deliberately not an entry.** §18.1 requires a turn drawn from history
   * and a turn watched arriving to produce the same result, and history has
   * no in-flight message: a reload folds no live frames at all and therefore
   * has no live slot, so the two agree by construction. Making it an entry
   * and remembering to remove it would be the same rule enforced by
   * discipline, which is weaker.
   *
   * Replaced, not appended to, by the committed message. They are two
   * identities — the in-flight one has no sequence, because it has no place
   * in the log yet — so the turn is never drawn twice.
   */
  readonly live?: { readonly text: string };
  /**
   * What the agent is reasoning about, while it reasons — §18.8.
   *
   * Held outside `entries` for the same reason `live` is, and more
   * emphatically: a message is eventually committed and a thought never is.
   * Nothing here is in the durable log, so a reload draws none of it and the
   * equivalence rule holds without anything having to be remembered.
   *
   * A reader who saw reasoning and reloaded to find it gone will otherwise
   * report data loss, which is why §18.8 says so on the surface rather than
   * only here.
   */
  readonly reasoning?: { readonly text: string };
}

export interface Usage {
  /**
   * What the turn consumed in total, which is what §18.5 asks be shown.
   *
   * Derived from the two halves rather than reported as one: the service
   * reports them separately and a reader watching a turn wants the figure,
   * not a breakdown to add up themselves. Absent until at least one half has
   * arrived — absence is a fact, never a zero.
   */
  readonly tokens?: number;
  /** Tokens the turn spent reading, as the service reports them. */
  readonly inputTokens?: number;
  /** Tokens the turn produced. */
  readonly outputTokens?: number;
  /** How full the context is, where the runtime says. */
  readonly contextUsed?: number;
  /** What it cost, where the runtime reports a figure. Never estimated. */
  readonly cost?: string;
}

/** One step of the agent's plan, in the kernel's own words. */
export interface PlanStep {
  readonly title: string;
  readonly status: string;
}

/** One setting the kernel says is settable, in its own words. */
export interface ConfigOption {
  readonly id: string;
  readonly name: string;
  readonly kind: string;
}

/** One action the owner may invoke, in the kernel's own words. */
export interface AvailableCommand {
  readonly name: string;
  readonly description: string;
}

export const EMPTY: Conversation = { entries: [], turnInFlight: false };
