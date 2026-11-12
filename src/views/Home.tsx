import { useEffect, useRef, useState } from "react";
import { commands, prKey, type AppError, type PrHit, type PrRef } from "../ipc";
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

export function Home({ active, onOpen, onSwitchAccount, refreshKey }: Props) {
  const [input, setInput] = useState("");
  const [results, setResults] = useState<Results>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [mine, setMine] = useState<PrHit[] | null>(null);
  const [mineError, setMineError] = useState<AppError | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  useEffect(() => {
    if (active) inputRef.current?.focus();
  }, [active]);

  useEffect(() => {
    setMineError(null);
    commands.involvingMe().then((r) => (r.status === "ok" ? setMine(r.data) : setMineError(r.error)));
  }, [refreshKey]);

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
              <PrList hits={results.hits} onOpen={onOpen} />
            )}
          </section>
        )}

        {!results && (
          <section>
            <h2 className={styles.heading}>Involving you</h2>
            {mineError && <ErrorNotice error={mineError} onSwitchAccount={onSwitchAccount} />}
            {!mine && !mineError && <p className="muted">Loading…</p>}
            {mine && mine.length === 0 && <p className="muted">No open pull requests involve you.</p>}
            {mine && mine.length > 0 && <PrList hits={mine} onOpen={onOpen} key={mine.map((h) => prKey(h.summary.pr)).join()} />}
          </section>
        )}
      </div>
    </div>
  );
}
