import { useId, useState } from 'react';
import type { KernelOffer, Session } from './types';
import { abilitiesFor } from './availability';
import { planeOf, switchFor, unavailableBecause } from './planeSwitch';
import { Modal } from '../ui/Modal';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { effectiveAppearance, storedAppearance } from '../theme/appearance';
import { resolveTheme, storeThemeId, storedThemeId, themesFor } from '../terminal/themes';
import { Button } from '../ui/Button';
import './SessionActions.css';
import { Menu } from '../ui/Menu';

/**
 * What can be done to the session being described — docs/Web_Style.md §12.
 *
 * One primary, one secondary, and the overflow menu (§7): a header carrying
 * four equally weighted keys makes the reader choose before they can read,
 * which is what this header used to do. The primary is Resume where §11
 * offers it — revive is the primary action wherever it is offered — and
 * Reconnect otherwise. The secondary is Details, which opens the drawer.
 * Everything else — moving the session's plane, pausing, terminating and
 * deleting it — lives behind the lifecycle or overflow menus.
 *
 * **Every disabled control states why, and one sentence covers all of them.**
 * `abilitiesFor` is the single source of enablement and it disables controls
 * for exactly one cause at a time — the session's state — so the reason is
 * drawn once per row, as a `.control-reason`, and each control points at it.
 *
 * The plane move confirms inline, in place of the row's own controls
 * (§18.6), stating the two costs that are not obvious. §7 forbids a second
 * dialog over a dialog, and the same rule holds for a header.
 */

interface SessionActionsProps {
  readonly session: Session;
  readonly onReconnect: () => void;
  readonly onRevive: () => void;
  readonly onStop: () => void;
  /** End the runtime and keep the session's transcript and history. */
  readonly onTerminate: () => void;
  /**
   * Delete the session and everything it recorded.
   *
   * Here rather than in the drawer, because the lifecycle is now one menu and
   * splitting it across two surfaces is how somebody looks for an action where
   * it used to be. It keeps its acknowledgement: §12.4 spends the tick on the
   * one action that loses something, and moving it must not be how that is
   * quietly dropped.
   */
  readonly onDelete: () => void;
  /** Told when the terminal palette changes, so the surface can be rebuilt. */
  readonly onTerminalThemeChange?: (id: string) => void;
  /** Opens and closes the details drawer. */
  readonly onDetails: () => void;
  readonly detailsOpen?: boolean;
  /**
   * What this deployment runs, and what each runtime can be.
   *
   * Decides whether a plane switch is offered at all. Empty means nothing is
   * known, and nothing known draws no switch: offering a move that will be
   * refused is worse than not offering one.
   */
  readonly kernels?: readonly KernelOffer[];
  /** Move this session onto the other plane. */
  readonly onChangePlane?: (to: 'terminal' | 'conversation') => void;
  readonly busy?: boolean;
  /**
   * Whether this session is waiting on its owner's sign-in.
   *
   * It is `provisioning` either way and takes no input either way, so nothing
   * about what can be done changes. What changes is the sentence: telling
   * somebody the session is still starting sends them to wait for a thing
   * that is waiting for them.
   */
  readonly signingIn?: boolean;
}

/** Chat lifecycle belongs to the platform, so its header exposes Details alone. */
export function ChatSessionActions({
  onDetails,
  detailsOpen = false,
}: {
  readonly onDetails: () => void;
  readonly detailsOpen?: boolean;
}) {
  return (
    <div className="actions">
      <Button size="sm" onClick={onDetails} aria-expanded={detailsOpen}>
        Details
      </Button>
    </div>
  );
}

