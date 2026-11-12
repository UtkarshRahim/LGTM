import {
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type Ref,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  commands,
  type DiffLayout,
  type DiffLine,
  type DiffScope,
  type Draft,
  type DraftTarget,
  type FileMeta,
  type Finding,
  type PrRef,
  type ReviewThread,
} from "../ipc";
import { DISMISS_REASONS, findingDraft, findingLine, findingText } from "../lib/findings";
import { FindingCard } from "./Finding";
import { errorText } from "../lib/errors";
import { highlighted } from "../lib/highlight";
import {
  FILE_H,
  anchorOf,
  buildLayout,
  fileRows,
  isDynamic,
  prepare,
  rowKey,
  rowSize,
  type Anchor,
  type LineDraft,
  type Prepared,
  type Row,
} from "../lib/diffRows";
import { Composer, DraftCard, rangeTitle, type ComposerValue } from "./Draft";
import { Thread } from "./Thread";
import styles from "./DiffView.module.css";

export interface DiffViewHandle {
  scrollToFile: (fi: number) => void;
  focusFinding: (id: number) => void;
}

interface Props {
  ref?: Ref<DiffViewHandle>;
  pr: PrRef;
  files: FileMeta[];
  layout: DiffLayout;
  /** The local mirror diff is still being prepared. */
  syncing: boolean;
  scope: DiffScope;
  headSha: string;
  threads: ReviewThread[];
  drafts: Draft[];
  /** Paths marked viewed whose change hasn't moved since. */
  viewed: Set<string>;
  active: boolean;
  onCurrentFile: (fi: number) => void;
  onToggleLayout: () => void;
  onViewed: (meta: FileMeta, viewed: boolean) => void;
  /** Each returns an error message, or null on success. */
  onAddDraft: (target: DraftTarget, value: ComposerValue) => Promise<string | null>;
  onUpdateDraft: (id: number, value: ComposerValue) => Promise<string | null>;
  onDeleteDraft: (id: number) => void;
  onResolve: (thread: ReviewThread, resolved: boolean) => Promise<void>;
  /** Open findings for this head that pass the severity filter. */
  findings: Finding[];
  /** Skill name by run id. */
  runSkills: Map<number, string>;
  /** Returns an error message, or null once the finding is a draft. */
  onAcceptFinding: (f: Finding, value?: ComposerValue) => Promise<string | null>;
  onDismissFinding: (f: Finding, reason: string) => void;
}

type ComposerTarget =
  | { mode: "new"; path: string; anchor: Anchor; start: number | null }
  | { mode: "edit"; id: number }
  | { mode: "reply"; threadId: string }
  | { mode: "finding"; id: number };

interface ComposerState {
  target: ComposerTarget;
  value: ComposerValue;
  saving: boolean;
  error: string | null;
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

export function DiffView(props: Props) {
  const { ref, pr, files, layout, syncing, scope, headSha, threads, drafts, viewed, active } = props;
  const { onCurrentFile, onToggleLayout, onViewed, onAddDraft, onUpdateDraft, onDeleteDraft, onResolve } = props;
  const { findings, runSkills, onAcceptFinding, onDismissFinding } = props;
  const rightOnly = scope === "since";
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
    (m: FileMeta) => (m.kind === "generated" || viewed.has(m.path)) !== toggled.has(m.path),
    [toggled, viewed],
  );
  const setViewed = useCallback(
    (m: FileMeta, on: boolean) => {
      setToggled((s) => {
        if (!s.has(m.path)) return s;
        const n = new Set(s);
        n.delete(m.path);
        return n;
      });
      onViewed(m, on);
    },
    [onViewed],
  );

