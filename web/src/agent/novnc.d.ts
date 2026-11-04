declare module '@novnc/novnc' {
  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, url: string | WebSocket);
    scaleViewport: boolean;
    resizeSession: boolean;
    viewOnly: boolean;
    background: string;
    focus(): void;
    blur(): void;
    sendKey(keysym: number, code: string | null, down?: boolean): void;
    disconnect(): void;
  }
}
