import { memo, type MouseEvent, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Draft, ReviewThread } from "../ipc";
import { DraftCard } from "./Draft";
import { ago } from "../lib/time";
import styles from "./Thread.module.css";

function openLinks(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a) return;
  e.preventDefault();
  if (a.href.startsWith("http")) openUrl(a.href);
}

interface Props {
  thread: ReviewThread;
  expanded: boolean;
  onToggle: () => void;
  /** Your queued replies to this thread. */
  replies?: Draft[];
  /** Replaces the Reply button while you're writing one. */
  composer?: ReactNode;
  resolving?: boolean;
  onResolve?: (resolved: boolean) => void;
  onReply?: () => void;
  onEditReply?: (d: Draft) => void;
  onDeleteReply?: (d: Draft) => void;
}

export const Thread = memo(function Thread({
  thread,
  expanded,
  onToggle,
  replies = NO_DRAFTS,
  composer,
  resolving,
  onResolve,
  onReply,
  onEditReply,
  onDeleteReply,
}: Props) {
  const first = thread.comments[0];
  const more = thread.comments.length - 1;
  const range =
    thread.start_line != null && thread.start_line !== thread.line
      ? `Lines ${thread.start_line}–${thread.line}`
      : thread.line != null
        ? `Line ${thread.line}`
        : null;

  if (!expanded) {
    return (
      <button className={styles.collapsed} onClick={onToggle}>
        <span className={styles.chev}>▸</span>
        {thread.resolved && <span className={styles.tag}>Resolved</span>}
        {thread.outdated && <span className={styles.tag}>Outdated</span>}
        <span className={styles.author}>{first?.author ?? "ghost"}</span>
        <span className={`${styles.preview} truncate`}>{first ? stripHtml(first.body_html) : ""}</span>
        {more > 0 && <span className="muted">+{more}</span>}
        {replies.length > 0 && <span className={styles.pending}>{replies.length} pending</span>}
      </button>
    );
  }

  return (
    <div className={styles.thread} data-resolved={thread.resolved}>
      <div className={styles.headRow}>
        <button className={styles.head} onClick={onToggle}>
          <span className={styles.chev}>▾</span>
          {thread.resolved && <span className={styles.tag}>Resolved</span>}
          {thread.outdated && <span className={styles.tag}>Outdated</span>}
          {range && <span className="muted">{range}</span>}
          {thread.line == null && thread.original_line != null && (
            <span className="muted mono">
              {thread.path}:{thread.original_line}
            </span>
          )}
        </button>
        {thread.can_resolve && onResolve && (
          <button className={styles.action} disabled={resolving} onClick={() => onResolve(!thread.resolved)}>
            {resolving ? "…" : thread.resolved ? "Unresolve" : "Resolve"}
          </button>
        )}
      </div>
      {thread.comments.map((c) => (
        <div key={c.id} className={styles.comment}>
          <div className={styles.meta}>
            <span className={styles.author}>{c.author ?? "ghost"}</span>
            <a
              className="muted"
              href={c.url}
              onClick={(e) => {
                e.preventDefault();
                openUrl(c.url);
              }}
            >
              {ago(c.created_at)} ago
            </a>
          </div>
          <div className={styles.body} onClick={openLinks} dangerouslySetInnerHTML={{ __html: c.body_html }} />
        </div>
      ))}
      {replies.map((d) => (
        <DraftCard
          key={d.id}
          draft={d}
          inline
          onEdit={onEditReply && (() => onEditReply(d))}
          onDelete={() => onDeleteReply?.(d)}
        />
      ))}
      {composer ??
        (thread.can_reply && onReply && (
          <div className={styles.replyRow}>
            <button className={styles.reply} onClick={onReply}>
              Reply…
            </button>
          </div>
        ))}
    </div>
  );
});

const NO_DRAFTS: Draft[] = [];

function stripHtml(html: string): string {
  return new DOMParser().parseFromString(html, "text/html").body.textContent?.replace(/\s+/g, " ").trim() ?? "";
}
