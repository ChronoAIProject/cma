import type { StoredPlatformSettings } from '@cma/client';

export type SpendAvailability = Pick<
  StoredPlatformSettings,
  | 'model_budget_writes_available'
  | 'model_request_budget_writes_available'
  | 'platform_spend_writes_available'
>;

export function spendAvailability(availability: SpendAvailability, budget: unknown) {
  const policy =
    typeof budget === 'object' && budget !== null && !Array.isArray(budget)
      ? (budget as Record<string, unknown>)
      : undefined;
  const models = policy?.request_models;
  const hasRequestModels = models !== undefined && (!Array.isArray(models) || models.length > 0);
  const platform = policy?.platform;
  const hasCloud =
    typeof platform === 'object' &&
    platform !== null &&
    (platform as Record<string, unknown>).cloud != null;
  // Mirrors BudgetSettings::is_empty_disabled. Disabled retained values still
  // require compatible readers, including on unrelated full-settings saves.
  const empty =
    budget === undefined ||
    (!!policy &&
      (policy.enabled === false || policy.enabled === undefined) &&
      policy.platform == null &&
      policy.default_policy == null &&
      Array.isArray(policy.owners) &&
      policy.owners.length === 0 &&
      !hasRequestModels);
  const edit =
    availability.model_budget_writes_available === true &&
    (!hasRequestModels || availability.model_request_budget_writes_available === true) &&
    (!hasCloud || availability.platform_spend_writes_available === true);
  return { edit, save: empty || edit };
}
