# ADR-0001：采用 Tauri 2 + 纯 Rust

- 状态：已接受
- 日期：2026-07-14
- 最近更新：2026-07-16

## 背景

Atlas Registry 需要以桌面应用形式统一访问 etcd、ZooKeeper 和 Nacos。早期研究推荐 Tauri + Go sidecar，以换取 etcd 官方 Go 客户端；项目最终选择纯 Rust，优先获得单进程、单一发布物和更窄的本地权限边界。

## 决策

采用 React/TypeScript WebView + Tauri 2 + Rust 核心：

- etcd 使用 `etcd-client` 0.19，并启用系统 TLS roots。
- ZooKeeper 使用 `zookeeper-client` 0.11.1，并启用 Tokio 与 TLS。
- Nacos 使用 `nacos-sdk` 0.8。Config/Naming 客户端分别承担配置 mutation/listener 与临时实例 session；配置正文、写前检查、写后确认和范围管理由 2.x legacy 管理接口或 3.x Admin API 权威读取。
- `RegistryCatalog` 声明 adapter descriptor 与能力；`RegistryService` 持有长生命周期会话，并执行可取消的连接、浏览、有界标识搜索、读取、监听和条件变更。
- 公共能力只统一连接与资源操作外形；lease、transaction、ACL、ephemeral、namespace、service 等原生语义不被抹平。

## 结果

- 删除 Go sidecar、进程监督、JSON-RPC framing 和外部二进制签名复杂度。
- Rust 二进制直接持有客户端连接与临时资源 session，Tauri managed state 成为生命周期边界。
- 三个 Rust 客户端均不是 etcd/Apache ZooKeeper 官方 binding；兼容性责任由项目的真实服务矩阵承担。
- `etcd-client` 构建依赖 `protoc`，开发与 CI 环境必须显式安装。
- ZooKeeper SASL 保持为未启用的扩展点；公开发布仍需要组织签名、公证和各平台安装证据。

## 当前实现

正式客户端已完成三协议长生命周期 session、分页/懒加载浏览、条件写入、脱敏审计、监听、搜索、导入导出、历史和协议原生操作：

- etcd 覆盖 Lease 创建绑定、绑定已有、单次续租、解绑、撤销，以及 2–32 项、总载荷 1 MiB 的原子 transaction。
- ZooKeeper 覆盖 ACL aversion 条件写与 persistent、persistent sequential、ephemeral、ephemeral sequential 四种节点模式。
- Nacos 覆盖 2.x/3.x namespace/service、persistent instance，以及由 Naming SDK session 持有并维持心跳的 ephemeral instance 注册、更新和注销。
- Nacos 配置 listener 每 5 秒使用权威 HTTP MD5 对账，避免把 SDK cache 当作当前状态。
- 管理写入确认页展示环境、endpoint、namespace、操作和影响范围；原生审计记录真实前后状态的大小、编码、版本与 SHA-256，而不保存正文。
- 诊断包仅包含运行时、adapter capability 和连接聚合计数，不含连接标识、endpoint、namespace、资源内容或凭据。

etcd 3.6/3.7、ZooKeeper 3.8/3.9、Nacos 2.5/3.2 六项真实服务矩阵已通过。macOS 安装版与 DMG 已完成本机签名结构、复制安装和视觉 smoke。Windows/Linux 构建与安装检查已进入 quality workflow，但必须等远端 job 实际变绿后才构成平台证据。

## 已知上游约束

`nacos-sdk` 0.8 的 `remove_listener` 尚不会在最后一个回调移除后清除 session 级 cache/listen，且 SDK cache 会写入用户目录。应用因此不从该 cache 读取当前配置：正文读取、写前检查、写后确认和周期对账均走对应版本的权威 HTTP API；SDK 只承担 gRPC mutation、listener 与 Naming 临时实例 session。后续升级或替换 SDK 时应继续收紧其内存、磁盘和后台监听生命周期。
