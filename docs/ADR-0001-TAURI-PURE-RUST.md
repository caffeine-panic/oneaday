# ADR-0001：采用 Tauri 2 + 纯 Rust

- 状态：已接受
- 日期：2026-07-14

## 背景

Atlas Registry 需要以桌面应用形式统一访问 etcd、ZooKeeper 和 Nacos。早期研究推荐 Tauri + Go sidecar，以换取 etcd 官方 Go 客户端；项目最终选择纯 Rust，优先获得单进程、单一发布物和更窄的本地权限边界。

## 决策

采用 React/TypeScript WebView + Tauri 2 + Rust 核心：

- etcd：`etcd-client` 0.19，启用系统 TLS roots。
- ZooKeeper：`zookeeper-client` 0.11.1，启用 Tokio 与 TLS。
- Nacos：`nacos-sdk` 0.8；Config 客户端负责连接与单配置读取，2.x legacy 管理接口和 3.x Admin API 负责配置分页列表；Naming 能力在后续垂直切片接入。
- `RegistryCatalog` 声明 adapter descriptor 与能力；`RegistryService` 持有会话，并执行可取消的连接探测、浏览、有界标识搜索、读取和条件变更操作。
- 公共能力可以统一；lease、transaction、ACL、ephemeral、namespace、service 等原生能力不能抹平。

## 结果

- 删除 Go sidecar、进程监督、JSON-RPC framing 和外部二进制签名复杂度。
- Rust 二进制直接持有客户端连接，后续必须在 Tauri managed state 中实现长生命周期 session pool。
- 三个 Rust 客户端均不是 etcd/Apache ZooKeeper 官方 binding；兼容性责任由项目承担。
- `etcd-client` 构建依赖 `protoc`，开发和 CI 环境必须显式安装。
- 首发前必须验证 mTLS、ZooKeeper ACL/TLS/SASL、watch 重连、Nacos 2.x/3.x 分版管理 API。

## 当前进展

技术 Spike 已结束。正式开发已实现长生命周期 session、可取消的按需浏览、资源读取、1 MiB WebView 大值边界，以及创建、条件更新/删除、冲突反馈、所有写入的连接名二次确认和本地脱敏审计。etcd 与 ZooKeeper 使用服务端原子条件；通用 ZooKeeper create 仅创建继承父 ACL 的持久节点，ephemeral/sequential 留给原生能力入口；Nacos 更新使用 MD5 CAS，创建和删除明确标记为检查后变更。连接配置已支持版本迁移，密码与 token 进入系统凭据库；etcd 用户名密码与 mTLS、ZooKeeper digest 与 TLS、Nacos 用户名密码和自定义鉴权上下文已接入原生会话。实时链路已接入 etcd revision watch、ZooKeeper one-shot watch 自动重新布防和 Nacos SDK listener；订阅具备显式取消、连接关闭联动清理、后端 64 槽有界事件通道，以及 reconnecting、compacted、session expired 等 UI 状态，事件边界不包含资源值。Nacos 通过管理 API 心跳把 SDK 内部重连状态显式映射到 UI。标识搜索不读取 value：etcd 使用固定扫描窗口和游标，ZooKeeper 仅搜索当前层 children，Nacos 使用服务端 dataId 模糊分页；精确地址由读取接口直接定位。导出默认 metadata-only，包含 value 必须显式选择；导入文件受 8 MiB/50 条限制并验证摘要，value 只保存在 Rust 的 10 分钟一次性计划中，WebView 仅接收 create/update/skip 脱敏预览，实际写入复用条件变更和审计链路。本地审计历史以严格 DTO 倒序分页，每页最多扫描 512 KiB，不把原始 JSONL 交给前端；Nacos 2.x legacy 与 3.x Admin API 的配置历史列表默认只返回元数据，历史 value 仅在用户选择具体 revision 后读取，暂不提供无条件恢复。首批只读原生入口已经接入：etcd key 可查看 Lease ID、剩余 TTL 和授予 TTL；为兼容 etcd 3.7 不返回 Lease 字段的 keys-only 优化，检查会对 exact key 复用 1 MiB 边界读取并立即丢弃 value，但不会展开 Lease 关联 key。ZooKeeper znode 可查看 ACL 版本与最多 256 条身份/权限规则。两者均使用可取消 command，UI 不提供原生写入。Nacos 鉴权隔离为独立策略模块，ZooKeeper SASL 仍沿用客户端 `Connector` 作为后续扩展入口，尚未启用具体机制。公共 catalog 声明 `probe`、`browse`、`search`、`read`、`watch`、`create`、`update` 和 `delete`，etcd 额外声明 `lease`、ZooKeeper 额外声明 `acl`、Nacos 额外声明 `history`。etcd 3.6/3.7、ZooKeeper 3.8/3.9、Nacos 2.5/3.2 六项真实服务矩阵均已通过；三平台 quality workflow、显式 command capability、严格 CSP、固定 action SHA 和四平台 Draft Release 已接入。ZooKeeper SASL、Windows/Linux 组织签名以及其余生产能力仍待后续切片执行。

Nacos SDK 0.8 的 `remove_listener` 会移除应用回调，但上游实现暂不会在最后一个回调移除后清除其 session 级 cache/listen；该 SDK cache 还会写入用户目录。为避免已删除配置从残留 cache 返回给 UI，配置读取、写前检查和写后确认均使用对应版本的权威 HTTP API，SDK 只承担 gRPC mutation 与 listener。应用事件通道不会接收配置值，但在发布加固阶段仍必须通过升级、上游补丁或替换 listener 实现来收紧 SDK 的内存、磁盘与后台监听生命周期。
