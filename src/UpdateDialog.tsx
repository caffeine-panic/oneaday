import type { AppUpdateInfo } from "./registry";

export type UpdateProgress = {
  phase: "downloading" | "installing";
  downloaded: number;
  contentLength?: number;
};

type UpdateDialogProps = {
  update: AppUpdateInfo;
  installing: boolean;
  progress?: UpdateProgress;
  onInstall: () => void;
  onClose: () => void;
};

export function UpdateDialog({
  update,
  installing,
  progress,
  onInstall,
  onClose,
}: UpdateDialogProps) {
  const percentage = progress?.contentLength
    ? Math.min(100, Math.round((progress.downloaded / progress.contentLength) * 100))
    : undefined;

  return (
    <div className="dialog-backdrop" onMouseDown={() => !installing && onClose()}>
      <section className="dialog update-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">SIGNED DESKTOP UPDATE</span><h2>发现新版本</h2></div>
          <button className="icon-button" disabled={installing} onClick={onClose}>×</button>
        </div>

        <div className="update-version">
          <div><span>当前版本</span><b>v{update.currentVersion}</b></div>
          <span className="update-arrow">→</span>
          <div><span>可用版本</span><b>v{update.version}</b></div>
        </div>

        <div className="update-security-note">
          更新包来自 GitHub Release，并在安装前通过应用内置公钥验证签名。
        </div>

        <div className="update-notes">
          <span>版本说明</span>
          <p>{update.notes?.trim() || "该版本未提供更新说明。"}</p>
        </div>

        {installing && (
          <div className="update-progress">
            <div>
              <span>{progress?.phase === "installing" ? "正在安装，随后自动重启…" : "正在下载更新…"}</span>
              <b>{percentage === undefined ? "—" : `${percentage}%`}</b>
            </div>
            <progress max={100} value={percentage ?? undefined} />
          </div>
        )}

        <div className="dialog-actions">
          <button className="button" disabled={installing} onClick={onClose}>稍后</button>
          <button className="button primary" disabled={installing} onClick={onInstall}>
            {installing ? "正在更新…" : "下载、安装并重启"}
          </button>
        </div>
      </section>
    </div>
  );
}
