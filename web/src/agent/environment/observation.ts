import { useSyncExternalStore } from 'react';
import type { ToolCall } from '../model';

interface OperationObservation {
  readonly toolCalls: readonly ToolCall[];
  readonly activeRunId?: string;
}

export class AgentComputerObservation {
  constructor(
    readonly scope: {
      readonly client: object;
      readonly subject: string;
      readonly agentId?: string;
    },
  ) {}

  private value: OperationObservation | undefined;
  private readonly listeners = new Set<() => void>();

  readonly snapshot = () => this.value;
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  publish(toolCalls: readonly ToolCall[] | undefined, activeRunId?: string) {
    if (this.value?.toolCalls === toolCalls && this.value?.activeRunId === activeRunId) return;
    this.value = toolCalls === undefined ? undefined : { toolCalls, activeRunId };
    for (const listener of this.listeners) listener();
  }
}

const empty = () => undefined;
const idle = () => () => {};

export function useComputerObservation(observation?: AgentComputerObservation) {
  return useSyncExternalStore(observation?.subscribe ?? idle, observation?.snapshot ?? empty);
}
