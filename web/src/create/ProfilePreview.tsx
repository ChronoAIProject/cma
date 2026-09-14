import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { ProfileKind } from '../routing/routes';
import { RevisionContents } from '../profiles/marketplace/RevisionContents';
import { placeMenu } from '../ui/menuPlacement';
import type { CreationClient, CreationProfile, ProfileChoice } from './client';
import '../profiles/marketplace/Marketplace.css';
import './ProfilePreview.css';

export function ProfilePreview({
  client,
  kind,
  choice,
  children,
}: {
  readonly client: CreationClient;
  readonly kind: ProfileKind;
  readonly choice: ProfileChoice;
  readonly children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [portalRoot, setPortalRoot] = useState<HTMLElement>();
  const row = useRef<HTMLLIElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const id = useId();
  const schedule = (show: boolean) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(
      () => {
        if (show)
          setPortalRoot(row.current?.closest<HTMLElement>('[role="dialog"]') ?? document.body);
        setOpen(show);
      },
      show ? 200 : 150,
    );
  };
  useEffect(() => () => clearTimeout(timer.current), []);
  useEffect(() => {
    if (!open) return;
    const dismiss = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.stopImmediatePropagation();
      if (panel.current?.contains(document.activeElement))
        row.current?.querySelector<HTMLElement>('a, button')?.focus();
      clearTimeout(timer.current);
      setOpen(false);
    };
    document.addEventListener('keydown', dismiss, true);
    return () => document.removeEventListener('keydown', dismiss, true);
  }, [open]);
  useLayoutEffect(() => {
    if (!open) return;
    const dialog = row.current?.closest<HTMLElement>('[role="dialog"]');
    const place = () => {
      const anchor = row.current?.getBoundingClientRect();
      const element = panel.current;
      if (anchor === undefined || element === null) return;
      // A hover preview must leave the dialog's close and submit controls reachable.
      const header = dialog?.querySelector('.modal__header')?.getBoundingClientRect();
      const footer = dialog?.querySelector('.modal__footer')?.getBoundingClientRect();
      const top = Math.max(12, header?.bottom ?? 12);
      const bottom = Math.min(innerHeight - 12, footer?.top ?? innerHeight - 12);
      element.style.maxHeight = `${Math.max(0, bottom - top)}px`;
      const position = placeMenu(
        anchor,
        { width: element.offsetWidth, height: element.offsetHeight },
        { width: innerWidth, height: innerHeight },
        innerWidth < 768 ? 'bottom' : 'right',
        'start',
      );
      element.style.left = `${position.left}px`;
      element.style.top = `${Math.max(top, Math.min(position.top, bottom - element.offsetHeight))}px`;
      element.style.visibility = 'visible';
    };
    place();
    const resize = new ResizeObserver(place);
    if (dialog) resize.observe(dialog);
    window.addEventListener('resize', place);
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && panel.current?.contains(event.target)) return;
      place();
    };
    window.addEventListener('scroll', onScroll, true);
    return () => {
      resize.disconnect();
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', onScroll, true);
    };
  }, [open]);
  return (
    <li
      ref={row}
      onMouseEnter={() => schedule(true)}
      onMouseLeave={() => schedule(false)}
      onFocusCapture={() => schedule(true)}
      onClickCapture={(event) => {
        if (event.target instanceof Node && row.current?.contains(event.target)) {
          clearTimeout(timer.current);
          setOpen(false);
        }
      }}
      onBlurCapture={(event) => {
        if (
          !(event.relatedTarget instanceof Node) ||
          (!row.current?.contains(event.relatedTarget) &&
            !panel.current?.contains(event.relatedTarget))
        )
          schedule(false);
      }}
      aria-describedby={open ? id : undefined}
    >
      {children}
      {!open
        ? null
        : createPortal(
            <div
              ref={panel}
              id={id}
              className="profile-preview"
              role="region"
              aria-label={`${choice.name} profile details`}
              style={{ visibility: 'hidden', left: 0, top: 0 }}
              onMouseEnter={() => clearTimeout(timer.current)}
              onMouseLeave={() => schedule(false)}
            >
              <h3>{choice.name}</h3>
              <p className="profile-preview__version">Version {choice.revision}</p>
              <div className="profile-preview__body" tabIndex={0} aria-label="Profile settings">
                <PreviewContents client={client} kind={kind} choice={choice} />
              </div>
            </div>,
            portalRoot ?? document.body,
          )}
    </li>
  );
}

function PreviewContents({
  client,
  kind,
  choice,
}: {
  readonly client: CreationClient;
  readonly kind: ProfileKind;
  readonly choice: ProfileChoice;
}) {
  const [profile, setProfile] = useState<CreationProfile>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const request = new AbortController();
    void client
      .readProfile(kind, choice, { signal: request.signal })
      .then((value) => {
        if (request.signal.aborted) return;
        if (
          value.kind !== kind ||
          value.revision.profileId !== choice.id ||
          value.revision.revision !== choice.revision
        )
          throw new Error('Profile identity changed');
        setProfile(value);
      })
      .catch(() => {
        if (!request.signal.aborted) setFailed(true);
      });
    return () => request.abort();
  }, [client, kind, choice]);
  if (failed)
    return (
      <p role="status">Profile details are unavailable. Open the profile page or try again.</p>
    );
  if (profile === undefined) return <p role="status">Loading profile details</p>;
  return <RevisionContents value={profile} names={[]} />;
}
