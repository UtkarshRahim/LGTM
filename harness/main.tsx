// Dev-only: renders PrView in a plain browser with IPC answered from a fixture
// written by `cargo test -p prism fixture`. Open /harness/?fixture=huge.
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "../src/styles/global.css";
import type { DiffLayout, Draft, FileHunks, Finding, LocalReview, PrRef, PrView as View, ReviewThread, Run, RunUpdate, Severity } from "../src/ipc";
import { PrView } from "../src/views/PrView";
import { Home } from "../src/views/Home";
import { Sidebar } from "../src/components/Sidebar";

interface Fixture {
  pr: PrRef;
  view: View;
  hunks: FileHunks[];
  threads: ReviewThread[];
}

const params = new URLSearchParams(location.search);
const fx: Fixture = await (await fetch(`/harness/fixtures/${params.get("fixture") ?? "huge"}.json`)).json();
fx.view.project_id = 1;
const HARNESS_PROJECT = {
  id: 1,
  path: `/tmp/${fx.pr.repo}`,
  name: fx.pr.repo,
  remotes: [{ remote: "origin", host: fx.pr.host, owner: fx.pr.owner, repo: fx.pr.repo }],
  status: null,
};
const byPath = new Map(fx.hunks.map((h) => [h.path, h]));
const latency = Number(params.get("latency") ?? 15);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
// `?reviewed=1` pretends you reviewed an earlier, force-pushed head.
const reviewed = params.get("reviewed");
const local: LocalReview = { drafts: [], viewed: [] };
let nextDraft = 1;

// Skills: `?skills=done` starts with a finished run, `?skills=stale` with one on an older head.
const SKILLS = [
  { id: "repo:review", name: "review", description: "General review of correctness and design", source: "repo", dir: "/tmp/r" },
  { id: "personal:bug-hunt", name: "bug-hunt", description: "Find correctness bugs and unhandled errors", source: "personal", dir: "/tmp/b" },
  { id: "personal:security", name: "security", description: "Look for injection, auth and secrets issues", source: "personal", dir: "/tmp/s" },
];
const AGENTS = [
  { id: "cursor", name: "Cursor", available: true },
  { id: "claude", name: "Claude Code", available: false },
];
let prefs = { pinned: "personal:bug-hunt" as string | null, agent: "cursor" };
const runs: Run[] = [];
let findings: Finding[] = [];
let nextRun = 1;
let nextFinding = 1;
const draftFinding = new Map<number, number>();
const now = () => Math.floor(Date.now() / 1000);

function fakeFindings(runId: number, head: string): Finding[] {
  const sev: Severity[] = ["blocker", "warning", "nit", "warning"];
  const out: Finding[] = [];
  for (const h of fx.hunks.slice(0, 4)) {
    const add = h.hunks.flatMap((x) => x.lines).find((l) => l.kind === "add" && l.new != null);
    if (!add) continue;
    const i = out.length;
    out.push({
      id: nextFinding++,
      run_id: runId,
      head_sha: head,
      path: h.path,
      line: add.new,
      end_line: null,
      severity: sev[i % sev.length],
      title: ["Error from the request is dropped", "Loop never exits when the list is empty", "Prefer a named constant", "Missing test for the new branch"][i],
      body: "The new code path ignores this case, so callers see a silent failure.\nHandle it or document why it can't happen.",
      suggestion: i === 1 ? add.text.replace(/\s+$/, "") + " // fixed" : null,
      status: "open",
      dismiss_reason: null,
    });
  }
  const last = fx.hunks[0];
  const maxLine = Math.max(...last.hunks.map((x) => x.new_start + x.new_lines));
  out.push({
    id: nextFinding++,
    run_id: runId,
    head_sha: head,
    path: last.path,
    line: maxLine + 40,
    end_line: maxLine + 44,
    severity: "warning",
    title: "Caller further down still uses the old signature",
    body: "This function's callers outside the diff weren't updated.",
    suggestion: null,
    status: "open",
    dismiss_reason: null,
  });
  return out;
}

