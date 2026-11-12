import type { CommentLabel, DraftTarget, Finding, Run, Severity } from "../ipc";

export const SEVERITIES: Severity[] = ["blocker", "warning", "nit"];
export const DISMISS_REASONS = ["Not an issue", "Intended", "Won't fix", "Already covered"];

/** The finding as a review comment: its line range on the new side, body and label. */
export function findingDraft(f: Finding): { target: DraftTarget; body: string; label: CommentLabel | null } {
  const line = f.end_line ?? f.line ?? 0;
  const start = f.end_line != null && f.line != null && f.line < f.end_line ? f.line : null;
  let body = f.title ? `**${f.title}**\n\n${f.body}` : f.body;
  if (f.suggestion != null) body += `\n\n\`\`\`suggestion\n${f.suggestion}\n\`\`\``;
  const label: CommentLabel | null = f.severity === "nit" ? "nit" : f.suggestion != null ? "suggestion" : null;
  return {
    target: { kind: "line", path: f.path, line, side: "RIGHT", start_line: start, start_side: start == null ? null : "RIGHT" },
    body: body.trim(),
    label,
  };
}

/** Markdown for pasting a finding that can't be placed on the diff. */
export function findingText(f: Finding): string {
  const where = f.line == null ? f.path : `${f.path}:${f.line}${f.end_line && f.end_line !== f.line ? `-${f.end_line}` : ""}`;
  let text = `${where}\n**${f.title}**\n\n${f.body}`;
  if (f.suggestion != null) text += `\n\n\`\`\`\n${f.suggestion}\n\`\`\``;
  return text;
}

/** The line a finding sits under in the diff. */
export const findingLine = (f: Finding) => f.end_line ?? f.line;

export const lineLabel = (f: Finding) =>
  f.line == null ? "file" : f.end_line != null && f.end_line !== f.line ? `L${f.line}–${f.end_line}` : `L${f.line}`;

/**
 * Findings worth showing for the current head: per skill, those from its
 * newest finished run there. Skills last run on an older head yield `stale`.
 */
export function currentFindings(runs: Run[], findings: Finding[], headSha: string) {
  const latest = new Map<string, Run>();
  const latestOld = new Map<string, Run>();
  for (const r of runs) {
    if (r.status !== "done") continue;
    const m = r.head_sha === headSha ? latest : latestOld;
    if (!m.has(r.skill)) m.set(r.skill, r);
  }
  const current = new Set([...latest.values()].map((r) => r.id));
  const old = new Set([...latestOld.entries()].filter(([s]) => !latest.has(s)).map(([, r]) => r.id));
  return {
    current: findings.filter((f) => current.has(f.run_id)),
    stale: findings.filter((f) => old.has(f.run_id) && f.status === "open"),
    staleRuns: [...latestOld.entries()].filter(([s]) => !latest.has(s)).map(([, r]) => r),
  };
}
