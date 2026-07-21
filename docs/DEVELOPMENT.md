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

默认情况下这些测试只执行连接、读取、列表与 value-free 标识搜索；配置了 fixture 时还会检查 etcd key 关联 Lease、ZooKeeper znode ACL，以及 Nacos 服务端历史列表和显式历史详情读取，不会写入测试集群。ZooKeeper fixture 凭据需要拥有读取该节点 ACL 的权限。

### 容器化兼容矩阵

本地 Docker daemon 可用时，可以逐项运行与 CI 相同的隔离契约：

```bash
./scripts/compatibility-test.sh etcd 3.7.0
./scripts/compatibility-test.sh zookeeper 3.9.5
./scripts/compatibility-test.sh nacos 3.2.3 v3
```

脚本只绑定 loopback 端口，为每次运行创建无持久卷的临时容器和 fixture；失败时输出最后 300 行容器日志，结束后始终删除容器。固定矩阵为 etcd 3.6.11/3.7.0、ZooKeeper 3.8.6/3.9.5、Nacos 2.5.2/3.2.3，分别覆盖当前与上一条受维护协议线，以及 Nacos 2.x/3.x 管理 API 断代。版本依据为 [etcd 官方发布与版本策略](https://etcd.io/docs/v3.7/op-guide/versioning/)、[ZooKeeper 官方发布策略](https://zookeeper.apache.org/releases/) 和 [Nacos 官方 Releases](https://github.com/alibaba/nacos/releases)。

如内部网络使用镜像仓库，可分别设置 `ATLAS_ETCD_IMAGE_REPOSITORY`、`ATLAS_ZOOKEEPER_IMAGE_REPOSITORY` 和 `ATLAS_NACOS_IMAGE_REPOSITORY`，值只包含仓库名，不包含版本 tag。Nacos 3.x 容器还会显式关闭仅用于隔离 fixture 的 Admin/Console 鉴权；这不改变应用对真实连接的认证处理。

`.github/workflows/compatibility.yml` 每周及相关主干变更时执行六项真实服务契约；`.github/workflows/quality.yml` 在 Ubuntu 22.04、Windows 2025 与 macOS 15 上执行前端构建、Rust 测试/Clippy、完整安装包构建与安装验证。Linux 验证 DEB 安装和 AppImage 解包，Windows 验证 MSI administrative install 与 NSIS silent install，macOS 验证 app/DMG、签名结构和复制安装。`.github/workflows/release.yml` 只接受匹配应用版本的 tag，并生成受保护的四平台 Draft Release。

tag 发版、Draft Release、macOS 签名/公证 secrets 以及公开发布检查见 [RELEASING.md](./RELEASING.md)。发布 workflow 默认不会公开 release；缺少组织签名凭据时产物只能用于内部验证。

认证测试可按协议提供 `ATLAS_TEST_<PROTOCOL>_USERNAME` 与
`ATLAS_TEST_<PROTOCOL>_PASSWORD`（`PROTOCOL` 为 `ETCD`、`ZOOKEEPER` 或
`NACOS`）。ZooKeeper 会使用 digest，其余两种协议使用用户名密码。etcd 与
ZooKeeper 的 TLS/mTLS 通过以下变量启用；测试代码只读取文件内容，不输出密钥：

```bash
ATLAS_TEST_ETCD_TLS=1 \
ATLAS_TEST_ETCD_TLS_CA=/path/to/ca.pem \
ATLAS_TEST_ETCD_TLS_CERT=/path/to/client.pem \
ATLAS_TEST_ETCD_TLS_KEY=/path/to/client-key.pem \
ATLAS_TEST_ETCD_TLS_SERVER_NAME=etcd.internal
```

将变量前缀替换为 `ATLAS_TEST_ZOOKEEPER` 即可验证 ZooKeeper TLS；ZooKeeper
必须提供 CA，且不支持单独覆盖 server name。

阿里云 MSE Nacos 的 RAM 鉴权改用以下变量；设置后优先于 Nacos 用户名密码：

```bash
ATLAS_TEST_NACOS_ACCESS_KEY_ID=your-access-key-id \
ATLAS_TEST_NACOS_ACCESS_KEY_SECRET=your-access-key-secret \
ATLAS_TEST_NACOS_ENDPOINT=mse-instance.nacos-ans.mse.aliyuncs.com:8848 \
cargo test --manifest-path src-tauri/Cargo.toml --test live_registry \
  nacos_live_session_can_browse_the_config_list -- --ignored --exact
```

AccessKey Secret 只作为 `ConnectionSecret` 进入测试进程，不写入连接配置或测试输出。

若要同时验证读取与元数据，提供已有的只读 fixture：

```bash
ATLAS_TEST_ETCD_KEY=/atlas/fixture \
ATLAS_TEST_ZOOKEEPER_PATH=/atlas/fixture \
ATLAS_TEST_NACOS_GROUP=DEFAULT_GROUP \
ATLAS_TEST_NACOS_DATA_ID=atlas-fixture.yaml
```

原生能力同样保持有界：etcd Lease 检查读取一次所选 exact key，并复用 1 MiB value 边界后立即丢弃内容；TTL 查询不要求服务端返回该 Lease 关联的全部 key。Lease 支持创建并绑定、绑定已有、单次续租、解绑和撤销，64 位 ID 以十进制字符串跨 IPC 传输。原子事务限制为 2–32 个不同 key，聚合载荷不超过 1 MiB，并为每个资源分别写入审计。

ZooKeeper ACL 最多接收 256 条规则，使用服务端 `aversion` 做条件更新，并强制保留至少一个 ADMIN 身份。节点创建支持 persistent、persistent sequential、ephemeral 与 ephemeral sequential；临时节点由当前桌面连接的 session 持有，断开时按 ZooKeeper 语义清理。单个父节点声明支持至 100,000 个直接子节点，列表和搜索都只传标识并使用游标；超过边界会返回明确的 `resourceExhausted`，不会无界扫描。

Nacos SDK 负责配置 gRPC mutation 与 listener；配置正文读取、写前检查和写后确认走对应 v2/v3 权威 HTTP API，避免 listener 移除后 SDK 残留 cache 把已删除配置作为当前值返回。监听同时每 5 秒做一次权威 MD5 对账，用于补偿 SDK 回调延迟或遗漏；事件仍只包含地址、变化类型和版本摘要。

namespace/service/instance 管理分别使用 Nacos 2.x Open API 与 3.x Admin API。原生管理 API 没有 CAS，因此客户端先比较读取时的 SHA-256 指纹，写入后最多进行 20 次、每次间隔 200 ms 的有界回读确认，并在确认页明确展示竞争窗口。持久实例继续使用管理 API；临时实例的注册、更新与注销由当前桌面连接的 Naming SDK session 执行并维持心跳，连接关闭后按 Nacos TTL 过期。Nacos 要求 service 与 instance 的生命周期一致，客户端会在远端提交前校验；2.x 实例响应不可靠返回生命周期字段时，以同一 service 的生命周期补全。

### 显式启用 mutation 循环

只在隔离测试集群中设置 `ATLAS_TEST_ENABLE_MUTATIONS=1`。每种协议会使用唯一资源名执行 create → 启动实时监听 → stale-version conflict → conditional update → 收到脱敏变化事件 → read → conditional delete，并继续覆盖协议原生写入：

```bash
ATLAS_TEST_ENABLE_MUTATIONS=1 \
ATLAS_TEST_ETCD_MUTATION_PREFIX=/atlas-registry-tests \
ATLAS_TEST_ZOOKEEPER_MUTATION_PARENT=/atlas-registry-tests \
ATLAS_TEST_NACOS_MUTATION_GROUP=ATLAS_REGISTRY_TEST \
cargo test --manifest-path src-tauri/Cargo.toml --test live_registry -- --ignored
```

ZooKeeper 的 mutation parent 必须预先存在。Nacos create/delete 没有服务端原子 CAS 条件，测试和 UI 都会将其报告为 `checkedBeforeMutation`；Nacos update 使用 SDK 的 MD5 CAS。

原生契约额外覆盖 etcd Lease 全生命周期与原子 transaction、ZooKeeper ACL 条件写和四种节点模式、Nacos namespace/service 以及 persistent/ephemeral instance 的创建、更新、回读确认与清理。当前实测证据见 [VERIFICATION.md](./VERIFICATION.md)。

## 本地审计

mutation command 会在应用配置目录的 `mutation-audit.jsonl` 写入 JSON Lines。started 事件在触发远端变更前同步落盘，并包含变更前的版本、大小、编码与 SHA-256 摘要；applied 事件记录远端返回的前后摘要。单条追加会在独立任务中完成 `write_all + sync_data`，不会被操作取消切断。取消、超时或提交后传输错误导致远端结果无法判定时会写入独立的 `mutationOutcomeUnknown` 事件；远端已确认成功但 applied 审计落盘失败则返回独立的 `auditIncomplete` 错误，不会伪装成远端结果未知。日志不记录资源 value、密码或 token。

批量导入逐条复用同一套条件变更与审计流程。遇到首个失败即停止，返回已应用、失败和未执行数量；导入计划在预览后只能使用一次。

应用内“历史”读取同一 JSONL 的严格脱敏 DTO，按字节游标从文件尾部倒序分页，每页最多扫描 512 KiB。Nacos 资源详情另提供服务端历史入口：列表不包含历史 content，选择具体 revision 后才调用详情接口并受 1 MiB 内联边界保护。

主界面的“诊断包”只在用户显式选择保存位置后写出 JSON。内容包括应用/运行时版本、adapter capability 描述，以及按协议、环境、TLS 和认证状态聚合的连接计数；不包含连接 ID/名称、endpoint、namespace、用户名、证书路径、资源地址/value 或任何凭据，并由 sentinel 单元测试约束。
