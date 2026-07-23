export type PanelId = "connections" | "resources";
export type PanelVisibility = "expanded" | "collapsed";
export type PanelLayout = Record<PanelId, PanelVisibility>;

export const DEFAULT_PANEL_LAYOUT: PanelLayout = {
  connections: "expanded",
  resources: "expanded",
};

type ReadableStorage = Pick<Storage, "getItem">;
type WritableStorage = Pick<Storage, "setItem">;

const STORAGE_KEY = "atlas.panelLayout";

function isPanelVisibility(value: unknown): value is PanelVisibility {
  return value === "expanded" || value === "collapsed";
}

export function loadPanelLayout(
  storage: ReadableStorage = globalThis.localStorage,
): PanelLayout {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_PANEL_LAYOUT };
    const saved = JSON.parse(raw) as {
      version?: unknown;
      layout?: Partial<PanelLayout>;
    };
    if (
      saved.version === 1 &&
      isPanelVisibility(saved.layout?.connections) &&
      isPanelVisibility(saved.layout.resources)
    ) {
      return {
        connections: saved.layout.connections,
        resources: saved.layout.resources,
      };
    }
  } catch {
    // 损坏或不可用的 WebView 存储不应阻止应用启动。
  }
  return { ...DEFAULT_PANEL_LAYOUT };
}

export function savePanelLayout(
  layout: PanelLayout,
  storage: WritableStorage = globalThis.localStorage,
): PanelLayout {
  try {
    storage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        version: 1,
        layout,
      }),
    );
  } catch {
    // 布局仍在当前会话生效；存储失败不应中断用户操作。
  }
  return layout;
}

export function togglePanel(layout: PanelLayout, panel: PanelId): PanelLayout {
  return {
    ...layout,
    [panel]: layout[panel] === "expanded" ? "collapsed" : "expanded",
  };
}
