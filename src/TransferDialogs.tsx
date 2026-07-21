import type {
  ConnectionProfile,
  ImportAction,
  ImportPreview,
  ResourceDocument,
} from "./registry";

type ExportDialogProps = {
  document: ResourceDocument;
  includeValue: boolean;
  busy: boolean;
  onIncludeValueChange: (include: boolean) => void;
  onCancel: () => void;
  onExport: () => void;
};

export function ExportDialog({
  document,
  includeValue,
  busy,
  onIncludeValueChange,
  onCancel,
  onExport,
}: ExportDialogProps) {
  return (
    <div
      className="dialog-backdrop"
      onMouseDown={() => {
        if (!busy) onCancel();
      }}
    >
      <section
        className="dialog transfer-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="dialog-heading">
          <div>
            <span className="eyebrow">SAFE EXPORT</span>
            <h2>导出资源</h2>
          </div>
          <button className="icon-button" disabled={busy} onClick={onCancel}>
            ×
          </button>
        </div>
        <div className="impact-grid">
          <span>资源</span>
          <b>{document.name}</b>
          <span>版本</span>
          <b>{document.version ?? "—"}</b>
          <span>大小</span>
          <b>{document.value.sizeBytes.toLocaleString()} B</b>
          <span>默认内容</span>
          <b>地址、元数据、版本、大小与 SHA-256</b>
        </div>
        <label className={`value-export ${includeValue ? "enabled" : ""}`}>
          <span>
            <input
              type="checkbox"
              checked={includeValue}
              disabled={busy}
              onChange={(event) => onIncludeValueChange(event.target.checked)}
            />
            在文件中包含资源 value
          </span>
          <small>
            默认关闭。开启后导出文件包含原始配置内容，应按敏感文件保管。
          </small>
        </label>
        <p className="form-note">
          文件由 Rust
          直接写入系统文件选择器返回的位置；前端不会获得文件路径，也不会重新拼装
          value。
        </p>
        <div className="dialog-actions">
          <button className="button" disabled={busy} onClick={onCancel}>
            取消
          </button>
          <button className="button primary" disabled={busy} onClick={onExport}>
            {busy ? "正在导出…" : "选择保存位置"}
          </button>
        </div>
      </section>
    </div>
  );
}

type ImportPreviewDialogProps = {
  preview: ImportPreview;
  profile: ConnectionProfile;
  confirmationText: string;
  busy: boolean;
  onConfirmationTextChange: (value: string) => void;
  onCancel: () => void;
  onApply: () => void;
  onCancelOperation: () => void;
};

const actionLabels: Record<ImportAction, string> = {
  create: "创建",
  update: "条件更新",
  skippedNoValue: "跳过（文件无 value）",
};

export function ImportPreviewDialog({
  preview,
  profile,
  confirmationText,
  busy,
  onConfirmationTextChange,
  onCancel,
  onApply,
  onCancelOperation,
}: ImportPreviewDialogProps) {
  const actionable = preview.creates + preview.updates;
  const canApply = !busy && actionable > 0 && confirmationText === profile.name;
  return (
    <div
      className="dialog-backdrop"
      onMouseDown={() => {
        if (!busy) onCancel();
      }}
    >
      <section
        className="dialog transfer-dialog import-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="dialog-heading">
          <div>
            <span className="eyebrow">IMPORT REVIEW</span>
            <h2>导入影响预览</h2>
          </div>
          <button className="icon-button" disabled={busy} onClick={onCancel}>
            ×
          </button>
        </div>
        <div className="import-summary">
          <div>
            <span>文件</span>
            <b>{preview.fileName}</b>
          </div>
          <div>
            <span>创建</span>
            <b>{preview.creates}</b>
          </div>
          <div>
            <span>更新</span>
            <b>{preview.updates}</b>
          </div>
          <div>
            <span>跳过</span>
            <b>{preview.skipped}</b>
          </div>
        </div>
        <div className="import-items">
          {preview.resources.map((item) => (
            <div
              className={`import-item ${item.action}`}
              key={JSON.stringify(item.address)}
            >
              <span className="import-action">{actionLabels[item.action]}</span>
              <b>{item.name}</b>
              <small>
                {item.sizeBytes.toLocaleString()} B · SHA-256{" "}
                {item.sha256.slice(0, 12)}…
              </small>
            </div>
          ))}
        </div>
        {profile.adapter === "nacos" && preview.creates > 0 && (
          <div className="mutation-warning">
            Nacos 创建是检查后发布，不是服务端原子
            create；预览与执行之间仍可能发生竞争。
          </div>
        )}
        {actionable === 0 ? (
          <p className="form-note">
            这是 metadata-only 导出，不含可写入的 value，因此所有条目都会跳过。
          </p>
        ) : (
          <label className="production-confirmation">
            导入会逐条执行并写入脱敏审计。请输入当前连接名 <b>{profile.name}</b>
            。
            <input
              value={confirmationText}
              onChange={(event) => onConfirmationTextChange(event.target.value)}
              autoFocus
              placeholder={profile.name}
            />
          </label>
        )}
        <p className="form-note">
          导入 value 仅保存在 Rust 的短期计划中，不会出现在预览响应。计划在{" "}
          {Math.round(preview.expiresInSeconds / 60)}{" "}
          分钟后过期，并且只能执行一次。
        </p>
        <div className="dialog-actions">
          <button
            className="button"
            onClick={busy ? onCancelOperation : onCancel}
          >
            {busy ? "取消当前写入" : "关闭"}
          </button>
          <button
            className="button primary"
            disabled={!canApply}
            onClick={onApply}
          >
            {busy ? "正在逐条应用…" : `确认写入 ${actionable} 项`}
          </button>
        </div>
      </section>
    </div>
  );
}