  const [composer, setComposer] = useState<ComposerState | null>(null);
  const [resolving, setResolving] = useState<Set<string>>(new Set());
  // Last gutter click, so shift-click can extend it into a range.
  const lastPick = useRef<{ path: string; anchor: Anchor; hi: number } | null>(null);
  const hovered = useRef<Picked | null>(null);
  // Dragging down (or up) the gutter "+" picks a range of lines.
  const [drag, setDrag] = useState<{ from: Picked; to: Picked } | null>(null);
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
  const { draftsByPath, repliesByThread, draftById } = useMemo(() => {
    const draftsByPath = new Map<string, LineDraft[]>();
    const repliesByThread = new Map<string, Draft[]>();
    for (const d of drafts) {
      if (d.target.kind === "line") {
        const p = d.target.path;
        draftsByPath.set(p, [...(draftsByPath.get(p) ?? []), d as LineDraft]);
      } else {
        const t = d.target.thread_id;
        repliesByThread.set(t, [...(repliesByThread.get(t) ?? []), d]);
      }
    }
    return { draftsByPath, repliesByThread, draftById: new Map(drafts.map((d) => [d.id, d])) };
  }, [drafts]);
  const composeNew = composer?.target.mode === "new" ? composer.target : null;

  const [focused, setFocused] = useState<number | null>(null);
  const [dismissing, setDismissing] = useState<number | null>(null);
  const pathIndex = useMemo(() => new Map(files.map((f, i) => [f.path, i])), [files]);
  const { findingsByPath, findingById, findingOrder } = useMemo(() => {
    const findingsByPath = new Map<string, Finding[]>();
    for (const f of findings) findingsByPath.set(f.path, [...(findingsByPath.get(f.path) ?? []), f]);
    const findingOrder = findings
      .filter((f) => pathIndex.has(f.path))
      .sort((a, b) => pathIndex.get(a.path)! - pathIndex.get(b.path)! || (findingLine(a) ?? 0) - (findingLine(b) ?? 0));
    return { findingsByPath, findingById: new Map(findings.map((f) => [f.id, f])), findingOrder };
  }, [findings, pathIndex]);

  const rowCache = useRef(new Map<string, { deps: unknown[]; rows: Row[] }>());
  const { rows, fileRow, hunkRows } = useMemo(() => {
    const perFile = files.map((meta, fi) => {
      const prep = current.loaded.get(meta.path);
      const ths = threadsByPath.get(meta.path) ?? NO_THREADS;
      const ds = draftsByPath.get(meta.path) ?? NO_DRAFTS;
      const fs = findingsByPath.get(meta.path) ?? NO_FINDINGS;
      const collapsed = isCollapsed(meta);
      const compose = composeNew?.path === meta.path ? composeNew.anchor : null;
      const deps = [meta, fi, prep, layout, collapsed, ths, ds, fs, compose, headSha, rightOnly, syncing];
      const hit = rowCache.current.get(meta.path);
      if (hit && hit.deps.every((d, i) => d === deps[i])) return hit.rows;
      const rows = fileRows({
        fi,
        meta,
        prep,
        layout,
        collapsed,
        threads: ths,
        drafts: ds,
        findings: fs,
        headSha,
        compose,
        rightOnly,
        syncing,
      });
      rowCache.current.set(meta.path, { deps, rows });
      return rows;
    });
    return buildLayout(perFile);
  }, [files, current.loaded, threadsByPath, draftsByPath, findingsByPath, isCollapsed, composeNew, headSha, rightOnly, layout, syncing]);

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
    commands.prHunks(pr, scope, want).then((r) => {
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
  }, [first, last, rows, files, pr, scope, current.failed]);

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

  // Focusing a finding expands its file, scrolls to it, and keeps trying
  // while the file's hunks load.
  const pendingFocus = useRef<{ id: number; scrolledToFile: boolean } | null>(null);
  const [focusTick, setFocusTick] = useState(0);
  const focusFinding = useCallback(
    (id: number) => {
      const f = findingById.get(id);
      const fi = f && pathIndex.get(f.path);
      if (!f || fi == null) return;
      if (isCollapsed(files[fi])) toggle(setToggled, f.path);
      setFocused(id);
      setDismissing(null);
      pendingFocus.current = { id, scrolledToFile: false };
      setFocusTick((t) => t + 1);
    },
    [findingById, pathIndex, isCollapsed, files],
  );
  useEffect(() => {
    const p = pendingFocus.current;
    if (!p) return;
    const i = rows.findIndex((r) => (r.t === "finding" && r.id === p.id) || (r.t === "offdiff" && r.ids.includes(p.id)));
    if (i !== -1) {
      pendingFocus.current = null;
      requestAnimationFrame(() => scrollToRow(i, FILE_H + 60));
      return;
    }
    const f = findingById.get(p.id);
    const fi = f && pathIndex.get(f.path);
    if (fi == null) {
      pendingFocus.current = null;
    } else if (!p.scrolledToFile) {
      p.scrolledToFile = true;
      scrollToFile(fi);
    }
  }, [rows, focusTick, findingById, pathIndex, scrollToFile, scrollToRow]);
  useImperativeHandle(ref, () => ({ scrollToFile, focusFinding }), [scrollToFile, focusFinding]);

