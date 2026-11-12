import { memo, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type Ref } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { commands, type DiffLayout, type DiffLine, type FileMeta, type PrRef, type ReviewThread } from "../ipc";
import { errorText } from "../lib/errors";
import { highlighted } from "../lib/highlight";
import { FILE_H, buildLayout, fileRows, isDynamic, prepare, rowKey, rowSize, type Prepared, type Row } from "../lib/diffRows";
import { Thread } from "./Thread";
import styles from "./DiffView.module.css";

export interface DiffViewHandle {
  scrollToFile: (fi: number) => void;
}

interface Props {
  ref?: Ref<DiffViewHandle>;
  pr: PrRef;
  files: FileMeta[];
  layout: DiffLayout;
  /** The local mirror diff is still being prepared. */
  syncing: boolean;
  threads: ReviewThread[];
  active: boolean;
  onCurrentFile: (fi: number) => void;
  onToggleLayout: () => void;
}

const BATCH = 40;
const PREFETCH_FILES = 3;
const PREFETCH_SCAN = 4000;

interface Store {
  files: FileMeta[];
  loaded: Map<string, Prepared>;
  failed: Map<string, string>;
}

const STATUS_LABEL: Record<FileMeta["status"], string> = {
  added: "A",
  deleted: "D",
  modified: "M",
  renamed: "R",
  copied: "C",
};