export function SessionActions({
  session,
  signingIn = false,
  onReconnect,
  onRevive,
  onStop,
  onTerminate,
  onDelete,
  onTerminalThemeChange,
  onDetails,
  detailsOpen = false,
  kernels = [],
  onChangePlane,
  busy = false,
}: SessionActionsProps) {
  const [confirmingSwitch, setConfirmingSwitch] = useState(false);
  const [confirmingTerminate, setConfirmingTerminate] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [lifecycleOpen, setLifecycleOpen] = useState(false);
  /*
   * The terminal palette. It sat in the account menu, where it applied to a
   * plane most sessions are not on — an account setting for a surface its
   * owner may never open. It belongs to the session that draws one.
   */
  const [terminalThemeId, setTerminalThemeId] = useState<string | null>(() => storedThemeId());
  const surface = effectiveAppearance(storedAppearance());
  const theme = resolveTheme(surface, terminalThemeId);
  const onTerminal = planeOf(session) === 'terminal';
  const reasonId = useId();
  const can = abilitiesFor(session, signingIn);
  // Absent rather than disabled where the kernel does not advertise the other
  // plane (§18.6): a control that can never work is not one that is currently
  // unavailable.
  const move = onChangePlane === undefined ? undefined : switchFor(session, kernels);
  /*
   * What the entry is called, from the plane it would move to. It names the
   * act rather than the destination — "Terminal" beside "Conversation" read as
   * two views to flip between, which is what the retired tab strip made this
   * look like. It is a rotation with a cost, and the words say so.
   */
  const moveLabel =
    move === undefined
      ? undefined
      : move.to === 'terminal'
        ? 'Interact directly with terminal'
        : 'Return to chat mode';

  /*
   * §7: one reason per row, beside the controls it explains. The session's
   * state disables for one cause at a time, so one sentence covers the
   * primary and the menu's items alike.
   */
  const describedBy = can.reason === undefined ? undefined : reasonId;

  /*
   * One lifecycle row. Disabled with its reason beside the label rather than
   * removed (§7): the set keeps its shape as a session moves through its life,
   * so somebody who paused one yesterday finds Resume where Pause was rather
   * than hunting a menu that has rearranged itself.
   */
  const row = (id: string, label: string, allowed: boolean) => ({
    id,
    label:
      allowed || can.reason === undefined ? (
        label
      ) : (
        <span className="actions__whynot">
          {label}
          <span className="actions__reason">{can.reason}</span>
        </span>
      ),
    disabled: busy || !allowed,
  });

  return (
    <div className="actions">
      {can.reason === undefined ? null : (
        <span className="control-reason actions__reason" id={reasonId}>
          {can.reason}
        </span>
      )}

      {/*
       * One key for what can be done to the session, rather than whichever
       * single action happened to apply. `Reconnect` and `Resume` were the
       * same slot showing different words, so which one a person saw depended
       * on a state they had not been told about — and `Reconnect` re-dials a
       * live session's transport while `Resume` gives a paused one a sandbox
       * back. Two different acts wearing one position.
       *
       * Every row that this state does not allow is drawn disabled with its
       * reason (§7) rather than removed, so the set does not change shape as a
       * session moves through its life.
       */}
      <Menu
        open={lifecycleOpen}
        label="Manage lifecycle"
        align="end"
        items={[
          row('resume', 'Resume', can.canRevive),
          row('reconnect', 'Reconnect', can.canReconnect),
          row('stop', 'Pause session', can.canStop),
          row('terminate', 'Terminate session', can.canTerminate),
          { type: 'separator', id: 'before-delete' },
          { ...row('delete', 'Delete', can.canDelete), danger: true },
        ]}
        onSelect={(id) => {
          setLifecycleOpen(false);
          if (id === 'resume') return onRevive();
          if (id === 'reconnect') return onReconnect();
          if (id === 'stop') return onStop();
          if (id === 'terminate') {
            setConfirmingTerminate(true);
            return;
          }
          if (id === 'delete') setConfirmingDelete(true);
        }}
        onClose={() => {
          setLifecycleOpen(false);
        }}
        anchor={
          <Button
            size="sm"
            aria-haspopup="menu"
            aria-expanded={lifecycleOpen}
            aria-describedby={describedBy}
            onClick={() => {
              setLifecycleOpen((open) => !open);
            }}
          >
            Manage lifecycle
          </Button>
        }
      />

      <Button size="sm" onClick={onDetails} aria-expanded={detailsOpen}>
        Details
      </Button>

      {/*
       * §18.6: the plane key names where it moves to, is absent — not
       * disabled — when the kernel does not advertise the other plane, and
       * disabled with its reason when the state refuses it.
       */}
      <Menu
        open={menuOpen}
        label="More actions"
        align="end"
        items={[
          ...(move === undefined
            ? []
            : [
                {
                  id: 'move',
                  /*
                   * The reason rides with the row. §7 asks a disabled control
                   * to say why where it is, and the row is the only place a
                   * person reading the menu will look.
                   */
                  label: move.available ? (
                    moveLabel
                  ) : (
                    <span className="actions__whynot">
                      {moveLabel}
                      <span className="actions__reason">{unavailableBecause(session)}</span>
                    </span>
                  ),
                  disabled: busy || !move.available,
                },
              ]),
          /*
           * Only on a terminal session. §7: a control that can never do
           * anything here is absent rather than drawn — a conversation has no
           * palette to change.
           */
          ...(onTerminal ? [{ id: 'palette', label: `Terminal theme: ${theme.label}` }] : []),
        ]}
        onSelect={(id) => {
          setMenuOpen(false);
          if (id === 'move') {
            setConfirmingSwitch(true);
            return;
          }
          if (id === 'palette') {
            // Only palettes suiting the current appearance are offered, so a
            // dark theme can never be placed on a light page.
            const options = themesFor(surface);
            const next =
              options[(options.findIndex((t) => t.id === theme.id) + 1) % options.length];
            if (next !== undefined) {
              setTerminalThemeId(next.id);
              storeThemeId(next.id);
              // Read when a terminal is constructed, so the change lands on
              // the next one rather than repainting a live screen underneath
              // somebody mid-command.
              onTerminalThemeChange?.(next.id);
            }
          }
        }}
        onClose={() => {
          setMenuOpen(false);
        }}
        anchor={
          <Button
            size="sm"
            placement="actions__more"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            aria-label="More actions"
            onClick={() => {
              setMenuOpen((open) => !open);
            }}
          >
            ···
          </Button>
        }
      />

      <Modal
        open={confirmingTerminate}
        title="Terminate this session?"
        onClose={() => {
          setConfirmingTerminate(false);
        }}
        footer={
          <>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => {
                setConfirmingTerminate(false);
              }}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={busy}
              onClick={() => {
                setConfirmingTerminate(false);
                onTerminate();
              }}
            >
              Terminate session
            </Button>
          </>
        }
      >
        <p className="actions__explain">
          The sandbox will be stopped and this session cannot be resumed. Its transcript and history
          will remain under Terminated until deleted.
        </p>
      </Modal>

      <RiskConfirmation
        open={confirmingDelete}
        title="Delete this session?"
        description="This deletes the session and everything it recorded — its transcript, its terminal output, and its history. Nothing here can be recovered afterwards."
        acknowledgeLabel="I understand this cannot be undone."
        confirmLabel="Delete permanently"
        busy={busy}
        onCancel={() => {
          setConfirmingDelete(false);
        }}
        onConfirm={() => {
          setConfirmingDelete(false);
          onDelete();
        }}
      />

      {/*
       * §12.4 and §18.6: a card in the middle of the page, because this is a
       * rotation and not a view change. It states the two things that are true
       * and not obvious before it happens — neither is recoverable by pressing
       * the key again, which is why they are said rather than reported after.
       *
       * It replaced an inline confirmation that took over the header's own
       * controls. That was quiet enough to be missed and put the question
       * somewhere the eye was not, for the one action in this menu that costs
       * something.
       */}
      {move === undefined ? null : (
        <Modal
          open={confirmingSwitch}
          title={moveLabel ?? ''}
          onClose={() => {
            setConfirmingSwitch(false);
          }}
          footer={
            <>
              <Button
                onClick={() => {
                  setConfirmingSwitch(false);
                }}
              >
                Stay here
              </Button>
              <Button
                variant="primary"
                disabled={busy}
                onClick={() => {
                  setConfirmingSwitch(false);
                  onChangePlane?.(move.to);
                }}
              >
                {move.to === 'terminal' ? 'Open the terminal' : 'Return to chat'}
              </Button>
            </>
          }
        >
          <p className="actions__explain">
            {move.to === 'terminal'
              ? 'This session restarts on its terminal, where you type to the agent’s shell directly rather than talking to it.'
              : 'This session restarts as a conversation, where you talk to the agent rather than typing to its shell.'}
          </p>
          <p className="actions__explain">
            It is not interactive while it moves, and the turn in flight is lost. Everything it has
            already shown stays where it is.
          </p>
        </Modal>
      )}
    </div>
  );
}
