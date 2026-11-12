import { openUrl } from "@tauri-apps/plugin-opener";
import type { AppError } from "../ipc";
import { describe } from "../lib/errors";
import styles from "./ErrorNotice.module.css";

interface Props {
  error: AppError;
  onSwitchAccount: () => void;
  onRetry?: () => void;
}

export function ErrorNotice({ error, onSwitchAccount, onRetry }: Props) {
  const d = describe(error);
  return (
    <div className={styles.notice} role="alert">
      <div className={styles.title}>{d.title}</div>
      {d.detail && <div className={styles.detail}>{d.detail}</div>}
      <div className={styles.actions}>
        {d.actions.map((a) =>
          a.kind === "open_url" ? (
            <button key={a.label} className="btn primary" onClick={() => openUrl(a.url)}>
              {a.label}
            </button>
          ) : (
            <button key={a.label} className="btn primary" onClick={onSwitchAccount}>
              {a.label}
            </button>
          ),
        )}
        {onRetry && (
          <button className="btn" onClick={onRetry}>
            Try again
          </button>
        )}
      </div>
    </div>
  );
}
