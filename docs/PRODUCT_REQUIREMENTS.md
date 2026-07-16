# Atlas Registry 产品需求

- 状态：功能实现完成；公开发布仍需组织签名凭据与 Draft Release 人工验收
- 产品方向：方案 A（三栏资源管理器）
- 技术基线：Tauri 2 + React/TypeScript + 纯 Rust

## 产品目标

Atlas Registry 是面向开发者与运维人员的跨平台桌面客户端，用一个一致的工作区安全地浏览和管理 etcd、ZooKeeper 与 Nacos，同时保留三种系统各自的并发、监听、权限和资源语义。

## 正式版本范围

### 连接管理

- 创建、编辑、复制、删除和测试连接；非敏感配置可持久化。
- 支持 etcd 多 endpoint、用户名密码与 mTLS。
- 支持 ZooKeeper 集群地址、chroot、digest、TLS，并为 SASL 保留扩展点。
- 支持 Nacos namespace、2.x/3.x API 版本、用户名密码和自定义鉴权扩展点。
- WebView 可持有 connection id 与用于展示/编辑的非敏感 profile；密码、token 和私钥不得进入前端持久化存储或日志。

### 资源工作区

- 三栏布局：连接列表、按需加载的资源树、资源详情与编辑器。
- etcd：prefix/key 浏览，二进制安全读取，revision/lease 元数据。
- ZooKeeper：znode 懒加载，数据与 Stat 元数据。
- Nacos：按 namespace 分页列举配置，展示 group/dataId，读取配置内容。
- 支持刷新、当前范围筛选、直接定位、分页/继续加载和大值保护；当前内联展示上限为 1 MiB。

### 变更与保护

- 创建、条件更新和删除；所有写入显示影响范围与环境。
- etcd 使用 mod revision，ZooKeeper 使用 version，Nacos 使用 MD5/CAS（可用时）避免静默覆盖。
- 递归删除、批量变更和生产环境写入必须二次确认。
- 编辑前后快照进入本地脱敏审计记录。

### 监听与生产能力

- etcd watch、ZooKeeper one-shot watch 自动续订、Nacos SDK listener。
- 断线重连、取消、compaction/session expiration 等状态在 UI 中明确呈现。
- 搜索、导入、导出和诊断包默认避免泄露 value 与凭据。
- 协议原生入口已实现：etcd lease/transaction、ZooKeeper ACL/ephemeral/sequential、Nacos namespace/service、persistent/SDK-managed ephemeral instance 与 history。

## 非功能要求

- 列表必须分页或按需加载，不允许无界读取整个集群。
- 网络操作有超时、可取消并返回结构化错误。
- 二进制 key/value 不得因 UTF-8 转换而损坏。
- 主窗口只授予声明过的 Tauri commands；默认 CSP 禁止任意远程内容。
- macOS、Windows、Linux 分别构建与测试；真实服务覆盖项目声明支持的版本矩阵。

## 开发切片与验收

1. **只读资源链路**：真实连接、会话复用、树浏览、读取和元数据展示。
2. **安全写入链路**：创建、条件保存、删除、确认与冲突反馈。
3. **连接持久化与认证**：系统凭据库、TLS、连接编辑和迁移。
4. **实时与批量能力**：监听、搜索、导入导出、历史与协议原生操作。
5. **发布加固**：真实服务兼容矩阵、大数据、泄漏扫描、签名和三平台安装包。

正式版本完成的证据必须同时包含：公共命令测试、三种真实服务集成测试、前端生产构建、Tauri release 构建，以及对应平台的安装与安全验证记录。当前实现证据和仍需在 CI/发布环境完成的项目见 [VERIFICATION.md](./VERIFICATION.md)。
