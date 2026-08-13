/**
 * The operator surface, as the console models it.
 *
 * A projection of what `/api/v1/admin/` reports, for display. It invents
 * nothing the control plane does not say and enforces nothing the control plane
 * does not enforce: every read and every control here is refused server-side on
 * its own capability, whatever this code renders.
 *
 * Two shapes deserve attention. `ProviderReport` is a union rather than a flag
 * beside an array, so a component has to acknowledge "nobody could be asked" in
 * order to compile — an empty list would otherwise be drawn as an idle
 * namespace, which is the misreading the whole inventory contract exists to
 * prevent. And an action or a session state arrives as a plain string, because
 * an operator console that refused to draw a row naming something it had not
 * been taught would hide exactly the row worth looking at.
 */

/** Somebody who has used the service. */
export interface Person {
  /** The authenticated principal. Opaque, and never how they are shown. */
  readonly subject: string;
  /** The account name the provider released, when it released one. */
  readonly displayName?: string;
  /** The account's email address, when the provider released it. */
  readonly email?: string;
  /** When this subject was first seen, milliseconds since the epoch. */
  readonly firstSeen?: number;
  /** When this subject was last seen. */
  readonly lastSeen?: number;
}

/** One allowlisted parameter of a recorded action. */
export interface DetailPair {
  readonly key: string;
  readonly value: string;
}

/** What an action was taken on. */
export interface ActivityTarget {
  readonly kind: string;
  readonly id: string;
}

/** One thing the service was asked to do. */
export interface ActivityEntry {
  readonly id: string;
  /**
   * When it happened, milliseconds since the epoch.
   *
   * Absent when the control plane sent a timestamp that could not be read.
   * Rendering "Invalid Date" in a record somebody is auditing is worse than
   * rendering nothing.
   */
  readonly at?: number;
  /** Who asked. Absent when the request carried no established identity. */
  readonly actor?: string;
  /** What they asked for, in the core's vocabulary. */
  readonly action: string;
  readonly target?: ActivityTarget;
  readonly outcome: string;
  /**
   * The parameters that were kept. Never a secret: an allowlist in the core
   * decides what may be recorded, and it has no key for a value, a credential,
   * a token or a header.
   */
  readonly detail: readonly DetailPair[];
}

/** Which entries a read of the record wants. */
export interface ActivityQuery {
  /** Narrow to one subject. The control plane does the narrowing. */
  readonly actor?: string;
  /** Most entries to return. The control plane clamps this. */
  readonly limit?: number;
}

/** A session, whoever owns it. */
export interface AdminSession {
  readonly id: string;
  /** The owning subject. Opaque; `ownerLabel` is what a screen draws. */
  readonly owner: string;
  /** How the owner should be shown, when the directory knows a name. */
  readonly ownerLabel?: string;
  /** Lifecycle state, in the control plane's vocabulary. */
  readonly status: string;
  readonly recoveryState?: string;
  readonly stopReason?: string;
  /** Which runtime implementation is serving it. */
  readonly kernel: string;
  readonly createdAt?: number;
  readonly runtimeVersion?: string;
}

/** One runtime in service. */
export interface RuntimeEntry {
  /** The provider's own identifier for it. Opaque; never parsed. */
  readonly runtime: string;
  readonly session?: string;
  readonly owner?: string;
  readonly status?: string;
  /** Whether nothing in the record accounts for this runtime. */
  readonly unaccounted: boolean;
}

/**
 * What one provider had to say.
 *
 * A union, deliberately: a provider that could not be asked has no runtime list
 * at all, so there is no empty array for a component to mistake for an idle
 * namespace.
 */
export type ProviderReport =
  | {
      readonly kernel: string;
      readonly observable: true;
      readonly runtimes: readonly RuntimeEntry[];
    }
  | { readonly kernel: string; readonly observable: false; readonly reason?: string };

/** Everything in service right now. */
export interface RuntimeInventory {
  /**
   * How many runtimes are in service, when that can be said at all.
   *
   * Absent as soon as one provider could not be asked. A partial total
   * presented as a total is the same lie as an empty list.
   */
  readonly running?: number;
  /** Whether every provider answered. */
  readonly fullyObserved: boolean;
  readonly providers: readonly ProviderReport[];
}

/**
 * Where the operator surface's data comes from.
 *
 * A port, mirroring the auth and environment-profile seams: components consume
 * this and never a concrete implementation, so the in-memory store the panel is
 * tested against and the REST-backed one are interchangeable without touching a
 * single consumer.
 *
 * Implementations must not throw. A failure is reported by rejecting with an
 * Error whose message is fit to show a person.
 */
export interface OperatorStore {
  /** Everybody who has used the service. */
  people(): Promise<readonly Person[]>;
  /** What the service was asked to do, newest first. */
  activity(query: ActivityQuery): Promise<readonly ActivityEntry[]>;
  /** What every provider is running, joined against the record. */
  runtimes(): Promise<RuntimeInventory>;
  /** Every session, whoever owns it. */
  sessions(): Promise<readonly AdminSession[]>;
  /** Release a session's runtime, keeping the session. */
  stopSession(id: string): Promise<void>;
  /** End a session. */
  terminateSession(id: string): Promise<void>;
}
