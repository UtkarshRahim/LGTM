use std::path::Path;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

/// Signals when a project's branch or index may have changed. Drop to stop.
pub struct ProjectWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Watches `git_dir` (non-recursively) for writes to `HEAD` or `index`.
/// Events are not debounced; git rewrites these files via lock-and-rename,
/// so one operation produces several events.
pub fn watch(git_dir: &Path, tx: UnboundedSender<()>) -> notify::Result<ProjectWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let relevant = event
            .paths
            .iter()
            .any(|p| matches!(p.file_name().and_then(|n| n.to_str()), Some("HEAD" | "index")));
        if relevant {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(git_dir, RecursiveMode::NonRecursive)?;
    Ok(ProjectWatcher { _watcher: watcher })
}
