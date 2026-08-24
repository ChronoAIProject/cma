/** A public interface exposed by the CMA control plane. */
export interface PublicInterface {
  readonly id: string;
  readonly priority: number;
  /** How the interface is written in the UI. */
  readonly label: string;
}

/** Mirrors the Layer 1 interface priority order defined in CLAUDE.md. */
export const interfaceCatalogue: readonly PublicInterface[] = [
  { id: 'agui', priority: 1, label: 'ag-ui' },
  { id: 'rest', priority: 2, label: 'rest' },
  { id: 'pty', priority: 3, label: 'pty' },
  { id: 'mcp', priority: 4, label: 'mcp' },
  { id: 'a2a', priority: 5, label: 'a2a' },
];

/** The interface list as the landing terminal prints it, in priority order. */
export function interfaceLine(): string {
  return [...interfaceCatalogue]
    .sort((a, b) => a.priority - b.priority)
    .map((i) => i.label)
    .join(' · ');
}
