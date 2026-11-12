import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Finding, Run, RunUpdate, Severity } from "../ipc";
import { SEVERITIES, lineLabel } from "../lib/findings";
import { Resizer, useStoredSize } from "./Resizer";
import styles from "./SkillsPanel.module.css";

interface Props {
  runs: Run[];
  headSha: string;
  /** Findings from each skill's newest run on this head. */
  findings: Finding[];
  /** Open findings from skills last run on an older head, and those runs. */
  stale: Finding[];
  staleRuns: Run[];
  /** Updates from the run in progress (or the last one this session). */
  log: RunUpdate[];
  severities: Set<Severity>;
  onSeverities: (s: Set<Severity>) => void;
  /** Name of the pinned skill, for the run button. */
  pinnedName: string | null;
  onOpenPalette: () => void;
  onRerun: (skillId: string) => void;
  onCancel: (runId: number) => void;
  onFocus: (f: Finding) => void;
  onRestore: (f: Finding) => void;
  onClose: () => void;
}

interface LogEntry {
  key: string;
  text: string;
  kind: "step" | "warning" | "tool" | "denied";
  status?: string | null;
}

function logEntries(log: RunUpdate[]): LogEntry[] {
  const out: LogEntry[] = [];
  const tools = new Map<string, LogEntry>();
  log.forEach((u, i) => {
    if (u.kind === "step") out.push({ key: `s${i}`, text: u.text, kind: u.warning ? "warning" : "step" });
    else if (u.kind === "agent") {
      const e = u.event;
      if (e.kind === "tool") {
        const t = tools.get(e.id);
        if (t) {
          if (e.title) t.text = e.title;
          if (e.status) t.status = e.status;
        } else {
          const entry: LogEntry = { key: `t${e.id}`, text: e.title ?? e.tool ?? "Tool", kind: "tool", status: e.status };
          tools.set(e.id, entry);
          out.push(entry);
        }
      } else if (e.kind === "denied") out.push({ key: `d${i}`, text: `Refused: ${e.title}`, kind: "denied" });
    }
  });
  return out;
}

