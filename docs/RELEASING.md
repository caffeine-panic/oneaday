# Atlas Registry 发布流程

发布入口是 `.github/workflows/release.yml`。它只响应与应用版本完全一致的语义版本 tag，例如当前 `src-tauri/tauri.conf.json` 为 `0.1.1` 时只能使用 `v0.1.1`。工作流在 Linux x64、Windows x64、macOS Apple Silicon 和 macOS Intel 上构建原生安装包，并始终创建 GitHub **Draft Release**，不会自动公开。

所有 GitHub Actions 均固定到完整 commit SHA；更新 action 时应先核对官方 tag 指向，再同时更新注释中的版本。`release` environment 建议配置 required reviewers，tag `v*` 建议配置保护规则。

## macOS 签名与公证

未配置 Apple 凭据时，工作流使用 Tauri 官方支持的 ad-hoc identity `-`，仅用于产生可测试的 draft bundle；它不能替代 Developer ID 签名或公证，不应直接公开分发。

在 `release` environment 中同时配置以下 secrets 后，工作流会导入证书、使用实际 identity 签名，并把 Apple ID 凭据交给 Tauri 进行公证：

- `APPLE_CERTIFICATE`：Developer ID Application `.p12` 的单行 Base64；
- `APPLE_CERTIFICATE_PASSWORD`：导出 `.p12` 时的密码；
- `APPLE_KEYCHAIN_PASSWORD`：CI 临时 keychain 密码；
- `APPLE_ID`：Apple ID 邮箱；
- `APPLE_PASSWORD`：app-specific password；
- `APPLE_TEAM_ID`：Developer Team ID。

任何一项缺失都应保持 draft，不得公开 macOS bundle。

## Windows 与 Linux

当前 workflow 会生成 Windows 和 Linux draft bundle，但仓库尚未配置 Windows 代码签名证书或 Linux 包签名密钥。发布者应在公开 release 前完成组织证书接入、签名验证和恶意软件扫描；不要把 PFX、私钥或密码提交到仓库。

质量门禁会实际展开或安装每种包格式：Ubuntu 安装 DEB 并检查 `/usr/bin` 可执行文件，同时展开 AppImage；Windows 分别执行 MSI administrative install 与 NSIS silent install，并检查安装后的 `atlas-registry.exe`；macOS 校验 app/DMG、复制安装后的 bundle id 与代码签名结构。CI 会输出包和安装结果的 SHA-256。没有组织证书时，`NotSigned` 只能用于内部验证；任何 `HashMismatch` 都会直接失败。

## 发布检查

1. 确认 `quality.yml` 三平台通过，`compatibility.yml` 六项真实服务契约通过。
2. 同步 `package.json`、`src-tauri/Cargo.toml` 与 `src-tauri/tauri.conf.json` 的版本。
3. 创建并推送匹配的 `vX.Y.Z` tag。
4. 检查 Draft Release 中四类 bundle；验证 macOS 签名/公证以及 Windows/Linux 签名状态，并与 CI SHA-256 对照。
5. 在隔离机器上完成安装、首次启动、系统凭据库、三类连接和协议原生写入 smoke test 后，再手动发布 draft。
