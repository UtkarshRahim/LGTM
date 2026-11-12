import { useEffect, useMemo, useRef, useState } from "react";
import { commands, prKey, type AppError, type NeedsYou, type PrHit, type PrRef } from "../ipc";
import { ErrorNotice } from "../components/ErrorNotice";
import { PrList } from "../components/PrList";
import styles from "./Home.module.css";

interface Props {
  active: boolean;
  onOpen: (pr: PrRef, title?: string) => void;
  onSwitchAccount: () => void;
  /** Bumped when accounts or projects change, to refetch. */
  refreshKey: number;
}

type Results = { label: string; hits: PrHit[] } | null;

const SECTIONS: { key: keyof NeedsYou; title: string; hint: string; empty: string }[] = [
  { key: "blocking", title: "Blocking", hint: "Your review is requested", empty: "No review requests." },
  { key: "rereview", title: "Re-review", hint: "New commits since your review", empty: "Nothing changed since your reviews." },
];

export function Home({ active, onOpen, onSwitchAccount, refreshKey }: Props) {
  const [input, setInput] = useState("");
  const [results, setResults] = useState<Results>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [queue, setQueue] = useState<NeedsYou | null>(null);
  const [queueError, setQueueError] = useState<AppError | null>(null);
  const [selected, setSelected] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  useEffect(() => {
    if (active) inputRef.current?.focus();
  }, [active]);

  const loadQueue = () => {
    setQueueError(null);
    commands.needsYou().then((r) => (r.status === "ok" ? setQueue(r.data) : setQueueError(r.error)));
  };
  useEffect(loadQueue, [refreshKey]);

  // Every row the keyboard can reach, in display order.
  const visible: PrHit[] = useMemo(
    () => (results ? results.hits : queue ? SECTIONS.flatMap((s) => queue[s.key]) : []),
    [results, queue],
  );
  useEffect(() => setSelected(-1), [visible]);

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const inInput = (e.target as HTMLElement | null)?.closest("input, textarea, [contenteditable]");
      const down = e.key === "ArrowDown" || (!inInput && e.key === "j");
      const up = e.key === "ArrowUp" || (!inInput && e.key === "k");
      if (down || up) {
        e.preventDefault();
        if (visible.length === 0) return;
        setSelected((i) => (down ? Math.min(i + 1, visible.length - 1) : Math.max(i - 1, 0)));
      } else if (e.key === "Enter" && selected >= 0 && (!inInput || input.trim() === "")) {
        e.preventDefault();
        const hit = visible[selected];
        if (hit) onOpen(hit.summary.pr, hit.summary.title);
      } else if (e.key === "/" && !inInput) {
        e.preventDefault();
        inputRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, visible, selected, input, onOpen]);

  async function run(text: string) {
    const id = ++seq.current;
    setError(null);
    if (!text.trim()) {
      setResults(null);
      return;
    }
    setSearching(true);
    const r = await commands.search(text);
    if (id !== seq.current) return;
    setSearching(false);
    if (r.status === "error") return setError(r.error);
    if (r.data.kind === "open") {
      setInput("");
      setResults(null);
      return onOpen(r.data.pr);
    }
    const q = r.data.query;
    const label = q.kind === "number" ? `#${q.number} in your projects` : q.kind === "text" ? `Results for “${q.text}”` : "";
    setResults({ label, hits: r.data.hits });
  }

  let offset = 0;

  return (
    <div className={styles.home}>
      <form
        className={styles.searchWrap}
        onSubmit={(e) => {
          e.preventDefault();
          run(input);
        }}
      >
        <input
          ref={inputRef}
          className={styles.search}
          value={input}
          placeholder="Paste a PR link, or search #1284, owner/repo#12, or a title"
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          onPaste={(e) => {
            const text = e.clipboardData.getData("text");
            if (/\/pull\/\d+/.test(text)) {
              e.preventDefault();
              run(text);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              if (!input && !results) return inputRef.current?.blur();
              setInput("");
              setResults(null);
              setError(null);
            }
          }}
        />
        {searching && <span className={styles.spinner}>Searching…</span>}
      </form>

      <div className={styles.body}>
        {error && <ErrorNotice error={error} onSwitchAccount={onSwitchAccount} onRetry={() => run(input)} />}

        {results && (
          <section>
            <h2 className={styles.heading}>{results.label}</h2>
            {results.hits.length === 0 ? (
              <p className="muted">Nothing found.</p>
            ) : (
              <PrList hits={results.hits} onOpen={onOpen} selected={selected} />
            )}
          </section>
        )}

        {!results && (
          <>
            <div className={styles.queueHead}>
              <h2 className={styles.title}>Needs you</h2>
              <span className="muted">
                <kbd>j</kbd> <kbd>k</kbd> to move, <kbd>↵</kbd> to open
              </span>
              <button className="btn ghost" onClick={loadQueue}>
                Refresh
              </button>
            </div>
            {queueError && <ErrorNotice error={queueError} onSwitchAccount={onSwitchAccount} onRetry={loadQueue} />}
            {!queue && !queueError && <p className="muted">Loading…</p>}
            {queue &&
              SECTIONS.map((s) => {
                const hits = queue[s.key];
                const start = offset;
                offset += hits.length;
                return (
                  <section key={s.key}>
                    <h3 className={styles.heading}>
                      {s.title} <span className={styles.count}>{hits.length}</span>
                      <span className={styles.hint}>{s.hint}</span>
                    </h3>
                    {hits.length === 0 ? (
                      <p className={styles.empty}>{s.empty}</p>
                    ) : (
                      <PrList
                        key={hits.map((h) => prKey(h.summary.pr)).join()}
                        hits={hits}
                        onOpen={onOpen}
                        selected={selected - start}
                      />
                    )}
                  </section>
                );
              })}
          </>
        )}
      </div>
    </div>
  );
}