  const placedFindings = useMemo(() => new Set(rows.flatMap((r) => (r.t === "finding" ? [r.id] : []))), [rows]);
  // Navigation follows the screen: a loaded file lists its off-diff findings first.
  const findingNav = useMemo(() => {
    const key = (f: Finding) => (current.loaded.has(f.path) && !placedFindings.has(f.id) ? -1 : (findingLine(f) ?? -1));
    return [...findingOrder].sort((a, b) => pathIndex.get(a.path)! - pathIndex.get(b.path)! || key(a) - key(b));
  }, [findingOrder, placedFindings, current.loaded, pathIndex]);
  /** Moves focus past a finding that's about to leave the list. */
  const focusAfter = useCallback(
    (id: number) => {
      const i = findingNav.findIndex((f) => f.id === id);
      const next = findingNav[i + 1] ?? findingNav[i - 1];
      setFocused(next && next.id !== id ? next.id : null);
      setDismissing(null);
    },
    [findingNav],
  );
  const acceptFinding = useCallback(
    async (f: Finding) => {
      focusAfter(f.id);
      const err = await onAcceptFinding(f);
      if (err) setFocused(f.id);
    },
    [focusAfter, onAcceptFinding],
  );
  const dismissFinding = useCallback(
    (f: Finding, reason: string) => {
      focusAfter(f.id);
      onDismissFinding(f, reason);
    },
    [focusAfter, onDismissFinding],
  );
  const editFinding = useCallback((f: Finding) => {
    const d = findingDraft(f);
    setFocused(f.id);
    setComposer({ target: { mode: "finding", id: f.id }, value: { body: d.body, label: d.label }, saving: false, error: null });
  }, []);

  // Keep the current file in view when the layout changes.
  const prevLayout = useRef(layout);
  const scrollToFileRef = useRef(scrollToFile);
  scrollToFileRef.current = scrollToFile;
  useEffect(() => {
    if (prevLayout.current === layout) return;
    prevLayout.current = layout;
    requestAnimationFrame(() => scrollToFileRef.current(currentRef.current));
  }, [layout]);

  const openNew = useCallback(
    ({ fi, anchor, hi }: Picked, extend: boolean, from?: Picked) => {
      const path = files[fi].path;
      const prev = from ? { path, anchor: from.anchor, hi: from.hi } : extend ? lastPick.current : null;
      let start: number | null = null;
      let end = anchor;
      if (prev && prev.path === path && prev.hi === hi && prev.anchor.side === anchor.side && prev.anchor.line !== anchor.line) {
        start = Math.min(prev.anchor.line, anchor.line);
        end = { side: anchor.side, line: Math.max(prev.anchor.line, anchor.line) };
      }
      lastPick.current = { path, anchor: from?.anchor ?? anchor, hi };
      setComposer((c) => ({
        target: { mode: "new", path, anchor: end, start },
        value: c?.target.mode === "new" ? c.value : EMPTY,
        saving: false,
        error: null,
      }));
    },
    [files],
  );

