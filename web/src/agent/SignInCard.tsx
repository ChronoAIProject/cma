import { useState } from 'react';
import type { RecoveryNotice } from './model';
import { Button } from '../ui/Button';
import { IconCopyOutline16, IconRightUpOutline16 } from '../ui/icons';
import './SignInCard.css';

function signInUrl(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  try {
    const url = new URL(value);
    return ['https:', 'http:'].includes(url.protocol) && url.username === '' && url.password === ''
      ? url.href
      : undefined;
  } catch {
    return undefined;
  }
}

export function SignInCard({ notice }: { readonly notice: RecoveryNotice }) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  const url = signInUrl(notice.url);
  const copy = async () => {
    try {
      if (notice.code === undefined || navigator.clipboard === undefined)
        throw new Error('Clipboard unavailable.');
      await navigator.clipboard.writeText(notice.code);
      setCopied(true);
      setCopyError(false);
    } catch {
      setCopyError(true);
    }
  };
  return (
    <section className="agent-sign-in" aria-label="Sign in to your account">
      <h2>Sign in to your account</h2>
      <p>{notice.detail}</p>
      <span>Needs you</span>
      {notice.code === undefined ? null : (
        <div className="agent-sign-in__code">
          <code>{notice.code}</code>
          <Button
            size="sm"
            icon={<IconCopyOutline16 />}
            aria-label="Copy sign-in code"
            title="Copy sign-in code"
            onClick={() => void copy()}
          />
        </div>
      )}
      {copied ? <p role="status">Copied</p> : null}
      {copyError ? <p role="alert">The code could not be copied.</p> : null}
      {url === undefined ? null : (
        <a href={url} target="_blank" rel="noreferrer noopener">
          Continue sign in <IconRightUpOutline16 />
        </a>
      )}
    </section>
  );
}
