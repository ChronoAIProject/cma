export interface SignupOptions {
  readonly socialProviders: readonly string[];
  readonly emailSignup: boolean;
  readonly inviteCodeRequired: boolean;
  readonly registerAvailable: boolean;
}

export async function readSignupOptions(signal?: AbortSignal): Promise<SignupOptions> {
  const response = await fetch('/auth/signup-options', {
    credentials: 'include',
    signal: AbortSignal.any([AbortSignal.timeout(8000), ...(signal === undefined ? [] : [signal])]),
  });
  if (!response.ok) throw new Error('Account options are unavailable.');
  const value: unknown = await response.json();
  if (typeof value !== 'object' || value === null)
    throw new Error('Account options are unavailable.');
  const body = value as Record<string, unknown>;
  if (
    !Array.isArray(body.social_providers) ||
    !body.social_providers.every((item) => typeof item === 'string') ||
    typeof body.email_signup !== 'boolean' ||
    typeof body.invite_code_required !== 'boolean' ||
    typeof body.register_available !== 'boolean'
  )
    throw new Error('Account options are unavailable.');
  return {
    socialProviders: body.social_providers as string[],
    emailSignup: body.email_signup,
    inviteCodeRequired: body.invite_code_required,
    registerAvailable: body.register_available,
  };
}
