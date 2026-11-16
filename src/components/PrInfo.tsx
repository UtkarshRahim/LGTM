import { useEffect, useRef, useState, type MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { commands, type Draft, type DraftTarget, type PrCheck, type PrComment, type PrContext, type PrRef, type ReviewThread } from "../ipc";
import type { ComposerValue } from "./Draft";
import { Thread } from "./Thread";
import { ago } from "../lib/time";
import { errorText } from "../lib/errors";
import styles from "./PrInfo.module.css";

interface Props {
  pr: PrRef;
  gen: number;
  threads: ReviewThread[];
  drafts: Draft[];
  onResolve: (thread: ReviewThread, resolved: boolean) => Promise<void>;
  onAddDraft: (target: DraftTarget, value: ComposerValue) => Promise<string | null>;
  onUpdateDraft: (id: number, value: ComposerValue) => Promise<string | null>;
  onDeleteDraft: (id: number) => void;
}

const BAD = new Set(["FAILURE", "ERROR", "TIMED_OUT", "STARTUP_FAILURE", "ACTION_REQUIRED", "CANCELLED", "STALE"]);
const WAIT = new Set(["PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED", "EXPECTED"]);

function tone(status: string): "bad" | "wait" | "ok" {
  if (BAD.has(status)) return "bad";
  if (WAIT.has(status)) return "wait";
  return "ok";
}

function checkLine(checks: PrCheck[]): string {
  if (checks.length === 0) return "No checks";
  let bad = 0;
  let wait = 0;
  let ok = 0;
  for (const c of checks) {
    const t = tone(c.status);
    if (t === "bad") bad++;
    else if (t === "wait") wait++;
    else ok++;
  }
  return [ok && `${ok} passed`, wait && `${wait} running`, bad && `${bad} failed`].filter(Boolean).join(", ");
}

export function PrInfo({ pr, gen, threads, drafts, onResolve, onAddDraft, onUpdateDraft, onDeleteDraft }: Props) {
  const [ctx, setCtx] = useState<PrContext | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [posting, setPosting] = useState(false);
  const [postError, setPostError] = useState<string | null>(null);
  const [shut, setShut] = useState<Set<string>>(new Set());
  const [resolving, setResolving] = useState<string | null>(null);
  const [box, setBox] = useState<{ threadId: string; draftId: number | null; body: string; saving: boolean; error: string | null } | null>(null);

  useEffect(() => {
    let dead = false;
    setError(null);
    commands.prContext(pr).then((r) => {
      if (dead) return;
      if (r.status === "ok") setCtx(r.data);
      else setError(errorText(r.error));
    });
    return () => {
      dead = true;
    };
  }, [pr, gen]);

  const post = async () => {
    const body = text.trim();
    if (!body || posting) return;
    setPosting(true);
    setPostError(null);
    const r = await commands.addPrComment(pr, body);
    setPosting(false);
    if (r.status === "error") return setPostError(errorText(r.error));
    setText("");
    setCtx((c) => (c ? { ...c, comments: [...c.comments, r.data] } : c));
  };

  const saveReply = async () => {
    if (!box || !box.body.trim() || box.saving) return;
    setBox({ ...box, saving: true, error: null });
    const value: ComposerValue = { body: box.body, label: null };
    const err = box.draftId == null ? await onAddDraft({ kind: "reply", thread_id: box.threadId }, value) : await onUpdateDraft(box.draftId, value);
    setBox((b) => (b && b.threadId === box.threadId ? (err ? { ...b, saving: false, error: err } : null) : b));
  };

  const comments = ctx?.comments.length ?? 0;
  const total = threads.length + comments;
  const summary = ctx ? `${checkLine(ctx.checks)} · ${total === 0 ? "No comments" : `${total} comments`}` : error ? "Couldn't load details" : "Loading details…";

  return (
    <details className={styles.details} open>
      <summary className={styles.summary}>{summary}</summary>
      <div className={styles.panel}>
        {error && <div className={styles.warn}>{error}</div>}
        {ctx?.warning && <div className={styles.warn}>{ctx.warning}</div>}
        {ctx && ctx.checks.length > 0 && (
          <div className={styles.checks}>
            {ctx.checks.map((c) => (
              <div key={`${c.name}:${c.status}`} className={styles.check} data-tone={tone(c.status)}>
                <span className={styles.dot} />
                {c.url ? (
                  <a
                    href={c.url}
                    onClick={(e) => {
                      e.preventDefault();
                      openUrl(c.url!);
                    }}
                  >
                    {c.name}
                  </a>
                ) : (
                  <span>{c.name}</span>
                )}
                <span className="muted">{c.status.toLowerCase().replaceAll("_", " ")}</span>
                {c.summary && <p className={styles.checkText}>{c.summary}</p>}
              </div>
            ))}
          </div>
        )}
        {ctx && ctx.body_html.trim() && (
          <Html pr={pr} html={ctx.body_html} />
        )}
        <form
          className={styles.form}
          onSubmit={(e) => {
            e.preventDefault();
            post();
          }}
        >
          <textarea
            className={styles.input}
            rows={3}
            placeholder="Comment on this pull request"
            value={text}
            disabled={!ctx || posting}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) post();
            }}
          />
          <div className={styles.formRow}>
            {postError && <span className={styles.warn}>{postError}</span>}
            <button className="btn primary" disabled={!ctx || posting || !text.trim()}>
              {posting ? "Posting…" : "Comment"}
            </button>
          </div>
        </form>
        {ctx?.comments.map((c) => (
          <Note key={c.id} pr={pr} comment={c} />
        ))}
        {threads.map((th) => {
          const replying = box?.threadId === th.id;
          const where = th.line != null ? `${th.path}:${th.line}` : undefined;
          return (
            <div key={th.id} className={styles.thread}>
              <Thread
                thread={th}
                where={where}
                expanded={!shut.has(th.id) || replying}
                onToggle={() =>
                  setShut((s) => {
                    const n = new Set(s);
                    if (!n.delete(th.id)) n.add(th.id);
                    return n;
                  })
                }
                replies={drafts.filter((d) => d.target.kind === "reply" && d.target.thread_id === th.id && d.id !== box?.draftId)}
                resolving={resolving === th.id}
                onResolve={async (on) => {
                  setResolving(th.id);
                  await onResolve(th, on);
                  setResolving(null);
                }}
                onReply={() => setBox({ threadId: th.id, draftId: null, body: "", saving: false, error: null })}
                onEditReply={(d) => setBox({ threadId: th.id, draftId: d.id, body: d.body, saving: false, error: null })}
                onDeleteReply={(d) => {
                  if (box?.draftId === d.id) setBox(null);
                  onDeleteDraft(d.id);
                }}
                composer={
                  replying ? (
                    <form
                      className={styles.form}
                      onSubmit={(e) => {
                        e.preventDefault();
                        saveReply();
                      }}
                    >
                      <textarea
                        autoFocus
                        className={styles.input}
                        rows={3}
                        placeholder="Reply, queued until you submit the review"
                        value={box.body}
                        onChange={(e) => setBox({ ...box, body: e.target.value })}
                        onKeyDown={(e) => {
                          if (e.key === "Escape") setBox(null);
                          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) saveReply();
                        }}
                      />
                      {box.error && <div className={styles.warn}>{box.error}</div>}
                      <div className={styles.formRow}>
                        <button type="button" className="btn ghost" onClick={() => setBox(null)}>
                          Cancel
                        </button>
                        <button className="btn primary" disabled={box.saving || !box.body.trim()}>
                          {box.saving ? "Saving…" : box.draftId == null ? "Queue reply" : "Save"}
                        </button>
                      </div>
                    </form>
                  ) : undefined
                }
              />
            </div>
          );
        })}
      </div>
    </details>
  );
}

