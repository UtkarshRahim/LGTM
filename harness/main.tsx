// Dev-only: renders PrView in a plain browser with IPC answered from a fixture
// written by `cargo test -p prism fixture`. Open /harness/?fixture=huge.
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "../src/styles/global.css";
import type { DiffLayout, FileHunks, FilePanel, PrRef, PrView as View, ReviewThread } from "../src/ipc";
import { PrView } from "../src/views/PrView";
import { Home } from "../src/views/Home";

interface Fixture {
  pr: PrRef;
  view: View;
  hunks: FileHunks[];
  threads: ReviewThread[];
}

const params = new URLSearchParams(location.search);
const fx: Fixture = await (await fetch(`/harness/fixtures/${params.get("fixture") ?? "huge"}.json`)).json();
const byPath = new Map(fx.hunks.map((h) => [h.path, h]));
const latency = Number(params.get("latency") ?? 15);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

mockWindows("main");
mockIPC(
  async (cmd, args) => {
    const a = args as Record<string, unknown>;
    switch (cmd) {
      case "open_pr":
        return fx.view;
      case "pr_threads":
        return fx.threads;
      case "pr_sync_local":
        return fx.view.files;
      case "needs_you": {
        const hit = (n: number, title: string) => ({
          summary: { ...fx.view.detail.summary, pr: { ...fx.pr, number: n }, title },
          project_id: n % 2 ? 1 : null,
        });
        return {
          blocking: [hit(101, "Add rate limiting to the uploads API"), hit(102, "Fix flaky checkout test")],
          rereview: [hit(103, "Migrate settings to TOML")],
        };
      }
      case "project_prs":
        return [101, 104, 105].map((n) => ({
          summary: { ...fx.view.detail.summary, pr: { ...fx.pr, number: n }, title: `Project PR ${n}` },
          project_id: 1,
        }));
      case "search":
        return { kind: "list", query: { kind: "text", text: a.input }, hits: [] };
      case "pr_hunks":
        await sleep(latency);
        return (a.paths as string[]).flatMap((p) => byPath.get(p) ?? []);
      default:
        throw new Error(`harness: unmocked command ${cmd}`);
    }
  },
  { shouldMockEvents: true },
);

function Harness() {
  const [layout, setLayout] = useState<DiffLayout>((params.get("layout") as DiffLayout) ?? "unified");
  const [panel, setPanel] = useState<FilePanel>((params.get("panel") as FilePanel) ?? "review_order");
  if (params.get("view") === "home") {
    return (
      <div style={{ height: "100vh" }}>
        <Home
          active
          onOpen={() => {}}
          onSwitchAccount={() => {}}
          refreshKey={0}
          project={
            params.get("project")
              ? { id: 1, path: "/tmp/cli", name: "cli", remotes: [fx.pr, { ...fx.pr, owner: "me" }].map((r) => ({ remote: "origin", host: r.host, owner: r.owner, repo: r.repo })), status: null }
              : null
          }
          onClearProject={() => {}}
        />
      </div>
    );
  }
  return (
    <div style={{ height: "100vh" }}>
      <PrView
        pr={fx.pr}
        active
        projects={[]}
        layout={layout}
        panel={panel}
        onLayout={setLayout}
        onPanel={setPanel}
        onTitle={() => {}}
        onSwitchAccount={() => {}}
      />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Harness />
  </StrictMode>,
);

/** Scrolls the diff at a steady speed and reports frame timings. */
async function bench(ms = 5000, pxPerFrame = 120) {
  const els = [...document.querySelectorAll<HTMLElement>("[class*='scroll']")];
  const el = els.sort((a, b) => b.scrollHeight - a.scrollHeight)[0];
  const deltas: number[] = [];
  let last = performance.now();
  const end = last + ms;
  await new Promise<void>((done) => {
    const step = (now: number) => {
      deltas.push(now - last);
      last = now;
      el.scrollTop += pxPerFrame;
      if (now < end) requestAnimationFrame(step);
      else done();
    };
    requestAnimationFrame(step);
  });
  deltas.shift();
  const sorted = [...deltas].sort((a, b) => a - b);
  const pct = (p: number) => Math.round(sorted[Math.floor((sorted.length - 1) * p)] * 10) / 10;
  return {
    frames: deltas.length,
    p50: pct(0.5),
    p95: pct(0.95),
    p99: pct(0.99),
    max: Math.round(sorted.at(-1)! * 10) / 10,
    over33ms: deltas.filter((d) => d > 33.4).length,
    scrolledPx: el.scrollTop,
    scrollHeight: el.scrollHeight,
    domNodes: document.getElementsByTagName("*").length,
  };
}

/** Jumps to random offsets (like dragging the scrollbar) and times the next paint. */
async function jumps(n = 30) {
  const els = [...document.querySelectorAll<HTMLElement>("[class*='scroll']")];
  const el = els.sort((a, b) => b.scrollHeight - a.scrollHeight)[0];
  const times: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    el.scrollTop = Math.random() * (el.scrollHeight - el.clientHeight);
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    times.push(performance.now() - t);
  }
  times.sort((a, b) => a - b);
  return { median: Math.round(times[n >> 1]), max: Math.round(times[n - 1]) };
}

Object.assign(window, { bench, jumps, __ready: true });

/** Long tasks (> 50 ms) observed since the last call. */
const longTasks: number[] = [];
new PerformanceObserver((l) => l.getEntries().forEach((e) => longTasks.push(Math.round(e.duration)))).observe({ type: "longtask" });
function takeLongTasks() {
  return longTasks.splice(0);
}
function scrollTop(px: number) {
  const els = [...document.querySelectorAll<HTMLElement>("[class*='scroll']")];
  els.sort((a, b) => b.scrollHeight - a.scrollHeight)[0].scrollTop = px;
}
Object.assign(window, { takeLongTasks, scrollTop });
