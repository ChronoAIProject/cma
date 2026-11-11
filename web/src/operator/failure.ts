import { ApiError, ValidationError } from '@chronoai/cma-sdk';

export interface OperatorFailure {
  readonly message: string;
  readonly code?: string;
  readonly requestId?: string;
  readonly field?: string;
  readonly conflict: boolean;
  readonly uncertain: boolean;
}

export function operatorFailure(error: unknown): OperatorFailure {
  if (error instanceof ApiError) {
    const messages: Readonly<Record<string, string>> = {
      not_an_operator: 'This account does not have the required operator capability.',
      forbidden: 'This account does not have permission for this operation.',
      sandbox_not_found: 'This Sandbox is no longer available to this account.',
      profile_not_found: 'This profile is no longer available to this account.',
      profile_protected: 'Deployment-owned profiles cannot be retired.',
      operator_conflict:
        'The resource changed before the operation completed. Refresh its current state.',
      settings_conflict: 'Platform settings changed. Your draft has been retained.',
      model_budget_writes_unavailable:
        'Saving spend controls is not available yet. Your draft has been retained.',
      invalid_request: 'Review the settings fields before saving again.',
      invalid_admin_request: 'Review the selected filters before trying again.',
      marketplace_disabled: 'The marketplace is disabled. The current default is retained.',
    };
    return {
      message: messages[error.code ?? ''] ?? error.message,
      code: error.code,
      requestId: error.requestId,
      field: error.path ?? error.field,
      conflict: error.status === 409,
      uncertain: error.status === 408 || error.status >= 500,
    };
  }
  return {
    message:
      error instanceof ValidationError
        ? error.message
        : 'The operation could not be confirmed. Refresh the current state before another change.',
    conflict: false,
    uncertain: !(error instanceof ValidationError),
  };
}
