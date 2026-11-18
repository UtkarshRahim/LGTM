import type { DiffLayout, DiffLine, DiffSide, Draft, FileHunks, FileMeta, Finding, Hunk, ReviewThread } from "../ipc";
import { findingLine } from "./findings";

export const ROW_H = 20;
export const FILE_H = 36;
export const NOTE_H = 36;
export const GAP_H = 12;
export const EXPAND_H = 28;
/** Unchanged lines fetched in one click. */
export const EXPAND_CHUNK = 20;
const THREAD_ESTIMATE = 96;

export type Row =
  | { t: "gap"; fi: number }
  | { t: "file"; fi: number }
  | { t: "note"; fi: number; text: string }
  /** Space reserved for a file whose hunks haven't loaded yet. */
  | { t: "pending"; fi: number; rows: number }
  | { t: "hunk"; fi: number; hi: number }
  /** A line of the file that the diff skipped, once its text is loaded. */
  | { t: "context"; fi: number; hi: number; old: number | null; new: number | null; text: string }
  /** A control that loads the unchanged lines between hunks. */
  | { t: "expand"; fi: number; key: string; chunks: ExpandChunk[] }
  | { t: "line"; fi: number; li: number }
  | { t: "pair"; fi: number; l: number | null; r: number | null }
  | { t: "thread"; fi: number; id: string }
  | { t: "outdated"; fi: number; ids: string[] }
  | { t: "draft"; fi: number; id: number }
  /** Drafts written against an older head, or whose line isn't in the shown hunks. */
  | { t: "stale"; fi: number; ids: number[]; why: "head" | "line" }
  | { t: "compose"; fi: number }
  | { t: "finding"; fi: number; id: number }
  /** Findings on lines this diff doesn't show. */
  | { t: "offdiff"; fi: number; ids: number[] };

/** One click's worth of unchanged lines. `to` null means the next chunk, toward the end of the file. */
export interface ExpandChunk {
  from: number;
  to: number | null;
  label: string;
}

/** Lines of a file loaded so the diff can show what hunks skip. */
export interface FileContext {
  lines: ReadonlyMap<number, string>;
  /** Last line that exists, once a fetch reached the end of the file. */
  eof: number | null;
}

/** A line you can comment on: the side of the diff and its line number there. */
export interface Anchor {
  side: DiffSide;
  line: number;
}

export type LineDraft = Draft & { target: { kind: "line" } };

/** A file's hunks flattened for rendering; `lines[i]` pairs with `hl[i]`. */
export interface Prepared {
  lines: DiffLine[];
  hl: (number[] | null)[];
  /** Index into `lines` where each hunk starts. */
  hunkStart: number[];
  /** Hunk each line belongs to; multi-line comments stay within one. */
  lineHunk: number[];
  hunks: FileHunks["hunks"];
  /** Widest line, in columns, with tabs counted as 4. */
  cols: number;
}

export function prepare(f: FileHunks): Prepared {
  const lines: DiffLine[] = [];
  const hunkStart: number[] = [];
  const lineHunk: number[] = [];
  let cols = 0;
  for (const [hi, h] of f.hunks.entries()) {
    hunkStart.push(lines.length);
    for (const l of h.lines) {
      lines.push(l);
      lineHunk.push(hi);
      let w = l.text.length;
      for (let i = l.text.indexOf("\t"); i !== -1; i = l.text.indexOf("\t", i + 1)) w += 3;
      if (w > cols) cols = w;
    }
  }
  const hl = f.highlights ? lines.map((_, i) => f.highlights![i] ?? null) : lines.map(() => null);
  return { lines, hl, hunkStart, lineHunk, hunks: f.hunks, cols };
}

export function rowSize(r: Row): number {
  switch (r.t) {
    case "gap":
      return GAP_H;
    case "file":
      return FILE_H;
    case "note":
      return NOTE_H;
    case "pending":
      return r.rows * ROW_H;
    case "expand":
      return EXPAND_H;
    case "thread":
    case "outdated":
    case "draft":
    case "stale":
    case "compose":
    case "finding":
    case "offdiff":
      return THREAD_ESTIMATE;
    default:
      return ROW_H;
  }
}

export const isDynamic = (r: Row) =>
  r.t === "thread" ||
  r.t === "outdated" ||
  r.t === "draft" ||
  r.t === "stale" ||
  r.t === "compose" ||
  r.t === "finding" ||
  r.t === "offdiff";