export function DiffView({ ref, pr, files, layout, syncing, threads, active, onCurrentFile, onToggleLayout }: Props) {
  const [store, setStore] = useState<Store>(() => ({ files, loaded: new Map(), failed: new Map() }));
  // Hunks belong to one file list; a new list (GitHub patches → local) starts over.
  if (store.files !== files) setStore({ files, loaded: new Map(), failed: new Map() });
  const current = store;
  const inflight = useRef<{ files: FileMeta[]; paths: Set<string> }>({ files, paths: new Set() });
  if (inflight.current.files !== files) inflight.current = { files, paths: new Set() };

  // Paths whose collapsed state differs from the default (generated files start collapsed).
  const [toggled, setToggled] = useState<Set<string>>(new Set());
  // Thread ids (and `outdated:<path>`) whose expanded state differs from the default.
  const [flipped, setFlipped] = useState<Set<string>>(new Set());

  const isCollapsed = useCallback(
    (m: FileMeta) => (m.kind === "generated") !== toggled.has(m.path),
    [toggled],
  );
  const toggle = <T,>(set: (f: (s: Set<T>) => Set<T>) => void, key: T) =>
    set((s) => {
      const n = new Set(s);
      if (!n.delete(key)) n.add(key);
      return n;
    });

  const threadsByPath = useMemo(() => {
    const m = new Map<string, ReviewThread[]>();
    for (const t of threads) m.set(t.path, [...(m.get(t.path) ?? []), t]);
    return m;
  }, [threads]);
  const threadById = useMemo(() => new Map(threads.map((t) => [t.id, t])), [threads]);

  const rowCache = useRef(new Map<string, { deps: unknown[]; rows: Row[] }>());
  const { rows, fileRow, hunkRows } = useMemo(() => {
    const perFile = files.map((meta, fi) => {
      const prep = current.loaded.get(meta.path);
      const ths = threadsByPath.get(meta.path) ?? NO_THREADS;
      const collapsed = isCollapsed(meta);
      const deps = [meta, fi, prep, layout, collapsed, ths, syncing];
      const hit = rowCache.current.get(meta.path);
      if (hit && hit.deps.every((d, i) => d === deps[i])) return hit.rows;
      const rows = fileRows({ fi, meta, prep, layout, collapsed, threads: ths, syncing });
      rowCache.current.set(meta.path, { deps, rows });
      return rows;
    });
    return buildLayout(perFile);
  }, [files, current.loaded, threadsByPath, isCollapsed, layout, syncing]);

  const cols = useMemo(() => {
    let c = 40;
    for (const p of current.loaded.values()) c = Math.max(c, p.cols);
    return c;
  }, [current.loaded]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const hbarRef = useRef<HTMLDivElement>(null);

  // Horizontal scrolling moves only the code (via --sx), so line numbers stay
  // put and both halves of a split view scroll together. The slim bar at the
  // bottom owns the offset; sideways wheel/trackpad input is forwarded to it.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const bar = hbarRef.current;
    if (!el || !bar) return;
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaX) <= Math.abs(e.deltaY)) return;
      e.preventDefault();
      bar.scrollLeft += e.deltaX;
    };
    const onBar = () => canvasRef.current?.style.setProperty("--sx", `${bar.scrollLeft}px`);
    el.addEventListener("wheel", onWheel, { passive: false });
    bar.addEventListener("scroll", onBar);
    return () => {
      el.removeEventListener("wheel", onWheel);
      bar.removeEventListener("scroll", onBar);
    };
  }, []);

  const estimateSize = useCallback((i: number) => rowSize(rows[i]), [rows]);
  const getItemKey = useCallback((i: number) => rowKey(rows[i]), [rows]);
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize,
    getItemKey,
    overscan: 12,
  });
  const items = v.getVirtualItems();
  const first = items[0]?.index ?? 0;
  const last = items.at(-1)?.index ?? -1;

  // Load hunks for files in (or just past) the rendered range.
  useEffect(() => {
    const busy = inflight.current.paths;
    const want: string[] = [];
    const add = (fi: number) => {
      const path = files[fi].path;
      if (!busy.has(path) && !current.failed.has(path) && !want.includes(path)) want.push(path);
    };
    for (let i = first; i <= last && want.length < BATCH; i++) if (rows[i]?.t === "pending") add(rows[i].fi);
    let extra = 0;
    for (let i = last + 1; i < rows.length && i < last + PREFETCH_SCAN && extra < PREFETCH_FILES; i++) {
      if (rows[i].t === "pending") {
        add(rows[i].fi);
        extra++;
      }
    }
    if (want.length === 0) return;
    want.forEach((p) => busy.add(p));
    const forFiles = files;
    commands.prHunks(pr, want).then((r) => {
      setStore((s) => {
        if (s.files !== forFiles) return s;
        const loaded = new Map(s.loaded);
        const failed = new Map(s.failed);
        if (r.status === "ok") {
          for (const f of r.data) loaded.set(f.path, prepare(f));
          for (const p of want) if (!loaded.has(p)) failed.set(p, "Not in this diff");
        } else {
          for (const p of want) failed.set(p, errorText(r.error));
        }
        return { files: forFiles, loaded, failed };
      });
    });
  }, [first, last, rows, files, pr, current.failed]);

  // Which file is at the top of the viewport.
  const offset = v.scrollOffset ?? 0;
  const topItem = items.find((it) => it.end > offset + 1);
  const currentFi = topItem ? rows[topItem.index].fi : 0;
  const currentRef = useRef(currentFi);
  currentRef.current = currentFi;
  useEffect(() => onCurrentFile(currentFi), [currentFi, onCurrentFile]);
  const showOverlay = !!topItem && rows[topItem.index].t !== "gap" && rows[topItem.index].t !== "file";

  const startOf = useCallback((i: number) => v.measurementsCache[i]?.start ?? 0, [v]);
  const scrollToRow = useCallback(
    (i: number, pad = 0) => v.scrollToOffset(Math.max(0, startOf(i) - pad)),
    [v, startOf],
  );
  const scrollToFile = useCallback(
    (fi: number) => {
      const i = fileRow[Math.max(0, Math.min(fi, fileRow.length - 1))];
      if (i != null) scrollToRow(i);
    },
    [fileRow, scrollToRow],
  );
  useImperativeHandle(ref, () => ({ scrollToFile }), [scrollToFile]);

  // Keep the current file in view when the layout changes.
  const prevLayout = useRef(layout);
  const scrollToFileRef = useRef(scrollToFile);
  scrollToFileRef.current = scrollToFile;
  useEffect(() => {
    if (prevLayout.current === layout) return;
    prevLayout.current = layout;
    requestAnimationFrame(() => scrollToFileRef.current(currentRef.current));
  }, [layout]);

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if ((e.target as HTMLElement | null)?.closest("input, textarea, [contenteditable]")) return;
      const top = (v.scrollOffset ?? 0) + 1;
      const fi = currentRef.current;
      switch (e.key) {
        case "j":
          scrollToFile(fi + 1);
          break;
        case "k":
          scrollToFile(startOf(fileRow[fi]) < top - 1 ? fi : fi - 1);
          break;
        case "n": {
          const next = hunkRows.find((i) => startOf(i) > top + FILE_H);
          if (next != null) scrollToRow(next, FILE_H);
          break;
        }
        case "p": {
          const prev = hunkRows.findLast((i) => startOf(i) < top + FILE_H - 2);
          if (prev != null) scrollToRow(prev, FILE_H);
          break;
        }
        case "x": {
          const meta = files[fi];
          if (!meta) return;
          toggle(setToggled, meta.path);
          if (startOf(fileRow[fi]) < top) requestAnimationFrame(() => scrollToFile(fi));
          break;
        }
        case "s":
          onToggleLayout();
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, v, files, fileRow, hunkRows, startOf, scrollToFile, scrollToRow, onToggleLayout]);

  const header = (fi: number) => {
    const meta = files[fi];
    return (
      <FileHeader
        meta={meta}
        collapsed={isCollapsed(meta)}
        threadCount={threadsByPath.get(meta.path)?.length ?? 0}
        onToggle={() => toggle(setToggled, meta.path)}
      />
    );
  };

  const renderRow = (r: Row) => {
    const meta = files[r.fi];
    const prep = current.loaded.get(meta.path);
    switch (r.t) {
      case "gap":
        return null;
      case "file":
        return header(r.fi);
      case "note":
        return <div className={styles.note}>{r.text}</div>;
      case "pending": {
        const err = current.failed.get(meta.path);
        return <div className={styles.pending}>{err ? `Couldn't load this file: ${err}` : null}</div>;
      }
      case "hunk": {
        const h = prep!.hunks[r.hi];
        return (
          <div className={styles.hunk}>
            @@ -{h.old_start},{h.old_lines} +{h.new_start},{h.new_lines} @@ <span>{h.section}</span>
          </div>
        );
      }
      case "line":
        return <UnifiedLine line={prep!.lines[r.li]} hl={prep!.hl[r.li]} />;
      case "pair":
        return (
          <div className={styles.pair}>
            <Half line={r.l == null ? null : prep!.lines[r.l]} hl={r.l == null ? null : prep!.hl[r.l]} side="old" />
            <Half line={r.r == null ? null : prep!.lines[r.r]} hl={r.r == null ? null : prep!.hl[r.r]} side="new" />
          </div>
        );
      case "thread": {
        const th = threadById.get(r.id);
        if (!th) return null;
        return (
          <div className={styles.threadRow}>
            <Thread thread={th} expanded={!th.resolved !== flipped.has(th.id)} onToggle={() => toggle(setFlipped, th.id)} />
          </div>
        );
      }
      case "outdated": {
        const key = `outdated:${meta.path}`;
        const open = flipped.has(key);
        return (
          <div className={styles.threadRow}>
            <button className={styles.outdated} onClick={() => toggle(setFlipped, key)}>
              {open ? "▾" : "▸"} {r.ids.length} {r.ids.length === 1 ? "thread" : "threads"} on lines outside this diff
            </button>
            {open &&
              r.ids.map((id) => {
                const th = threadById.get(id);
                return th ? (
                  <Thread key={id} thread={th} expanded={!th.resolved !== flipped.has(id)} onToggle={() => toggle(setFlipped, id)} />
                ) : null;
              })}
          </div>
        );
      }
    }
  };

  // Scrollable width: one code column plus the gutters beside it.
  const hbarWidth = layout === "split" ? `calc(50% + ${cols}ch + 84px)` : `calc(${cols}ch + 136px)`;

  if (files.length === 0) return <div className={styles.empty}>No files changed.</div>;

  return (
    <div className={styles.wrap}>
      <div ref={scrollRef} className={styles.scroll}>
        <div ref={canvasRef} className={styles.canvas} style={{ height: v.getTotalSize() }}>
          {items.map((it) => {
            const r = rows[it.index];
            const dynamic = isDynamic(r);
            return (
              <div
                key={it.key}
                data-index={it.index}
                ref={dynamic ? v.measureElement : undefined}
                className={styles.row}
                style={{ transform: `translateY(${it.start}px)`, height: dynamic ? undefined : it.size }}
              >
                {renderRow(r)}
              </div>
            );
          })}
        </div>
      </div>
      {showOverlay && <div className={styles.overlay}>{header(currentFi)}</div>}
      <div ref={hbarRef} className={styles.hbar}>
        <div style={{ width: hbarWidth }} />
      </div>
    </div>
  );
}

