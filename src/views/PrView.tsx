import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  commands,
  events,
  type AppError,
  type DiffLayout,
  type PrRef,
  type PrView as View,
  type Project,
  type ReviewThread,
} from "../ipc";
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
  const diffRef = useRef<DiffViewHandle>(null);
  const started = useRef(0);
  const onTitleRef = useRef(onTitle);
  onTitleRef.current = onTitle;

  const load = useCallback(async () => {
    started.current = performance.now();
    setError(null);
    setUpdated(false);
    setOpenMs(null);
    setThreadsError(null);
    commands.prThreads(pr).then((r) => (r.status === "ok" ? setThreads(r.data) : setThreadsError(errorText(r.error))));
    const r = await commands.openPr(pr);
    if (r.status === "error") return setError(r.error);
    setView(r.data);
    onTitleRef.current(r.data.detail.summary.title);
    if (r.data.source === "api") {
      setLocalState("loading");
      const local = await commands.prSyncLocal(pr);
      if (local.status === "ok") {
        setView((v) => (v ? { ...v, files: local.data, source: "local" } : v));
        setLocalState("idle");
      } else {
        setLocalState("failed");
        setLocalError(errorText(local.error));
      }
    }
  }, [pr]);

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
            <div className={styles.segmented} title="Toggle layout (s)">
              <button data-on={layout === "unified"} onClick={() => onLayout("unified")}>
                Unified
              </button>
              <button data-on={layout === "split"} onClick={() => onLayout("split")}>
                Split
              </button>
            </div>
        </div>
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
          files={view.files}
          current={currentFile}
          onPick={pick}
          threadCounts={threadCounts}
        />
        <DiffView
          ref={diffRef}
          pr={pr}
          files={view.files}
          layout={layout}
          syncing={localState === "loading"}
          threads={threads}
          active={active}
          onCurrentFile={setCurrentFile}
          onToggleLayout={toggleLayout}
        />
      </div>
    </div>
  );
}