export function rowKey(r: Row): string {
  switch (r.t) {
    case "note":
    case "gap":
    case "file":
    case "pending":
      return `${r.fi}:${r.t}`;
    case "hunk":
      return `${r.fi}:h${r.hi}`;
    case "context":
      return `${r.fi}:c${r.new ?? 0}:${r.old ?? 0}`;
    case "expand":
      return `${r.fi}:x${r.key}`;
    case "line":
      return `${r.fi}:l${r.li}`;
    case "pair":
      return `${r.fi}:p${r.l}:${r.r}`;
    case "thread":
      return `t:${r.id}`;
    case "outdated":
      return `${r.fi}:outdated`;
    case "draft":
      return `d:${r.id}`;
    case "stale":
      return `${r.fi}:stale:${r.why}`;
    case "compose":
      return `${r.fi}:compose`;
    case "finding":
      return `f:${r.id}`;
    case "offdiff":
      return `${r.fi}:offdiff`;
  }
}

const anchor = (side: DiffSide, line: number) => `${side}:${line}`;

/** Lines in the old file are commented on the LEFT; added and context lines on the RIGHT. */
export function anchorOf(l: DiffLine, side?: "old" | "new"): Anchor | null {
  if (side === "old" || (!side && (l.kind === "del" || l.new == null))) return l.old == null ? null : { side: "LEFT", line: l.old };
  return l.new == null ? null : { side: "RIGHT", line: l.new };
}

interface FileInput {
  fi: number;
  meta: FileMeta;
  prep: Prepared | undefined;
  layout: DiffLayout;
  collapsed: boolean;
  threads: ReviewThread[];
  drafts: LineDraft[];
  /** Open findings for the current head, already filtered for display. */
  findings: Finding[];
  headSha: string;
  /** Unchanged lines already fetched for this file, keyed by the line number in the blob. */
  context: FileContext | null;
  /** Where the open composer for a new comment sits, if it's in this file. */
  compose: Anchor | null;
  /** Only RIGHT-side anchors mean the same thing here ("since your review"),
   * so nothing else is placed or listed as outdated. */
  rightOnly: boolean;
  syncing: boolean;
}

