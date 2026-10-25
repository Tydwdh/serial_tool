# 发布与维护

本文说明 Hardware Workbench 的版本号约定、发布准备、`v*` 标签触发的 CI 自动流程、`update.json`
清单以及插件发布链。发布包的打包与安装细节见 [INSTALLER.md](INSTALLER.md)。

## 支持平台

- Windows x64：主开发与发布平台。
- Linux x86_64（Ubuntu 22.04+）：`.deb` 打包并经 CI 冒烟测试。
- 浏览器在线模式：见 [web.md](web.md)。
- macOS **不受支持**：仓库没有 `target_os = "macos"` 的代码分支（全仓 grep 无命中），CI 不构建
  macOS，也没有对应发布包；`transport` 在该平台只会落到未经验证的非 Windows `serialport` 路径。

## 版本号与提交约定

版本号唯一权威来源是根 `Cargo.toml` 的 `[workspace.package].version`（当前 `1.3.0`）
（`Cargo.toml:19`）。发布时四者必须一致：

| 载体 | 约定 |
|------|------|
| Git tag | `v<version>`，例如 `v1.3.0` |
| `Cargo.toml` `[workspace.package].version` | 去掉 tag 的 `v` 前缀后的版本号 |
| `CHANGELOG.md` 版本标题 | `## [<version>] - YYYY-MM-DD` |
| `docs/releases/<tag>.md` | 与 tag 同名的发布说明，作为 GitHub Release 正文 |

CI 的 `test` 作业在推送 `v*` 标签时校验前两者一致，并校验 `docs/releases/<tag>.md` 存在
（`.github/workflows/ci.yml:52`，退出码非 0 即失败）。

CHANGELOG 的版本条目用 `- ` 列表记录变更；CI 生成 `update.json` 时从该版本的 `- ` 行抽取
`changelog` 数组（`.github/workflows/ci.yml:337`）。条目为空会让发布作业失败。

发布说明正文可参照 `docs/releases/TEMPLATE.md`。

## 发布准备

