/**
 * What a session is created with.
 *
 * Mirrors the control plane's own vocabulary rather than any one agent's, so a
 * second kernel can be offered without renaming what a person chose. The labels
 * here are the only place these become words a person reads.
 */

/** How much deliberation to ask of the model. */
export type ReasoningEffort = 'minimal' | 'low' | 'medium' | 'high' | 'xhigh';

/** How much the agent may do without asking. */
export type PermissionMode = 'guarded' | 'autonomous' | 'unrestricted';

/** Whose credential pays for model calls. */
export type CredentialMode = 'delegated' | 'own';

export interface AgentOptions {
  readonly model?: string;
  readonly reasoning_effort: ReasoningEffort;
  readonly permission: PermissionMode;
  readonly credential: CredentialMode;
}

/**
 * What a session gets when nobody chooses.
 *
 * Guarded on purpose: acting without asking should be something a person
 * selected, never something they received by leaving a form alone.
 */
export const DEFAULT_OPTIONS: AgentOptions = {
  reasoning_effort: 'medium',
  permission: 'guarded',
  credential: 'delegated',
};

/** The kernels on offer, and whether they can be chosen yet. */
export interface KernelChoice {
  readonly id: string;
  readonly label: string;
  /** Shown but unselectable, with a reason. */
  readonly unavailable?: string;
}

export const KERNELS: readonly KernelChoice[] = [
  { id: 'codex', label: 'Codex' },
  // Listed so the shape of the product is visible — this is a service that
  // runs kernels, not one that runs a kernel — and disabled because nothing
  // behind it works yet. Offering it as selectable would be a lie.
  { id: 'claude-code', label: 'Claude Code', unavailable: 'Not available yet' },
];

export const EFFORTS: readonly { id: ReasoningEffort; label: string; hint: string }[] = [
  { id: 'minimal', label: 'Minimal', hint: 'Fastest, for mechanical work' },
  { id: 'low', label: 'Low', hint: '' },
  { id: 'medium', label: 'Medium', hint: 'Default' },
  { id: 'high', label: 'High', hint: '' },
  { id: 'xhigh', label: 'Extra high', hint: 'Slowest, for the hardest problems' },
];

export const PERMISSIONS: readonly {
  id: PermissionMode;
  label: string;
  hint: string;
  /** Whether choosing this warrants a visible warning. */
  dangerous?: boolean;
}[] = [
  {
    id: 'guarded',
    label: 'Ask first',
    hint: 'Confirms before acting outside the workspace',
  },
  {
    id: 'autonomous',
    label: 'Work unattended',
    hint: 'Edits freely in the workspace, asks when something fails',
  },
  {
    id: 'unrestricted',
    label: 'Full access',
    hint: 'No approvals and no confinement. The sandbox is still the outer boundary.',
    dangerous: true,
  },
];

export const CREDENTIALS: readonly { id: CredentialMode; label: string; hint: string }[] = [
  { id: 'delegated', label: 'ChronoAI', hint: 'Uses your platform access; nothing to set up' },
  {
    id: 'own',
    label: 'My own account',
    hint: 'Sign in to your own subscription inside the session',
  },
];

/** Whether a chosen mode should be shown as the exceptional one it is. */
export function isDangerous(permission: PermissionMode): boolean {
  return permission === 'unrestricted';
}
