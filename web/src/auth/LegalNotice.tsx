import type { LegalTerms } from './types';

/** The sentence that accompanies every way into an account when terms are published. */
export function LegalNotice({
  terms,
  className,
}: {
  readonly terms: LegalTerms;
  readonly className?: string;
}) {
  return (
    <p className={className}>
      By continuing, you agree to the{' '}
      <a href={terms.termsUrl} target="_blank" rel="noopener noreferrer">
        Terms of Service
      </a>{' '}
      and acknowledge the{' '}
      <a href={terms.privacyUrl} target="_blank" rel="noopener noreferrer">
        Privacy Notice
      </a>
      .
    </p>
  );
}
