import type {
  ChatHostFailure as SdkChatHostFailure,
  ChatSettings,
  StoredChatSettings,
} from '@chronoai/cma-sdk';

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

/** One deployment-managed shared chat runtime in the operator inventory. */
export type ChatHostFailure = SdkChatHostFailure;

export interface ChatHost {
  readonly id: string;
  readonly owner: string;
  readonly status: 'provisioning' | 'active' | 'pausing' | 'paused' | 'reviving' | 'retired';
  readonly threadCount: number;
  readonly runtimeVersion?: string;
  readonly pausedAt?: number;
  readonly revivedAt?: number;
  readonly checkpointCapturedAt?: number;
  readonly lastFailure?: ChatHostFailure;
  readonly settingsRevision: number;
  readonly createdAt: number;
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

/** Editable prose surrounding the generated granted-services list. */
export interface SessionBriefing {
  readonly before: readonly string[];
  readonly reachHeading: string;
  readonly reachLead: string;
  readonly reachClose: string;
  readonly reachNone: string;
  readonly after: readonly string[];
}

/** Inclusive bounds for a size a caller states explicitly. */
export interface CapacityBounds {
  readonly minCpuMillis: number;
  readonly maxCpuMillis: number;
  readonly minMemoryMib: number;
  readonly maxMemoryMib: number;
}

/** The numeric cost assigned to a session with no recorded size. */
export interface AssumedSessionSize {
  readonly cpuMillis: number;
  readonly memoryMib: number;
}

/** Totals this deployment declares it is prepared to reserve. */
export interface DeploymentCapacity {
  /** `null` leaves the CPU dimension undeclared. Zero is an explicit ceiling. */
  readonly cpuMillis: number | null;
  /** `null` leaves the memory dimension undeclared. Zero is an explicit ceiling. */
  readonly memoryMib: number | null;
  /** `null` leaves any ledger containing an unsized session incomplete. */
  readonly assumedSessionSize: AssumedSessionSize | null;
}

/** Deployment-wide settings as the operator surface edits them. */
export interface PlatformSettings {
  readonly briefing: SessionBriefing;
  readonly deploymentCapacity: DeploymentCapacity;
  readonly sizeBounds: CapacityBounds;
  /** `null` means the deployment imposes no per-subject live-session limit. */
  readonly liveSessions: number | null;
}

/** A settings snapshot and the revision a conditional write must name. */
export interface StoredPlatformSettings {
  readonly revision: number;
  readonly settings: PlatformSettings;
  /** The complete object replaced by the latest write, when one exists. */
  readonly previous?: PlatformSettings;
}

/** Somebody else wrote platform settings after this editor read them. */
export class PlatformSettingsConflict extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PlatformSettingsConflict';
  }
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
  /** Every deployment-managed shared chat host, newest first. */
  chatHosts(): Promise<readonly ChatHost[]>;
  /** Release a session's runtime, keeping the session. */
  stopSession(id: string): Promise<void>;
  /** End a session. */
  terminateSession(id: string): Promise<void>;
  /** Retire a shared chat host without deleting its retained histories. */
  retireChatHost(id: string): Promise<void>;
  /** Read deployment-wide settings and their conditional-write revision. */
  readPlatformSettings(): Promise<StoredPlatformSettings>;
  /** Replace all settings, provided nobody has written the named revision. */
  changePlatformSettings(
    expectedRevision: number,
    settings: PlatformSettings,
  ): Promise<StoredPlatformSettings>;
  /** Restore shipped briefing prose and remove Chat, preserving sealed and capacity policy. */
  resetPlatformSettings(expectedRevision: number): Promise<StoredPlatformSettings>;
  /** Read deployment-owned chat settings and their conditional-write revision. */
  readChatSettings(): Promise<StoredChatSettings>;
  /** Replace or remove only chat settings under the platform revision CAS. */
  changeChatSettings(
    expectedRevision: number,
    settings: ChatSettings | null,
  ): Promise<StoredChatSettings>;
}

export type { ChatSettings, StoredChatSettings };
