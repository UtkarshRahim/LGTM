import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { commands, type CheckStatus, type PrChecks, type PrRef } from "../ipc";
import { errorText } from "../lib/errors";
import styles from "./Checks.module.css";

export interface CheckSummary {
  failing: number;
  pending: number;
  total: number;
}

const LABEL: Record<CheckStatus, string> = {
  failure: "Failing",
  pending: "Pending",
  success: "Passing",
  neutral: "Neutral",
  skipped: "Skipped",
};

interface Props {
  pr: PrRef;
  headSha: string;
  onSummary: (s: CheckSummary | null) => void;
}

export function Checks({ pr, headSha, onSummary }: Props) {
  const [data, setData] = useState<PrChecks | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let cancel = false;
    setLoading(true);
    setError(null);
    commands
      .prChecks(pr)
      .then((r) => {
        if (cancel) return;
        setLoading(false);
        if (r.status === "error") {
          setData(null);
          setError(errorText(r.error));
          onSummary(null);
          return;
        }
        setData(r.data);
        onSummary(summarize(r.data));
      })
      .catch((e: unknown) => {
        if (cancel) return;
        setLoading(false);
        setData(null);
        setError(e instanceof Error ? e.message : "Couldn't load checks");
        onSummary(null);
      });
    return () => {
      cancel = true;
    };
  }, [pr, headSha, reload, onSummary]);

  return (
    <div className={styles.page}>
      <div className={styles.bar}>
        <span>{loading ? "Loading checks…" : data ? summaryText(data) : "Checks"}</span>
        <button className="btn ghost" onClick={() => setReload((n) => n + 1)} disabled={loading}>
          Refresh
        </button>
      </div>
      {error && <div className={styles.error}>{error}</div>}
      {data && data.checks.length === 0 && !loading && <div className={styles.empty}>No checks on this commit.</div>}
      {data?.checks.map((c) => (
        <button key={c.name} className={styles.row} disabled={!c.url} onClick={() => c.url && openUrl(c.url)}>
          <span className={styles.dot} data-status={c.status} />
          <span className={styles.name}>{c.name}</span>
          <span className={styles.state} data-status={c.status}>
            {LABEL[c.status]}
          </span>
        </button>
      ))}
      {data?.truncated && <div className={styles.note}>Showing the first 100 checks.</div>}
    </div>
  );
}

function summarize(data: PrChecks): CheckSummary {
  return {
    failing: data.checks.filter((c) => c.status === "failure").length,
    pending: data.checks.filter((c) => c.status === "pending").length,
    total: data.checks.length,
  };
}

function summaryText(data: PrChecks): string {
  if (data.checks.length === 0) return "No checks";
  const counts: Record<CheckStatus, number> = { failure: 0, pending: 0, success: 0, neutral: 0, skipped: 0 };
  for (const c of data.checks) counts[c.status]++;
  const parts = [];
  if (counts.failure) parts.push(`${counts.failure} failing`);
  if (counts.pending) parts.push(`${counts.pending} pending`);
  if (counts.success) parts.push(`${counts.success} passing`);
  const quiet = counts.neutral + counts.skipped;
  if (quiet) parts.push(`${quiet} skipped`);
  return parts.join(" · ");
}
