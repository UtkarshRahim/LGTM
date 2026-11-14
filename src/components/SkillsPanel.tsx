import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { commands, events, prKey, type ChatMessage, type Finding, type PrRef, type Run, type RunUpdate, type Severity } from "../ipc";
import { errorText } from "../lib/errors";
import { SEVERITIES, lineLabel } from "../lib/findings";
import { Resizer, useStoredSize } from "./Resizer";
import styles from "./SkillsPanel.module.css";

interface Props {
  pr: PrRef;
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
  const { pr, runs, headSha, findings, stale, staleRuns, log, severities, onSeverities, pinnedName } = props;
  const { onOpenPalette, onRerun, onCancel, onFocus, onRestore, onClose } = props;
  const [logOpen, setLogOpen] = useState<boolean | null>(null);
  const [mode, setMode] = useState<"full" | "suggestions">(() =>
    localStorage.getItem("prism.reviewMode") === "suggestions" ? "suggestions" : "full",
  );
  const [tab, setTab] = useState<"review" | "chat">(() => (localStorage.getItem("prism.sideTab") === "chat" ? "chat" : "review"));
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
        <div className={`${styles.segmented} ${styles.tabs}`}>
          <button
            data-tab="review"
            data-on={tab === "review"}
            onClick={() => {
              setTab("review");
              localStorage.setItem("prism.sideTab", "review");
            }}
          >
            Review
          </button>
          <button
            data-tab="chat"
            data-on={tab === "chat"}
            onClick={() => {
              setTab("chat");
              localStorage.setItem("prism.sideTab", "chat");
            }}
          >
            Chat
          </button>
        </div>
        <span className={styles.spacer} />
        {tab === "review" && (
          <button className="btn" onClick={onOpenPalette} disabled={!!running} title="Choose a review (⌘R)">
            {pinnedName ? `Run ${pinnedName}` : "Run a review"} <kbd>⌘R</kbd>
          </button>
        )}
        <button className={styles.close} onClick={onClose} title="Hide">
          ×
        </button>
      </div>
      {tab === "review" && latest?.report && (
        <div className={styles.modeBar}>
          <div className={styles.segmented}>
            <button
              data-on={mode === "full"}
              onClick={() => {
                setMode("full");
                localStorage.setItem("prism.reviewMode", "full");
              }}
            >
              Full review
            </button>
            <button
              data-on={mode === "suggestions"}
              onClick={() => {
                setMode("suggestions");
                localStorage.setItem("prism.reviewMode", "suggestions");
              }}
            >
              Suggestions
            </button>
          </div>
        </div>
      )}

      <div className={styles.scroll} hidden={tab !== "review"}>
        {!latest && (
          <div className={styles.empty}>
            Run an auto review on this pull request with <kbd>⌘R</kbd>. The skill's full writeup stays in this panel.
            Findings also show on the diff, where you can accept them as draft comments.
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
            {latest.report && mode === "full" && <pre className={styles.report}>{latest.report}</pre>}
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
      <Chat pr={pr} hidden={tab !== "chat"} />
    </aside>
  );
}

function Chat({ pr, hidden }: { pr: PrRef; hidden: boolean }) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<string | null>(null);
  const [chatError, setChatError] = useState<string | null>(null);
  const bottom = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    commands.prChat(pr).then((r) => {
      if (!live || r.status !== "ok") return;
      setMessages((ms) => {
        const ids = new Set(r.data.map((m) => m.id));
        return [...r.data, ...ms.filter((m) => !ids.has(m.id))];
      });
    });
    return () => {
      live = false;
    };
  }, [pr]);

  useEffect(() => {
    const un = events.chatEvent.listen(({ payload }) => {
      if (prKey(payload.pr) !== prKey(pr)) return;
      const update = payload.update;
      if (update.kind === "delta") setPending((p) => (p ?? "") + update.text);
      else if (update.kind === "done") {
        setPending(null);
        setMessages((ms) => (ms.some((m) => m.id === update.id) ? ms : [...ms, { id: update.id, role: "assistant", body: update.body }]));
      } else {
        setPending(null);
        setChatError(update.error);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [pr]);

  useLayoutEffect(() => {
    if (!hidden) bottom.current?.scrollIntoView({ block: "end" });
  }, [messages, pending, hidden, chatError]);

  async function send() {
    const text = draft.trim();
    if (!text || pending !== null) return;
    setChatError(null);
    setDraft("");
    setPending("");
    const r = await commands.askChat(pr, text);
    if (r.status === "error") {
      setPending(null);
      setDraft(text);
      setChatError(errorText(r.error));
      return;
    }
    setMessages((ms) => (ms.some((m) => m.id === r.data.id) ? ms : [...ms, r.data]));
  }

  return (
    <div className={styles.chat} hidden={hidden}>
      <div className={styles.transcript}>
        {messages.length === 0 && pending === null && (
          <div className={styles.empty}>Ask about this pull request. The title, description and diff are included.</div>
        )}
        {messages.map((m) => (
          <div key={m.id} className={styles.bubble} data-role={m.role}>
            <span className={styles.who}>{m.role === "user" ? "You" : "Agent"}</span>
            {m.body}
          </div>
        ))}
        {pending !== null && (
          <div className={styles.bubble} data-role="assistant">
            <span className={styles.who}>Agent</span>
            {pending || "…"}
          </div>
        )}
        <div ref={bottom} />
      </div>
      {chatError && <div className={styles.error}>{chatError}</div>}
      <div className={styles.composer}>
        <textarea
          value={draft}
          rows={2}
          placeholder="Ask about this pull request"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        {pending !== null ? (
          <button className="btn ghost" onClick={() => commands.cancelChat(pr)}>
            Stop
          </button>
        ) : (
          <button className="btn" data-send disabled={!draft.trim()} onClick={send}>
            Send
          </button>
        )}
      </div>
    </div>
  );
}

function statusText(r: Run): string {
  if (r.status === "failed") return "failed";
  if (r.status === "cancelled") return "cancelled";
  const secs = r.ended_at != null ? r.ended_at - r.started_at : null;
  return secs == null ? "done" : `done in ${secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`}`;
}
