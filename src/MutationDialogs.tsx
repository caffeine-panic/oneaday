import { connectionEnvironmentLabels } from "./registry";
import type {
  AdapterId,
  ConnectionProfile,
  ResourceAddress,
  ResourceMutation,
  ZookeeperCreateMode,
} from "./registry";

export type NewResourceDraft = {
  keyOrPath: string;
  group: string;
  dataId: string;
  content: string;
  contentType: string;
  zookeeperMode: "persistent" | ZookeeperCreateMode;
};

type NewResourceDialogProps = {
  adapter: AdapterId;
  draft: NewResourceDraft;
  onChange: (draft: NewResourceDraft) => void;
  onCancel: () => void;
  onContinue: () => void;
};

export function NewResourceDialog({
  adapter,
  draft,
  onChange,
  onCancel,
  onContinue,
}: NewResourceDialogProps) {
  return (
    <div className="dialog-backdrop" onMouseDown={onCancel}>
      <section
        className="dialog resource-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="dialog-heading">
          <div>
            <span className="eyebrow">CREATE RESOURCE</span>
            <h2>新建资源</h2>
          </div>
          <button className="icon-button" onClick={onCancel}>
            ×
          </button>
        </div>
        {adapter === "nacos" ? (
          <div className="form-grid equal">
            <label>
              Group
              <input
                autoFocus
                value={draft.group}
                onChange={(event) =>
                  onChange({ ...draft, group: event.target.value })
                }
                placeholder="DEFAULT_GROUP"
              />
            </label>
            <label>
              Data ID
              <input
                value={draft.dataId}
                onChange={(event) =>
                  onChange({ ...draft, dataId: event.target.value })
                }
                placeholder="application.yaml"
              />
            </label>
          </div>
        ) : (
          <label>
            {adapter === "etcd" ? "Key" : "ZNode 路径"}
            <input
              autoFocus
              value={draft.keyOrPath}
              onChange={(event) =>
                onChange({ ...draft, keyOrPath: event.target.value })
              }
              placeholder={
                adapter === "etcd"
                  ? "/services/payment/config"
                  : "/services/payment"
              }
            />
          </label>
        )}
        {adapter === "zookeeper" && (
          <label>
            节点模式
            <select
              value={draft.zookeeperMode}
              onChange={(event) =>
                onChange({
                  ...draft,
                  zookeeperMode: event.target
                    .value as NewResourceDraft["zookeeperMode"],
                })
              }
            >
              <option value="persistent">持久节点</option>
              <option value="persistentSequential">持久顺序节点</option>
              <option value="ephemeral">临时节点</option>
              <option value="ephemeralSequential">临时顺序节点</option>
            </select>
          </label>
        )}
        <label>
          内容类型
          <input
            value={draft.contentType}
            onChange={(event) =>
              onChange({ ...draft, contentType: event.target.value })
            }
            placeholder="text / json / yaml"
          />
        </label>
        <label>
          初始内容
          <textarea
            value={draft.content}
            onChange={(event) =>
              onChange({ ...draft, content: event.target.value })
            }
            spellCheck={false}
          />
        </label>
        <p className="form-note">
          创建会先检查资源是否存在。etcd 与 ZooKeeper 使用原子 create；ZooKeeper
          节点继承父 ACL，临时节点由当前桌面连接的长生命周期 session 持有；Nacos
          创建是检查后发布，确认页会明确提示竞争窗口。
        </p>
        <div className="dialog-actions">
          <button className="button" onClick={onCancel}>
            取消
          </button>
          <button className="button primary" onClick={onContinue}>
            查看影响并确认
          </button>
        </div>
      </section>
    </div>
  );
}

type MutationConfirmationDialogProps = {
  mutation: ResourceMutation;
  profile: ConnectionProfile;
  confirmationText: string;
  busy: boolean;
  onConfirmationTextChange: (value: string) => void;
  onCancel: () => void;
  onConfirm: () => void;
  onCancelOperation: () => void;
};