1. 修改 `Cargo.toml` 的 `[workspace.package].version`。
2. 在 `CHANGELOG.md` 新增 `## [<version>] - <日期>` 一节，列出变更。
3. 新建 `docs/releases/v<version>.md`（发布说明正文）。
4. 本地跑一遍 CI 等价检查：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test -p tool-panels -p tool-databus -- --ignored
```

5. 提交准备变更并打标签，推送 `main` 与标签：

```bash
git add -A
git commit -m "release: v1.3.0"
git tag -a v1.3.0 -m "v1.3.0"
git push origin main
git push origin v1.3.0
```

## CI 自动流程

工作流定义在 `.github/workflows/ci.yml`，触发条件为 `workflow_dispatch`、推送到 `main`、
推送 `v*` 标签或针对 `main` 的 PR（`.github/workflows/ci.yml:3`）。共 4 个作业：

| 作业 | Runner | 作用 |
|------|--------|------|
| `test`（Test & Lint） | `windows-latest` | 格式、clippy、测试、压力门禁；标签上再打包并构建安装器 |
| `linux-deb`（Ubuntu .deb） | `ubuntu-22.04` | Linux 检查/测试，构建 `.deb` 并安装冒烟 |
| `wasm`（WASM check） | `ubuntu-latest` | `wasm32-unknown-unknown` 编译与 lint |
| `release`（GitHub Release） | `windows-latest` | 仅在标签上运行，依赖前三个作业 |

### `test`

始终执行：`cargo fmt --all --check`（`:35`）、`cargo clippy --workspace --all-targets -- -D warnings`
（`:38`）、`cargo test --workspace --all-targets`（`:41`）、压力门禁
`cargo test -p tool-panels -p tool-databus -- --ignored`（`:49`）。

推送 `v*` 标签时追加：

1. **校验发布元数据**：tag 版本 == `hardware-workbench-app` 包版本，且 `docs/releases/<tag>.md` 存在
   （`:52`）。
2. **便携包**：运行 `.\package.bat`（`:67`）。
3. **安装器**：`choco install innosetup`，再 `.\installer\build-installer.ps1 -SkipPackage`（`:71`、`:75`）。
4. **校验和**：为 `dist/hardware-workbench-app.zip` 与 `dist/HardwareWorkbenchSetup.exe` 生成
   `dist/SHA256SUMS.txt`（`:79`）。
5. 上传 `hardware-workbench-windows` artifact（zip、安装器、`SHA256SUMS.txt`）（`:93`）。

### `linux-deb`

安装 `libgl1-mesa-dev`、`libssl-dev`、`libudev-dev`、`libwayland-dev`、`libx11-dev`、
`libxkbcommon-dev`、`pkg-config`（`:115`），然后 `cargo check --workspace`（`:127`）、Linux clippy
（`:130`，对除 `hardware-workbench-app` 外的成员加 `-D warnings`，缺口说明见
[known-gaps.md](known-gaps.md)）、`cargo test --workspace --all-targets`（`:141`）。

随后构建 `.deb`（`bash ./installer/linux/build-deb.sh`，`:144`），校验包元数据（`:147`），并在
`/usr/lib/hardware-workbench/`、桌面项、图标与字体上做安装冒烟（`:153`）。标签上再生成
`dist/SHA256SUMS-linux.txt`（`:172`）并上传 `hardware-workbench-linux` artifact（`:178`）。

### `wasm`

安装 `wasm32-unknown-unknown` 目标（`:198`），对可在 wasm32 上编译的成员执行
`cargo check`（`:211`）与 clippy（`:214`），排除 native-only 的 `tool-extension`、`tool-lua-host`、
`tool-marketplace`、`tool-transport`、`tool-updater`。

### `release`

仅在 `v*` 标签上运行（`:221`）。步骤：

1. 下载两个 artifact。
2. **记录将要上传的 zip 摘要**——必须在创建 Release 之前，哈希的对象就是即将上传的那一份
   （`:237`）。
3. 创建 Release：正文取 `docs/releases/<tag>.md`，附件为 zip、安装器、`.deb`、`SHA256SUMS.txt`、
   `SHA256SUMS-linux.txt`，并 `generate_release_notes: true`（`:251`）。
4. 切回默认分支，重新下载 zip 只为计算摘要（`:264`、`:272`）。
5. 生成 `update.json` 并提交回默认分支（`:278`）。

## update.json

桌面端启动时读取 `https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json`，结构见
`crates/updater/src/update_info.rs:9`：

| 字段 | 必填 | 说明 |
|------|------|------|
| `version` | 是 | 最新版本号 |
| `date` | 是 | 发布日期（CI 用中国标准时间，格式 `yyyy-MM-dd`） |
| `download_url` | 是 | 指向 GitHub Release 中 `hardware-workbench-app.zip` 的 URL |
| `sha256` | 是 | 发布产物的 SHA256，恰好 64 位十六进制 |
| `changelog` | 否 | 该版本 CHANGELOG 的 `- ` 条目 |

`sha256` 为**必填**：缺失或不满足 64 位十六进制时，客户端直接拒绝并报「解析更新信息失败：…」，
不会静默接受（`crates/updater/src/update_info.rs:30`、`:57`）。下载流哈希与外置固定值一致前不
落最终名，apply 前再与磁盘文件比对一次。当前**没有代码签名**，信任根是 GitHub 仓库写权限 + TLS。

发布作业对写入的摘要做双重校核（`.github/workflows/ci.yml:296` 起）：

1. **与 artifact 自带清单比对**：同一份 artifact 里的 `SHA256SUMS.txt` 必须对
   `hardware-workbench-app.zip` 恰好声明 1 行，且与该摘要一致。
2. **与已上传 Release 的 zip 比对**：拿创建 Release 之前记录的摘要再对一次，避免「pin 描述的
   不是用户拿到的字节」。

此外有下限把关：待发布 zip 小于 1 MiB 时拒绝使用其摘要（`:290`），因为截断的 zip 同样能算出
格式合法的 SHA256。

该更新流程用于 **Windows 便携版**；Ubuntu `.deb` 用户下载新版本 `.deb` 后用系统包管理器升级。
浏览器版不读 `update.json`。

## 发布插件

插件源码、市场索引与版本化 ZIP 均在仓库内维护。发布脚本为
`plugin-marketplace/scripts/publish.ps1`：

```powershell
.\plugin-marketplace\scripts\publish.ps1 `
  -PluginId gcode-sender `
  -Version 0.4.0
```

脚本默认从 `plugins/<plugin-id>/` 读取源码，把产物写入 `plugin-marketplace/`，并更新
`registry.json` 的版本、下载 URL、SHA256 与文件大小。完整流程见
[../plugin-marketplace/README.md](../plugin-marketplace/README.md)。

## 常见问题

### Windows 上构建报找不到 `windows-sys`

`windows-sys` 属于 `transport` crate 的 Windows 专用实现。`transport` 的 `windows_native`
模块以 `#[cfg(windows)]` 编译（`crates/transport/src/lib.rs:19`），因此该路径只应在 Windows 上
构建；跨平台构建时不应触发该依赖。

### 改了插件脚本但运行时仍是旧版

`crates/app/build.rs` 负责把仓库 `plugins/`（以及 `assets/themes/`）同步到
`target/<profile>/plugins`（`crates/app/build.rs`）。构建脚本报错时先确认 `plugins/` 目录存在。

### macOS 上能构建吗

不能作为受支持场景对待。判定依据与替代路径见本文开头「支持平台」一节。
