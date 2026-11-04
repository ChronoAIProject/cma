import { createContext } from 'react';
import type { Cma } from '@chronoai/cma-sdk';

export interface AnswerDeliveryAccess {
  readonly agentId: string;
  readonly viewerId: string;
  readonly client: Pick<
    Cma['agents'],
    'answerDeliverables' | 'readAnswerDeliverable' | 'downloadAnswerDeliverable'
  >;
  readonly authority?: unknown;
}

export const AnswerDeliveryContext = createContext<AnswerDeliveryAccess | undefined>(undefined);

export function answerProse(
  content: string,
  streaming = false,
): { content: string; references: number } {
  if (!content.includes('<!--cma-deliverable:')) return { content, references: 0 };
  let fence: { character: string; width: number } | undefined;
  const markers = new Set<string>();
  const lines = content.split('\n').filter((line) => {
    const opening = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
    if (opening) {
      const delimiter = opening[1]!;
      if (!fence) fence = { character: delimiter[0]!, width: delimiter.length };
      else if (
        fence.character === delimiter[0] &&
        delimiter.length >= fence.width &&
        opening[2]!.trim() === ''
      )
        fence = undefined;
      return true;
    }
    if (!fence && /^<!--cma-deliverable:v1:[A-Za-z0-9_-]{1,4096}-->\r?$/.test(line)) {
      markers.add(line);
      return false;
    }
    if (!fence && streaming && /^<!--cma-deliverable:v1:[A-Za-z0-9_-]{0,4096}>?$/.test(line))
      return false;
    return true;
  });
  return { content: lines.join('\n'), references: Math.min(markers.size, 32) };
}
