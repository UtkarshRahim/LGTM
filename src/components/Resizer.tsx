import { useCallback, useState, type PointerEvent } from "react";
import styles from "./Resizer.module.css";

/** A size in px remembered across launches; `reset` goes back to `initial`. */
export function useStoredSize(key: string, initial: number) {
  const [size, setSize] = useState(() => {
    const v = Number(localStorage.getItem(key));
    return v > 0 ? v : initial;
  });
  const save = useCallback(
    (n: number) => {
      setSize(n);
      localStorage.setItem(key, String(n));
    },
    [key],
  );
  const reset = useCallback(() => {
    setSize(initial);
    localStorage.removeItem(key);
  }, [key, initial]);
  return [size, save, reset] as const;
}

interface Props {
  /** Which edge of its (positioned) parent the handle sits on. */
  edge: "left" | "right" | "bottom";
  size: number;
  min: number;
  /** Upper bound, or a function of the window for limits like "half the width". */
  max: number | (() => number);
  onSize: (px: number) => void;
  onReset: () => void;
}

/** Drag handle that resizes its parent panel. Double-click resets. */
export function Resizer({ edge, size, min, max, onSize, onReset }: Props) {
  const start = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const origin = edge === "bottom" ? e.clientY : e.clientX;
    const hi = typeof max === "function" ? max() : max;
    const sign = edge === "left" ? -1 : 1;
    let last = size;
    const move = (ev: globalThis.PointerEvent) => {
      const delta = (edge === "bottom" ? ev.clientY : ev.clientX) - origin;
      const next = Math.round(Math.max(min, Math.min(hi, size + sign * delta)));
      if (next !== last) onSize((last = next));
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      document.body.removeAttribute("data-resizing");
    };
    document.body.setAttribute("data-resizing", edge === "bottom" ? "y" : "x");
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  };
  return (
    <div
      className={styles.handle}
      data-edge={edge}
      role="separator"
      aria-orientation={edge === "bottom" ? "horizontal" : "vertical"}
      title="Drag to resize, double-click to reset"
      onPointerDown={start}
      onDoubleClick={onReset}
    />
  );
}
