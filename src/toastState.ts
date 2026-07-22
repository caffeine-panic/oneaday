export type ToastTone = "success" | "info" | "warning" | "error";

export type ToastMessage = Readonly<{
  id: number;
  text: string;
  tone: ToastTone;
}>;

export function nextToast(
  current: ToastMessage | undefined,
  text: string,
  tone: ToastTone,
): ToastMessage {
  return {
    id: (current?.id ?? 0) + 1,
    text,
    tone,
  };
}

export function toastAutoDismisses(tone: ToastTone) {
  return tone === "success" || tone === "info";
}

export function toastRole(tone: ToastTone): "status" | "alert" {
  return toastAutoDismisses(tone) ? "status" : "alert";
}