function Note({ pr, comment }: { pr: PrRef; comment: PrComment }) {
  return (
    <div className={styles.note}>
      <div className={styles.meta}>
        {comment.tag && <span className={styles.tag}>{comment.tag}</span>}
        <span className={styles.author}>{comment.author ?? "ghost"}</span>
        <a
          className="muted"
          href={comment.url}
          onClick={(e) => {
            e.preventDefault();
            openUrl(comment.url);
          }}
        >
          {ago(comment.created_at)} ago
        </a>
      </div>
      <Html pr={pr} html={comment.body_html} />
    </div>
  );
}

function assetUrl(host: string, url: string): boolean {
  try {
    const u = new URL(url);
    const h = u.hostname;
    return u.protocol === "https:" && (h === host || h === "github.com" || h === "www.github.com" || h.endsWith(".githubusercontent.com"));
  } catch {
    return false;
  }
}

function Html({ pr, html }: { pr: PrRef; html: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<string, string>());
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    let dead = false;
    const load = (el: HTMLElement, src: string) => {
      const hit = cache.current.get(src);
      if (hit) {
        el.setAttribute("src", hit);
        return;
      }
      commands.prAsset(pr, src).then((r) => {
        if (dead || r.status !== "ok") return;
        const url = `data:${r.data.mime};base64,${r.data.base64}`;
        cache.current.set(src, url);
        el.setAttribute("src", url);
      });
    };
    root.querySelectorAll<HTMLAnchorElement>("a[href]").forEach((a) => {
      const href = a.getAttribute("href");
      if (!href || !assetUrl(pr.host, href) || a.querySelector("img, video") || a.nextElementSibling?.getAttribute("data-preview")) return;
      const video = /\.(mp4|webm|mov)(\?|#|$)/i.test(href);
      const image = video || /\.(png|jpe?g|gif|webp)(\?|#|$)/i.test(href) || href.includes("/user-attachments/assets/");
      if (!image) return;
      const node = document.createElement(video ? "video" : "img");
      node.setAttribute("data-preview", "1");
      if (video) node.setAttribute("controls", "");
      a.after(node);
      load(node, href);
    });
    root.querySelectorAll<HTMLElement>("img[src], video[src], source[src]").forEach((el) => {
      const src = el.getAttribute("src");
      el.removeAttribute("srcset");
      if (!src || !assetUrl(pr.host, src) || el.getAttribute("data-preview")) return;
      load(el, src);
    });
    return () => {
      dead = true;
    };
  }, [html, pr]);

  return <div ref={ref} className={styles.html} onClick={openLinks} dangerouslySetInnerHTML={{ __html: html }} />;
}

function openLinks(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a) return;
  e.preventDefault();
  if (a.href.startsWith("http")) openUrl(a.href);
}
