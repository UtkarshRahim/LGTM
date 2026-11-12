import { memo } from "react";
import type { FileDiff } from "../ipc";
import styles from "./Diff.module.css";

const STATUS_LABEL: Record<FileDiff["status"], string> = {
  added: "A",
  deleted: "D",
  modified: "M",
  renamed: "R",
  copied: "C",
};

export const FileCard = memo(function FileCard({ file, loadingLocal }: { file: FileDiff; loadingLocal: boolean }) {
  return (
    <section className={styles.file} id={`file-${file.path}`}>
      <header className={styles.fileHeader}>
        <span className={styles.status} data-status={file.status}>
          {STATUS_LABEL[file.status]}
        </span>
        <span className={`${styles.path} truncate mono`}>
          {file.old_path && <span className="muted">{file.old_path} → </span>}
          {file.path}
        </span>
        <span className={styles.add}>+{file.additions}</span>
        <span className={styles.del}>−{file.deletions}</span>
      </header>
      {file.binary ? (
        <div className={styles.note}>Binary file</div>
      ) : file.patch_missing ? (
        <div className={styles.note}>{loadingLocal ? "Large diff, loading from the local mirror…" : "Diff not available"}</div>
      ) : file.hunks.length === 0 ? (
        <div className={styles.note}>{file.status === "renamed" ? "Renamed without changes" : "No content changes"}</div>
      ) : (
        <table className={styles.table}>
          <tbody>
            {file.hunks.map((h, hi) => (
              <HunkRows key={hi} hunk={h} />
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
});

function HunkRows({ hunk }: { hunk: FileDiff["hunks"][number] }) {
  return (
    <>
      <tr className={styles.hunkHeader}>
        <td colSpan={3} className="mono">
          @@ -{hunk.old_start},{hunk.old_lines} +{hunk.new_start},{hunk.new_lines} @@ <span>{hunk.section}</span>
        </td>
      </tr>
      {hunk.lines.map((l, i) => (
        <tr key={i} className={styles.line} data-kind={l.kind}>
          <td className={styles.num}>{l.old ?? ""}</td>
          <td className={styles.num}>{l.new ?? ""}</td>
          <td className={styles.code}>
            <span className={styles.sign}>{l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}</span>
            {l.text}
          </td>
        </tr>
      ))}
    </>
  );
}
