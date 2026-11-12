import { memo, useState } from "react";
import type { Finding } from "../ipc";
import { DISMISS_REASONS, lineLabel } from "../lib/findings";
import styles from "./Finding.module.css";

interface Props {
  finding: Finding;
  /** Skill that reported it, shown in the bar. */
  skill?: string;
  focused: boolean;
  /** Off-diff findings can only be copied, not posted on a line. */
  offDiff?: boolean;
  /** Reason chips are open (toggled with `d`). */
  dismissing: boolean;
  onDismissing: (open: boolean) => void;
  onAccept: () => void;
  onEdit: () => void;
  onCopy: () => void;
  onDismiss: (reason: string) => void;
}

export const FindingCard = memo(function FindingCard(props: Props) {
  const { finding: f, skill, focused, offDiff, dismissing, onDismissing, onAccept, onEdit, onCopy, onDismiss } = props;
  const [copied, setCopied] = useState(false);
  return (
    <div className={styles.card} data-severity={f.severity} data-focused={focused} data-finding={f.id}>
      <div className={styles.bar}>
        <span className={styles.severity} data-severity={f.severity}>
          {f.severity}
        </span>
        <span className={styles.title}>{f.title}</span>
        {offDiff && <span className="muted mono">{lineLabel(f)}</span>}
        <span className={styles.spacer} />
        {skill && <span className={styles.skill}>{skill}</span>}
      </div>
      {f.body && <div className={styles.body}>{f.body}</div>}
      {f.suggestion != null && <pre className={styles.suggestion}>{f.suggestion}</pre>}
      <div className={styles.actions}>
        {dismissing ? (
          <>
            <span className="muted">Dismiss because</span>
            {DISMISS_REASONS.map((r) => (
              <button key={r} className={styles.reason} onClick={() => onDismiss(r)}>
                {r}
              </button>
            ))}
            <span className={styles.spacer} />
            <button className={styles.link} onClick={() => onDismissing(false)}>
              Cancel
            </button>
          </>
        ) : (
          <>
            {offDiff ? (
              <button
                className="btn"
                title="This line isn't in the diff, so it can't be a line comment"
                onClick={() => {
                  onCopy();
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                }}
              >
                {copied ? "Copied" : "Copy"}
              </button>
            ) : (
              <>
                <button className="btn primary" title="Add to your review as a draft (a)" onClick={onAccept}>
                  Accept
                </button>
                <button className="btn" onClick={onEdit}>
                  Edit
                </button>
              </>
            )}
            <button className="btn ghost" title="Dismiss (d)" onClick={() => onDismissing(true)}>
              Dismiss
            </button>
            {offDiff && <span className="muted">Outside the changed lines</span>}
          </>
        )}
      </div>
    </div>
  );
});
