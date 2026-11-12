import type { ReactNode } from "react";
import { HIGHLIGHT_CLASSES } from "../ipc";

const CLASS = HIGHLIGHT_CLASSES.map((c) => `hl-${c.replace(/\./g, "-")}`);

/** Renders `text` with flat `[start, end, class]` spans (UTF-16 offsets). */
export function highlighted(text: string, spans: number[] | null): ReactNode {
  if (!spans || spans.length === 0) return text;
  const out: ReactNode[] = [];
  let pos = 0;
  for (let i = 0; i + 2 < spans.length; i += 3) {
    const start = Math.max(spans[i], pos);
    const end = Math.min(spans[i + 1], text.length);
    if (end <= start) continue;
    if (start > pos) out.push(text.slice(pos, start));
    out.push(
      <span key={i} className={CLASS[spans[i + 2]]}>
        {text.slice(start, end)}
      </span>,
    );
    pos = end;
  }
  if (pos < text.length) out.push(text.slice(pos));
  return out;
}
