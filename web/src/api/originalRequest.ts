export type OriginalRequestRead =
  | {
      readonly kind: 'ready';
      readonly agentId: string;
      readonly text: string;
      readonly source: 'admitted-first-task';
      readonly turnSequence: number | null;
    }
  | { readonly kind: 'unavailable'; readonly agentId: string };