  const save = useCallback(async () => {
    const c = composer;
    if (!c) return;
    setComposer({ ...c, saving: true, error: null });
    let err: string | null;
    if (c.target.mode === "edit") err = await onUpdateDraft(c.target.id, c.value);
    else if (c.target.mode === "finding") {
      const f = findingById.get(c.target.id);
      err = f ? await onAcceptFinding(f, c.value) : "This finding is gone";
    } else {
      const target: DraftTarget =
        c.target.mode === "reply"
          ? { kind: "reply", thread_id: c.target.threadId }
          : {
              kind: "line",
              path: c.target.path,
              line: c.target.anchor.line,
              side: c.target.anchor.side,
              start_line: c.target.start,
              start_side: c.target.start == null ? null : c.target.anchor.side,
            };
      err = await onAddDraft(target, c.value);
    }
    setComposer((now) => (now?.target !== c.target ? now : err ? { ...now, saving: false, error: err } : null));
  }, [composer, onAddDraft, onUpdateDraft, onAcceptFinding, findingById]);
  const cancel = useCallback(() => setComposer(null), []);
  const editDraft = useCallback(
    (d: Draft) => setComposer({ target: { mode: "edit", id: d.id }, value: { body: d.body, label: d.label }, saving: false, error: null }),
    [],
  );
  const reply = useCallback(
    (threadId: string) => setComposer({ target: { mode: "reply", threadId }, value: EMPTY, saving: false, error: null }),
    [],
  );
  const resolve = useCallback(
    async (th: ReviewThread, on: boolean) => {
      setResolving((s) => new Set(s).add(th.id));
      await onResolve(th, on);
      setResolving((s) => {
        const n = new Set(s);
        n.delete(th.id);
        return n;
      });
    },
    [onResolve],
  );