function startRun(skillId: string, head: string, finishNow: boolean): Run {
  const skill = SKILLS.find((s) => s.id === skillId)!;
  const run: Run = {
    id: nextRun++,
    skill: skill.id,
    skill_name: skill.name,
    agent: "cursor",
    head_sha: head,
    status: "running",
    error: null,
    report: null,
    started_at: now(),
    ended_at: null,
  };
  runs.unshift(run);
  const finish = () => {
    const fs = fakeFindings(run.id, head);
    findings = [...findings, ...fs];
    Object.assign(run, {
      status: "done",
      ended_at: now(),
      report: "Feasible. The same behavior fits in the existing helper.\n\nTest gaps\n- MISSING: empty id returns 404\n- PASS: the happy path\n\nNo blockers.",
    });
    return fs;
  };
  if (finishNow) {
    finish();
    return run;
  }
  const send = (update: RunUpdate) => emit("skill-run-event", { run_id: run.id, pr: fx.pr, update });
  const script: [number, RunUpdate][] = [
    [100, { kind: "step", text: "Preparing a checkout of the PR head", warning: false }],
    [400, { kind: "step", text: "Starting Cursor", warning: false }],
    [700, { kind: "step", text: "Using Cursor's Ask mode (read-only)", warning: false }],
    [800, { kind: "step", text: `Running ${skill.name}`, warning: false }],
    [1100, { kind: "agent", event: { kind: "tool", id: "t1", title: "Read .prism/pr.diff", tool: "read", status: "completed" } }],
    [1400, { kind: "agent", event: { kind: "tool", id: "t2", title: `Read ${fx.hunks[0].path}`, tool: "read", status: "completed" } }],
    [1700, { kind: "agent", event: { kind: "denied", title: "`go test ./...`", tool: "execute" } }],
    [2000, { kind: "agent", event: { kind: "tool", id: "t3", title: "grep \"func New\"", tool: "search", status: "completed" } }],
  ];
  for (const [ms, u] of script) setTimeout(() => run.status === "running" && send(u), ms);
  setTimeout(() => {
    if (run.status !== "running") return;
    const fs = finish();
    send({ kind: "step", text: `${fs.length} findings`, warning: false });
    send({ kind: "finished", run: { ...run }, findings: structuredClone(fs) });
  }, Number(params.get("runms") ?? 2600));
  return run;
}
if (params.get("skills") === "done") startRun("personal:bug-hunt", fx.view.detail.head_sha, true);
if (params.get("skills") === "stale") startRun("repo:review", "0123456789abcdef0123456789abcdef01234567", true);

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
      case "pr_local_review":
        return structuredClone(local);
      case "add_draft": {
        const d = { id: nextDraft++, target: a.target, body: a.body, label: a.label, head_sha: a.headSha } as Draft;
        local.drafts = [...local.drafts, d];
        return d;
      }
      case "update_draft":
        local.drafts = local.drafts.map((d) => (d.id === a.id ? { ...d, body: a.body as string, label: a.label as Draft["label"] } : d));
        return null;
      case "delete_draft":
        local.drafts = local.drafts.filter((d) => d.id !== a.id);
        findings = findings.map((f) => (f.id === draftFinding.get(a.id as number) ? { ...f, status: "open" } : f));
        return null;
      case "set_viewed":
        local.viewed = local.viewed.filter((v) => v.path !== a.path);
        if (a.patchHash) local.viewed.push({ path: a.path as string, patch_hash: a.patchHash as string });
        return null;
      case "set_thread_resolved":
        await sleep(200);
        return null;
      case "submit_review":
        await sleep(400);
        if (local.drafts.length === 0 && !(a.body as string).trim()) throw { kind: "invalid", message: "Write a comment or a summary first" };
        local.drafts = [];
        return `https://github.com/${fx.pr.owner}/${fx.pr.repo}/pull/${fx.pr.number}#pullrequestreview-1`;
      case "pr_since_review":
        await sleep(300);
        if (!reviewed) return null;
        return {
          reviewed_sha: "538b4682e8c3b58a81397e331afbdc0be7d5674d",
          head_sha: fx.view.detail.head_sha,
          rewritten: true,
          files: fx.view.files.filter((_, i) => i % 2 === 0),
        };
      case "list_skills":
        await sleep(80);
        return { skills: SKILLS, agents: AGENTS, ...prefs };
      case "set_skill_prefs":
        prefs = { pinned: a.pinned as string | null, agent: a.agent as string };
        return null;
      case "pr_skill_state":
        return structuredClone({ runs, findings });
      case "skill_run_log":
        return [];
      case "run_skill":
        if (runs[0]?.status === "running") throw { kind: "invalid", message: "An auto review is already running on this pull request" };
        return { ...startRun(a.skillId as string, fx.view.detail.head_sha, false) };
      case "cancel_skill_run": {
        const run = runs.find((r) => r.id === a.runId);
        if (run?.status === "running") {
          Object.assign(run, { status: "cancelled", ended_at: now() });
          emit("skill-run-event", { run_id: run.id, pr: fx.pr, update: { kind: "finished", run: { ...run }, findings: [] } });
        }
        return null;
      }
      case "accept_finding": {
        const d = { id: nextDraft++, target: a.target, body: a.body, label: a.label, head_sha: a.headSha } as Draft;
        local.drafts = [...local.drafts, d];
        draftFinding.set(d.id, a.findingId as number);
        findings = findings.map((f) => (f.id === a.findingId ? { ...f, status: "accepted" } : f));
        return d;
      }
      case "dismiss_finding":
        findings = findings.map((f) =>
          f.id === a.findingId ? { ...f, status: a.reason == null ? "open" : "dismissed", dismiss_reason: a.reason as string | null } : f,
        );
        return null;
      default:
        throw new Error(`harness: unmocked command ${cmd}`);
    }
  },
  { shouldMockEvents: true },
);

function Harness() {
  const [layout, setLayout] = useState<DiffLayout>((params.get("layout") as DiffLayout) ?? "unified");
  if (params.get("view") === "sidebar") {
    const tabs = [1, 2, 3, 4, 5, 6].map((n) => ({ pr: { ...fx.pr, number: n }, title: `Review ${n}` }));
    const projects = ["prism", "cli", "tauri"].map((name, id) => ({ id, path: `/tmp/${name}`, name, remotes: [], status: null }));
    return (
      <div style={{ height: "100vh", width: 260, display: "flex" }}>
        <Sidebar
          tabs={tabs}
          active="home"
          onSelect={() => {}}
          onClose={() => {}}
          projects={projects}
          prCounts={{}}
          selectedProject={null}
          onSelectProject={() => {}}
          onAddProject={() => {}}
          onRemoveProject={() => {}}
          accounts={null}
          onPickAccount={() => {}}
          onRefreshAccounts={() => {}}
          accountMenuOpen={false}
          setAccountMenuOpen={() => {}}
        />
      </div>
    );
  }
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
        projects={[HARNESS_PROJECT]}
        layout={layout}
        onLayout={setLayout}
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
