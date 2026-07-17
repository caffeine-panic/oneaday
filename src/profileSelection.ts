export type ProfileSelectionPlan = "preserve" | "reload" | "clear";

export function planProfileSelection(
  currentProfileId: string | undefined,
  nextProfileId: string,
  hasOpenSession: boolean,
): ProfileSelectionPlan {
  if (!hasOpenSession) return "clear";
  if (currentProfileId === nextProfileId) return "preserve";
  return "reload";
}
