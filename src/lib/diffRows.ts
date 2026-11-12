import type { DiffLayout, DiffLine, FileHunks, FileMeta, ReviewThread } from "../ipc";

export const ROW_H = 20;
export const FILE_H = 36;
export const NOTE_H = 36;
export const GAP_H = 12;
const THREAD_ESTIMATE = 96;

export type Row =
  | { t: "gap"; fi: number }
  | { t: "file"; fi: number }
  | { t: "note"; fi: number; text: string }
  /** Space reserved for a file whose hunks haven't loaded yet. */
  | { t: "pending"; fi: number; rows: number }
  | { t: "hunk"; fi: number; hi: number }
  | { t: "line"; fi: number; li: number }
  | { t: "pair"; fi: number; l: number | null; r: number | null }
  | { t: "thread"; fi: number; id: string }
  | { t: "outdated"; fi: number; ids: string[] };

/** A file's hunks flattened for rendering; `lines[i]` pairs with `hl[i]`. */
export interface Prepared {
  lines: DiffLine[];
  hl: (number[] | null)[];
  /** Index into `lines` where each hunk starts. */
  hunkStart: number[];
  hunks: FileHunks["hunks"];
  /** Widest line, in columns, with tabs counted as 4. */
  cols: number;
}

export function prepare(f: FileHunks): Prepared {
  const lines: DiffLine[] = [];
  const hunkStart: number[] = [];
  let cols = 0;
  for (const h of f.hunks) {
    hunkStart.push(lines.length);
    for (const l of h.lines) {
      lines.push(l);
      let w = l.text.length;
      for (let i = l.text.indexOf("\t"); i !== -1; i = l.text.indexOf("\t", i + 1)) w += 3;
      if (w > cols) cols = w;
    }
  }
  const hl = f.highlights ? lines.map((_, i) => f.highlights![i] ?? null) : lines.map(() => null);
  return { lines, hl, hunkStart, hunks: f.hunks, cols };
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
    case "thread":
    case "outdated":
      return THREAD_ESTIMATE;
    default:
      return ROW_H;
  }
}

export const isDynamic = (r: Row) => r.t === "thread" || r.t === "outdated";

export function rowKey(r: Row): string {
  switch (r.t) {
    case "note":
    case "gap":
    case "file":
    case "pending":
      return `${r.fi}:${r.t}`;
    case "hunk":
      return `${r.fi}:h${r.hi}`;
    case "line":
      return `${r.fi}:l${r.li}`;
    case "pair":
      return `${r.fi}:p${r.l}:${r.r}`;
    case "thread":
      return `t:${r.id}`;
    case "outdated":
      return `${r.fi}:outdated`;
  }
}

const anchor = (side: "LEFT" | "RIGHT", line: number) => `${side}:${line}`;

interface FileInput {
  fi: number;
  meta: FileMeta;
  prep: Prepared | undefined;
  layout: DiffLayout;
  collapsed: boolean;
  threads: ReviewThread[];
  syncing: boolean;
}

export function fileRows({ fi, meta, prep, layout, collapsed, threads, syncing }: FileInput): Row[] {
  const rows: Row[] = [{ t: "gap", fi }, { t: "file", fi }];
  if (collapsed) return rows;
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
    if (th.line == null) continue;
    const k = anchor(th.side, th.line);
    byAnchor.set(k, [...(byAnchor.get(k) ?? []), th]);
  }
  const placed = new Set<string>();
  const body: Row[] = [];
  const threadsAt = (side: "LEFT" | "RIGHT", line: number | null) => {
    if (line == null) return;
    for (const th of byAnchor.get(anchor(side, line)) ?? []) {
      if (placed.has(th.id)) continue;
      placed.add(th.id);
      body.push({ t: "thread", fi, id: th.id });
    }
  };
  const threadsFor = (l: DiffLine) => {
    if (l.kind !== "add") threadsAt("LEFT", l.old);
    if (l.kind !== "del") threadsAt("RIGHT", l.new);
  };

  prep.hunks.forEach((h, hi) => {
    body.push({ t: "hunk", fi, hi });
    const start = prep.hunkStart[hi];
    const end = start + h.lines.length;
    if (layout === "unified") {
      for (let li = start; li < end; li++) {
        body.push({ t: "line", fi, li });
        threadsFor(prep.lines[li]);
      }
      return;
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
  });

  const unplaced = threads.filter((th) => !placed.has(th.id)).map((th) => th.id);
  if (unplaced.length) rows.push({ t: "outdated", fi, ids: unplaced });
  return [...rows, ...body];
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