  // The gutter "+" and `c` find their line through data attributes, so line
  // components stay memoized.
  const onCanvasOver = (e: MouseEvent) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-a]");
    const a = el ? parseAnchor(el) : null;
    hovered.current = a;
    if (a && drag && sameHunk(a, drag.from) && a.anchor.line !== drag.to.anchor.line) setDrag({ ...drag, to: a });
  };
  const onCanvasDown = (e: MouseEvent) => {
    if (e.button !== 0) return;
    const plus = (e.target as HTMLElement).closest(`.${styles.plus}`);
    const el = plus?.closest<HTMLElement>("[data-a]");
    const a = el && parseAnchor(el);
    if (!a) return;
    e.preventDefault();
    if (e.shiftKey) openNew(a, true);
    else setDrag({ from: a, to: a });
  };
  const dragRef = useRef(drag);
  dragRef.current = drag;
  const dragging = drag != null;
  useEffect(() => {
    if (!dragging) return;
    const up = () => {
      const d = dragRef.current;
      setDrag(null);
      if (d) openNew(d.to, false, d.from);
    };
    window.addEventListener("mouseup", up);
    return () => window.removeEventListener("mouseup", up);
  }, [dragging, openNew]);

  // Lines highlighted as picked: the drag in progress, or the open composer's range.
  const picked = useMemo(() => {
    if (drag) {
      const [a, b] = [drag.from.anchor.line, drag.to.anchor.line];
      return { fi: drag.from.fi, side: drag.from.anchor.side, from: Math.min(a, b), to: Math.max(a, b) };
    }
    if (!composeNew) return null;
    const fi = pathIndex.get(composeNew.path);
    const { side, line } = composeNew.anchor;
    return fi == null ? null : { fi, side, from: composeNew.start ?? line, to: line };
  }, [drag, composeNew, pathIndex]);
  const isPicked = (fi: number, line: DiffLine | null, side?: "old" | "new") => {
    if (!picked || picked.fi !== fi || !line) return false;
    const a = anchorOf(line, side);
    return !!a && a.side === picked.side && a.line >= picked.from && a.line <= picked.to;
  };

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if ((e.target as HTMLElement | null)?.closest("input, textarea, [contenteditable]")) return;
      const top = (v.scrollOffset ?? 0) + 1;
      const fi = currentRef.current;
      const focusedF = focused == null ? undefined : findingById.get(focused);
      if (dismissing != null && focusedF) {
        const reason = DISMISS_REASONS[Number(e.key) - 1];
        if (reason) dismissFinding(focusedF, reason);
        else if (e.key === "Escape") setDismissing(null);
        else return;
        e.preventDefault();
        return;
      }
      switch (e.key) {
        case "]":
        case "[": {
          if (findingNav.length === 0) return;
          const i = findingNav.findIndex((f) => f.id === focused);
          let next: Finding | undefined;
          if (i === -1) {
            next =
              e.key === "]"
                ? (findingNav.find((f) => pathIndex.get(f.path)! >= fi) ?? findingNav[0])
                : (findingNav.findLast((f) => pathIndex.get(f.path)! <= fi) ?? findingNav.at(-1));
          } else {
            const n = findingNav.length;
            next = findingNav[(i + (e.key === "]" ? 1 : n - 1)) % n];
          }
          if (next) focusFinding(next.id);
          break;
        }
        case "a":
          if (!focusedF || !placedFindings.has(focusedF.id)) return;
          acceptFinding(focusedF);
          break;
        case "d":
          if (!focusedF) return;
          setDismissing(focusedF.id);
          break;
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
        case "v": {
          const meta = files[fi];
          if (!meta) return;
          const on = !viewed.has(meta.path);
          setViewed(meta, on);
          if (on && startOf(fileRow[fi]) < top) requestAnimationFrame(() => scrollToFile(fi));
          break;
        }
        case "c":
        case "C": {
          const h = hovered.current;
          if (!h) return;
          openNew(h, e.shiftKey);
          break;
        }
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    active,
    v,
    files,
    fileRow,
    hunkRows,
    startOf,
    scrollToFile,
    scrollToRow,
    onToggleLayout,
    viewed,
    setViewed,
    openNew,
    focused,
    dismissing,
    findingById,
    findingNav,
    pathIndex,
    placedFindings,
    focusFinding,
    acceptFinding,
    dismissFinding,
  ]);

  const header = (fi: number) => {
    const meta = files[fi];
    return (
      <FileHeader
        meta={meta}
        collapsed={isCollapsed(meta)}
        viewed={viewed.has(meta.path)}
        threadCount={threadsByPath.get(meta.path)?.length ?? 0}
        onToggle={() => toggle(setToggled, meta.path)}
        onViewed={(on) => setViewed(meta, on)}
      />
    );
  };

  const composerFor = (title: string, suggestion: string | null, saveLabel: string) =>
    composer && (
      <Composer
        value={composer.value}
        title={title}
        suggestion={suggestion}
        saving={composer.saving}
        error={composer.error}
        saveLabel={saveLabel}
        onChange={(value) => setComposer((c) => c && { ...c, value })}
        onSave={save}
        onCancel={cancel}
      />
    );

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
      case "line": {
        const line = prep!.lines[r.li];
        return (
          <UnifiedLine
            fi={r.fi}
            hi={prep!.lineHunk[r.li]}
            line={line}
            hl={prep!.hl[r.li]}
            rightOnly={rightOnly}
            picked={isPicked(r.fi, line)}
          />
        );
      }
      case "pair": {
        const hi = prep!.lineHunk[r.l ?? r.r!];
        const left = r.l == null ? null : prep!.lines[r.l];
        const right = r.r == null ? null : prep!.lines[r.r];
        return (
          <div className={styles.pair}>
            <Half
              fi={r.fi}
              hi={hi}
              line={left}
              hl={r.l == null ? null : prep!.hl[r.l]}
              side="old"
              rightOnly={rightOnly}
              picked={isPicked(r.fi, left, "old")}
            />
            <Half
              fi={r.fi}
              hi={hi}
              line={right}
              hl={r.r == null ? null : prep!.hl[r.r]}
              side="new"
              rightOnly={rightOnly}
              picked={isPicked(r.fi, right, "new")}
            />
          </div>
        );
      }
      case "thread": {
        const th = threadById.get(r.id);
        if (!th) return null;
        return <div className={styles.threadRow}>{threadEl(th)}</div>;
      }
      case "draft": {
        const d = draftById.get(r.id);
        if (!d || d.target.kind !== "line") return null;
        const editing = composer?.target.mode === "edit" && composer.target.id === d.id;
        return (
          <div className={styles.threadRow}>
            {editing ? (
              composerFor(rangeTitle(d.target.start_line, d.target.line), null, "Save")
            ) : (
              <DraftCard draft={d} onEdit={() => editDraft(d)} onDelete={() => onDeleteDraft(d.id)} />
            )}
          </div>
        );
      }
      case "stale":
        return (
          <div className={styles.threadRow}>
            <div className={styles.staleNote}>
              {r.ids.length === 1 ? "This draft was" : "These drafts were"} written on an earlier version and won't be submitted
            </div>
            {r.ids.map((id) => {
              const d = draftById.get(id);
              if (!d || d.target.kind !== "line") return null;
              const where = `${d.target.side === "LEFT" ? "old " : ""}${rangeTitle(d.target.start_line, d.target.line).toLowerCase()}`;
              return <DraftCard key={id} draft={d} where={where} onDelete={() => onDeleteDraft(id)} />;
            })}
          </div>
        );
      case "compose": {
        if (!composeNew || !prep) return null;
        const { anchor, start } = composeNew;
        return (
          <div className={styles.threadRow}>
            {composerFor(rangeTitle(start, anchor.line), suggestionText(prep, anchor, start), "Add to review")}
          </div>
        );
      }
      case "finding": {
        const f = findingById.get(r.id);
        if (!f) return null;
        const editing = composer?.target.mode === "finding" && composer.target.id === f.id;
        return (
          <div className={styles.threadRow}>
            {editing ? composerFor(`Accept ${rangeTitle(f.end_line != null ? f.line : null, findingLine(f) ?? 0).toLowerCase()}`, null, "Add to review") : findingEl(f)}
          </div>
        );
      }
      case "offdiff":
        return (
          <div className={styles.threadRow}>
            <div className={styles.staleNote}>
              {r.ids.length === 1 ? "A finding is" : `${r.ids.length} findings are`} on lines outside this diff
            </div>
            {r.ids.map((id) => {
              const f = findingById.get(id);
              return f ? <div key={id}>{findingEl(f, true)}</div> : null;
            })}
          </div>
        );
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
                return th ? <div key={id}>{threadEl(th)}</div> : null;
              })}
          </div>
        );
      }
    }
  };

  function findingEl(f: Finding, offDiff = false) {
    return (
      <FindingCard
        finding={f}
        skill={runSkills.get(f.run_id)}
        focused={focused === f.id}
        offDiff={offDiff}
        dismissing={dismissing === f.id}
        onDismissing={(open) => {
          setFocused(f.id);
          setDismissing(open ? f.id : null);
        }}
        onAccept={() => acceptFinding(f)}
        onEdit={() => editFinding(f)}
        onCopy={() => navigator.clipboard.writeText(findingText(f))}
        onDismiss={(reason) => dismissFinding(f, reason)}
      />
    );
  }

  function threadEl(th: ReviewThread) {
    const replying = composer?.target.mode === "reply" && composer.target.threadId === th.id;
    const editingReply = composer?.target.mode === "edit" ? composer.target.id : null;
    const replies = repliesByThread.get(th.id) ?? NO_DRAFTS;
    return (
      <Thread
        thread={th}
        expanded={!th.resolved !== flipped.has(th.id) || replying}
        onToggle={() => toggle(setFlipped, th.id)}
        replies={replies.filter((d) => d.id !== editingReply)}
        composer={
          replying
            ? composerFor("Reply", null, "Queue reply")
            : replies.some((d) => d.id === editingReply)
              ? composerFor("Reply", null, "Save")
              : undefined
        }
        resolving={resolving.has(th.id)}
        onResolve={(on) => resolve(th, on)}
        onReply={() => reply(th.id)}
        onEditReply={editDraft}
        onDeleteReply={(d) => onDeleteDraft(d.id)}
      />
    );
  }

  // Scrollable width: one code column plus the gutters beside it.
  const hbarWidth = layout === "split" ? `calc(50% + ${cols}ch + 84px)` : `calc(${cols}ch + 136px)`;

  if (files.length === 0) return <div className={styles.empty}>No files changed.</div>;

  return (
    <div className={styles.wrap}>
      <div ref={scrollRef} className={styles.scroll}>
        <div
          ref={canvasRef}
          className={styles.canvas}
          style={{ height: v.getTotalSize() }}
          data-dragging={dragging}
          onMouseOver={onCanvasOver}
          onMouseDown={onCanvasDown}
        >
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
const NO_DRAFTS: LineDraft[] = [];
const NO_FINDINGS: Finding[] = [];
const EMPTY: ComposerValue = { body: "", label: null };

/** A commentable line under the pointer: its file, anchor and hunk. */
interface Picked {
  fi: number;
  anchor: Anchor;
  hi: number;
}

function parseAnchor(el: HTMLElement): Picked | null {
  const [fi, side, line, hi] = (el.dataset.a ?? "").split(":");
  if (side !== "LEFT" && side !== "RIGHT") return null;
  return { fi: Number(fi), anchor: { side, line: Number(line) }, hi: Number(hi) };
}

const sameHunk = (a: Picked, b: Picked) => a.fi === b.fi && a.hi === b.hi && a.anchor.side === b.anchor.side;

/** The commented lines as they read now, for a ```suggestion block. */
function suggestionText(prep: Prepared, anchor: Anchor, start: number | null): string | null {
  if (anchor.side !== "RIGHT") return null;
  const from = start ?? anchor.line;
  const lines = prep.lines.filter((l) => l.new != null && l.new >= from && l.new <= anchor.line && l.kind !== "del");
  return lines.map((l) => l.text).join("\n");
}

const FileHeader = memo(function FileHeader({
  meta,
  collapsed,
  viewed,
  threadCount,
  onToggle,
  onViewed,
}: {
  meta: FileMeta;
  collapsed: boolean;
  viewed: boolean;
  threadCount: number;
  onToggle: () => void;
  onViewed: (on: boolean) => void;
}) {
  const slash = meta.path.lastIndexOf("/");
  return (
    <div
      role="button"
      className={styles.fileHeader}
      data-viewed={viewed}
      onClick={onToggle}
      title={collapsed ? "Expand (x)" : "Collapse (x)"}
    >
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
      <label className={styles.viewed} title="Mark viewed (v)" onClick={(e) => e.stopPropagation()}>
        <input type="checkbox" checked={viewed} onChange={(e) => onViewed(e.target.checked)} />
        Viewed
      </label>
    </div>
  );
});

const SIGN = { add: "+", del: "−", context: " " } as const;

interface LineProps {
  fi: number;
  hi: number;
  line: DiffLine;
  hl: number[] | null;
  rightOnly: boolean;
  /** Part of the range being commented on. */
  picked: boolean;
}

function anchorAttr(fi: number, hi: number, line: DiffLine, rightOnly: boolean, side?: "old" | "new"): string | undefined {
  const a = anchorOf(line, side);
  if (!a || (rightOnly && a.side !== "RIGHT")) return undefined;
  return `${fi}:${a.side}:${a.line}:${hi}`;
}

const UnifiedLine = memo(function UnifiedLine({ fi, hi, line, hl, rightOnly, picked }: LineProps) {
  const a = anchorAttr(fi, hi, line, rightOnly);
  return (
    <div className={styles.line} data-kind={line.kind} data-a={a} data-picked={picked || undefined}>
      {a && <span className={styles.plus}>+</span>}
      <span className={styles.num}>{line.old ?? ""}</span>
      <span className={styles.num}>{line.new ?? ""}</span>
      <span className={styles.sign}>{SIGN[line.kind]}</span>
      <Code line={line} hl={hl} />
    </div>
  );
});

const Half = memo(function Half({
  fi,
  hi,
  line,
  hl,
  side,
  rightOnly,
  picked,
}: Omit<LineProps, "line"> & { line: DiffLine | null; side: "old" | "new" }) {
  if (!line) return <div className={styles.half} data-kind="empty" />;
  const a = anchorAttr(fi, hi, line, rightOnly, side);
  return (
    <div className={styles.half} data-kind={line.kind} data-a={a} data-picked={picked || undefined}>
      {a && <span className={styles.plus}>+</span>}
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
