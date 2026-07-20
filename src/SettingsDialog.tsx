import { useState } from "react";
import {
  normalizeUpdateProxySettings,
  type UpdateProxySettings,
} from "./updateSettings";

type SettingsDialogProps = {
  settings: UpdateProxySettings;
  onSave: (settings: UpdateProxySettings) => void;
  onCancel: () => void;
};

export function SettingsDialog({ settings, onSave, onCancel }: SettingsDialogProps) {
  const [draft, setDraft] = useState<UpdateProxySettings>(settings);
  const [manualUrl, setManualUrl] = useState(settings.mode === "manual" ? settings.url : "");
  const [error, setError] = useState<string>();

  const selectMode = (mode: UpdateProxySettings["mode"]) => {
    setError(undefined);
    setDraft(mode === "manual" ? { mode, url: manualUrl } : { mode });
  };

  const save = () => {
    try {
      onSave(normalizeUpdateProxySettings(
        draft.mode === "manual" ? { mode: "manual", url: manualUrl } : draft,
      ));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  return (
    <div className="dialog-backdrop" onMouseDown={onCancel}>
      <section className="dialog settings-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">APPLICATION SETTINGS</span><h2>设置</h2></div>
          <button className="icon-button" onClick={onCancel}>×</button>
        </div>

        <div className="form-section">
          <div className="form-section-title">应用更新网络</div>
          <div className="proxy-options">
            <label className={draft.mode === "system" ? "selected" : ""}>
              <input type="radio" checked={draft.mode === "system"} onChange={() => selectMode("system")} />
              <span><b>跟随系统代理</b><small>macOS 和 Windows 读取系统 HTTP/HTTPS 代理；Linux 读取代理环境变量。</small></span>
            </label>
            <label className={draft.mode === "manual" ? "selected" : ""}>
              <input type="radio" checked={draft.mode === "manual"} onChange={() => selectMode("manual")} />
              <span><b>手动设置</b><small>只用于检查和下载 Atlas Registry 更新。</small></span>
            </label>
            {draft.mode === "manual" && (
              <input
                autoFocus
                value={manualUrl}
                onChange={(event) => {
                  setManualUrl(event.target.value);
                  setDraft({ mode: "manual", url: event.target.value });
                  setError(undefined);
                }}
                placeholder="http://127.0.0.1:7897"
                spellCheck={false}
              />
            )}
            <label className={draft.mode === "disabled" ? "selected" : ""}>
              <input type="radio" checked={draft.mode === "disabled"} onChange={() => selectMode("disabled")} />
              <span><b>不使用代理</b><small>更新请求始终直连，忽略系统代理和代理环境变量。</small></span>
            </label>
          </div>
          {error && <div className="form-error">{error}</div>}
        </div>

        <div className="dialog-actions">
          <button className="button" onClick={onCancel}>取消</button>
          <button className="button primary" onClick={save}>保存设置</button>
        </div>
      </section>
    </div>
  );
}
