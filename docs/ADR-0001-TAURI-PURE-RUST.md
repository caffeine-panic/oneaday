# ADR-0001：采用 Tauri 2 + 纯 Rust

- 状态：已接受
- 日期：2026-07-14

## 背景

Atlas Registry 需要以桌面应用形式统一访问 etcd、ZooKeeper 和 Nacos。早期研究推荐 Tauri + Go sidecar，以换取 etcd 官方 Go 客户端；项目最终选择纯 Rust，优先获得单进程、单一发布物和更窄的本地权限边界。

## 决策

采用 React/TypeScript WebView + Tauri 2 + Rust 核心：

- etcd：`etcd-client` 0.19，启用系统 TLS roots。
- ZooKeeper：`zookeeper-client` 0.11.1，启用 Tokio 与 TLS。
- Nacos：`nacos-sdk` 0.8；当前通过 Config 客户端实现连通性探测，Naming 与管理面能力在后续垂直切片接入。
- `RegistryCatalog` 是前端与协议实现之间的公共边界，返回 adapter descriptor，并提供连接探测。
- 公共能力可以统一；lease、transaction、ACL、ephemeral、namespace、service 等原生能力不能抹平。

## 结果

- 删除 Go sidecar、进程监督、JSON-RPC framing 和外部二进制签名复杂度。
- Rust 二进制直接持有客户端连接，后续必须在 Tauri managed state 中实现长生命周期 session pool。
- 三个 Rust 客户端均不是 etcd/Apache ZooKeeper 官方 binding；兼容性责任由项目承担。
- `etcd-client` 构建依赖 `protoc`，开发和 CI 环境必须显式安装。
- 首发前必须验证 mTLS、ZooKeeper ACL/TLS/SASL、watch 重连、Nacos 2.x/3.x 分版管理 API。

## 当前边界

本次落地完成 adapter catalog、真实客户端链接和连接探测，catalog 仅声明已经实现的 `probe` 能力。节点浏览、条件写入、删除、监听、凭据存储与长连接池属于后续垂直切片。
