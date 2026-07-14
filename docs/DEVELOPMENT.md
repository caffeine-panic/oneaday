# 开发与验证

## 快速检查

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle
```

`etcd-client` 在构建时需要 `protoc`。如果它不在 `PATH`，可显式设置 `PROTOC=/path/to/protoc`。

## 真实服务契约

默认测试套件不会假装存在本地注册中心。准备测试集群后，通过 ignored tests 验证连接、会话和根资源浏览：

```bash
ATLAS_TEST_ETCD_ENDPOINT=127.0.0.1:2379 \
ATLAS_TEST_ZOOKEEPER_ENDPOINT=127.0.0.1:2181 \
ATLAS_TEST_NACOS_ENDPOINT=127.0.0.1:8848 \
cargo test --manifest-path src-tauri/Cargo.toml --test live_registry -- --ignored
```

Nacos 3.x 额外设置：

```bash
ATLAS_TEST_NACOS_VERSION=v3
ATLAS_TEST_NACOS_NAMESPACE=public
```

默认情况下这些测试只执行连接、读取与列表操作，不会写入测试集群。

若要同时验证读取与元数据，提供已有的只读 fixture：

```bash
ATLAS_TEST_ETCD_KEY=/atlas/fixture \
ATLAS_TEST_ZOOKEEPER_PATH=/atlas/fixture \
ATLAS_TEST_NACOS_GROUP=DEFAULT_GROUP \
ATLAS_TEST_NACOS_DATA_ID=atlas-fixture.yaml
```

### 显式启用 mutation 循环

只在隔离测试集群中设置 `ATLAS_TEST_ENABLE_MUTATIONS=1`。每种协议会使用唯一资源名执行 create → stale-version conflict → conditional update → read → conditional delete：

```bash
ATLAS_TEST_ENABLE_MUTATIONS=1 \
ATLAS_TEST_ETCD_MUTATION_PREFIX=/atlas-registry-tests \
ATLAS_TEST_ZOOKEEPER_MUTATION_PARENT=/atlas-registry-tests \
ATLAS_TEST_NACOS_MUTATION_GROUP=ATLAS_REGISTRY_TEST \
cargo test --manifest-path src-tauri/Cargo.toml --test live_registry -- --ignored
```

ZooKeeper 的 mutation parent 必须预先存在。Nacos create/delete 没有服务端原子 CAS 条件，测试和 UI 都会将其报告为 `checkedBeforeMutation`；Nacos update 使用 SDK 的 MD5 CAS。

## 本地审计

mutation command 会在应用配置目录的 `mutation-audit.jsonl` 写入 JSON Lines。started 事件在触发远端变更前同步落盘，并包含变更前的版本、大小、编码与 SHA-256 摘要；applied 事件记录远端返回的前后摘要。单条追加会在独立任务中完成 `write_all + sync_data`，不会被操作取消切断。取消、超时或提交后传输错误导致远端结果无法判定时会写入独立的 `mutationOutcomeUnknown` 事件；远端已确认成功但 applied 审计落盘失败则返回独立的 `auditIncomplete` 错误，不会伪装成远端结果未知。日志不记录资源 value、密码或 token。
