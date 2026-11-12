import { useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { commands, type CheckoutMode, type PrRef, type Project } from "../ipc";
import { errorText } from "../lib/errors";
import styles from "./CheckoutButton.module.css";

interface Props {
  project: Project;
  pr: PrRef;
}

type Phase =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "choose"; changed: number }
  | { kind: "done"; message: string; path?: string }
  | { kind: "error"; message: string; canForce: boolean; mode: CheckoutMode };

export function CheckoutButton({ project, pr }: Props) {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });

  async function run(mode: CheckoutMode, force = false) {
    setPhase({ kind: "busy" });
    const r = await commands.checkoutPr(project.id, pr, mode, false, force);
    if (r.status === "error") {
      const message = errorText(r.error);
      return setPhase({ kind: "error", message, canForce: !force && /force|diverge|overwrit/i.test(message), mode });
    }
    const o = r.data;
    if (o.kind === "needs_choice") return setPhase({ kind: "choose", changed: o.changed });
    if (o.kind === "worktree") return setPhase({ kind: "done", message: "Checked out into a worktree", path: o.path });
    setPhase({
      kind: "done",
      message: `Checked out ${o.branch ?? "detached HEAD"} in ${project.name}${o.stashed ? " (your changes are stashed)" : ""}`,
    });
  }

  return (
    <div className={styles.wrap}>
      <button className="btn primary" disabled={phase.kind === "busy"} onClick={() => run("auto")} title={`Check out in ${project.name} (⌘↵)`} data-checkout>
        {phase.kind === "busy" ? "Checking out…" : "Checkout locally"}
      </button>

      {phase.kind === "choose" && (
        <div className={styles.popover} role="dialog">
          <div className={styles.popTitle}>
            {project.name} has {phase.changed} uncommitted {phase.changed === 1 ? "change" : "changes"}
          </div>
          <p className="muted">Prism never stashes silently. How should it check out #{pr.number}?</p>
          <div className={styles.choices}>
            <button className="btn primary" onClick={() => run("worktree")}>
              Use a separate worktree
            </button>
            <button className="btn" onClick={() => run("stash")}>
              Stash and check out
            </button>
            <button className="btn ghost" onClick={() => setPhase({ kind: "idle" })}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {phase.kind === "done" && (
        <div className={styles.popover}>
          <div>{phase.message}</div>
          {phase.path && <div className="mono muted truncate">{phase.path}</div>}
          <div className={styles.choices}>
            {phase.path && (
              <button className="btn" onClick={() => revealItemInDir(phase.path!)}>
                Reveal in Finder
              </button>
            )}
            <button className="btn ghost" onClick={() => setPhase({ kind: "idle" })}>
              Dismiss
            </button>
          </div>
        </div>
      )}

      {phase.kind === "error" && (
        <div className={styles.popover} role="alert">
          <div className={styles.popTitle}>Checkout failed</div>
          <pre className={styles.err}>{phase.message}</pre>
          <div className={styles.choices}>
            {phase.canForce && (
              <button
                className="btn"
                onClick={() => {
                  if (confirm("Force checkout? This resets the local branch to the PR head and discards its local commits.")) {
                    run(phase.mode, true);
                  }
                }}
              >
                Force checkout…
              </button>
            )}
            <button className="btn ghost" onClick={() => setPhase({ kind: "idle" })}>
              Dismiss
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
