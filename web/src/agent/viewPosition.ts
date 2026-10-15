/** Ephemeral layout state; owned by one mounted account's Console. */
export interface AgentViewPosition {
  top: number;
  pinned: boolean;
}

export class AgentViewPositions {
  private readonly values = new Map<string, AgentViewPosition>();
  forAgent(id: string | undefined): AgentViewPosition | undefined {
    if (id === undefined) return undefined;
    const value = this.values.get(id) ?? { top: 0, pinned: true };
    this.values.delete(id);
    this.values.set(id, value);
    if (this.values.size > 6) this.values.delete(this.values.keys().next().value!);
    return value;
  }
}