const NO_THREADS: ReviewThread[] = [];

const FileHeader = memo(function FileHeader({
  meta,
  collapsed,
  threadCount,
  onToggle,
}: {
  meta: FileMeta;
  collapsed: boolean;
  threadCount: number;
  onToggle: () => void;
}) {
  const slash = meta.path.lastIndexOf("/");
  return (
    <button className={styles.fileHeader} onClick={onToggle} title={collapsed ? "Expand (x)" : "Collapse (x)"}>
      <span className={styles.chev}>{collapsed ? "▸" : "▾"}</span>
      <span className={styles.status} data-status={meta.status}>
        {STATUS_LABEL[meta.status]}
      </span>
      <span className={`${styles.path} truncate`}>
        {meta.old_path && <span className="muted">{meta.old_path} → </span>}
        <span className="muted">{meta.path.slice(0, slash + 1)}</span>
        {meta.path.slice(slash + 1)}
      </span>
      {meta.kind !== "source" && <span className={styles.kind}>{meta.kind}</span>}
      {threadCount > 0 && (
        <span className={styles.threads}>
          {threadCount} {threadCount === 1 ? "thread" : "threads"}
        </span>
      )}
      <span className={styles.add}>+{meta.additions}</span>
      <span className={styles.del}>−{meta.deletions}</span>
    </button>
  );
});

const SIGN = { add: "+", del: "−", context: " " } as const;

const UnifiedLine = memo(function UnifiedLine({ line, hl }: { line: DiffLine; hl: number[] | null }) {
  return (
    <div className={styles.line} data-kind={line.kind}>
      <span className={styles.num}>{line.old ?? ""}</span>
      <span className={styles.num}>{line.new ?? ""}</span>
      <span className={styles.sign}>{SIGN[line.kind]}</span>
      <Code line={line} hl={hl} />
    </div>
  );
});

const Half = memo(function Half({ line, hl, side }: { line: DiffLine | null; hl: number[] | null; side: "old" | "new" }) {
  if (!line) return <div className={styles.half} data-kind="empty" />;
  return (
    <div className={styles.half} data-kind={line.kind}>
      <span className={styles.num}>{side === "old" ? line.old : line.new}</span>
      <span className={styles.sign}>{SIGN[line.kind]}</span>
      <Code line={line} hl={hl} />
    </div>
  );
});

function Code({ line, hl }: { line: DiffLine; hl: number[] | null }) {
  return (
    <span className={styles.clip}>
      <span className={styles.code}>{highlighted(line.text, hl)}</span>
    </span>
  );
}
