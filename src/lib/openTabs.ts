/** An open pull request tab. The review itself (drafts, viewed files, findings) lives in the database. */
export interface OpenTab {
  pr: { host: string; owner: string; repo: string; number: number };
  title?: string;
}

export const OPEN_TABS_KEY = "prism.openTabs";

export const tabKey = (pr: OpenTab["pr"]) => `${pr.host}/${pr.owner}/${pr.repo}#${pr.number}`;

const home = { tabs: [] as OpenTab[], active: "home" };

/** Reads the tabs saved on the last quit. Bad entries are dropped. */
export function readOpenTabs(raw: string | null): { tabs: OpenTab[]; active: string } {
  if (!raw) return home;
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return home;
  }
  if (!data || typeof data !== "object") return home;
  const tabsIn = (data as { tabs?: unknown }).tabs;
  if (!Array.isArray(tabsIn)) return home;
  const tabs: OpenTab[] = [];
  const seen = new Set<string>();
  for (const item of tabsIn) {
    const tab = asTab(item);
    if (!tab) continue;
    const key = tabKey(tab.pr);
    if (seen.has(key)) continue;
    seen.add(key);
    tabs.push(tab);
  }
  const active = (data as { active?: unknown }).active;
  if (active === "home" || (typeof active === "string" && seen.has(active))) return { tabs, active };
  return { tabs, active: tabs.length ? tabKey(tabs[tabs.length - 1].pr) : "home" };
}

function asTab(item: unknown): OpenTab | null {
  if (!item || typeof item !== "object") return null;
  const pr = (item as { pr?: unknown }).pr;
  if (!pr || typeof pr !== "object") return null;
  const p = pr as Record<string, unknown>;
  if (typeof p.host !== "string" || !p.host || typeof p.owner !== "string" || !p.owner || typeof p.repo !== "string" || !p.repo) return null;
  if (typeof p.number !== "number" || !Number.isInteger(p.number) || p.number < 1) return null;
  const tab: OpenTab = { pr: { host: p.host, owner: p.owner, repo: p.repo, number: p.number } };
  const title = (item as { title?: unknown }).title;
  if (typeof title === "string" && title) tab.title = title;
  return tab;
}

/** Smallest check that fails if tab restore breaks. Run: `node --experimental-strip-types src/lib/openTabs.ts` */
function selfCheckOpenTabs() {
  const raw = JSON.stringify({
    tabs: [
      { pr: { host: "github.com", owner: "acme", repo: "api", number: 7 }, title: "Fix" },
      { pr: { host: "github.com", owner: "acme", repo: "api", number: 7 } },
      { pr: { host: "nope" } },
    ],
    active: "github.com/acme/api#7",
  });
  const got = readOpenTabs(raw);
  if (got.tabs.length !== 1 || got.tabs[0].title !== "Fix" || got.active !== "github.com/acme/api#7") throw new Error("drops bad tabs");
  if (readOpenTabs(JSON.stringify({ tabs: got.tabs, active: "home" })).active !== "home") throw new Error("home stays");
  if (readOpenTabs(JSON.stringify({ tabs: [], active: "github.com/acme/api#7" })).active !== "home") throw new Error("empty falls back");
  if (readOpenTabs("nope").tabs.length !== 0) throw new Error("bad json");
  const fallen = readOpenTabs(JSON.stringify({ tabs: got.tabs, active: "missing" }));
  if (fallen.active !== "github.com/acme/api#7") throw new Error("unknown active uses the last tab");
}

const argv = (globalThis as { process?: { argv?: string[] } }).process?.argv;
if (argv?.[1]?.endsWith("openTabs.ts")) {
  selfCheckOpenTabs();
  console.log("open tabs restore ok");
}
