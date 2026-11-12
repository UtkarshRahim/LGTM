import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { commands, events, type AppError, type PrRef, type PrView as View, type Project } from "../ipc";
import { ErrorNotice } from "../components/ErrorNotice";
import { FileCard } from "../components/Diff";
import { CheckoutButton } from "../components/CheckoutButton";
import { errorText } from "../lib/errors";
import styles from "./PrView.module.css";

interface Props {
  pr: PrRef;
  active: boolean;
  projects: Project[];
  onTitle: (title: string) => void;
  onSwitchAccount: () => void;
}

export function PrView({ pr, active, projects, onTitle, onSwitchAccount }: Props) {
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [localState, setLocalState] = useState<"idle" | "loading" | "failed">("idle");
  const [localError, setLocalError] = useState<string | null>(null);
  const [updated, setUpdated] = useState(false);
  const [openMs, setOpenMs] = useState<number | null>(null);
  const started = useRef(0);
  const onTitleRef = useRef(onTitle);
  onTitleRef.current = onTitle;

  const load = useCallback(async () => {
    started.current = performance.now();
    setError(null);
    setUpdated(false);
    setOpenMs(null);
    const r = await commands.openPr(pr);
    if (r.status === "error") return setError(r.error);
    setView(r.data);
    onTitleRef.current(r.data.detail.summary.title);
    if (r.data.source === "api") {
      setLocalState("loading");
      const local = await commands.prLocalDiff(pr);
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
            {project && <CheckoutButton project={project} number={pr.number} />}
          </div>
        </div>
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
            {d.changed_files} files <span className={styles.add}>+{d.additions}</span> <span className={styles.del}>−{d.deletions}</span>
          </span>
          {project && <span className={styles.local}>in project {project.name}</span>}
          <span className={styles.timing} title="Time to first diff render">
            {view.source === "api" ? "GitHub patches" : "local"}
            {openMs !== null && ` · ${openMs} ms`}
            {localState === "loading" && " · syncing mirror…"}
          </span>
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
      </header>
      <div className={styles.files}>
        {view.files.map((f) => (
          <FileCard key={f.path} file={f} loadingLocal={localState === "loading"} />
        ))}
      </div>
    </div>
  );
}
