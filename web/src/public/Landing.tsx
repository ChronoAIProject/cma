import { useEffect, useState } from 'react';
import type { TriggerPreview } from '@cma/client';
import { readSignupOptions, type SignupOptions } from '../auth/signup';
import { Button } from '../ui/Button';
import { LegalNotice } from '../auth/LegalNotice';
import type { LegalTerms } from '../auth/types';
import { buttonNames, publicCopy, signupSentence } from '../vocabulary/needs';
import { Details } from './Details';
import { WhatHappensNext } from './WhatHappensNext';

export function Landing({
  preview,
  anonymous,
  legalTerms,
  busy,
  onStart,
  onRegister,
}: {
  readonly preview: TriggerPreview;
  readonly anonymous: boolean;
  /** Published terms the visitor accepts by creating an account. */
  readonly legalTerms?: LegalTerms;
  readonly busy: boolean;
  readonly onStart: () => void;
  readonly onRegister: () => void;
}) {
  const [signup, setSignup] = useState<SignupOptions | 'unavailable'>();
  useEffect(() => {
    if (!anonymous) return;
    const controller = new AbortController();
    void readSignupOptions(controller.signal).then(
      (value) => {
        if (!controller.signal.aborted) setSignup(value);
      },
      () => {
        if (!controller.signal.aborted) setSignup('unavailable');
      },
    );
    return () => controller.abort();
  }, [anonymous]);
  return (
    <>
      <header className="public-heading">
        <p>{publicCopy.from(preview.publisher_display_name)}</p>
        <h1>{preview.title}</h1>
        <p>{preview.description}</p>
      </header>
      {preview.availability !== 'published' ? (
        <p role="status">{publicCopy.unavailable}</p>
      ) : (
        <>
          <WhatHappensNext sentences={preview.what_happens} />
          <div className="public-actions">
            <Button variant="primary" disabled={busy} onClick={onStart}>
              {buttonNames[preview.button_label]}
            </Button>
          </div>
          {anonymous && signup !== undefined ? (
            <div className="public-signup">
              <p>
                {signup === 'unavailable' ? publicCopy.signupUnavailable : signupSentence(signup)}
              </p>
              {signup !== 'unavailable' && signup.registerAvailable ? (
                <>
                  <Button onClick={onRegister} disabled={busy}>
                    {publicCopy.createAccount}
                  </Button>
                  {preview.signup_hint === undefined ? null : <p>{preview.signup_hint}</p>}
                  {legalTerms === undefined ? null : <LegalNotice terms={legalTerms} />}
                </>
              ) : null}
            </div>
          ) : null}
          <Details preview={preview} />
        </>
      )}
    </>
  );
}
