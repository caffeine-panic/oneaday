export type UpdateProxySettings =
  | { mode: "system" }
  | { mode: "manual"; url: string }
  | { mode: "disabled" };

type ReadableStorage = Pick<Storage, "getItem">;
type WritableStorage = Pick<Storage, "setItem">;

const STORAGE_KEY = "atlas.updateProxySettings";
const DEFAULT_SETTINGS: UpdateProxySettings = { mode: "system" };

export function normalizeUpdateProxySettings(settings: UpdateProxySettings): UpdateProxySettings {
  if (settings.mode !== "manual") return { mode: settings.mode };

  const rawUrl = settings.url.trim();
  let proxy: URL;
  try {
    proxy = new URL(rawUrl);
  } catch {
    throw new Error("请输入有效的 HTTP 或 HTTPS 代理地址");
  }
  if (proxy.protocol !== "http:" && proxy.protocol !== "https:") {
    throw new Error("更新代理仅支持 HTTP 或 HTTPS 地址");
  }
  if (proxy.username || proxy.password) {
    throw new Error("代理地址不能包含用户名或密码，以免凭据写入 WebView 存储");
  }
  if (proxy.pathname !== "/" || proxy.search || proxy.hash) {
    throw new Error("代理地址只能包含协议、主机和端口");
  }
  return { mode: "manual", url: proxy.toString() };
}

export function loadUpdateProxySettings(
  storage: ReadableStorage = globalThis.localStorage,
): UpdateProxySettings {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_SETTINGS;
    const parsed = JSON.parse(raw) as Partial<UpdateProxySettings>;
    if (parsed.mode === "system" || parsed.mode === "disabled") return { mode: parsed.mode };
    if (parsed.mode === "manual" && typeof parsed.url === "string") {
      return normalizeUpdateProxySettings({ mode: "manual", url: parsed.url });
    }
  } catch {
    // Invalid or unavailable WebView storage should never prevent the app from starting.
  }
  return DEFAULT_SETTINGS;
}

export function saveUpdateProxySettings(
  settings: UpdateProxySettings,
  storage: WritableStorage = globalThis.localStorage,
): UpdateProxySettings {
  const normalized = normalizeUpdateProxySettings(settings);
  storage.setItem(STORAGE_KEY, JSON.stringify(normalized));
  return normalized;
}
