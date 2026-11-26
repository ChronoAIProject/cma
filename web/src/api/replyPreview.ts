export interface AssistantReplyPreview {
  readonly text: string;
  readonly truncated: boolean;
  readonly messageSequence: number;
  readonly turnSequence: number;
  readonly appendedAt: string;
}

export type ReplyPreview =
  | {
      readonly kind: 'ready';
      readonly agentId: string;
      readonly sourceThroughSeq: number;
      readonly reply: AssistantReplyPreview;
    }
  | { readonly kind: 'none'; readonly agentId: string; readonly sourceThroughSeq: number }
  | {
      readonly kind: 'unavailable';
      readonly agentId: string;
      readonly sourceThroughSeq: number | null;
    };
