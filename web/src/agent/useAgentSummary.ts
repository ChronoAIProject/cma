import { useEffect, type Dispatch, type SetStateAction } from 'react';
import type { AgentDetail, AgentSummary } from '../api/types';
import type { AgentResourceState } from './controllerTypes';
import { sameAgent } from './status';
import { mergeRuntimeMetadata } from './runtimeMetadata';

/** Refresh navigation metadata without replacing the retained conversation. */
export function useAgentSummary(
  agentId: string | undefined,
  detail: AgentDetail | undefined,
  summary: AgentSummary | undefined,
  setState: Dispatch<SetStateAction<AgentResourceState>>,
) {
  useEffect(() => {
    if (summary === undefined || detail === undefined || summary.id !== agentId) return;
    let cancelled = false;
    // Reuse fresh tree metadata without reloading or replacing conversation history.
    queueMicrotask(() => {
      if (cancelled) return;
      setState((current) => {
        if (
          current.status !== 'ready' ||
          !sameAgent(current.detail, summary) ||
          !(Date.parse(summary.updatedAt) >= Date.parse(current.detail.updatedAt))
        )
          return current;
        const status =
          Date.parse(summary.updatedAt) > Date.parse(current.detail.updatedAt)
            ? summary.status
            : current.detail.status;
        const title = summary.title?.trim() ? summary.title : current.detail.title;
        const runtimeMetadata = mergeRuntimeMetadata(
          current.detail.runtimeMetadata,
          summary.runtimeMetadata,
        );
        if (
          current.detail.status === status &&
          current.detail.title === title &&
          current.detail.runtimeMetadata === runtimeMetadata
        )
          return current;
        return {
          ...current,
          detail: {
            ...current.detail,
            status,
            title,
            runtimeMetadata,
            updatedAt: summary.updatedAt,
          },
        };
      });
    });
    return () => {
      cancelled = true;
    };
  }, [agentId, detail, summary, setState]);
}