export function fileRows(input: FileInput): Row[] {
  const { fi, meta, prep, layout, collapsed, threads, drafts, findings, headSha, context, compose, rightOnly, syncing } = input;
  const rows: Row[] = [{ t: "gap", fi }, { t: "file", fi }];
  if (collapsed) {
    const ids = threads.filter((t) => !rightOnly || t.side === "RIGHT").map((t) => t.id);
    if (ids.length) rows.push({ t: "outdated", fi, ids });
    return rows;
  }
  if (meta.binary) return [...rows, { t: "note", fi, text: "Binary file" }];
  if (meta.patch_missing) {
    const text = syncing ? "Large diff, loading from the local mirror…" : "Diff not available";
    return [...rows, { t: "note", fi, text }];
  }
  if (meta.unified_rows === 0) {
    const text = meta.status === "renamed" ? "Renamed without changes" : "No content changes";
    return [...rows, { t: "note", fi, text }];
  }
  if (!prep) {
    return [...rows, { t: "pending", fi, rows: layout === "split" ? meta.split_rows : meta.unified_rows }];
  }

  const byAnchor = new Map<string, ReviewThread[]>();
  for (const th of threads) {
    if (th.line == null || (rightOnly && th.side !== "RIGHT")) continue;
    const k = anchor(th.side, th.line);
    byAnchor.set(k, [...(byAnchor.get(k) ?? []), th]);
  }
  const draftsAt = new Map<string, LineDraft[]>();
  const oldHead: number[] = [];
  for (const d of drafts) {
    if (d.head_sha !== headSha) {
      oldHead.push(d.id);
      continue;
    }
    if (rightOnly && d.target.side !== "RIGHT") continue;
    const k = anchor(d.target.side, d.target.line);
    draftsAt.set(k, [...(draftsAt.get(k) ?? []), d]);
  }
  const findingsAt = new Map<string, Finding[]>();
  const offDiff: number[] = [];
  for (const f of findings) {
    const line = findingLine(f);
    if (line == null) offDiff.push(f.id);
    else findingsAt.set(anchor("RIGHT", line), [...(findingsAt.get(anchor("RIGHT", line)) ?? []), f]);
  }
  const placedFindings = new Set<number>();
  const composeKey = compose && anchor(compose.side, compose.line);
  const placed = new Set<string>();
  const placedDrafts = new Set<number>();
  let composed = false;
  const body: Row[] = [];
  const threadsAt = (side: DiffSide, line: number | null) => {
    if (line == null || (rightOnly && side !== "RIGHT")) return;
    const k = anchor(side, line);
    for (const th of byAnchor.get(k) ?? []) {
      if (placed.has(th.id)) continue;
      placed.add(th.id);
      body.push({ t: "thread", fi, id: th.id });
    }
    for (const d of draftsAt.get(k) ?? []) {
      if (placedDrafts.has(d.id)) continue;
      placedDrafts.add(d.id);
      body.push({ t: "draft", fi, id: d.id });
    }
    for (const f of findingsAt.get(k) ?? []) {
      if (placedFindings.has(f.id)) continue;
      placedFindings.add(f.id);
      body.push({ t: "finding", fi, id: f.id });
    }
    if (k === composeKey && !composed) {
      composed = true;
      body.push({ t: "compose", fi });
    }
  };
  const threadsFor = (l: DiffLine) => {
    if (l.kind !== "add") threadsAt("LEFT", l.old);
    if (l.kind !== "del") threadsAt("RIGHT", l.new);
  };

  const loaded = context?.lines ?? NO_LINES;
  const deleted = meta.status === "deleted";
  const place = (extra: Row[]) => {
    for (const r of extra) {
      body.push(r);
      if (r.t === "context") threadsFor({ kind: "context", old: r.old, new: r.new, text: r.text });
    }
  };

  for (let hi = 0; hi < prep.hunks.length; hi++) {
    const h = prep.hunks[hi];
    place(gapRows(fi, hi, hi === 0 ? null : prep.hunks[hi - 1], h, loaded, deleted));
    body.push({ t: "hunk", fi, hi });
    const start = prep.hunkStart[hi];
    const end = start + h.lines.length;
    if (layout === "unified") {
      for (let li = start; li < end; li++) {
        body.push({ t: "line", fi, li });
        threadsFor(prep.lines[li]);
      }
      continue;
    }
    let li = start;
    while (li < end) {
      if (prep.lines[li].kind === "context") {
        body.push({ t: "pair", fi, l: li, r: li });
        threadsFor(prep.lines[li]);
        li++;
        continue;
      }
      const dels: number[] = [];
      const adds: number[] = [];
      while (li < end && prep.lines[li].kind === "del") dels.push(li++);
      while (li < end && prep.lines[li].kind === "add") adds.push(li++);
      for (let i = 0; i < Math.max(dels.length, adds.length); i++) {
        const l = dels[i] ?? null;
        const r = adds[i] ?? null;
        body.push({ t: "pair", fi, l, r });
        if (l != null) threadsFor(prep.lines[l]);
        if (r != null) threadsFor(prep.lines[r]);
      }
    }
  }
  if (prep.hunks.length > 0) {
    const last = prep.hunks.length - 1;
    place(tailRows(fi, last, prep.hunks[last], loaded, context?.eof ?? null, deleted, meta.status === "added"));
  }

  if (!rightOnly) {
    for (const fs of findingsAt.values()) for (const f of fs) if (!placedFindings.has(f.id)) offDiff.push(f.id);
    if (offDiff.length) rows.push({ t: "offdiff", fi, ids: offDiff });
    const hidden: number[] = [];
    for (const ds of draftsAt.values()) for (const d of ds) if (!placedDrafts.has(d.id)) hidden.push(d.id);
    if (oldHead.length) rows.push({ t: "stale", fi, ids: oldHead, why: "head" });
    if (hidden.length) rows.push({ t: "stale", fi, ids: hidden, why: "line" });
    const unplaced = threads.filter((th) => !placed.has(th.id)).map((th) => th.id);
    if (unplaced.length) rows.push({ t: "outdated", fi, ids: unplaced });
  }
  return [...rows, ...body];
}

const NO_LINES: ReadonlyMap<number, string> = new Map();

/** Last line a hunk displays. A zero count means the start is the line before the change. */
function spanEnd(start: number, count: number): number {
  return count === 0 ? start : start + count - 1;
}

