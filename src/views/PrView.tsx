import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  commands,
  events,
  type AppError,
  type DiffLayout,
  type DiffScope,
  type DraftTarget,
  type FileMeta,
  type LocalReview,
  type PrRef,
  type PrView as View,
  type Project,
  type ReviewEvent,
  type ReviewThread,
  type SinceReview,
} from "../ipc";
import type { ComposerValue } from "../components/Draft";
import { ReviewSubmit } from "../components/ReviewSubmit";
import { ErrorNotice } from "../components/ErrorNotice";
import { DiffView, type DiffViewHandle } from "../components/DiffView";
import { FilePanel } from "../components/FilePanel";
import { CheckoutButton } from "../components/CheckoutButton";
import { errorText } from "../lib/errors";
import styles from "./PrView.module.css";

interface Props {
  pr: PrRef;
  active: boolean;
  projects: Project[];
  layout: DiffLayout;
  onLayout: (l: DiffLayout) => void;
  onTitle: (title: string) => void;
  onSwitchAccount: () => void;
}

const NO_THREADS: ReviewThread[] = [];
const NO_REVIEW: LocalReview = { drafts: [], viewed: [] };

/** `undefined` while checking; `null` if you haven't reviewed. */
type Since = SinceReview | null | undefined | { error: string };

