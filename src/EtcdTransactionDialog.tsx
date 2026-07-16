import { connectionEnvironmentLabels } from "./registry";
import type { ConnectionProfile, ValueEncoding } from "./registry";

export type EtcdTransactionDraftItem = {
  operation: "create" | "update" | "delete";
  key: string;
  value: string;
  encoding: ValueEncoding;
  expectedVersion: string;
};

type Props = {
  profile: ConnectionProfile;
  items: EtcdTransactionDraftItem[];
  confirmationText: string;
  busy: boolean;
  onItemsChange: (items: EtcdTransactionDraftItem[]) => void;
  onConfirmationTextChange: (value: string) => void;
  onCancel: () => void;
  onExecute: () => void;
  onCancelOperation: () => void;
};

export function emptyEtcdTransactionItem(): EtcdTransactionDraftItem {
  return {
    operation: "create",
    key: "",
    value: "",
    encoding: "utf8",
    expectedVersion: "",
  };
}

export function EtcdTransactionDialog({
  profile,
  items,
  confirmationText,
  busy,
  onItemsChange,
  onConfirmationTextChange,
  onCancel,
  onExecute,
  onCancelOperation,
}: Props) {
  const canSubmit = !busy
    && items.length >= 2
    && items.every((item) => item.key.trim()
      && (item.operation === "create" || item.expectedVersion.trim()))
    && confirmationText === profile.name;

  const replace = (index: number, next: EtcdTransactionDraftItem) => {
    const copy = [...items];
    copy[index] = next;
    onItemsChange(copy);
  };

  return (
    <div className="dialog-backdrop" onMouseDown={() => { if (!busy) onCancel(); }}>
      <section className="dialog etcd-transaction-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <div className="dialog-heading">
          <div><span className="eyebrow">ETCD TRANSACTION</span><h2>原子批量事务</h2></div>
          <button className="icon-button" disabled={busy} onClick={onCancel}>×</button>
        </div>

        <div className="impact-grid transaction-summary">
          <span>环境</span><b>{connectionEnvironmentLabels[profile.environment]}</b>
          <span>Endpoint</span><b>{profile.endpoint}</b>
          <span>操作数量</span><b>{items.length} / 32</b>
          <span>提交语义</span><b>全部 compare 成功后一次原子提交</b>
        </div>

        <div className="transaction-items">
          {items.map((item, index) => (
            <section className="transaction-item" key={index}>
              <div className="transaction-item-heading">
                <b>操作 {index + 1}</b>
                <button
                  className="button danger"
                  disabled={busy || items.length <= 2}
                  onClick={() => onItemsChange(items.filter((_, itemIndex) => itemIndex !== index))}
                >移除</button>
              </div>
              <div className="form-grid transaction-operation-grid">
                <label>操作
                  <select
                    value={item.operation}
                    onChange={(event) => replace(index, {
                      ...item,
                      operation: event.target.value as EtcdTransactionDraftItem["operation"],
                    })}
                  >
                    <option value="create">创建（key 必须不存在）</option>
                    <option value="update">条件更新</option>
                    <option value="delete">条件删除</option>
                  </select>
                </label>
                <label>Key
                  <input
                    value={item.key}
                    onChange={(event) => replace(index, { ...item, key: event.target.value })}
                    placeholder="/services/payment/config 或 base64:…"
                  />
                </label>
                {item.operation !== "create" && <label>期望 Mod Revision
                  <input
                    inputMode="numeric"
                    value={item.expectedVersion}
                    onChange={(event) => replace(index, { ...item, expectedVersion: event.target.value })}
                    placeholder="例如 42"
                  />
                </label>}
              </div>
              {item.operation !== "delete" && <>
                <label className="transaction-encoding">值编码
                  <select
                    value={item.encoding}
                    onChange={(event) => replace(index, {
                      ...item,
                      encoding: event.target.value as ValueEncoding,
                    })}
                  >
                    <option value="utf8">UTF-8</option>
                    <option value="base64">Base64（二进制）</option>
                  </select>
                </label>
                <label>新值
                  <textarea
                    value={item.value}
                    onChange={(event) => replace(index, { ...item, value: event.target.value })}
                    spellCheck={false}
                  />
                </label>
              </>}
            </section>
          ))}
        </div>

        <button
          className="button transaction-add"
          disabled={busy || items.length >= 32}
          onClick={() => onItemsChange([...items, emptyEtcdTransactionItem()])}
        >＋ 添加操作</button>

        <div className="mutation-warning">同一个 key 不能在一次事务中出现两次。任一 key 的版本条件不满足时，整个事务都不会执行；提交结果未知时请先刷新，切勿直接重试。</div>
        <label className="production-confirmation">请输入当前连接名 <b>{profile.name}</b> 确认一次性提交全部操作。
          <input
            value={confirmationText}
            onChange={(event) => onConfirmationTextChange(event.target.value)}
            placeholder={profile.name}
          />
        </label>
        <p className="form-note">事务中每个资源都会分别写入 started/applied 脱敏审计，只记录版本、大小、编码和 SHA-256。</p>
        <div className="dialog-actions">
          <button className="button" onClick={busy ? onCancelOperation : onCancel}>{busy ? "取消请求" : "取消"}</button>
          <button className="button primary" disabled={!canSubmit} onClick={onExecute}>{busy ? "正在提交…" : "确认原子提交"}</button>
        </div>
      </section>
    </div>
  );
}
