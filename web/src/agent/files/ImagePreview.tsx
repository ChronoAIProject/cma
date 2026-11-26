import { useCallback, useEffect, useRef, useState } from 'react';
import { Button } from '../../ui/Button';
import { IconCloseOutline16, IconFullscreenOutline16 } from '../../ui/icons';
import './ImagePreview.css';

type Point = { x: number; y: number };

function distance(first: Point, second: Point) {
  return Math.hypot(first.x - second.x, first.y - second.y);
}

export function ImageViewer({ src, name }: { readonly src: string; readonly name: string }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [expanded, setExpanded] = useState(false);
  return (
    <>
      <Viewer
        src={src}
        name={name}
        onExpand={() => {
          dialog.current?.showModal();
          setExpanded(true);
        }}
      />
      <dialog
        ref={dialog}
        aria-label={name}
        className="file-image-preview__dialog"
        onClose={() => setExpanded(false)}
        onKeyDown={(event) => {
          if (event.key === 'Tab' || event.key === 'Escape') event.stopPropagation();
        }}
      >
        {expanded ? <Viewer src={src} name={name} onClose={() => dialog.current?.close()} /> : null}
      </dialog>
    </>
  );
}

function Viewer({
  src,
  name,
  onExpand,
  onClose,
}: {
  readonly src: string;
  readonly name: string;
  readonly onExpand?: () => void;
  readonly onClose?: () => void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const pointers = useRef(new Map<number, Point>());
  const [natural, setNatural] = useState({ width: 0, height: 0 });
  const [zoom, setZoom] = useState<number>();
  const zoomBy = useCallback((factor: number) => {
    const img = image.current;
    if (!img?.naturalWidth) return;
    const displayed = img.getBoundingClientRect().width / img.naturalWidth;
    setZoom((current) => Math.min(4, Math.max(0.001, (current ?? displayed) * factor)));
  }, []);

  useEffect(() => {
    const element = viewport.current!;
    if (onClose) element.focus({ preventScroll: true });
    const wheel = (event: WheelEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      event.preventDefault();
      zoomBy(Math.exp(-event.deltaY * (event.deltaMode ? 0.04 : 0.002)));
    };
    let scale = 1;
    const gesture = (event: Event) => {
      event.preventDefault();
      if (event.type === 'gesturestart') scale = 1;
      else {
        const next = (event as Event & { scale: number }).scale;
        if (next > 0) {
          zoomBy(next / scale);
          scale = next;
        }
      }
    };
    element.addEventListener('wheel', wheel, { passive: false });
    element.addEventListener('gesturestart', gesture, { passive: false });
    element.addEventListener('gesturechange', gesture, { passive: false });
    return () => {
      element.removeEventListener('wheel', wheel);
      element.removeEventListener('gesturestart', gesture);
      element.removeEventListener('gesturechange', gesture);
    };
  }, [onClose, zoomBy]);

  const fit = `min(100cqw, ${(100 * natural.width) / (natural.height || 1)}cqh, ${natural.width}px)`;
  return (
    <div
      className="file-image-preview"
      onKeyDown={(event) => {
        if (event.metaKey || event.ctrlKey || event.altKey) return;
        if (event.key === '+' || event.key === '=') zoomBy(1.25);
        else if (event.key === '-' || event.key === '−') zoomBy(0.8);
        else if (event.key === '0') setZoom(undefined);
        else if (event.key === 'Escape' && onClose) onClose();
        else return;
        event.preventDefault();
        event.stopPropagation();
      }}
    >
      <header
        className="file-image-preview__controls"
        role="group"
        aria-label="Image preview controls"
      >
        <Button size="sm" aria-label="Zoom out" onClick={() => zoomBy(0.8)}>
          −
        </Button>
        <Button size="sm" aria-label="Zoom in" onClick={() => zoomBy(1.25)}>
          +
        </Button>
        <Button size="sm" aria-pressed={zoom === undefined} onClick={() => setZoom(undefined)}>
          Fit
        </Button>
        <Button size="sm" aria-pressed={zoom === 1} onClick={() => setZoom(1)}>
          100%
        </Button>
        <Button
          size="sm"
          aria-label={onClose ? 'Exit full screen' : 'Full screen'}
          icon={onClose ? <IconCloseOutline16 /> : <IconFullscreenOutline16 />}
          onClick={(event) => {
            if (onExpand) event.currentTarget.focus({ preventScroll: true });
            (onClose ?? onExpand)?.();
          }}
        />
      </header>
      <div
        ref={viewport}
        className="file-image-preview__viewport"
        role="region"
        aria-label="Image viewport"
        tabIndex={0}
        onPointerDown={(event) => {
          if (event.button !== 0 || (event.pointerType !== 'touch' && zoom === undefined)) return;
          pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerMove={(event) => {
          const previous = pointers.current.get(event.pointerId);
          if (!previous) return;
          const next = { x: event.clientX, y: event.clientY };
          pointers.current.set(event.pointerId, next);
          const other = [...pointers.current].find(([id]) => id !== event.pointerId)?.[1];
          if (other) {
            const prior = distance(previous, other);
            if (prior > 0) zoomBy(distance(next, other) / prior);
          } else {
            event.currentTarget.scrollBy(previous.x - next.x, previous.y - next.y);
          }
        }}
        onLostPointerCapture={(event) => pointers.current.delete(event.pointerId)}
      >
        <img
          ref={image}
          src={src}
          alt={name}
          className="agent-files__image"
          draggable={false}
          style={{ width: zoom === undefined ? fit : natural.width * zoom }}
          onLoad={({ currentTarget }) =>
            setNatural({ width: currentTarget.naturalWidth, height: currentTarget.naturalHeight })
          }
        />
      </div>
    </div>
  );
}
