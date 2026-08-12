/** A public interface exposed by the chrono-bot control plane. */
export interface PublicInterface {
  readonly id: string;
  readonly priority: number;
}

/** Mirrors the Layer 1 interface priority order defined in CLAUDE.md. */
export const interfaceCatalogue: readonly PublicInterface[] = [
  { id: 'agui', priority: 1 },
  { id: 'rest', priority: 2 },
  { id: 'pty', priority: 3 },
  { id: 'mcp', priority: 4 },
  { id: 'a2a', priority: 5 },
];
