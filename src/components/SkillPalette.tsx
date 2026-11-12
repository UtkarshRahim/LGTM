import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { Skill, SkillsView } from "../ipc";
import styles from "./SkillPalette.module.css";

interface Props {
  view: SkillsView | null;
  error: string | null;
  /** A run is already going on this PR. */
  busy: boolean;
  onRun: (skill: Skill) => void;
  onPin: (id: string | null) => void;
  onAgent: (id: string) => void;
  onClose: () => void;
}

const SOURCES = [
  { source: "repo", title: "This repository" },
  { source: "personal", title: "Personal" },
] as const;

export function SkillPalette({ view, error, busy, onRun, onPin, onAgent, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const match = (s: Skill) => !q || s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q);
    const skills = (view?.skills ?? []).filter(match);
    return SOURCES.map((g) => ({ ...g, skills: skills.filter((s) => s.source === g.source) })).filter((g) => g.skills.length);
  }, [view, query]);
  const flat = useMemo(() => groups.flatMap((g) => g.skills), [groups]);

  // The pinned skill starts selected, so ⌘R then Enter runs it.
  const selected = flat.find((s) => s.id === sel) ?? flat.find((s) => s.id === view?.pinned) ?? flat[0];
  useEffect(() => {
    listRef.current?.querySelector(`[data-id="${CSS.escape(selected?.id ?? "")}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const agent = view?.agents.find((a) => a.id === view.agent);
  const canRun = !!selected && !busy && !!agent?.available;

  const onKey = (e: KeyboardEvent) => {
    const i = selected ? flat.indexOf(selected) : -1;
    if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
      e.preventDefault();
      if (flat.length) setSel(flat[(i + 1) % flat.length].id);
    } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
      e.preventDefault();
      if (flat.length) setSel(flat[(i - 1 + flat.length) % flat.length].id);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (canRun) onRun(selected);
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };

  return (
    <div className={styles.backdrop} onMouseDown={onClose}>
      <div className={styles.palette} onMouseDown={(e) => e.stopPropagation()} onKeyDown={onKey}>
        <input
          autoFocus
          className={styles.input}
          placeholder="Run a review skill…"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setSel(null);
          }}
        />
        <div ref={listRef} className={styles.list}>
          {error && <div className={styles.empty}>{error}</div>}
          {!error && !view && <div className={styles.empty}>Looking for skills…</div>}
          {view && view.skills.length === 0 && (
            <div className={styles.empty}>
              No skills found. Add a folder with a <span className="mono">SKILL.md</span> to{" "}
              <span className="mono">.claude/skills</span>, <span className="mono">.cursor/skills</span> or{" "}
              <span className="mono">.agents/skills</span>, in the repository or your home folder.
            </div>
          )}
          {view && view.skills.length > 0 && flat.length === 0 && <div className={styles.empty}>No skills match.</div>}
          {groups.map((g) => (
            <div key={g.source}>
              <div className={styles.group}>{g.title}</div>
              {g.skills.map((s) => {
                const pinned = view?.pinned === s.id;
                return (
                  <div
                    key={s.id}
                    data-id={s.id}
                    className={styles.item}
                    data-selected={s.id === selected?.id}
                    onMouseMove={() => sel !== s.id && setSel(s.id)}
                    onClick={() => canRun && onRun(s)}
                  >
                    <div className={styles.text}>
                      <div className={styles.name}>{s.name}</div>
                      {s.description && <div className={`${styles.desc} truncate`}>{s.description}</div>}
                    </div>
                    <button
                      className={styles.pin}
                      data-on={pinned}
                      title={pinned ? "Unpin" : "Pin: ⌘R then Enter runs it"}
                      onClick={(e) => {
                        e.stopPropagation();
                        onPin(pinned ? null : s.id);
                      }}
                    >
                      {pinned ? "★" : "☆"}
                    </button>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
        <div className={styles.footer}>
          <label className={styles.agent}>
            Agent
            <select value={view?.agent ?? ""} onChange={(e) => onAgent(e.target.value)} disabled={!view}>
              {view?.agents.map((a) => (
                <option key={a.id} value={a.id} disabled={!a.available}>
                  {a.name}
                  {a.available ? "" : " (not installed)"}
                </option>
              ))}
            </select>
          </label>
          <span className={styles.readOnly} title="Runs in a separate checkout; edits and commands are refused">
            read-only
          </span>
          <span className={styles.spacer} />
          {busy ? (
            <span className="muted">A skill is already running</span>
          ) : agent && !agent.available ? (
            <span className="muted">{agent.name} isn't installed</span>
          ) : (
            <span className="muted">
              <kbd>↵</kbd> run · <kbd>esc</kbd> close
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
