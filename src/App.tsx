import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { commands, events, prKey, type Accounts, type PrRef, type Project, type Settings } from "./ipc";
import { errorText } from "./lib/errors";
import { Sidebar, type Tab } from "./components/Sidebar";
import { Home } from "./views/Home";
import { PrView } from "./views/PrView";
import styles from "./App.module.css";

export function App() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState("home");
  const [projects, setProjects] = useState<Project[]>([]);
  const [prCounts, setPrCounts] = useState<Record<number, number | null>>({});
  const [accounts, setAccounts] = useState<Accounts | null>(null);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [dropping, setDropping] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [selectedProject, setSelectedProject] = useState<number | null>(null);
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const activeRef = useRef(active);
  activeRef.current = active;

  const flash = useCallback((msg: string) => {
    setToast(msg);
    setTimeout(() => setToast((t) => (t === msg ? null : t)), 4000);
  }, []);

  const loadProjects = useCallback(async () => {
    const r = await commands.listProjects();
    if (r.status === "ok") setProjects(r.data);
    const c = await commands.projectPrCounts();
    if (c.status === "ok") setPrCounts(Object.fromEntries(c.data.map((x) => [x.project_id, x.open])));
  }, []);

  useEffect(() => {
    loadProjects();
    commands.accounts().then((r) => r.status === "ok" && setAccounts(r.data));
    commands.getSettings().then(setSettings);
  }, [loadProjects]);

  const openPr = useCallback((pr: PrRef, title?: string) => {
    const key = prKey(pr);
    if (!tabsRef.current.some((t) => prKey(t.pr) === key)) {
      setTabs((ts) => [...ts, { pr, title }]);
    }
    setActive(key);
  }, []);

  const closeTab = useCallback((key: string) => {
    const ts = tabsRef.current;
    const i = ts.findIndex((t) => prKey(t.pr) === key);
    const next = ts.filter((t) => prKey(t.pr) !== key);
    setTabs(next);
    if (activeRef.current === key) {
      const neighbor = next[Math.min(i, next.length - 1)];
      setActive(neighbor ? prKey(neighbor.pr) : "home");
    }
  }, []);

  const addProjects = useCallback(
    async (paths: string[]) => {
      for (const path of paths) {
        const r = await commands.addProject(path);
        if (r.status === "error") flash(errorText(r.error));
      }
      loadProjects();
    },
    [flash, loadProjects],
  );

  const pickFolder = useCallback(async () => {
    const picked = await openDialog({ directory: true, multiple: true, title: "Add project" });
    if (picked) addProjects(Array.isArray(picked) ? picked : [picked]);
  }, [addProjects]);

  const openFromText = useCallback(
    async (text: string) => {
      const r = await commands.search(text);
      if (r.status === "ok" && r.data.kind === "open") openPr(r.data.pr);
    },
    [openPr],
  );

  // Keyboard: ⌘1 home, ⌘2–9 tabs, ⌘W close, ⌘O add project, ⌘↵ check out.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey) return;
      if (e.key >= "1" && e.key <= "9") {
        e.preventDefault();
        const n = Number(e.key);
        if (n === 1) setActive("home");
        else if (tabsRef.current[n - 2]) setActive(prKey(tabsRef.current[n - 2].pr));
      } else if (e.key === "w") {
        e.preventDefault();
        if (activeRef.current !== "home") closeTab(activeRef.current);
      } else if (e.key === "o") {
        e.preventDefault();
        pickFolder();
      } else if (e.key === "Enter") {
        const btn = document.querySelector<HTMLButtonElement>('[data-active="true"] [data-checkout]');
        if (btn) {
          e.preventDefault();
          btn.click();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [closeTab, pickFolder]);

  // Pasting a PR link anywhere outside a text field opens it.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable]")) return;
      const text = e.clipboardData?.getData("text") ?? "";
      if (/\/pull\/\d+/.test(text)) {
        e.preventDefault();
        openFromText(text);
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [openFromText]);

  // Drag folders onto the window to add them as projects.
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "enter" || e.payload.type === "over") setDropping(true);
      else if (e.payload.type === "leave") setDropping(false);
      else if (e.payload.type === "drop") {
        setDropping(false);
        addProjects(e.payload.paths);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [addProjects]);

  // Branch/dirty updates: watcher events for HEAD/index, plus a refresh on
  // focus to catch edits made in an editor (which don't touch the index).
  useEffect(() => {
    const un = events.projectStatusChanged.listen((e) => {
      setProjects((ps) => ps.map((p) => (p.id === e.payload.id ? { ...p, status: e.payload.status } : p)));
    });
    const onFocus = async () => {
      const r = await commands.listProjects();
      if (r.status === "ok") setProjects(r.data);
    };
    window.addEventListener("focus", onFocus);
    return () => {
      un.then((f) => f());
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  const updateUi = useCallback(
    (patch: Partial<Settings["ui"]>) => {
      setSettings((s) => {
        if (!s) return s;
        const next = { ...s, ui: { ...s.ui, ...patch } };
        commands.saveSettings(next).then((r) => r.status === "error" && flash(errorText(r.error)));
        return next;
      });
    },
    [flash],
  );
  const setLayout = useCallback((diff_layout: Settings["ui"]["diff_layout"]) => updateUi({ diff_layout }), [updateUi]);
  const setPanel = useCallback((file_panel: Settings["ui"]["file_panel"]) => updateUi({ file_panel }), [updateUi]);

  const afterAccountChange = (a: Accounts) => {
    setAccounts(a);
    setAccountMenuOpen(false);
    setRefreshKey((k) => k + 1);
    loadProjects();
  };

  return (
    <div className={styles.shell}>
      <Sidebar
        tabs={tabs}
        active={active}
        onSelect={setActive}
        onClose={closeTab}
        projects={projects}
        prCounts={prCounts}
        selectedProject={active === "home" ? selectedProject : null}
        onSelectProject={(id) => {
          setSelectedProject((cur) => (cur === id && activeRef.current === "home" ? null : id));
          setActive("home");
        }}
        onAddProject={pickFolder}
        onRemoveProject={async (id) => {
          if (selectedProject === id) setSelectedProject(null);
          await commands.removeProject(id);
          loadProjects();
        }}
        accounts={accounts}
        onPickAccount={async (key) => {
          const r = await commands.setDefaultAccount(key);
          if (r.status === "ok") afterAccountChange(r.data);
        }}
        onRefreshAccounts={async () => {
          const r = await commands.refreshAccounts();
          if (r.status === "ok") afterAccountChange(r.data);
        }}
        accountMenuOpen={accountMenuOpen}
        setAccountMenuOpen={setAccountMenuOpen}
      />
      <main className={styles.main}>
        <div className={styles.topbar} data-tauri-drag-region />
        <div className={styles.view} hidden={active !== "home"}>
          <Home
            active={active === "home"}
            onOpen={openPr}
            onSwitchAccount={() => setAccountMenuOpen(true)}
            refreshKey={refreshKey}
            project={projects.find((p) => p.id === selectedProject) ?? null}
            onClearProject={() => setSelectedProject(null)}
          />
        </div>
        {tabs.map((t) => {
          const key = prKey(t.pr);
          return (
            <div key={key} className={styles.view} hidden={active !== key} data-active={active === key}>
              <PrView
                pr={t.pr}
                active={active === key}
                projects={projects}
                layout={settings?.ui.diff_layout ?? "unified"}
                panel={settings?.ui.file_panel ?? "review_order"}
                onLayout={setLayout}
                onPanel={setPanel}
                onTitle={(title) => setTabs((ts) => ts.map((x) => (prKey(x.pr) === key ? { ...x, title } : x)))}
                onSwitchAccount={() => setAccountMenuOpen(true)}
              />
            </div>
          );
        })}
      </main>
      {dropping && <div className={styles.drop}>Drop folders to add them as projects</div>}
      {toast && <div className={styles.toast}>{toast}</div>}
    </div>
  );
}
