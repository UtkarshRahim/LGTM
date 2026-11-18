const ESC: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };

function esc(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ESC[c]);
}

/** Inline markdown on already-escaped text, with code spans held out of the rest. */
function inline(src: string): string {
  const codes: string[] = [];
  let s = esc(src).replace(/`([^`]+)`/g, (_, code: string) => {
    codes.push(code);
    return `\u0000${codes.length - 1}\u0000`;
  });
  s = s.replace(/!\[([^\]]*)\]\((https?:\/\/[^\s)]+)\)/g, (_, alt: string, url: string) => `<img alt="${alt}" src="${url}">`);
  s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, text: string, url: string) => `<a href="${url}">${text}</a>`);
  s = s.replace(/(^|[\s(])(https?:\/\/[^\s<]+)/g, (_, pre: string, url: string) => `${pre}<a href="${url}">${url}</a>`);
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>");
  return s.replace(/\u0000(\d+)\u0000/g, (_, n: string) => `<code>${codes[Number(n)]}</code>`);
}

/** Marker width in columns, including the space after it. `1. ` is 3, `- ` is 2. */
function listMarker(line: string): { ordered: boolean; width: number; rest: string } | null {
  const m = /^(\s*)([-*+]|\d+\.)(\s+)(.*)$/.exec(line);
  if (!m) return null;
  return { ordered: /^\d+\./.test(m[2]), width: m[1].length + m[2].length + m[3].length, rest: m[4] };
}

function dedent(line: string, width: number): string {
  let n = 0;
  while (n < width && n < line.length && line[n] === " ") n++;
  return line.slice(n);
}

function isFence(line: string): boolean {
  return /^ {0,3}```/.test(line);
}

function isHeading(line: string): boolean {
  return /^ {0,3}#{1,6}\s+/.test(line);
}

function isRule(line: string): boolean {
  return /^ {0,3}(-{3,}|\*{3,})$/.test(line.trimEnd()) && /^ {0,3}[-*]/.test(line);
}

function isQuote(line: string): boolean {
  return /^ {0,3}>/.test(line);
}

/** A small Markdown subset for pull request descriptions. Output is escaped. */
export function renderMarkdown(src: string): string {
  const lines = src.replace(/\r\n/g, "\n").replace(/<!--[\s\S]*?-->/g, "").split("\n");
  const html: string[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === "") {
      i++;
      continue;
    }
    if (isFence(line)) {
      const buf: string[] = [];
      i++;
      while (i < lines.length && !isFence(lines[i])) buf.push(lines[i++]);
      if (i < lines.length) i++;
      html.push(`<pre><code>${esc(buf.join("\n"))}</code></pre>`);
      continue;
    }
    const heading = /^ {0,3}(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      const level = heading[1].length;
      html.push(`<h${level}>${inline(heading[2])}</h${level}>`);
      i++;
      continue;
    }
    if (isRule(line)) {
      html.push("<hr>");
      i++;
      continue;
    }
    if (isQuote(line)) {
      const buf: string[] = [];
      while (i < lines.length && isQuote(lines[i])) buf.push(lines[i++].replace(/^ {0,3}>\s?/, ""));
      html.push(`<blockquote>${renderMarkdown(buf.join("\n"))}</blockquote>`);
      continue;
    }
    const marker = listMarker(line);
    if (marker) {
      const ordered = marker.ordered;
      const base = marker.width;
      const items: string[] = [];
      while (i < lines.length) {
        const item = listMarker(lines[i]);
        if (!item || item.ordered !== ordered || item.width < base) break;
        if (item.width > base && items.length > 0) break;
        const buf = [item.rest];
        i++;
        while (i < lines.length) {
          if (lines[i].trim() === "") {
            let j = i + 1;
            while (j < lines.length && lines[j].trim() === "") j++;
            const next = listMarker(lines[j] ?? "");
            if (lines[j] == null || (next && next.width <= item.width)) break;
            buf.push("");
            i++;
            continue;
          }
          const next = listMarker(lines[i]);
          if (next && next.width <= item.width) break;
          buf.push(dedent(lines[i], item.width));
          i++;
        }
        const box = /^\[( |x|X)\]\s+([\s\S]*)$/.exec(buf[0]);
        if (box) buf[0] = box[2];
        const body = renderMarkdown(buf.join("\n"));
        items.push(
          box
            ? `<li class="task"><input type="checkbox" disabled${box[1].toLowerCase() === "x" ? " checked" : ""}> ${body}</li>`
            : `<li>${body}</li>`,
        );
      }
      html.push(`<${ordered ? "ol" : "ul"}>${items.join("")}</${ordered ? "ol" : "ul"}>`);
      continue;
    }
    const buf: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !isFence(lines[i]) &&
      !isHeading(lines[i]) &&
      !isQuote(lines[i]) &&
      !isRule(lines[i]) &&
      !listMarker(lines[i])
    ) {
      buf.push(lines[i++]);
    }
    html.push(`<p>${inline(buf.join(" "))}</p>`);
  }
  return html.join("");
}

function sugRow(cls: string, sign: string, line: string): string {
  return `<div class="${cls}"><span>${sign}</span><code>${line === "" ? " " : esc(line)}</code></div>`;
}

/** A ```suggestion fence, with `original` as the lines being replaced. */
function suggestionHtml(original: string | null, raw: string): string {
  const proposed = raw.replace(/\n$/, "");
  const dels = original == null ? [] : original.split("\n");
  const adds = proposed === "" ? [] : proposed.split("\n");
  const body = dels.map((l) => sugRow("sugDel", "−", l)).join("") + adds.map((l) => sugRow("sugAdd", "+", l)).join("");
  return `<div class="sug"><div class="sugHead">Suggested change</div>${body}</div>`;
}

/**
 * Markdown plus GitHub suggestion fences. `original` is the commented code,
 * shown as the lines the suggestion replaces.
 */
export function renderComment(src: string, original: string | null): string {
  const text = src.replace(/\r\n/g, "\n");
  const re = / {0,3}```suggestion[^\n]*\n([\s\S]*?)```/g;
  let html = "";
  let last = 0;
  for (const m of text.matchAll(re)) {
    const index = m.index ?? 0;
    html += renderMarkdown(text.slice(last, index));
    html += suggestionHtml(original, m[1]);
    last = index + m[0].length;
  }
  html += renderMarkdown(text.slice(last));
  return html;
}
