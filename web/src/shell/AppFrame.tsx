import { DragHandle } from './ColumnDragHandle';
import { SidebarContext } from './sidebarContext';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import {
  CENTER_MIN,
  SIDEBAR_AUTO_COLLAPSE,
  SIDEBAR_DEFAULT,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  DETAILS_MAX,
  DETAILS_MIN,
  clampWidth,
  computeColumns,
  computeWorkspaceColumns,
} from './columns';
import { IconPanelLeftOutline16 } from '../ui/icons';
import './AppFrame.css';
import { usePlateFocus } from '../ui/plate';

/**
 * The page — the shared style contracts
 *
 * Three columns filling the viewport, and nothing above or below them. There
 * is no app bar and no footer: every band that used to hold chrome is column
 * width that the surface now has.
 *
 * Widths are **preferences, not settings**. They are not remembered across a
 * reload, deliberately: a width is a response to the window somebody has open
 * right now, and restoring yesterday's into today's window is how a layout
 * arrives wrong on a machine with a different screen.
 *
 * The frame measures **its own box** rather than the window. They are the
 * same today and would stop being the same the moment anything is placed
 * beside it, and a layout that silently assumed otherwise would be wrong in a
 * way nobody could see in the code.
 */

interface AppFrameProps {
  readonly sidebar:
    | ReactNode
    | ((state: {
        readonly collapsed: boolean;
        readonly onToggle: () => void;
        readonly onNavigate: () => void;
      }) => ReactNode);
  readonly center: ReactNode;
  /** Always rendered. A closed details column is zero-wide, never unmounted. */
  readonly details?: ReactNode;
  /** Whether the details column is asked for. */
  readonly detailsOpen: boolean;
  readonly detailsExpanded?: boolean;
  readonly workspaceDetails?: boolean;
  readonly taskLayout?: boolean;
  readonly compactDetailsOverlay?: boolean;
  readonly compactDetailsInline?: boolean;
  readonly onDetailsClose?: () => void;
  /**
   * Whether the owner has asked for the sidebar to be closed.
   *
   * The frame owns the column widths, so the toggle has to reach it to change
   * anything. Without this the sidebar re-laid its own contents into icon mode
   * inside a track that was still 280px wide, and the control read as broken.
   */
  readonly sidebarCollapsed?: boolean;
  /** Landing pages retain a collapsed rail without any expansion interaction. */
  readonly sidebarLocked?: boolean;
  /** Records an explicit collapse or expansion; automatic collapse stays derived. */
  readonly onSidebarCollapsedChange?: (collapsed: boolean) => void;
  /** Things that float over every column — a banner, a dialog's own layer. */
  readonly overlay?: ReactNode;
  /** What the phone top bar names, beside its navigation button. */
  readonly phoneTitle?: string;
}

