import { useRef, useState, type PointerEvent } from "react";
import type { Accounts, PrRef, Project } from "../ipc";
import { prKey } from "../ipc";
import styles from "./Sidebar.module.css";

export interface Tab {
  pr: PrRef;
  title?: string;
}

interface Props {
  tabs: Tab[];
  active: string;
  onSelect: (key: string) => void;
  onClose: (key: string) => void;
  projects: Project[];
  prCounts: Record<number, number | null>;
  /** Project whose PRs Home is showing. */
  selectedProject: number | null;
  onSelectProject: (id: number) => void;
  onAddProject: () => void;
  onRemoveProject: (id: number) => void;
  accounts: Accounts | null;
  onPickAccount: (key: string) => void;
  onRefreshAccounts: () => void;
  accountMenuOpen: boolean;
  setAccountMenuOpen: (open: boolean) => void;
}

const TABS_HEIGHT_KEY = "prism.sidebar.tabsHeight";
const MIN_SECTION = 48;

export function Sidebar(p: Props) {
  const navRef = useRef<HTMLElement>(null);
  const tabsRef = useRef<HTMLDivElement>(null);
  // `null` means "fit the content" (capped by CSS); dragging the divider pins a height.
  const [tabsHeight, setTabsHeight] = useState<number | null>(() => {
    const v = Number(localStorage.getItem(TABS_HEIGHT_KEY));
    return v > 0 ? v : null;
  });

  const startResize = (e: PointerEvent<HTMLDivElement>) => {
    const tabs = tabsRef.current;
    const nav = navRef.current;
    if (!tabs || !nav) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const startY = e.clientY;
    const startH = tabs.getBoundingClientRect().height;
    // Leave room for the Projects header, a few projects and the account footer.
    const maxH = nav.getBoundingClientRect().bottom - tabs.getBoundingClientRect().top - 180;
    let h = startH;
    const move = (ev: globalThis.PointerEvent) => {
      h = Math.round(Math.max(MIN_SECTION, Math.min(maxH, startH + ev.clientY - startY)));
      setTabsHeight(h);
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      localStorage.setItem(TABS_HEIGHT_KEY, String(h));
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  };

  const resetResize = () => {
    setTabsHeight(null);
    localStorage.removeItem(TABS_HEIGHT_KEY);
  };

  return (
    <nav ref={navRef} className={styles.sidebar}>
      <div className={styles.drag} data-tauri-drag-region />
      <button className={styles.row} data-active={p.active === "home"} onClick={() => p.onSelect("home")}>
        <span className={styles.icon}>⌂</span>
        <span className={styles.grow}>Home</span>
        <kbd>⌘1</kbd>
      </button>

      {p.tabs.length > 0 && <div className={styles.section}>Open reviews</div>}
      <div
        ref={tabsRef}
        className={styles.tabs}
        style={tabsHeight != null && p.tabs.length > 0 ? { height: tabsHeight, maxHeight: "none" } : undefined}
      >
        {p.tabs.map((t, i) => {
          const key = prKey(t.pr);
          return (
            <div
              key={key}
              className={styles.tab}
              data-active={p.active === key}
              onAuxClick={(e) => e.button === 1 && p.onClose(key)}
            >
              <button className={styles.tabMain} onClick={() => p.onSelect(key)} title={t.title}>
                <span className={styles.tabRef}>
                  {t.pr.repo}
                  <span className="muted">#{t.pr.number}</span>
                </span>
                <span className={`${styles.tabTitle} truncate`}>{t.title ?? "Loading…"}</span>
              </button>
              {i < 8 && <kbd className={styles.tabKbd}>⌘{i + 2}</kbd>}
              <button className={styles.close} onClick={() => p.onClose(key)} aria-label="Close tab" title="Close (⌘W)">
                ×
              </button>
            </div>
          );
        })}
      </div>

      {p.tabs.length > 0 && (
        <div
          className={styles.splitter}
          onPointerDown={startResize}
          onDoubleClick={resetResize}
          title="Drag to resize, double-click to reset"
          role="separator"
          aria-orientation="horizontal"
        />
      )}

      <div className={styles.section}>
        <span className={styles.grow}>Projects</span>
        <button className={styles.add} onClick={p.onAddProject} title="Add project (⌘O)">
          +
        </button>
      </div>
      <div className={styles.projects}>
        {p.projects.length === 0 && (
          <button className={styles.empty} onClick={p.onAddProject}>
            Drop a folder here or press <kbd>⌘O</kbd>
          </button>
        )}
        {p.projects.map((proj) => (
          <div
            key={proj.id}
            className={styles.project}
            data-selected={p.selectedProject === proj.id}
            title={proj.path}
            role="button"
            tabIndex={0}
            onClick={() => p.onSelectProject(proj.id)}
            onKeyDown={(e) => e.key === "Enter" && p.onSelectProject(proj.id)}
          >
            <div className={styles.projectTop}>
              <span className={`${styles.grow} truncate`}>{proj.name}</span>
              {p.prCounts[proj.id] != null && (
                <span className={styles.count} title="Open pull requests">
                  {p.prCounts[proj.id]}
                </span>
              )}
              <button
                className={styles.remove}
                onClick={(e) => {
                  e.stopPropagation();
                  p.onRemoveProject(proj.id);
                }}
                aria-label="Remove project"
              >
                ×
              </button>
            </div>
            <div className={`${styles.projectMeta} truncate`}>
              {proj.status ? (
                <>
                  <span className="mono">{proj.status.branch ?? proj.status.head_sha?.slice(0, 7) ?? "?"}</span>
                  {proj.status.changed > 0 && (
                    <span className={styles.dirty} title={`${proj.status.changed} changed files`}>
                      ● {proj.status.changed}
                    </span>
                  )}
                </>
              ) : (
                <span className={styles.missing}>folder unavailable</span>
              )}
              {proj.remotes.length === 0 && <span className={styles.missing}>no GitHub remote</span>}
              {proj.remotes.length > 1 && (
                <span title={proj.remotes.map((r) => `${r.remote}: ${r.owner}/${r.repo}`).join("\n")}>
                  {proj.remotes.length} remotes
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      <div className={styles.footer}>
        <button className={styles.account} onClick={() => p.setAccountMenuOpen(!p.accountMenuOpen)}>
          <span className={styles.avatar}>{p.accounts?.active?.split("/")[1]?.[0]?.toUpperCase() ?? "?"}</span>
          <span className={`${styles.grow} truncate`}>{p.accounts?.active?.split("/")[1] ?? "No account"}</span>
          <span className="muted">⌄</span>
        </button>
        {p.accountMenuOpen && p.accounts && (
          <div className={styles.menu}>
            {p.accounts.accounts.map((a) => {
              const key = `${a.host}/${a.login}`;
              return (
                <button key={key} className={styles.menuItem} onClick={() => p.onPickAccount(key)}>
                  <span className={styles.check}>{p.accounts?.active === key ? "✓" : ""}</span>
                  <span className={styles.grow}>
                    {a.login}
                    <span className="muted"> · {a.host}</span>
                  </span>
                  <span className={styles.source}>{a.source.kind === "env" ? `$${a.source.var}` : "gh CLI"}</span>
                </button>
              );
            })}
            {p.accounts.accounts.length === 0 && <div className={styles.menuEmpty}>Run `gh auth login`, then refresh.</div>}
            <button className={styles.menuItem} onClick={p.onRefreshAccounts}>
              <span className={styles.check} />
              Refresh accounts
            </button>
          </div>
        )}
      </div>
    </nav>
  );
}
