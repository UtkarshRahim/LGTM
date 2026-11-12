import { useEffect, useState } from "react";
import { commands, type AppInfo, type Settings } from "./ipc";
import styles from "./App.module.css";

export function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    commands.appInfo().then((r) => (r.status === "ok" ? setInfo(r.data) : setError(r.error.message)));
    commands.getSettings().then(setSettings);
  }, []);

  return (
    <div className={styles.shell}>
      <header className={styles.titlebar} data-tauri-drag-region>
        Prism
      </header>
      <main className={styles.main}>
        {error && <p className={styles.error}>{error}</p>}
        {info && settings && (
          <dl className={styles.status}>
            <dt>Version</dt>
            <dd>{info.version}</dd>
            <dt>Schema</dt>
            <dd>v{info.schema_version}</dd>
            <dt>Database</dt>
            <dd className="mono">{info.db_path}</dd>
            <dt>Settings</dt>
            <dd className="mono">{info.settings_path}</dd>
            <dt>Fonts</dt>
            <dd>
              {settings.ui.ui_font} / {settings.ui.code_font}
            </dd>
            <dt>Shell env</dt>
            <dd>
              {info.shell_env.source} · {info.shell_env.var_count} vars
              {info.shell_env.error && ` · ${info.shell_env.error}`}
            </dd>
          </dl>
        )}
      </main>
    </div>
  );
}
