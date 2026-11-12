import { prKey, type PrHit, type PrRef } from "../ipc";
import styles from "./PrList.module.css";

interface Props {
  hits: PrHit[];
  onOpen: (pr: PrRef, title?: string) => void;
}

function ago(iso: string): string {
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

export function PrList({ hits, onOpen }: Props) {
  return (
    <ul className={styles.list}>
      {hits.map(({ summary: s, project_id }) => (
        <li key={prKey(s.pr)}>
          <button className={styles.item} onClick={() => onOpen(s.pr, s.title)}>
            <span className={styles.state} data-state={s.is_draft ? "DRAFT" : s.state} title={s.is_draft ? "Draft" : s.state.toLowerCase()} />
            <span className={`${styles.title} truncate`}>{s.title}</span>
            <span className={`${styles.meta} truncate`}>
              {s.pr.owner}/{s.pr.repo}#{s.pr.number}
              {s.author && ` · ${s.author}`}
              {project_id != null && <span className={styles.local}> · local</span>}
            </span>
            <span className={styles.ago}>{ago(s.updated_at)}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
