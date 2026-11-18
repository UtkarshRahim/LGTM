import { memo, useEffect, useRef, useState, type MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { CommentLabel, Draft } from "../ipc";
import { renderComment } from "../lib/markdown";
import styles from "./Draft.module.css";

function openMdLinks(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a?.href.startsWith("http")) return;
  e.preventDefault();
  openUrl(a.href);
}

export function CommentPreview({ body, original, className }: { body: string; original?: string | null; className?: string }) {
  if (!body.trim()) {
    return (
      <div className={className}>
        <span className="muted">Nothing to preview</span>
      </div>
    );
  }
  return (
    <div className={className ? `md ${className}` : "md"} onClick={openMdLinks} dangerouslySetInnerHTML={{ __html: renderComment(body, original ?? null) }} />
  );
}

export interface ComposerValue {
  body: string;
  label: CommentLabel | null;
}

const LABELS: CommentLabel[] = ["praise", "nit", "suggestion", "question"];

interface ComposerProps {
  value: ComposerValue;
  /** "Line 12", "Lines 10–12" or "Reply". */
  title: string;
  /** Current text of the commented lines, used to start a suggestion block. */
  suggestion: string | null;
  saving: boolean;
  error: string | null;
  saveLabel: string;
  /** Drops the indent that lines the box up with diff text. */
  plain?: boolean;
  placeholder?: string;
  onChange: (v: ComposerValue) => void;
  onSave: () => void;
  onCancel: () => void;
}

export function Composer({ value, title, suggestion, saving, error, saveLabel, plain, placeholder, onChange, onSave, onCancel }: ComposerProps) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const [mode, setMode] = useState<"write" | "preview">("write");
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);

  const pick = (l: CommentLabel) => {
    const label = value.label === l ? null : l;
    let body = value.body;
    if (label === "suggestion" && suggestion != null && !body.includes("```suggestion")) {
      body = `${body}${body && !body.endsWith("\n") ? "\n" : ""}\`\`\`suggestion\n${suggestion}\n\`\`\`\n`;
    }
    onChange({ body, label });
    ref.current?.focus();
  };

  return (
    <div
      className={plain ? `${styles.card} ${styles.plain}` : styles.card}
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          if (value.body.trim() && !saving) onSave();
        } else if (e.key === "Escape") {
          e.preventDefault();
          onCancel();
        }
      }}
    >
      <div className={styles.bar}>
        <span className="muted">{title}</span>
        <button type="button" className={styles.chip} data-on={mode === "write"} onClick={() => setMode("write")}>
          Write
        </button>
        <button type="button" className={styles.chip} data-on={mode === "preview"} onClick={() => setMode("preview")}>
          Preview
        </button>
        <span className={styles.spacer} />
        {LABELS.map((l) => (
          <button key={l} className={styles.chip} data-label={l} data-on={value.label === l} onClick={() => pick(l)}>
            {l}
          </button>
        ))}
      </div>
      {mode === "write" ? (
        <textarea
          ref={ref}
          className={styles.input}
          rows={Math.min(14, Math.max(3, value.body.split("\n").length + 1))}
          placeholder={placeholder ?? "Leave a comment"}
          value={value.body}
          onChange={(e) => onChange({ ...value, body: e.target.value })}
        />
      ) : (
        <CommentPreview body={value.body} original={suggestion} className={styles.preview} />
      )}
      {error && <div className={styles.error}>{error}</div>}
      <div className={styles.actions}>
        <span className="muted">⌘↵ to save · Esc to cancel</span>
        <span className={styles.spacer} />
        <button className="btn ghost" onClick={onCancel}>
          Cancel
        </button>
        <button className="btn primary" disabled={!value.body.trim() || saving} onClick={onSave}>
          {saving ? "Saving…" : saveLabel}
        </button>
      </div>
    </div>
  );
}

interface CardProps {
  draft: Draft;
  /** Shown above the body, e.g. the line for drafts listed out of place. */
  where?: string;
  /** Replies sit inside their thread and drop the card chrome. */
  inline?: boolean;
  onEdit?: () => void;
  onDelete: () => void;
}

export const DraftCard = memo(function DraftCard({ draft, where, inline, onEdit, onDelete }: CardProps) {
  return (
    <div className={inline ? styles.inline : styles.card} data-draft>
      <div className={styles.bar}>
        <span className={styles.pending}>Pending</span>
        {draft.label && (
          <span className={styles.chip} data-label={draft.label} data-on>
            {draft.label}
          </span>
        )}
        {where && <span className="muted mono">{where}</span>}
        <span className={styles.spacer} />
        {onEdit && (
          <button className={styles.link} onClick={onEdit}>
            Edit
          </button>
        )}
        <button className={styles.link} onClick={onDelete}>
          Delete
        </button>
      </div>
      <CommentPreview body={draft.body} className={styles.body} />
    </div>
  );
});

export function rangeTitle(start: number | null, line: number): string {
  return start != null && start !== line ? `Lines ${start}–${line}` : `Line ${line}`;
}
