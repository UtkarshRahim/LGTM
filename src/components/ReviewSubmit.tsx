import { useEffect, useRef, useState } from "react";
import type { ReviewEvent } from "../ipc";
import styles from "./ReviewSubmit.module.css";

interface Props {
  /** Drafts and replies that will be posted. */
  pending: number;
  /** Line drafts left behind on an older head. */
  stale: number;
  onSubmit: (event: ReviewEvent, body: string) => Promise<string | null>;
}

const EVENTS: { event: ReviewEvent; label: string; hint: string }[] = [
  { event: "COMMENT", label: "Comment", hint: "Feedback without a verdict" },
  { event: "APPROVE", label: "Approve", hint: "Good to merge" },
  { event: "REQUEST_CHANGES", label: "Request changes", hint: "Must be addressed before merging" },
];

export function ReviewSubmit({ pending, stale, onSubmit }: Props) {
  const [open, setOpen] = useState(false);
  const [event, setEvent] = useState<ReviewEvent>("COMMENT");
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const submit = async () => {
    setBusy(true);
    setError(null);
    const err = await onSubmit(event, body);
    setBusy(false);
    if (err) return setError(err);
    setBody("");
    setEvent("COMMENT");
    setOpen(false);
  };

  return (
    <div className={styles.wrap} ref={wrap}>
      <button className="btn primary" onClick={() => setOpen((o) => !o)}>
        Review{pending > 0 && <span className={styles.count}>{pending}</span>}
      </button>
      {open && (
        <div
          className={styles.panel}
          onKeyDown={(e) => {
            if (e.key === "Escape") setOpen(false);
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !busy) submit();
          }}
        >
          <textarea
            autoFocus
            className={styles.input}
            rows={5}
            placeholder="Summary (optional)"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
          <div className={styles.events}>
            {EVENTS.map((x) => (
              <label key={x.event} className={styles.event} data-on={event === x.event}>
                <input type="radio" name="event" checked={event === x.event} onChange={() => setEvent(x.event)} />
                <span>
                  {x.label}
                  <span className="muted"> · {x.hint}</span>
                </span>
              </label>
            ))}
          </div>
          {stale > 0 && (
            <div className={styles.note}>
              {stale} {stale === 1 ? "draft was" : "drafts were"} written on an earlier version and won't be posted.
            </div>
          )}
          {error && <div className={styles.error}>{error}</div>}
          <div className={styles.actions}>
            <span className="muted">
              {pending === 0 ? "No pending comments" : `${pending} pending ${pending === 1 ? "comment" : "comments"}`}
            </span>
            <button className="btn primary" disabled={busy} onClick={submit}>
              {busy ? "Submitting…" : "Submit review"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