export function PrView({ pr, active, projects, layout, onLayout, onTitle, onSwitchAccount }: Props) {
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [localState, setLocalState] = useState<"idle" | "loading" | "failed">("idle");
  const [localError, setLocalError] = useState<string | null>(null);
  const [threads, setThreads] = useState<ReviewThread[]>(NO_THREADS);
  const [threadsError, setThreadsError] = useState<string | null>(null);
  const [updated, setUpdated] = useState(false);
  const [openMs, setOpenMs] = useState<number | null>(null);
  const [currentFile, setCurrentFile] = useState(0);
  const [local, setLocal] = useState<LocalReview>(NO_REVIEW);
  const [scope, setScope] = useState<DiffScope>("all");
  const [since, setSince] = useState<Since>(undefined);
  const [posted, setPosted] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const diffRef = useRef<DiffViewHandle>(null);
  const started = useRef(0);
  const onTitleRef = useRef(onTitle);
  onTitleRef.current = onTitle;

  const loadThreads = useCallback(
    () => commands.prThreads(pr).then((r) => (r.status === "ok" ? setThreads(r.data) : setThreadsError(errorText(r.error)))),
    [pr],
  );
  const loadSince = useCallback(async () => {
    setSince(undefined);
    const r = await commands.prSinceReview(pr);
    setSince(r.status === "ok" ? r.data : { error: errorText(r.error) });
    if (r.status === "ok" && !r.data) setScope("all");
  }, [pr]);

  const load = useCallback(async () => {
    started.current = performance.now();
    setError(null);
    setUpdated(false);
    setOpenMs(null);
    setThreadsError(null);
    loadThreads();
    const r = await commands.openPr(pr);
    if (r.status === "error") return setError(r.error);
    setView(r.data);
    onTitleRef.current(r.data.detail.summary.title);
    commands.prLocalReview(pr).then((l) => l.status === "ok" && setLocal(l.data));
    if (r.data.source === "api") {
      setLocalState("loading");
      const local = await commands.prSyncLocal(pr);
      if (local.status === "ok") {
        setView((v) => (v ? { ...v, files: local.data, source: "local" } : v));
        setLocalState("idle");
      } else {
        setLocalState("failed");
        setLocalError(errorText(local.error));
        return;
      }
    }
    loadSince();
  }, [pr, loadThreads, loadSince]);

  useEffect(() => {
    load();
  }, [load]);

  useLayoutEffect(() => {
    if (view && openMs === null) {
      requestAnimationFrame(() => setOpenMs(Math.round(performance.now() - started.current)));
    }
  }, [view, openMs]);

  useEffect(() => {
    const un = events.prUpdated.listen((e) => {
      const p = e.payload.pr;
      if (p.host === pr.host && p.owner === pr.owner && p.repo === pr.repo && p.number === pr.number) setUpdated(true);
    });
    return () => {
      un.then((f) => f());
    };
  }, [pr]);

  const threadCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const t of threads) m.set(t.path, (m.get(t.path) ?? 0) + 1);
    return m;
  }, [threads]);

  const toggleLayout = useCallback(() => onLayout(layout === "split" ? "unified" : "split"), [layout, onLayout]);

  const headSha = view?.detail.head_sha ?? "";
  const allFiles = view?.files;
  const sinceReady = since && !("error" in since) ? since : null;
  const shown = scope === "since" && sinceReady ? sinceReady.files : allFiles;

  // A viewed mark holds only while the file's change is the one you saw.
  const viewed = useMemo(() => {
    const now = new Map((allFiles ?? []).map((f) => [f.path, f.patch_hash]));
    return new Set(local.viewed.filter((v) => now.get(v.path) === v.patch_hash).map((v) => v.path));
  }, [allFiles, local.viewed]);

  const onViewed = useCallback(
    (meta: FileMeta, on: boolean) => {
      const hash = allFiles?.find((f) => f.path === meta.path)?.patch_hash;
      if (!hash) return;
      setLocal((l) => ({
        ...l,
        viewed: [...l.viewed.filter((v) => v.path !== meta.path), ...(on ? [{ path: meta.path, patch_hash: hash }] : [])],
      }));
      commands.setViewed(pr, meta.path, on ? hash : null).then((r) => {
        if (r.status === "error") setActionError(errorText(r.error));
      });
    },
    [pr, allFiles],
  );
  const onAddDraft = useCallback(
    async (target: DraftTarget, v: ComposerValue) => {
      const r = await commands.addDraft(pr, headSha, target, v.body, v.label);
      if (r.status === "error") return errorText(r.error);
      setLocal((l) => ({ ...l, drafts: [...l.drafts, r.data] }));
      return null;
    },
    [pr, headSha],
  );
  const onUpdateDraft = useCallback(async (id: number, v: ComposerValue) => {
    const r = await commands.updateDraft(id, v.body, v.label);
    if (r.status === "error") return errorText(r.error);
    setLocal((l) => ({ ...l, drafts: l.drafts.map((d) => (d.id === id ? { ...d, body: v.body, label: v.label } : d)) }));
    return null;
  }, []);
  const onDeleteDraft = useCallback((id: number) => {
    setLocal((l) => ({ ...l, drafts: l.drafts.filter((d) => d.id !== id) }));
    commands.deleteDraft(id).then((r) => r.status === "error" && setActionError(errorText(r.error)));
  }, []);
  const onResolve = useCallback(
    async (th: ReviewThread, resolved: boolean) => {
      const r = await commands.setThreadResolved(pr, th.id, resolved);
      if (r.status === "error") return setActionError(errorText(r.error));
      setThreads((ts) => ts.map((t) => (t.id === th.id ? { ...t, resolved } : t)));
    },
    [pr],
  );
  const submittable = local.drafts.filter((d) => d.target.kind === "reply" || d.head_sha === headSha).length;
  const onSubmit = useCallback(
    async (event: ReviewEvent, body: string) => {
      const r = await commands.submitReview(pr, headSha, event, body);
      if (r.status === "error") return errorText(r.error);
      setPosted(r.data);
      commands.prLocalReview(pr).then((l) => l.status === "ok" && setLocal(l.data));
      loadThreads();
      loadSince();
      return null;
    },
    [pr, headSha, loadThreads, loadSince],
  );
  const pick = useCallback((fi: number) => diffRef.current?.scrollToFile(fi), []);

  if (error) {
    return (
      <div className={styles.center}>
        <ErrorNotice error={error} onSwitchAccount={onSwitchAccount} onRetry={load} />
      </div>
    );
  }
  if (!view) return <div className={styles.center + " muted"}>Opening {pr.owner}/{pr.repo}#{pr.number}…</div>;

  const d = view.detail;
  const files = shown ?? view.files;
  const project = projects.find((p) => p.id === view.project_id);
  const unresolved = threads.filter((t) => !t.resolved).length;

  return (
    <div className={styles.pr} data-active={active}>
      <header className={styles.header}>
        <div className={styles.titleRow}>
          <h1 className={styles.title}>
            {d.summary.title} <span className="muted">#{pr.number}</span>
          </h1>
          <div className={styles.actions}>
            <button className="btn ghost" onClick={() => openUrl(d.summary.url)}>
              GitHub ↗
            </button>
            {project && <CheckoutButton project={project} pr={pr} />}
            <ReviewSubmit pending={submittable} stale={local.drafts.length - submittable} onSubmit={onSubmit} />
          </div>
        </div>
        <div className={styles.metaRow}>
          <div className={styles.meta}>
            <span className={styles.state} data-state={d.summary.is_draft ? "DRAFT" : d.summary.state}>
              {d.summary.is_draft ? "Draft" : d.summary.state.toLowerCase()}
            </span>
            <span>{d.summary.author ?? "ghost"}</span>
            <span className="mono">
              {d.head_repo && d.head_repo !== `${pr.owner}/${pr.repo}` ? `${d.head_repo.split("/")[0]}:` : ""}
              {d.head_ref} → {d.base_ref}
            </span>
            <span>
              {d.changed_files} files <span className={styles.add}>+{d.additions}</span>{" "}
              <span className={styles.del}>−{d.deletions}</span>
            </span>
            {threads.length > 0 && (
              <span>
                {threads.length} threads{unresolved > 0 && `, ${unresolved} unresolved`}
              </span>
            )}
            {project && <span className={styles.local}>in project {project.name}</span>}
            <span className={styles.timing} title="Time to first diff render">
              {view.source === "api" ? "GitHub patches" : "local"}
              {openMs !== null && ` · ${openMs} ms`}
              {localState === "loading" && " · syncing mirror…"}
            </span>
          </div>
          <div className={styles.toggles}>
            <div className={styles.segmented} title={sinceTitle(since)}>
              <button data-on={scope === "all"} onClick={() => setScope("all")}>
                All changes
              </button>
              <button data-on={scope === "since"} disabled={!sinceReady} onClick={() => setScope("since")}>
                Since your review{since === undefined && localState !== "failed" ? "…" : ""}
              </button>
            </div>
            <div className={styles.segmented} title="Toggle layout (s)">
              <button data-on={layout === "unified"} onClick={() => onLayout("unified")}>
                Unified
              </button>
              <button data-on={layout === "split"} onClick={() => onLayout("split")}>
                Split
              </button>
            </div>
          </div>
        </div>
        {scope === "since" && sinceReady && (
          <div className={styles.info}>
            {sinceReady.files.length === 0
              ? "Nothing changed since your review"
              : `${sinceReady.files.length} ${sinceReady.files.length === 1 ? "file" : "files"} changed since your review`}{" "}
            at <span className="mono">{sinceReady.reviewed_sha.slice(0, 7)}</span>
            {sinceReady.rewritten &&
              ". The branch was force-pushed, so only files whose changes differ from what you reviewed are shown"}
            .
          </div>
        )}
        {posted && (
          <div className={styles.banner}>
            Review posted.{" "}
            <button className="btn ghost" onClick={() => openUrl(posted)}>
              View on GitHub ↗
            </button>
            <button className="btn ghost" onClick={() => setPosted(null)}>
              Dismiss
            </button>
          </div>
        )}
        {actionError && (
          <div className={styles.warn}>
            {actionError}{" "}
            <button className="btn ghost" onClick={() => setActionError(null)}>
              Dismiss
            </button>
          </div>
        )}
        {updated && (
          <div className={styles.banner}>
            New commits were pushed.{" "}
            <button className="btn" onClick={load}>
              Reload
            </button>
          </div>
        )}
        {localState === "failed" && <div className={styles.warn}>Showing GitHub's patches; local diff failed: {localError}</div>}
        {threadsError && <div className={styles.warn}>Couldn't load review threads: {threadsError}</div>}
      </header>
      <div className={styles.body}>
        <FilePanel
          files={files}
          current={currentFile}
          onPick={pick}
          threadCounts={threadCounts}
        />
        <DiffView
          key={scope}
          ref={diffRef}
          pr={pr}
          files={files}
          layout={layout}
          syncing={localState === "loading"}
          scope={scope}
          headSha={headSha}
          threads={threads}
          drafts={local.drafts}
          viewed={viewed}
          active={active}
          onCurrentFile={setCurrentFile}
          onToggleLayout={toggleLayout}
          onViewed={onViewed}
          onAddDraft={onAddDraft}
          onUpdateDraft={onUpdateDraft}
          onDeleteDraft={onDeleteDraft}
          onResolve={onResolve}
        />
      </div>
    </div>
  );
}

function sinceTitle(since: Since): string {
  if (since === undefined) return "Checking for your last review…";
  if (since === null) return "You haven't reviewed this pull request";
  if ("error" in since) return `Couldn't compare with your review: ${since.error}`;
  return `Changes since your review at ${since.reviewed_sha.slice(0, 7)}`;
}