export function AppFrame({
  sidebar,
  center,
  details,
  detailsOpen,
  detailsExpanded = false,
  workspaceDetails = false,
  taskLayout = false,
  compactDetailsOverlay = false,
  compactDetailsInline = false,
  onDetailsClose,
  overlay,
  sidebarCollapsed = false,
  sidebarLocked = false,
  onSidebarCollapsedChange,
  phoneTitle = 'CMA',
}: AppFrameProps) {
  const frameRef = useRef<HTMLDivElement>(null);
  const detailsRef = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState(0);
  /*
   * Whether the tracks have been resolved at a measured width. Until then they
   * are a placeholder solve for a zero-wide box, and easing out of it grew the
   * centre in from nothing on every mount: its controls slid for the whole
   * transition, so a press that landed early could release over another
   * element and never become a click. The first measured layout is drawn as
   * it is; only later width changes animate.
   */
  const [settled, setSettled] = useState(false);
  const [taskSidebarChoice, setTaskSidebarChoice] = useState<boolean>();
  const [sidebarWidth, setSidebarWidth] = useState(taskLayout ? SIDEBAR_MIN : SIDEBAR_DEFAULT);
  const [detailsWidth, setDetailsWidth] = useState(DETAILS_MIN);
  const [workspaceWidth, setWorkspaceWidth] = useState<number>();
  const [dragging, setDragging] = useState(false);
  /*
   * A handle reports displacement from pointer-down, so its width must be
   * anchored at pointer-down too. Reading `columns` on every rendered move
   * re-added the cumulative displacement to the width from the move before it:
   * a slow 60px drag reached the 420px clamp after only a few frames.
   */
  const sidebarDragStart = useRef(SIDEBAR_DEFAULT);
  const detailsDragStart = useRef(DETAILS_MIN);
  /**
   * Whether the sidebar was opened by hand at a width that collapses it.
   *
   * Without this the toggle does nothing below 1024px: the auto-collapse would
   * win on the next render and the panel would shut again immediately, which
   * reads as a broken control rather than as a deliberate rule.
   */
  const [expandedAnyway, setExpandedAnyway] = useState(false);

  // Measured before the first paint, so a new frame never shows the placeholder.
  useLayoutEffect(() => {
    const element = frameRef.current;
    if (element === null) {
      return;
    }
    const measure = () => {
      const width = element.getBoundingClientRect().width;
      if (width > 0) {
        setViewport(width);
      }
    };
    let frame: number | null = null;
    const scheduleMeasure = () => {
      // Coalesced to one measurement per frame: a resize drag fires this far
      // faster than anything can be laid out in response.
      frame ??= requestAnimationFrame(() => {
        frame = null;
        measure();
      });
    };
    const measureOnForeground = () => {
      if (document.visibilityState !== 'visible') {
        return;
      }
      /*
       * A background tab can defer requestAnimationFrame indefinitely. Its
       * first ResizeObserver delivery may therefore still be queued when the
       * user returns, leaving the centre at the zero-width initial solve.
       * Foregrounding is rare and needs the repaired layout immediately, so
       * bypass the frame throttle for this lifecycle edge.
       */
      measure();
    };
    const observer = new ResizeObserver(scheduleMeasure);
    // ResizeObserver normally reports once after observe(), but a direct-load
    // frame must not depend on that delivery to give its center a width.
    measure();
    observer.observe(element);
    window.addEventListener('resize', scheduleMeasure);
    window.addEventListener('focus', measureOnForeground);
    document.addEventListener('visibilitychange', measureOnForeground);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', scheduleMeasure);
      window.removeEventListener('focus', measureOnForeground);
      document.removeEventListener('visibilitychange', measureOnForeground);
      if (frame !== null) {
        cancelAnimationFrame(frame);
      }
    };
  }, []);

  useLayoutEffect(() => {
    const element = frameRef.current;
    if (settled || viewport === 0 || element === null) {
      return;
    }
    // Resolve the measured tracks while transitions are still off, so turning
    // them on leaves nothing earlier for the columns to ease from.
    element.getBoundingClientRect();
    setSettled(true);
  }, [settled, viewport]);

  const narrow =
    viewport > 0 &&
    (taskLayout
      ? viewport < CENTER_MIN || (detailsOpen && viewport < 1180)
      : viewport < SIDEBAR_AUTO_COLLAPSE);
  /*
   * Asked for, or forced by the width. Both reach the solver as a preference
   * of zero, which is what `computeColumns` already reads as closed — so the
   * two routes to a closed sidebar produce one result rather than two.
   *
   * An explicit collapse outranks `expandedAnyway`: that flag exists to keep
   * a hand-opened sidebar open against the automatic rule, not against its
   * owner asking a second time for it to close.
   */
  const effectiveSidebarCollapsed =
    sidebarLocked ||
    (taskLayout
      ? taskSidebarChoice === undefined
        ? sidebarCollapsed || narrow
        : onSidebarCollapsedChange
          ? sidebarCollapsed
          : taskSidebarChoice
      : sidebarCollapsed || (narrow && !expandedAnyway));
  const sidebarPreference = effectiveSidebarCollapsed ? 0 : sidebarWidth;
  /*
   * A phone has no room for a permanent rail: 56px is a seventh of a 375px
   * screen. Below the centre's floor the sidebar is therefore either gone or
   * a sheet over the page, and a top bar holds the way back to it.
   */
  const phone = viewport > 0 && viewport < CENTER_MIN;
  const solved =
    workspaceDetails && detailsOpen
      ? computeWorkspaceColumns(viewport, sidebarPreference, workspaceWidth)
      : computeColumns(viewport, sidebarPreference, detailsOpen ? detailsWidth : 0);
  const columns = phone
    ? { sidebar: effectiveSidebarCollapsed ? 0 : solved.sidebar, center: viewport, details: 0 }
    : solved;
  const sidebarOverlay = phone && !effectiveSidebarCollapsed;

  const toggleSidebar = useCallback(() => {
    if (sidebarLocked) return;
    setTaskSidebarChoice(!effectiveSidebarCollapsed);
    if (effectiveSidebarCollapsed) {
      onSidebarCollapsedChange?.(false);
      if (narrow) {
        setExpandedAnyway(true);
      }
      return;
    }

    onSidebarCollapsedChange?.(true);
    setExpandedAnyway(false);
  }, [effectiveSidebarCollapsed, narrow, onSidebarCollapsedChange, sidebarLocked]);

  const sidebarOwnsToggle = typeof sidebar === 'function';
  const renderedSidebar = sidebarOwnsToggle
    ? sidebar({
        collapsed: effectiveSidebarCollapsed,
        onToggle: toggleSidebar,
        onNavigate: () => {
          if (narrow && !taskLayout) setExpandedAnyway(false);
          if (taskLayout && phone) {
            setTaskSidebarChoice(true);
            onSidebarCollapsedChange?.(true);
          }
        },
      })
    : sidebar;

  /* Escape closes the sheet, as it closes any other layer over the page. */
  useEffect(() => {
    if (!sidebarOverlay) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.target instanceof Element && event.target.closest('[role="dialog"]')) return;
      if (event.key === 'Escape' && !event.defaultPrevented) toggleSidebar();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [sidebarOverlay, toggleSidebar]);

  const onSidebarDrag = useCallback((to: number) => {
    setSidebarWidth(clampWidth(to, SIDEBAR_MIN, SIDEBAR_MAX));
  }, []);
  const onDetailsDrag = useCallback(
    (to: number) => {
      if (workspaceDetails) setWorkspaceWidth(clampWidth(to, 360, viewport - solved.sidebar - 420));
      else setDetailsWidth(clampWidth(to, DETAILS_MIN, DETAILS_MAX));
    },
    [workspaceDetails, viewport, solved.sidebar],
  );
  /*
   * An unmeasured frame solves every column for a zero-wide box, where the
   * details column has no room. That is not a compact frame: treating it as one
   * moved focus into the panel and made the page inert on a wide screen.
   */
  const compactDetails = viewport > 0 && detailsOpen && columns.details === 0;
  const detailsInline = compactDetails && compactDetailsInline;
  const detailsOverlay = compactDetails && compactDetailsOverlay && !detailsInline;
  const detailsModal = detailsOpen && (detailsOverlay || detailsExpanded);
  usePlateFocus(
    detailsRef,
    onDetailsClose ?? (() => {}),
    detailsModal && onDetailsClose !== undefined,
  );

  const content = (
    <div
      ref={frameRef}
      className="frame"
      data-workspace={taskLayout || undefined}
      data-dragging={dragging ? '' : undefined}
      data-measuring={settled ? undefined : ''}
      data-details-collapsed={columns.details === 0 ? '' : undefined}
      data-details-overlay={detailsOverlay ? '' : undefined}
      data-details-expanded={detailsOpen && detailsExpanded ? '' : undefined}
      data-details-inline={detailsInline ? '' : undefined}
      data-sidebar-overlay={sidebarOverlay ? '' : undefined}
      data-phone={phone ? '' : undefined}
      style={{
        gridTemplateColumns: sidebarOverlay
          ? `0px ${String(viewport)}px 0px`
          : `${String(columns.sidebar)}px ${String(columns.center)}px ${String(columns.details)}px`,
      }}
    >
      <div
        className="frame__sidebar"
        inert={detailsModal || undefined}
        hidden={phone && effectiveSidebarCollapsed}
        style={
          sidebarOverlay
            ? { width: Math.min(columns.sidebar, Math.max(0, viewport - 48)) }
            : undefined
        }
      >
        {renderedSidebar}
      </div>
      <div className="frame__center" inert={sidebarOverlay || detailsModal || undefined}>
        {phone && sidebarOwnsToggle ? (
          <div className="frame__topbar">
            {sidebarLocked ? null : (
              <button
                type="button"
                className="frame__menu"
                aria-label="Show navigation"
                aria-expanded={!effectiveSidebarCollapsed}
                onClick={toggleSidebar}
              >
                <IconPanelLeftOutline16 />
              </button>
            )}
            <span className="frame__title">{phoneTitle}</span>
          </div>
        ) : null}
        {center}
      </div>
      {sidebarOverlay ? (
        <div className="frame__scrim" aria-hidden="true" onClick={toggleSidebar} />
      ) : null}
      <div
        className="frame__details"
        ref={detailsRef}
        role={detailsModal ? 'dialog' : undefined}
        aria-modal={detailsModal || undefined}
        aria-label={detailsModal ? 'Agent panel' : undefined}
      >
        {details}
      </div>

      {/*
        The handles are children of the frame rather than of the columns they
        move: a column clips its own overflow, so a strip straddling its border
        would be cut in half by the thing it is there to resize.
      */}
      {sidebarLocked ? null : (
        <DragHandle
          side="sidebar"
          at={columns.sidebar}
          label="Resize navigation"
          value={columns.sidebar}
          min={SIDEBAR_MIN}
          max={SIDEBAR_MAX}
          onResize={onSidebarDrag}
          onDrag={(dx) => {
            onSidebarDrag(sidebarDragStart.current + dx);
          }}
          onDraggingChange={(active) => {
            if (active) {
              sidebarDragStart.current = columns.sidebar;
            }
            setDragging(active);
          }}
        />
      )}
      {columns.details === 0 ? null : (
        <DragHandle
          side="details"
          at={columns.sidebar + columns.center}
          label="Resize the details column"
          value={columns.details}
          min={workspaceDetails ? 360 : DETAILS_MIN}
          max={
            workspaceDetails
              ? viewport - columns.sidebar - 360
              : Math.min(DETAILS_MAX, viewport - columns.sidebar - CENTER_MIN)
          }
          onResize={onDetailsDrag}
          onDrag={(dx) => {
            // Dragging left widens: the column grows from its leading edge.
            onDetailsDrag(detailsDragStart.current - dx);
          }}
          onDraggingChange={(active) => {
            if (active) {
              detailsDragStart.current = columns.details;
            }
            setDragging(active);
          }}
        />
      )}

      <div className="frame__overlay">{overlay}</div>
      {narrow && !sidebarOwnsToggle && !sidebarLocked ? (
        <button
          type="button"
          className="frame__narrowtoggle"
          aria-expanded={!effectiveSidebarCollapsed}
          onClick={toggleSidebar}
        >
          {effectiveSidebarCollapsed ? 'Show navigation' : 'Hide navigation'}
        </button>
      ) : null}
    </div>
  );
  return (
    <SidebarContext value={{ collapsed: effectiveSidebarCollapsed, toggle: toggleSidebar }}>
      {content}
    </SidebarContext>
  );
}
