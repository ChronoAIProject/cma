import type { KernelOffer, SessionPlane } from './types';

/**
 * What a deployment can run, in the words a person reads.
 *
 * The identifiers and the planes come from `GET /api/v1/kernels`; this file
 * adds nothing to them but labels and a sentence for a refusal. Deliberately:
 * a list of runtimes kept in the client is wrong the first time a deployment
 * changes, and being wrong here means offering a session the control plane
 * will refuse.
 */

/** The two planes, in the order a form offers them. */
export const PLANES: readonly { readonly id: SessionPlane; readonly label: string }[] = [
  { id: 'terminal', label: 'Terminal' },
  { id: 'conversation', label: 'Conversation' },
];

const NAMES: Readonly<Record<string, string>> = {
  codex: 'Codex',
  // The control plane registers this one as `claude_code`; the hyphenated
  // spelling is kept beside it because both have been seen on the wire and a
  // label is not the place to argue about which is canonical.
  claude_code: 'Claude Code',
  'claude-code': 'Claude Code',
  dsh: 'DeepSeek Harness',
};

/**
 * What a runtime is called.
 *
 * Falls back to the identifier, so a deployment running something this build
 * has never heard of offers it under its own name rather than under a blank.
 */
export function kernelLabel(id: string): string {
  return NAMES[id] ?? id;
}

/**
 * What this runtime can be, or `undefined` where nothing is known about it.
 *
 * The two are different answers and are kept apart everywhere below: a list
 * that could not be read refuses nothing, and a list that was read refuses
 * exactly what it does not contain. Treating the first as the second would
 * empty a form because a request failed.
 */
export function modesOf(
  kernels: readonly KernelOffer[],
  id: string,
): readonly SessionPlane[] | undefined {
  if (kernels.length === 0) {
    return undefined;
  }
  return kernels.find((kernel) => kernel.id === id)?.modes ?? [];
}

/**
 * Why this runtime cannot serve this plane, or `undefined` where it can.
 *
 * Names the plane, per `Web_Style.md` §18.7 and `API_Overview.md` rule 4: the
 * refusal a person reads is the ability that is missing, not "that did not
 * work". It is computed while they are still choosing, which is the whole
 * reason the deployment publishes what each runtime can be.
 */
export function planeRefusal(
  kernels: readonly KernelOffer[],
  id: string,
  plane: SessionPlane,
): string | undefined {
  const modes = modesOf(kernels, id);
  if (modes === undefined || modes.includes(plane)) {
    return undefined;
  }
  const other = PLANES.filter((p) => modes.includes(p.id)).map((p) => p.label.toLowerCase());
  const alternative =
    other.length === 0 ? 'It serves no session plane at all.' : `It serves ${other.join(' and ')}.`;
  return `${kernelLabel(id)} does not advertise “${plane}”. ${alternative}`;
}
