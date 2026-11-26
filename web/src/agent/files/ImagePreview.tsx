import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { KeyboardEvent, PointerEvent } from 'react';
import { Button } from '../../ui/Button';
import { IconCloseOutline16, IconFullscreenOutline16 } from '../../ui/icons';
import './ImagePreview.css';

type Point = { x: number; y: number };
type Size = { width: number; height: number };
type Anchor = { position: Point; fraction: Point };

const maxScale = 4;
const zoomStep = Math.SQRT2;

function center(rect: DOMRect): Point {
  return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
}

function pinch(points: Map<number, Point>) {
  const [first, second] = points.values();
  if (!first || !second) return;
  return {
    distance: Math.hypot(first.x - second.x, first.y - second.y),
    position: { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 },
  };
}

export function ImagePreview({
  name,
  source,
  expanded = false,
  initialFocus = false,
  onToggle,
}: {
  readonly name: string;
  readonly source: string | Blob;
  readonly expanded?: boolean;
  readonly initialFocus?: boolean;
  readonly onToggle: () => void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const controls = useRef<HTMLElement>(null);
  const anchor = useRef<Anchor | undefined>(undefined);
  const pointers = useRef(new Map<number, Point>());
  const pinchStart = useRef<{ distance: number; scale: number } | undefined>(undefined);
  const [natural, setNatural] = useState<Size>();
  const [frame, setFrame] = useState<Size>();
  const [zoom, setZoom] = useState<number>();
  const [dragging, setDragging] = useState(false);
  const [failed, setFailed] = useState(false);
  const fit =
    natural && frame ? Math.min(1, frame.width / natural.width, frame.height / natural.height) : 1;
  const scale = zoom ?? fit;
  const loaded = natural !== undefined && frame !== undefined && fit > 0 && !failed;
  const canPan = !!(
    loaded &&
    frame &&
    natural &&
    (natural.width * scale > frame.width + 1 || natural.height * scale > frame.height + 1)
  );

  useEffect(() => {
    if (initialFocus && loaded)
      controls.current?.querySelector<HTMLButtonElement>('[data-initial-focus]')?.focus();
  }, [initialFocus, loaded]);

  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      const { clientWidth: width, clientHeight: height } = element;
      setFrame({ width, height });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const element = image.current;
    if (!element || typeof source === 'string') return;
    const url = URL.createObjectURL(source);
    element.src = url;
    return () => URL.revokeObjectURL(url);
  }, [source]);

  useLayoutEffect(() => {
    const view = viewport.current;
    const item = image.current;
    const target = anchor.current;
    anchor.current = undefined;
    if (!view || !item || !target) return;
    const rect = item.getBoundingClientRect();
    view.scrollBy({
      left: rect.left + target.fraction.x * rect.width - target.position.x,
      top: rect.top + target.fraction.y * rect.height - target.position.y,
      behavior: 'instant',
    });
  }, [scale, zoom]);

  function zoomAt(value: number, position?: Point, previousPosition = position) {
    const view = viewport.current;
    const item = image.current;
    if (!loaded || !view || !item) return;
    const next = Math.min(maxScale, Math.max(fit, value));
    const destination = position ?? center(view.getBoundingClientRect());
    const origin = previousPosition ?? destination;
    if (next === scale && zoom !== undefined) {
      view.scrollLeft += origin.x - destination.x;
      view.scrollTop += origin.y - destination.y;
      return;
    }
    const rect = item.getBoundingClientRect();
    anchor.current = {
      position: destination,
      fraction: { x: (origin.x - rect.left) / rect.width, y: (origin.y - rect.top) / rect.height },
    };
    setZoom(next);
  }

  function restoreFit() {
    anchor.current = undefined;
    viewport.current?.scrollTo(0, 0);
    setZoom(undefined);
  }

  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      if ((!event.ctrlKey && !event.metaKey) || !loaded || !event.deltaY) return;
      event.preventDefault();
      zoomAt(scale * (event.deltaY < 0 ? zoomStep : 1 / zoomStep), {
        x: event.clientX,
        y: event.clientY,
      });
    };
    element.addEventListener('wheel', wheel, { passive: false });
    return () => element.removeEventListener('wheel', wheel);
  });

  function keyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (expanded && event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onToggle();
      return;
    }
    if (!loaded || event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === '+' || event.key === '=') zoomAt(scale * zoomStep);
    else if (event.key === '-' || event.key === '−') zoomAt(scale / zoomStep);
    else if (event.key === '0') restoreFit();
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  function pointerDown(event: PointerEvent<HTMLDivElement>) {
    if (!loaded || event.button !== 0 || pointers.current.size === 2) return;
    if (event.pointerType === 'mouse' && !canPan) return;
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const pair = pinch(pointers.current);
    if (pair) pinchStart.current = { distance: pair.distance, scale };
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
  }

  function pointerMove(event: PointerEvent<HTMLDivElement>) {
    const point = pointers.current.get(event.pointerId);
    if (!point) return;
    const previous = pinch(pointers.current);
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const current = pinch(pointers.current);
    const start = pinchStart.current;
    if (current && previous && start && start.distance > 0) {
      zoomAt(
        start.scale * (current.distance / start.distance),
        current.position,
        previous.position,
      );
    } else if (canPan) {
      event.currentTarget.scrollLeft += point.x - event.clientX;
      event.currentTarget.scrollTop += point.y - event.clientY;
    }
  }

  function pointerEnd(event: PointerEvent<HTMLDivElement>) {
    if (!pointers.current.delete(event.pointerId)) return;
    pinchStart.current = undefined;
    setDragging(pointers.current.size > 0);
  }

  return (
    <div className="agent-image-preview" data-expanded={expanded} onKeyDown={keyDown}>
      <header
        ref={controls}
        className="agent-image-preview__controls"
        role="group"
        aria-label="Image zoom controls"
      >
        <Button
          size="sm"
          aria-label="Zoom out"
          disabled={!loaded || scale <= fit}
          onClick={() => zoomAt(scale / zoomStep)}
        >
          −
        </Button>
        <output aria-label="Image zoom">{loaded ? `${Math.round(scale * 100)}%` : '—'}</output>
        <Button
          size="sm"
          aria-label="Zoom in"
          disabled={!loaded || scale >= maxScale}
          onClick={() => zoomAt(scale * zoomStep)}
        >
          +
        </Button>
        <Button size="sm" aria-pressed={zoom === undefined} disabled={!loaded} onClick={restoreFit}>
          Fit
        </Button>
        <Button size="sm" aria-pressed={zoom === 1} disabled={!loaded} onClick={() => zoomAt(1)}>
          100%
        </Button>
        <Button
          size="sm"
          icon={expanded ? <IconCloseOutline16 /> : <IconFullscreenOutline16 />}
          aria-label={expanded ? 'Close full-screen image' : 'Open full-screen image'}
          title={expanded ? 'Close full-screen image' : 'Open full-screen image'}
          data-initial-focus={expanded || initialFocus ? 'true' : undefined}
          disabled={!expanded && !loaded}
          onClick={onToggle}
        />
      </header>
      <div
        ref={viewport}
        className="agent-image-preview__viewport"
        role="region"
        aria-label="Image viewport"
        tabIndex={0}
        data-pan={canPan}
        data-dragging={dragging}
        onPointerDown={pointerDown}
        onPointerMove={pointerMove}
        onPointerUp={pointerEnd}
        onPointerCancel={pointerEnd}
        onLostPointerCapture={pointerEnd}
      >
        <div className="agent-image-preview__canvas">
          {failed ? (
            <p role="status">Image preview unavailable.</p>
          ) : (
            <img
              ref={image}
              alt={name}
              src={typeof source === 'string' ? source : undefined}
              decoding="async"
              draggable={false}
              data-ready={loaded}
              width={loaded && natural ? Math.floor(natural.width * scale) : undefined}
              height={loaded && natural ? Math.floor(natural.height * scale) : undefined}
              onLoad={({ currentTarget }) =>
                setNatural({
                  width: currentTarget.naturalWidth,
                  height: currentTarget.naturalHeight,
                })
              }
              onError={() => setFailed(true)}
            />
          )}
        </div>
      </div>
    </div>
  );
}
