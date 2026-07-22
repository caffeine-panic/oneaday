import { useEffect, useRef, useState } from "react";
import { toastAutoDismisses, toastRole, type ToastMessage } from "./toastState";

const toastIcons = {
  success: "✓",
  info: "i",
  warning: "!",
  error: "×",
} as const;

type ToastProps = {
  toast: ToastMessage;
  onDismiss: (id: number) => void;
};

export function Toast({ toast, onDismiss }: ToastProps) {
  const autoDismiss = toastAutoDismisses(toast.tone);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const remainingMs = useRef(4000);
  const onDismissRef = useRef(onDismiss);

  useEffect(() => {
    onDismissRef.current = onDismiss;
  }, [onDismiss]);

  useEffect(() => {
    if (!autoDismiss || hovered || focused) return;
    const startedAt = performance.now();
    const timeout = globalThis.setTimeout(
      () => onDismissRef.current(toast.id),
      remainingMs.current,
    );
    return () => {
      globalThis.clearTimeout(timeout);
      remainingMs.current = Math.max(
        0,
        remainingMs.current - (performance.now() - startedAt),
      );
    };
  }, [autoDismiss, focused, hovered, toast.id]);

  return (
    <section
      className={`toast toast--${toast.tone}${autoDismiss ? " toast--auto-dismiss" : ""}`}
      role={toastRole(toast.tone)}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onFocus={() => setFocused(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setFocused(false);
        }
      }}
    >
      <span className="toast__icon" aria-hidden="true">
        {toastIcons[toast.tone]}
      </span>
      <span className="toast__message">{toast.text}</span>
      <button
        className="toast__close"
        type="button"
        aria-label="关闭提示"
        onClick={() => onDismiss(toast.id)}
      >
        ×
      </button>
    </section>
  );
}