export function MutationConfirmationDialog({
  mutation,
  profile,
  confirmationText,
  busy,
  onConfirmationTextChange,
  onCancel,
  onConfirm,
  onCancelOperation,
}: MutationConfirmationDialogProps) {
  const nonAtomic =
    profile.adapter === "nacos" && mutation.operation !== "update";
  const canConfirm = !busy && confirmationText === profile.name;
  const operationLabel =
    mutation.operation === "create"
      ? "创建"
      : mutation.operation === "update"
        ? "条件更新"
        : nonAtomic
          ? "检查后删除"
          : "条件删除";

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={() => {
        if (!busy) onCancel();
      }}
    >
      <section
        className="dialog mutation-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="dialog-heading">
          <div>
            <span className="eyebrow">MUTATION REVIEW</span>
            <h2>确认{operationLabel}</h2>
          </div>
          <button className="icon-button" disabled={busy} onClick={onCancel}>
            ×
          </button>
        </div>
        <div className="impact-grid">
          <span>环境</span>
          <b>{connectionEnvironmentLabels[profile.environment]}</b>
          <span>Endpoint</span>
          <b>{profile.endpoint}</b>
          <span>协议</span>
          <b>{profile.adapter}</b>
          <span>影响范围</span>
          <b>单个资源</b>
          <span>目标</span>
          <b>{addressText(mutation.address)}</b>
          <span>版本条件</span>
          <b>
            {"expectedVersion" in mutation
              ? mutation.expectedVersion
              : "必须不存在"}
          </b>
          <span>一致性</span>
          <b>{nonAtomic ? "检查后变更（非原子）" : "原子条件变更"}</b>
          {"value" in mutation && (
            <>
              <span>新值大小</span>
              <b>{mutationValueSize(mutation).toLocaleString()} B</b>
            </>
          )}
        </div>
        {nonAtomic && (
          <div className="mutation-warning">
            Nacos 的该操作没有服务端 CAS
            条件接口。客户端会先校验版本/不存在状态，但校验与变更之间仍存在竞争窗口。
          </div>
        )}
        <label className="production-confirmation">
          所有写入均需二次确认。请输入当前连接名 <b>{profile.name}</b>。
          <input
            value={confirmationText}
            onChange={(event) => onConfirmationTextChange(event.target.value)}
            autoFocus
            placeholder={profile.name}
          />
        </label>
        <p className="form-note">
          审计日志只保存版本、编码、大小与
          SHA-256，不保存资源原文。变更开始前会先落盘 started 事件。
        </p>
        <div className="dialog-actions">
          <button
            className="button"
            onClick={busy ? onCancelOperation : onCancel}
          >
            {busy ? "取消请求" : "返回"}
          </button>
          <button
            className={`button ${mutation.operation === "delete" ? "danger" : "primary"}`}
            disabled={!canConfirm}
            onClick={onConfirm}
          >
            {busy ? "正在提交…" : `确认${operationLabel}`}
          </button>
        </div>
      </section>
    </div>
  );
}

function addressText(address: ResourceAddress) {
  switch (address.type) {
    case "root":
      return "/";
    case "etcd":
      return `etcd:${decodeEtcdKey(address.keyBase64)}`;
    case "etcdPrefix":
      return `etcd-prefix:${address.prefixBase64}`;
    case "zookeeper":
      return address.path;
    case "nacosConfig":
      return `${address.group} / ${address.dataId}`;
  }
}

function decodeEtcdKey(encoded: string) {
  try {
    const bytes = Uint8Array.from(atob(encoded), (character) =>
      character.charCodeAt(0),
    );
    return new TextDecoder(undefined, { fatal: true }).decode(bytes);
  } catch {
    return `base64:${encoded}`;
  }
}

function mutationValueSize(
  mutation: Extract<ResourceMutation, { operation: "create" | "update" }>,
) {
  if (mutation.value.encoding === "utf8") {
    return new TextEncoder().encode(mutation.value.content).byteLength;
  }
  try {
    return atob(mutation.value.content).length;
  } catch {
    return 0;
  }
}
