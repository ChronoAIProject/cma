import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent, PointerEvent as ReactPointerEvent } from 'react';
import { CanvasNode } from './CanvasNode';
import { gridColumns, isCanvasArrow, nextNodeIndex, selectLiveWindow } from './layout';
import type { LiveSession } from '../sessions/live';
import { tabLabel } from '../sessions/sessionTime';
import './SessionCanvas.css';

/**
 * Every session at once, on a surface that can be panned.
 *
 * Exactly one node is focused. Focus is a property of the canvas rather than of
 * a node, because "which terminal takes the keys" is a statement about the set:
 * two nodes each believing they are focused is the one failure mode this view
 * must not have.
 *
 * The canvas replaces the middle column of the console and nothing else. The
 * session rail still lists the sessions and the details panel still describes
 * the focused one, because those are the same facts in either arrangement.
 */

interface SessionCanvasProps {
  /** Every listed session and its driver, in display order. */
  readonly nodes: readonly LiveSession[];
  readonly focusedId: string | null;
  /**
   * Session ids, most recently focused first. The canvas spends its attachment
   * budget from this rather than from display order: the sessions somebody is
   * actually working with are the ones worth a connection.
   */
  readonly focusHistory: readonly string[];
  readonly onFocus: (id: string) => void;
}

export function SessionCanvas({ nodes, focusedId, focusHistory, onFocus }: SessionCanvasProps) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const handles = useRef(new Map<string, HTMLButtonElement>());
  const [panning, setPanning] = useState(false);
  const panOrigin = useRef<{ x: number; y: number; left: number; top: number } | null>(null);

  const columns = gridColumns(nodes.length);

  /*
   * Only sessions that have something to connect to spend the budget. A session
   * that is still starting holds no terminal and no socket, so counting it
   * would leave a connection unspent on a node that cannot use one.
   */
  const connectable = useMemo(
    () => nodes.filter((node) => node.driver !== null).map((node) => node.session.id),
    [nodes],
  );
  const live = useMemo(
    () => selectLiveWindow(connectable, focusHistory),
    [connectable, focusHistory],
  );
  const dropped = connectable.length - live.size;

  const registerHandle = useCallback((id: string, element: HTMLButtonElement | null) => {
    if (element === null) {
      handles.current.delete(id);
    } else {
      handles.current.set(id, element);
    }
  }, []);

  function handleKeyDown(event: KeyboardEvent<HTMLElement>, index: number) {
    if (!isCanvasArrow(event.key)) {
      return;
    }
    const target = nextNodeIndex(index, event.key, nodes.length, columns);
    const next = target < 0 ? undefined : nodes[target];
    // Claimed even when the move is a no-op at the edge, so the canvas does not
    // scroll out from under somebody who is only trying to change node.
    event.preventDefault();
    if (next === undefined) {
      return;
    }
    onFocus(next.session.id);
    handles.current.get(next.session.id)?.focus();
  }

  function startPan(event: ReactPointerEvent<HTMLDivElement>) {
    // Only a gesture that begins on the bare surface pans. Inside a node the
    // same gesture belongs to the terminal, which uses it to select text.
    if (event.target !== event.currentTarget || event.button !== 0) {
      return;
    }
    const viewport = viewportRef.current;
    if (viewport === null) {
      return;
    }
    panOrigin.current = {
      x: event.clientX,
      y: event.clientY,
      left: viewport.scrollLeft,
      top: viewport.scrollTop,
    };
    setPanning(true);
  }

  useEffect(() => {
    if (!panning) {
      return;
    }
    // On window rather than the surface: a drag that leaves the canvas, or
    // releases over another element, must still end the pan.
    const move = (event: PointerEvent) => {
      const viewport = viewportRef.current;
      const origin = panOrigin.current;
      if (viewport === null || origin === null) {
        return;
      }
      // The surface follows the pointer, so the scroll offset moves against it.
      viewport.scrollLeft = origin.left - (event.clientX - origin.x);
      viewport.scrollTop = origin.top - (event.clientY - origin.y);
    };
    const end = () => {
      panOrigin.current = null;
      setPanning(false);
    };

    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', end);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', end);
      window.removeEventListener('pointercancel', end);
    };
  }, [panning]);

  const focusedIndex = nodes.findIndex((node) => node.session.id === focusedId);
  const focused = focusedIndex < 0 ? undefined : nodes[focusedIndex];

  return (
    <div className="canvas">
      <div className="canvas__viewport" ref={viewportRef}>
        <div
          className="canvas__surface"
          data-panning={panning}
          style={{ '--canvas-columns': columns } as CSSProperties}
          onPointerDown={startPan}
        >
          {nodes.map((node, index) => (
            <CanvasNode
              key={node.session.id}
              session={node.session}
              driver={node.driver}
              focused={node.session.id === focusedId}
              attached={live.has(node.session.id)}
              index={index}
              total={nodes.length}
              onFocus={() => {
                onFocus(node.session.id);
              }}
              onKeyDown={(event) => {
                handleKeyDown(event, index);
              }}
              registerHandle={(element) => {
                registerHandle(node.session.id, element);
              }}
            />
          ))}
        </div>
      </div>

      {/*
       * What the budget cost, said out loud, and outside the scrolling
       * viewport so it cannot be panned away from. Truncating silently would
       * leave someone looking at a frame with no terminal in it and no account
       * of why, which reads as sessions having broken rather than as a limit
       * being enforced.
       */}
      {dropped > 0 ? (
        <p className="canvas__budget" role="status">
          {dropped} of {connectable.length} sessions are not attached, to bound what this page
          costs. Select one to attach it.
        </p>
      ) : null}

      {/* Announces the move, since the visual focus treatment is not available
          to a screen reader and the node label alone does not say "focused". */}
      <p className="sr-only" aria-live="polite">
        {focused === undefined
          ? 'No session focused.'
          : `Session ${tabLabel(focused.session.createdAt)} focused, node ${focusedIndex + 1} of ${nodes.length}.`}
      </p>
    </div>
  );
}