function useElapsed(since: number | null) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    if (since == null) return;
    const t = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(t);
  }, [since]);
  if (since == null) return "";
  const s = Math.max(0, Math.round(now - since));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function SkillsPanel(props: Props) {
  const { runs, headSha, findings, stale, staleRuns, log, severities, onSeverities, pinnedName } = props;
  const { onOpenPalette, onRerun, onCancel, onFocus, onRestore, onClose } = props;
  const [logOpen, setLogOpen] = useState<boolean | null>(null);
  const [width, setWidth, resetWidth] = useStoredSize("prism.reviewPanelWidth", 320);
  const [logHeight, setLogHeight, resetLogHeight] = useStoredSize("prism.reviewLogHeight", 220);
  const [showDismissed, setShowDismissed] = useState(false);

  const latest = runs[0] ?? null;
  const running = latest?.status === "running" ? latest : null;
  const elapsed = useElapsed(running?.started_at ?? null);
  const entries = useMemo(() => logEntries(log), [log]);
  const showLog = logOpen ?? (!!running || latest?.status === "failed");
  const logRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries, showLog]);

  const open = findings.filter((f) => f.status === "open");
  const counts = Object.fromEntries(SEVERITIES.map((s) => [s, open.filter((f) => f.severity === s).length])) as Record<Severity, number>;
  const shown = findings.filter((f) => f.status !== "dismissed" && severities.has(f.severity));
  const dismissed = findings.filter((f) => f.status === "dismissed");
  const byPath = useMemo(() => {
    const m = new Map<string, Finding[]>();
    for (const f of shown) m.set(f.path, [...(m.get(f.path) ?? []), f]);
    return [...m.entries()];
  }, [shown]);

  return (
    <aside className={styles.panel} style={{ width }}>
      <Resizer
        edge="left"
        size={width}
        min={240}
        max={() => window.innerWidth * 0.5}
        onSize={setWidth}
        onReset={resetWidth}
      />
      <div className={styles.head}>
        <span className={styles.heading}>Auto review</span>
        <span className={styles.spacer} />
        <button className="btn" onClick={onOpenPalette} disabled={!!running} title="Choose a review (⌘R)">
          {pinnedName ? `Run ${pinnedName}` : "Run a review"} <kbd>⌘R</kbd>
        </button>
        <button className={styles.close} onClick={onClose} title="Hide">
          ×
        </button>
      </div>

      <div className={styles.scroll}>
        {!latest && (
          <div className={styles.empty}>
            Run an auto review on this pull request with <kbd>⌘R</kbd>. Its findings show up here and on the diff, where
            you can accept them as draft comments.
          </div>
        )}

        {latest && (
          <div className={styles.run} data-status={latest.status}>
            <div className={styles.runHead}>
              <span className={styles.dot} data-status={latest.status} />
              <span className={styles.runName}>{latest.skill_name || latest.skill}</span>
              <span className="muted">{latest.agent}</span>
              {latest.head_sha !== headSha && (
                <span className="muted mono" title="Ran on an earlier commit">
                  @{latest.head_sha.slice(0, 7)}
                </span>
              )}
              <span className={styles.spacer} />
              {running ? (
                <>
                  <span className="muted mono">{elapsed}</span>
                  <button className="btn ghost" onClick={() => onCancel(running.id)}>
                    Cancel
                  </button>
                </>
              ) : (
                <span className="muted">{statusText(latest)}</span>
              )}
            </div>
            {latest.error && <div className={styles.error}>{latest.error}</div>}
            {entries.length > 0 && (
              <>
                <button className={styles.logToggle} onClick={() => setLogOpen(!showLog)}>
                  {showLog ? "▾" : "▸"} Activity ({entries.length})
                </button>
                {showLog && (
                  <div className={styles.logWrap}>
                    <div ref={logRef} className={styles.log} style={{ maxHeight: logHeight }}>
                      {entries.map((e) => (
                        <div key={e.key} className={styles.entry} data-kind={e.kind} data-status={e.status ?? undefined}>
                          {e.text}
                        </div>
                      ))}
                    </div>
                    <Resizer
                      edge="bottom"
                      size={logHeight}
                      min={60}
                      max={() => window.innerHeight * 0.7}
                      onSize={setLogHeight}
                      onReset={resetLogHeight}
                    />
                  </div>
                )}
              </>
            )}
          </div>
        )}

        {findings.length > 0 && (
          <>
            <div className={styles.filters}>
              {SEVERITIES.map((s) => (
                <button
                  key={s}
                  className={styles.filter}
                  data-severity={s}
                  data-on={severities.has(s)}
                  onClick={() => {
                    const n = new Set(severities);
                    if (!n.delete(s)) n.add(s);
                    onSeverities(n);
                  }}
                >
                  {s} {counts[s]}
                </button>
              ))}
              <span className={styles.spacer} />
              <span className="muted" title="Next / previous finding">
                <kbd>]</kbd> <kbd>[</kbd>
              </span>
            </div>
            {byPath.length === 0 && <div className={styles.empty}>No open findings at these severities.</div>}
            {byPath.map(([path, fs]) => (
              <div key={path} className={styles.file}>
                <div className={`${styles.path} mono truncate`} title={path}>
                  {path}
                </div>
                {fs.map((f) => (
                  <button key={f.id} className={styles.finding} data-status={f.status} onClick={() => onFocus(f)}>
                    <span className={styles.sev} data-severity={f.severity} />
                    <span className={`${styles.ftitle} truncate`}>{f.title}</span>
                    <span className={styles.where}>{f.status === "accepted" ? "drafted" : lineLabel(f)}</span>
                  </button>
                ))}
              </div>
            ))}
            {dismissed.length > 0 && (
              <div className={styles.section}>
                <button className={styles.logToggle} onClick={() => setShowDismissed(!showDismissed)}>
                  {showDismissed ? "▾" : "▸"} {dismissed.length} dismissed
                </button>
                {showDismissed &&
                  dismissed.map((f) => (
                    <div key={f.id} className={styles.dismissed}>
                      <span className={`${styles.ftitle} truncate`} title={f.title}>
                        {f.title}
                      </span>
                      <span className={styles.where}>{f.dismiss_reason}</span>
                      <button className={styles.link} onClick={() => onRestore(f)}>
                        Restore
                      </button>
                    </div>
                  ))}
              </div>
            )}
          </>
        )}

        {staleRuns.length > 0 && (
          <div className={styles.section}>
            <div className={styles.staleHead}>From an earlier commit</div>
            {staleRuns.map((r) => {
              const n = stale.filter((f) => f.run_id === r.id).length;
              return (
                <div key={r.id} className={styles.staleRun}>
                  <span className="truncate">
                    {r.skill_name || r.skill}: {n} open {n === 1 ? "finding" : "findings"} at{" "}
                    <span className="mono">{r.head_sha.slice(0, 7)}</span>
                  </span>
                  <span className={styles.spacer} />
                  <button className="btn" disabled={!!running} onClick={() => onRerun(r.skill)}>
                    Re-run
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </aside>
  );
}

function statusText(r: Run): string {
  if (r.status === "failed") return "failed";
  if (r.status === "cancelled") return "cancelled";
  const secs = r.ended_at != null ? r.ended_at - r.started_at : null;
  return secs == null ? "done" : `done in ${secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`}`;
}
