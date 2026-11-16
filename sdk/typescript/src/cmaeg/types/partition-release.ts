/** Exact owner-scoped partition head; inspection grants no release authority. */
export interface HeadPreview {
  partition_id: string;
  head: string;
  epoch: number;
  version: number;
  phase: string;
  event_expires_at: number;
  releasable: boolean;
}

/** Release requires an explicit recovery grant and the current head proof. */
export interface ReleaseRequest {
  partition_id: string;
  expected_head: string;
  expected_epoch: number;
  expected_version: number;
  operation_id: string;
  provider_terminal_confirmed: boolean;
  suppress_delayed_replies: boolean;
}

/** An audited disposition preserves uncertainty; it is not delivery evidence. */
export interface ReleaseResult {
  operation_id: string;
  released_head: string;
  disposition: string;
  epoch: number;
  released_at_ms: number;
}
