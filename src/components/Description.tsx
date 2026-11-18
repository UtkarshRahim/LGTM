import type { MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { renderMarkdown } from "../lib/markdown";
import styles from "./Description.module.css";

export function Description({ body }: { body: string }) {
  if (!body.trim()) return <div className={styles.empty}>No description.</div>;
  return (
    <article
      className={styles.body}
      onClick={(e: MouseEvent) => {
        const a = (e.target as HTMLElement).closest("a");
        if (!a?.href.startsWith("http")) return;
        e.preventDefault();
        openUrl(a.href);
      }}
      dangerouslySetInnerHTML={{ __html: renderMarkdown(body) }}
    />
  );
}
