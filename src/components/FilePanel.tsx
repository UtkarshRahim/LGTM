import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { FileMeta } from "../ipc";
import styles from "./FilePanel.module.css";

interface Props {
  files: FileMeta[];
  current: number;
  onPick: (fi: number) => void;
  threadCounts: Map<string, number>;
}

type Item =
  | { t: "file"; fi: number; depth: number; name: string }
  | { t: "dir"; key: string; name: string; depth: number; open: boolean };

const ITEM_H = 24;

interface Dir {
  dirs: Map<string, Dir>;
  files: number[];
}

/** Files arrive in tree order (directories first, by name), so insertion order is kept. */
function tree(files: FileMeta[], closed: Set<string>): Item[] {
  const root: Dir = { dirs: new Map(), files: [] };
  files.forEach((f, fi) => {
    const parts = f.path.split("/");
    let d = root;
    for (const p of parts.slice(0, -1)) {
      if (!d.dirs.has(p)) d.dirs.set(p, { dirs: new Map(), files: [] });
      d = d.dirs.get(p)!;
    }
    d.files.push(fi);
  });
  const items: Item[] = [];
  const walk = (d: Dir, prefix: string, depth: number) => {
    for (const name of d.dirs.keys()) {
      // Collapse chains of single-child directories into one row: `src/lib/util`.
      let label = name;
      let sub = d.dirs.get(name)!;
      while (sub.files.length === 0 && sub.dirs.size === 1) {
        const [next, child] = [...sub.dirs.entries()][0];
        label += `/${next}`;
        sub = child;
      }
      const key = prefix + label + "/";
      const open = !closed.has(key);
      items.push({ t: "dir", key, name: label, depth, open });
      if (open) walk(sub, key, depth + 1);
    }
    for (const fi of d.files) items.push({ t: "file", fi, depth, name: files[fi].path.slice(files[fi].path.lastIndexOf("/") + 1) });
  };
  walk(root, "", 0);
  return items;
}

export const FilePanel = memo(function FilePanel({ files, current, onPick, threadCounts }: Props) {
  const [closed, setClosed] = useState<Set<string>>(() => new Set());
  const items = useMemo(() => tree(files, closed), [files, closed]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ITEM_H,
    overscan: 20,
  });

  useEffect(() => {
    const i = items.findIndex((it) => it.t === "file" && it.fi === current);
    if (i >= 0) v.scrollToIndex(i, { align: "auto" });
  }, [current, items, v]);

  const flip = (key: string) =>
    setClosed((s) => {
      const n = new Set(s);
      if (!n.delete(key)) n.add(key);
      return n;
    });

  return (
    <nav className={styles.panel}>
      <div className={styles.top}>
        <span>Files</span>
        <span className={styles.count}>{files.length}</span>
      </div>
      <div ref={scrollRef} className={styles.scroll}>
        <div style={{ height: v.getTotalSize(), position: "relative" }}>
          {v.getVirtualItems().map((vi) => {
            const it = items[vi.index];
            const style = { transform: `translateY(${vi.start}px)` };
            if (it.t !== "file") {
              return (
                <button
                  key={it.key}
                  className={styles.item}
                  data-kind={it.t}
                  style={{ ...style, paddingLeft: 10 + it.depth * 12 }}
                  onClick={() => flip(it.key)}
                >
                  <span className={styles.chev}>{it.open ? "▾" : "▸"}</span>
                  <span className="truncate">{it.name}</span>
                </button>
              );
            }
            const f = files[it.fi];
            const threads = threadCounts.get(f.path) ?? 0;
            return (
              <button
                key={f.path}
                className={styles.item}
                data-current={it.fi === current}
                data-kind={f.kind}
                style={{ ...style, paddingLeft: 24 + it.depth * 12 }}
                onClick={() => onPick(it.fi)}
                title={f.path}
              >
                <span className={styles.dot} data-status={f.status} />
                <span className={`${styles.name} truncate`}>{it.name}</span>
                {threads > 0 && <span className={styles.threads}>{threads}</span>}
              </button>
            );
          })}
        </div>
      </div>
    </nav>
  );
});
