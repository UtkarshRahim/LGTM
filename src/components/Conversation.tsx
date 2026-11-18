import { useEffect, useState, type MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { commands, type ConversationItem, type PrRef } from "../ipc";
import { Composer, type ComposerValue } from "./Draft";
import { Description } from "./Description";
import { errorText } from "../lib/errors";
import { ago } from "../lib/time";
import styles from "./Conversation.module.css";

const EMPTY: ComposerValue = { body: "", label: null };

const REVIEW: Record<string, string> = {
  APPROVED: "Approved",
  CHANGES_REQUESTED: "Changes requested",
  COMMENTED: "Reviewed",
  DISMISSED: "Dismissed",
};

function openLinks(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a?.href.startsWith("http")) return;
  e.preventDefault();
  openUrl(a.href);
}

function postedBody(v: ComposerValue): string {
  return v.label ? `**${v.label}:** ${v.body}` : v.body;
}

export function Conversation({ pr, body }: { pr: PrRef; body: string }) {
  const [items, setItems] = useState<ConversationItem[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [value, setValue] = useState<ComposerValue>(EMPTY);
  const [composerKey, setComposerKey] = useState(0);
  const [saving, setSaving] = useState(false);
  const [postError, setPostError] = useState<string | null>(null);

  useEffect(() => {
    let cancel = false;
    setLoading(true);
    setError(null);
    commands.prConversation(pr).then((r) => {
      if (cancel) return;
      setLoading(false);
      if (r.status === "error") {
        setError(errorText(r.error));
        return;
      }
      setTruncated(r.data.truncated);
      setItems((prev) => {
        const byId = new Map(r.data.items.map((item) => [item.id, item]));
        for (const item of prev) if (!byId.has(item.id)) byId.set(item.id, item);
        return [...byId.values()].sort((a, b) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id));
      });
    });
    return () => {
      cancel = true;
    };
  }, [pr, reload]);

  const post = async () => {
    setSaving(true);
    setPostError(null);
    const r = await commands.commentOnPr(pr, postedBody(value));
    setSaving(false);
    if (r.status === "error") {
      setPostError(errorText(r.error));
      return;
    }
    setItems((prev) =>
      [...prev.filter((item) => item.id !== r.data.id), r.data].sort((a, b) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id)),
    );
    setValue(EMPTY);
    setComposerKey((n) => n + 1);
  };

  return (
    <div className={styles.wrap}>
      <Description body={body} />
      <section className={styles.talk}>
        <div className={styles.heading}>
          <h2>Conversation</h2>
          <button type="button" className={styles.refresh} onClick={() => setReload((n) => n + 1)}>
            Refresh
          </button>
        </div>
        {loading && items.length === 0 && <div className={styles.note}>Loading comments…</div>}
        {error && <div className={styles.error}>{error}</div>}
        {!loading && !error && items.length === 0 && <div className={styles.note}>No comments yet.</div>}
        {truncated && <div className={styles.note}>Showing the latest 100 comments and reviews.</div>}
        {items.map((item) => (
          <article key={item.id} className={styles.item}>
            <header className={styles.head}>
              <span className={styles.author}>{item.author ?? "someone"}</span>
              {item.review_state && (
                <span className={styles.state} data-state={item.review_state}>
                  {REVIEW[item.review_state] ?? item.review_state}
                </span>
              )}
              <span className="muted">{ago(item.created_at)}</span>
              <button type="button" className={styles.link} onClick={() => openUrl(item.url)}>
                ↗
              </button>
            </header>
            {item.body_html.trim() !== "" && (
              <div className={styles.body} onClick={openLinks} dangerouslySetInnerHTML={{ __html: item.body_html }} />
            )}
          </article>
        ))}
        <Composer
          key={composerKey}
          plain
          value={value}
          title="Comment on this pull request"
          suggestion={null}
          saving={saving}
          error={postError}
          saveLabel="Comment"
          placeholder="Comment on this pull request"
          onChange={setValue}
          onSave={post}
          onCancel={() => {
            setValue(EMPTY);
            setPostError(null);
            setComposerKey((n) => n + 1);
          }}
        />
      </section>
    </div>
  );
}