function spanFirst(start: number, count: number): number {
  return count === 0 ? start + 1 : start;
}

function chunksFor(from: number, to: number): ExpandChunk[] {
  const n = to - from + 1;
  if (n <= 0) return [];
  if (n <= EXPAND_CHUNK) return [{ from, to, label: n === 1 ? "Show 1 line" : `Show ${n} lines` }];
  return [
    { from, to: from + EXPAND_CHUNK - 1, label: `Show ${EXPAND_CHUNK} lines below` },
    { from: to - EXPAND_CHUNK + 1, to, label: `Show ${EXPAND_CHUNK} lines above` },
  ];
}

function fill(
  fi: number,
  hi: number,
  from: number,
  to: number,
  lines: ReadonlyMap<number, string>,
  pos: (n: number) => { old: number | null; new: number | null },
): Row[] {
  if (to < from) return [];
  const rows: Row[] = [];
  let n = from;
  while (n <= to) {
    if (lines.has(n)) {
      while (n <= to && lines.has(n)) {
        const p = pos(n);
        rows.push({ t: "context", fi, hi, old: p.old, new: p.new, text: lines.get(n) ?? "" });
        n++;
      }
    } else {
      const start = n;
      while (n <= to && !lines.has(n)) n++;
      const chunks = chunksFor(start, n - 1);
      if (chunks.length) rows.push({ t: "expand", fi, key: `${hi}:${start}`, chunks });
    }
  }
  return rows;
}

/** Unchanged lines between the previous hunk and this one. */
function gapRows(fi: number, hi: number, prev: Hunk | null, next: Hunk, lines: ReadonlyMap<number, string>, deleted: boolean): Row[] {
  const oldFrom = (prev ? spanEnd(prev.old_start, prev.old_lines) : 0) + 1;
  const oldTo = spanFirst(next.old_start, next.old_lines) - 1;
  const newFrom = (prev ? spanEnd(prev.new_start, prev.new_lines) : 0) + 1;
  const newTo = spanFirst(next.new_start, next.new_lines) - 1;
  if (deleted) return fill(fi, hi, oldFrom, oldTo, lines, (n) => ({ old: n, new: null }));
  const oldCount = oldTo - oldFrom + 1;
  const newCount = newTo - newFrom + 1;
  const mapOld = (n: number): number | null => (oldCount === newCount && newCount > 0 ? oldFrom + (n - newFrom) : null);
  return fill(fi, hi, newFrom, newTo, lines, (n) => ({ old: mapOld(n), new: n }));
}

/** Unchanged lines after the last hunk, until the file ends. */
function tailRows(
  fi: number,
  hi: number,
  last: Hunk,
  lines: ReadonlyMap<number, string>,
  eof: number | null,
  deleted: boolean,
  added: boolean,
): Row[] {
  const newEnd = spanEnd(last.new_start, last.new_lines);
  const oldEnd = spanEnd(last.old_start, last.old_lines);
  const from = (deleted ? oldEnd : newEnd) + 1;
  const offset = newEnd - oldEnd;
  const rows: Row[] = [];
  let n = from;
  while (lines.has(n)) {
    const mapped = n - offset;
    const old = deleted ? n : added || mapped <= 0 ? null : mapped;
    rows.push({ t: "context", fi, hi, old, new: deleted ? null : n, text: lines.get(n) ?? "" });
    n++;
  }
  if (eof != null && n > eof) return rows;
  rows.push({ t: "expand", fi, key: `t${n}`, chunks: [{ from: n, to: null, label: `Show ${EXPAND_CHUNK} more lines` }] });
  return rows;
}

export interface Layout {
  rows: Row[];
  /** Row index of each file's header. */
  fileRow: number[];
  /** Row indexes of every hunk header, ascending. */
  hunkRows: number[];
}

export function buildLayout(perFile: Row[][]): Layout {
  const rows: Row[] = [];
  const fileRow: number[] = [];
  const hunkRows: number[] = [];
  for (const fr of perFile) {
    for (const r of fr) {
      if (r.t === "file") fileRow.push(rows.length);
      else if (r.t === "hunk") hunkRows.push(rows.length);
      rows.push(r);
    }
  }
  return { rows, fileRow, hunkRows };
}
