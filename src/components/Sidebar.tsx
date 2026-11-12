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
  onAddProject: () => void;
  onRemoveProject: (id: number) => void;
  accounts: Accounts | null;
  onPickAccount: (key: string) => void;
  onRefreshAccounts: () => void;
  accountMenuOpen: boolean;
  setAccountMenuOpen: (open: boolean) => void;
}

export function Sidebar(p: Props) {
  return (
    <nav className={styles.sidebar}>
      <div className={styles.drag} data-tauri-drag-region />
      <button className={styles.row} data-active={p.active === "home"} onClick={() => p.onSelect("home")}>
        <span className={styles.icon}>⌂</span>
        <span className={styles.grow}>Home</span>
        <kbd>⌘1</kbd>
      </button>

      {p.tabs.length > 0 && <div className={styles.section}>Open reviews</div>}
      <div className={styles.tabs}>
        {p.tabs.map((t, i) => {
          const key = prKey(t.pr);
          return (
            <div key={key} className={styles.tab} data-active={p.active === key}>
              <button className={styles.tabMain} onClick={() => p.onSelect(key)} title={t.title}>
                <span className={styles.tabRef}>
                  {t.pr.repo}
                  <span className="muted">#{t.pr.number}</span>
                </span>
                <span className={`${styles.tabTitle} truncate`}>{t.title ?? "Loading…"}</span>
              </button>
              {i < 8 && <kbd className={styles.tabKbd}>⌘{i + 2}</kbd>}
              <button className={styles.close} onClick={() => p.onClose(key)} aria-label="Close tab">
                ×
              </button>
            </div>
          );
        })}
      </div>

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
          <div key={proj.id} className={styles.project} title={proj.path}>
            <div className={styles.projectTop}>
              <span className={`${styles.grow} truncate`}>{proj.name}</span>
              {p.prCounts[proj.id] != null && (
                <span className={styles.count} title="Open pull requests">
                  {p.prCounts[proj.id]}
                </span>
              )}
              <button className={styles.remove} onClick={() => p.onRemoveProject(proj.id)} aria-label="Remove project">
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
              {!proj.remote && <span className={styles.missing}>no GitHub remote</span>}
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
