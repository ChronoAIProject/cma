import { useSyncExternalStore } from 'react';
import type { ToolCall } from '../model';
import { FileNavigationMemory } from '../files/useFileNavigation';

interface OperationObservation {
  readonly toolCalls: readonly ToolCall[];
  readonly activeRunId?: string;
  readonly historyLoaded: boolean;
}

/** The retained conversation supplies Terminal output without another event subscription. */
export class AgentComputerObservation {
  constructor(
    readonly scope: {
      readonly client: object;
      readonly subject: string;
      readonly agentId?: string;
      readonly authEpoch?: number;
    },
    private readonly openTerminal?: () => void,
    private readonly closeComputer?: () => void,
  ) {}
  readonly fileNavigation = new FileNavigationMemory();
  private value: OperationObservation | undefined;
  private readonly listeners = new Set<() => void>();
  private terminal: { readonly id: string; readonly revision: number } | undefined;
  private readonly terminalListeners = new Set<() => void>();
  private conversationFocus?: () => void;
  private readonly conversationListeners = new Set<() => void>();
  readonly conversationSnapshot = () => this.conversationFocus;
  readonly subscribeConversation = (listener: () => void) => {
    this.conversationListeners.add(listener);
    return () => {
      this.conversationListeners.delete(listener);
    };
  };
  offerConversationFocus(focus: () => void) {
    this.conversationFocus = focus;
    for (const listener of this.conversationListeners) listener();
    return () => {
      if (this.conversationFocus !== focus) return;
      this.conversationFocus = undefined;
      for (const listener of this.conversationListeners) listener();
    };
  }
  readonly returnToConversation = () => {
    const focus = this.conversationFocus;
    if (!focus) return;
    this.closeComputer?.();
    requestAnimationFrame(() => {
      if (this.conversationFocus === focus) focus();
    });
  };
  readonly snapshot = () => this.value;
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  readonly terminalSnapshot = () => this.terminal;
  readonly subscribeTerminal = (listener: () => void) => {
    this.terminalListeners.add(listener);
    return () => {
      this.terminalListeners.delete(listener);
    };
  };
  readonly selectTerminal = (id: string) => {
    this.terminal = { id, revision: (this.terminal?.revision ?? 0) + 1 };
    this.openTerminal?.();
    for (const listener of this.terminalListeners) listener();
  };
  publish(toolCalls: readonly ToolCall[] | undefined, activeRunId?: string, historyLoaded = true) {
    if (
      this.value?.toolCalls === toolCalls &&
      this.value?.activeRunId === activeRunId &&
      this.value?.historyLoaded === historyLoaded
    )
      return;
    this.value = toolCalls === undefined ? undefined : { toolCalls, activeRunId, historyLoaded };
    for (const listener of this.listeners) listener();
  }
}
const empty = () => undefined;
const idle = () => () => {};
export function useComputerObservation(observation?: AgentComputerObservation) {
  return useSyncExternalStore(observation?.subscribe ?? idle, observation?.snapshot ?? empty);
}
export function useTerminalSelection(observation?: AgentComputerObservation) {
  return useSyncExternalStore(
    observation?.subscribeTerminal ?? idle,
    observation?.terminalSnapshot ?? empty,
  );
}
export function useConversationReturn(observation?: AgentComputerObservation) {
  const focus = useSyncExternalStore(
    observation?.subscribeConversation ?? idle,
    observation?.conversationSnapshot ?? empty,
  );
  return focus ? observation?.returnToConversation : undefined;
}
