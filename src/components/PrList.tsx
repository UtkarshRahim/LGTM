import { prKey, type PrHit, type PrRef } from "../ipc";
import { ago } from "../lib/time";
import styles from "./PrList.module.css";

interface Props {
  hits: PrHit[];
  onOpen: (pr: PrRef, title?: string) => void;
  /** Index of the keyboard-selected row. */
  selected?: number;
}

export function PrList({ hits, onOpen, selected }: Props) {
  return (
    <ul className={styles.list}>
      {hits.map(({ summary: s, project_id }, i) => (
        <li key={prKey(s.pr)}>
          <button
            className={styles.item}
            data-selected={i === selected}
            ref={i === selected ? (el) => el?.scrollIntoView({ block: "nearest" }) : undefined}
            onClick={() => onOpen(s.pr, s.title)}
          >
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
