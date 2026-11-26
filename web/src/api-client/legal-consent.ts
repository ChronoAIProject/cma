import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { input, read } from './resources.js';
import { nullable, object, text, type Output } from './schema.js';

const consent = object({
  version: text,
  terms_url: text,
  privacy_url: text,
  accepted_at: nullable(text),
});
const acceptance = object({ version: text });
export type LegalConsent = Output<typeof consent>;
export interface LegalConsentAcceptance {
  readonly version: string;
}

/** The verified owner's acceptance of the deployment's published terms. */
export class LegalConsents {
  constructor(private readonly config: ResolvedConfig) {}

  status(options: OperationOptions = {}): Promise<LegalConsent> {
    return read(this.config, '/api/v1/account/legal-consent', consent, options);
  }

  accept(body: LegalConsentAcceptance, options: OperationOptions = {}): Promise<LegalConsent> {
    input(acceptance, body);
    return read(this.config, '/api/v1/account/legal-consent', consent, {
      ...options,
      method: 'PUT',
      body,
    });
  }
}
