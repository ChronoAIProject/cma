export const feedbackReasons = [
  'inaccurate',
  'unanswered',
  'unusable_steps',
  'incomplete',
  'unclear',
  'other',
] as const;
export type FeedbackReason = (typeof feedbackReasons)[number];
export interface FeedbackReasonRequest {
  readonly expected_version: number;
  readonly request_id: string;
  readonly reasons: readonly FeedbackReason[];
  readonly comment: string;
}
export interface FeedbackDetails {
  readonly viewer_id: string;
  readonly agent_id: string;
  readonly message_id: string;
  readonly vote: 'up' | 'down' | null;
  readonly version: number;
  readonly reasons: readonly FeedbackReason[];
  readonly comment: string;
  readonly updated_at: string | null;
  readonly request_id: string | null;
}
