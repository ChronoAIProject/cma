import { createHash } from "node:crypto";

export const ORIGIN = "https://cma.nyxid.ai";
// Keep Chromium's normal background scheduling and popup protection.
export const launchOptions = {
  headless: false,
  ignoreDefaultArgs: [
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--disable-popup-blocking",
  ],
};
export const hash = (value) => createHash("sha256").update(value).digest("hex");
export const phases = [
  "A1",
  "A2",
  "A3",
  "A4",
  "A5_focus",
  "A5_return",
  "A6",
  "A7",
];
const id = "[A-Za-z0-9_-]+";
const routes = [
  ["DISCOVERY", "/openapi.json"],
  ["CAPABILITIES", "/api/v2/ui/task-experience"],
  ["ORGANIZATIONS", "/api/v1/organizations"],
  ["AUTH", "/auth/session"],
  ["BUILD", "/build-info.json"],
  ["INVENTORY", "/api/(?:v1|agent-manager/v1)/(?:sandboxes|agents|workspaces)"],
  ["TRANSPORT", "/api/v2/ui/transport"],
  ["DEFAULTS", "/api/v1/(?:defaults|capabilities)"],
  ["SANDBOX", `/api/v1/sandboxes/${id}`],
  ["WORKSPACE", `/api/v1/workspaces/${id}`],
  ["WORKTREE", `/api/v1/worktrees/${id}`],
  ["AGENT", `/api/v1/agents/${id}`],
  ["HISTORY", `/api/v1/agents/${id}/(?:history|transcript|events)`],
  ["USAGE", `/api/v1/agents/${id}/usage`],
  [
    "SOURCE",
    `/api/v1/agents/${id}/(?:source|provenance|environment|resources)`,
  ],
  [
    "SETTINGS",
    `/api/(?:v1|agent-manager/v1)/agents/${id}/(?:settings|preferences|models|skills|service-access|message-feedback)`,
  ],
  [
    "ACCESS",
    `/api/(?:v1|agent-manager/v1)/agents/${id}/(?:access-requests|approval-requests)`,
  ],
  ["HEAD", `/api/v2/agents/${id}/events/head`],
  ["SNAPSHOT", `/api/v2/agents/${id}/events/snapshot`],
  ["STREAM", `/api/v2/agents/${id}/events`],
  ["QUEUE", `/api/v2/agents/${id}/queued-inputs`],
  ["RESPONSE", `/api/v2/agents/${id}/responses/${id}`],
  ["COMMAND", `/api/v2/commands/${id}`],
  ["AGUI", "/api/v1/agui/events"],
].map(([label, pattern]) => [label, new RegExp(`^${pattern}$`)]);

export function category(raw, origin = ORIGIN) {
  try {
    const url = new URL(raw);
    if (url.origin !== origin || url.username || url.password) return "OTHER";
    if (
      url.pathname === "/" ||
      new RegExp(`^/agents/${id}$`).test(url.pathname)
    )
      return "PAGE";
    if (/^\/assets\/[A-Za-z0-9_.-]+$/.test(url.pathname)) return "ASSET";
    if (
      /^\/(?:favicon\.ico|favicon\.svg|manifest\.webmanifest|logo\.svg)$/.test(
        url.pathname,
      )
    )
      return "STATIC";
    return (
      routes.find(([, pattern]) => pattern.test(url.pathname))?.[0] ?? "OTHER"
    );
  } catch {
    return "OTHER";
  }
}

export function quiet(now = new Date()) {
  return (now.getUTCHours() + 8) % 24 === 2;
}

// Stop an observation rather than changing the application's scheduling cadence.
export class Guard {
  constructor(origin = ORIGIN, maximum = 800) {
    this.origin = origin;
    this.maximum = maximum;
    this.count = 0;
    this.burst = [];
    this.stopped = false;
  }
  admit(raw, method, now = Date.now()) {
    if (this.stopped) return "STOPPED";
    if (quiet(new Date(now))) {
      this.stopped = true;
      return "QUIET";
    }
    if (!["GET", "HEAD", "OPTIONS"].includes(method)) return "WRITE";
    const route = category(raw, this.origin);
    if (route === "OTHER") return "DESTINATION";
    this.burst = this.burst.filter((time) => now - time < 1000);
    if (this.count >= this.maximum || this.burst.length >= 100) {
      this.stopped = true;
      return "BUDGET";
    }
    this.burst.push(now);
    this.count += 1;
    return "ALLOW";
  }
}

export async function protect(page, guard, blocked) {
  await page.setBypassServiceWorker(true);
  // Chromium's Network.setBlockedURLs does not intercept WebSocket upgrades.
  // Refuse that transport before construction, as the read-only contract requires.
  await page.exposeFunction("__acceptanceSocketDenied", () => {
    blocked.SOCKET += 1;
  });
  await page.evaluateOnNewDocument(() => {
    Object.defineProperty(globalThis, "WebSocket", {
      configurable: false,
      writable: false,
      value: class {
        constructor() {
          void globalThis.__acceptanceSocketDenied();
          throw new DOMException("Read-only transport policy", "SecurityError");
        }
      },
    });
  });
  await page.setRequestInterception(true);
  page.on("request", async (request) => {
    const decision = guard.admit(request.url(), request.method());
    try {
      if (decision === "ALLOW") await request.continue();
      else {
        blocked[decision] += 1;
        await request.abort("blockedbyclient");
      }
    } catch {
      guard.stopped = true;
    }
  });
  page.on("dialog", (dialog) => {
    void dialog.dismiss();
  });
  page.on("popup", (popup) => {
    guard.stopped = true;
    void popup.close();
  });
  // The current Agent tokenizer is a dedicated worker. Puppeteer attaches the
  // page network manager to dedicated workers, so HTTP reads/writes use this gate.
}
