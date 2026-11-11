import { category, hash } from "./policy.mjs";

export class Observer {
  constructor(page, origin) {
    this.origin = origin;
    this.zero = performance.now();
    this.phase = "A1";
    this.hidden = false;
    this.rows = [];
    this.pending = new Map();
    this.inflight = new Map();
    this.scriptErrors = 0;
    page.on("pageerror", () => {
      this.scriptErrors += 1;
    });
    page.on("request", (request) => {
      if (this.rows.length >= 1600) return;
      // Exact URLs are used only in memory to detect overlap, never serialized.
      const key = hash(request.method() + " " + request.url());
      const row = {
        phase: this.phase,
        route: category(request.url(), origin),
        method: ["GET", "HEAD", "OPTIONS"].includes(request.method())
          ? request.method()
          : "WRITE",
        start_ms: this.now(),
        end_ms: null,
        status: null,
        canceled: false,
        censored: true,
        hidden_start: this.hidden,
        duplicate: (this.inflight.get(key) ?? 0) > 0,
        sse: false,
        correlation_sha256: null,
      };
      this.inflight.set(key, (this.inflight.get(key) ?? 0) + 1);
      this.pending.set(request, { row, key });
      this.rows.push(row);
    });
    page.on("response", (response) => {
      const row = this.pending.get(response.request())?.row;
      if (!row) return;
      row.status = response.status();
      const headers = response.headers();
      row.sse = (headers["content-type"] ?? "").startsWith("text/event-stream");
      const correlation = headers["x-request-id"];
      if (correlation) row.correlation_sha256 = hash(correlation);
    });
    page.on("requestfinished", (request) => this.finish(request, false));
    page.on("requestfailed", (request) => this.finish(request, true));
  }
  now() {
    return Math.round(performance.now() - this.zero);
  }
  finish(request, canceled) {
    const pending = this.pending.get(request);
    if (!pending) return;
    Object.assign(pending.row, {
      end_ms: this.now(),
      canceled,
      censored: false,
    });
    this.inflight.set(
      pending.key,
      Math.max(0, (this.inflight.get(pending.key) ?? 1) - 1),
    );
    this.pending.delete(request);
  }
  begin(phase, hidden = false) {
    this.phase = phase;
    this.hidden = hidden;
    return this.now();
  }
  result() {
    // Browser completions are not claimed as independently joined server spans.
    return { requests: this.rows, script_errors: this.scriptErrors };
  }
}

export async function contentFingerprint(page) {
  return page.evaluate(async () => {
    const digest = async (text) =>
      Array.from(
        new Uint8Array(
          await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)),
        ),
      )
        .map((byte) => byte.toString(16).padStart(2, "0"))
        .join("");
    const input = document.querySelector("#agent-message");
    const messages = [...document.querySelectorAll("[data-message-key]")];
    return {
      history_count: messages.length,
      history_sha256: await digest(
        messages.map((node) => node.textContent).join("\n"),
      ),
      draft_sha256: await digest(input?.value ?? ""),
      composer_ready: !!input && !input.disabled,
    };
  });
}
