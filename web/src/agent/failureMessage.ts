type FailureCategory =
  | 'upstream_unavailable'
  | 'upstream_timeout'
  | 'insufficient_credits'
  | 'permission_denied'
  | 'agent_log_capacity_exceeded'
  | 'agent_turn_timeout'
  | 'approval_timeout'
  | 'approval_denied'
  | 'approval_expired'
  | 'approval_wait_disconnected';

export interface FailureMessage {
  readonly detail: string;
  readonly nextStep: string;
  readonly technicalDetails?: {
    readonly category: FailureCategory;
    readonly httpStatus?: 402 | 503 | 524;
  };
}

const nextStep =
  'A new message continues this Agent conversation; it does not automatically replay the failed run. Repeating a task may repeat effects that already occurred.';

// Only normalized classes select copy. Upstream bodies are never presentation data,
// even when they contain a recognizable status or appear to be plain text.
export function failureMessage(category: unknown): FailureMessage {
  switch (category) {
    case 'agent_turn_timeout':
      return {
        detail: 'This turn reached its time limit and has ended.',
        nextStep,
        technicalDetails: { category },
      };
    case 'approval_timeout':
      return {
        detail: 'The approval wait timed out. This turn has ended.',
        nextStep,
        technicalDetails: { category },
      };
    case 'approval_denied':
      return {
        detail: 'Approval was denied. This turn has ended.',
        nextStep,
        technicalDetails: { category },
      };
    case 'approval_expired':
      return {
        detail: 'The approval request expired. This turn has ended.',
        nextStep,
        technicalDetails: { category },
      };
    case 'approval_wait_disconnected':
      return {
        detail: 'The approval request disconnected and could not continue. This turn has ended.',
        nextStep,
        technicalDetails: { category },
      };
    case 'upstream_unavailable':
      return {
        detail:
          'The Agent run failed because the upstream service is temporarily unavailable (503). Task effects may be incomplete. Review the conversation before sending another message.',
        nextStep,
        technicalDetails: { category, httpStatus: 503 },
      };
    case 'upstream_timeout':
      return {
        detail:
          'The Agent run failed because the upstream request timed out (524). Task effects may be incomplete. Review the conversation before sending another message.',
        nextStep,
        technicalDetails: { category, httpStatus: 524 },
      };
    case 'insufficient_credits':
      return {
        detail:
          'The Agent run ended because the model billing account has insufficient credits (402). Earlier files, tool results, and partial output remain available. Ask the Sandbox owner to top up the account used for model requests, then send a new message to continue.',
        nextStep,
        technicalDetails: { category, httpStatus: 402 },
      };
    case 'permission_denied':
      return {
        detail:
          'The Agent run failed because permission was denied. Review the required access with the Sandbox owner and check the conversation before sending another message.',
        nextStep,
        technicalDetails: { category },
      };
    case 'agent_log_capacity_exceeded':
      return {
        detail:
          'The Agent run failed because its conversation history reached a storage limit. Any available retained partial output remains with this turn. Review the conversation before sending another message.',
        nextStep,
        technicalDetails: { category },
      };
    default:
      return {
        detail:
          'The Agent run failed. Its delivery or task effects may be incomplete. Review the conversation before sending another message.',
        nextStep,
      };
  }
}
