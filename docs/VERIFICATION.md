# 实现与发布验证记录

本记录对应 Atlas Registry `0.1.2` 当前工作树，验证日期为 2026-07-15 至 2026-07-17。它区分已经在本机取得的证据与必须由 GitHub Actions/组织发布环境完成的证据，避免把“配置了门禁”写成“所有平台已经通过”。

## 真实服务兼容性

以下六条契约均在 loopback 隔离实例上通过，mutation 模式会创建唯一 fixture 并在结束时清理：

| 协议 | 服务版本 | 已验证契约 |
| --- | --- | --- |
| etcd | 3.6.11、3.7.0 | 连接、分页浏览、读取、条件写、watch、冲突、Lease 全生命周期、64 位 Lease ID、原子 transaction |
| ZooKeeper | 3.8.6、3.9.5 | 连接、懒加载、读写、one-shot watch 续订、version 冲突、ACL aversion 条件写、persistent/sequential/ephemeral 节点与 session 清理 |
| Nacos | 2.5.2、3.2.3 | 配置列表/读写/MD5 冲突、SDK listener 与权威 MD5 对账、服务端历史、namespace/service、persistent instance 管理，以及 Naming SDK ephemeral instance 注册/更新/注销与写后回读 |

etcd 使用仓库兼容脚本：

```bash
./scripts/compatibility-test.sh etcd 3.6.11
./scripts/compatibility-test.sh etcd 3.7.0
```

ZooKeeper 与 Nacos 使用官方二进制发行包启动，并运行 `src-tauri/tests/live_registry.rs` 的 ignored mutation 契约。CI 中等价的六项固定矩阵由 `.github/workflows/compatibility.yml` 执行。

Nacos 3.2.3 的 SDK listener 在一次实测中未及时交付回调；加入每 5 秒一次的权威配置 MD5 对账后，同一契约重新执行通过。对账事件只携带地址、变化类型和版本摘要，不把配置正文送入 WebView。

## 规模、取消与安全边界

已通过的自动测试包括：

- 单个 ZooKeeper 父节点 100,000 个直接子节点，首尾游标页均保持 100 项，标识搜索不读取 value；100,001 个子节点返回明确的 `resourceExhausted`。
- 正好 1 MiB 的资源可进入内联展示，1 MiB + 1 byte 返回不可重试的 `valueTooLarge`。
- 取消会中断已注册操作并删除 cancellation registration；mutation 在提交结果无法判定时返回 `mutationOutcomeUnknown`，最终审计落盘不会被取消截断。
- 审计 JSONL、历史列表、watch 事件、Nacos HTTP 错误、管理操作 target、默认导出和导入预览均有 sentinel 测试，证明资源正文、metadata 敏感值、密码和 token 不跨越对应边界。
- 诊断包 sentinel 测试证明连接 ID/名称、endpoint、namespace、用户名、证书路径和凭据不进入导出，只保留 adapter/environment/TLS/auth 聚合计数。
- 系统凭据库测试证明连接 profile 只持久化非敏感字段，凭据替换、保留、清除和失败回滚分别受测。

## 安装版视觉与 macOS 产物

本机从 Tauri release 产物复制安装并启动 `Atlas Registry.app`，使用真实 etcd 3.7.0、ZooKeeper 3.8.6、Nacos 3.2.3 完成视觉 smoke：

- etcd 两项 transaction 原子提交成功，随后创建并绑定 Lease；UI 无损显示 `7587896224427818763` 这一超过 JavaScript 安全整数范围的 ID。
- ZooKeeper ACL 编辑器显示 aversion、逐项权限与 ADMIN 防误锁；节点模式菜单包含四种原生模式。
- Nacos 3 Admin API 列出 public namespace，创建服务和 persistent instance 后均完成权威回读，并展示脱敏审计结果。
- 视觉验收发现并修复了“实例变更成功后 service selection 被刷新清空、实例页变为空白”的状态机问题；修复后实例操作会保留当前服务并重载实例列表。
- `0.1.1` 修复全局结果 Toast 位于模态遮罩下方、被 `backdrop-filter` 模糊的问题；新增 UI 层级契约测试，要求 Toast 的唯一权威层级高于模态遮罩。
- `0.1.2` 修复在已打开连接之间切换后必须手动刷新才能看到数据的问题；重新选择当前连接会保留现有资源，切换到其他已打开连接会自动重载根目录，未打开连接仍等待用户主动连接。

视觉 smoke 完成后，又以同一 ignored mutation 契约分别连接官方 Nacos 2.5.2 与 3.2.3 发行包，验证临时 service/instance 的 Naming SDK 注册、权重/metadata 更新、心跳可见性和显式注销。2.x 管理响应省略/误报实例生命周期的差异由 service 生命周期补全，并已纳入回归路径。

本机 DMG：

```text
src-tauri/target/release/bundle/dmg/Atlas Registry_0.1.2_aarch64.dmg
SHA-256 4d7e8f64bdb80a6a1270c5ca3ee0e537afa09d5d6d1f8dd403028c11671b6f95
bundle id dev.oneaday.atlas-registry
```

构建时显式使用与 CI 相同的 `APPLE_SIGNING_IDENTITY=-`。`hdiutil verify`、DMG 内 app 的 `codesign --verify --deep --strict`、复制安装后的同一签名检查、bundle id 和可执行文件检查均通过。该产物使用 ad-hoc 签名，只能用于内部验证；没有 Developer ID 和 Apple 公证时不能作为公开 macOS 发行证据。

## 三平台发布门禁

`.github/workflows/quality.yml` 已配置以下必过项：

- Ubuntu 22.04：构建 DEB/AppImage，安装 DEB、检查安装后的可执行文件、展开 AppImage，并输出 SHA-256。
- Windows 2025：构建 MSI/NSIS，分别执行 MSI administrative install 与 NSIS silent install，检查两个安装目录中的 `atlas-registry.exe`、Authenticode 完整性与 SHA-256。
- macOS 15：构建 app/DMG，校验签名结构和 DMG，复制安装并核对 bundle id 与 SHA-256。

这些 Linux/Windows job 只有在远端 CI 实际变绿后才算平台证据；本记录不宣称它们已在当前 macOS 主机执行。公开发布还必须在 Draft Release 中补齐 Apple Developer ID/公证、Windows 组织代码签名和组织要求的 Linux 包签名/恶意软件扫描。

## 回归命令

交付前执行：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --quiet
npm run build
APPLE_SIGNING_IDENTITY=- npm run tauri build -- --bundles dmg
```
