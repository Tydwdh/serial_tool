# Hardware Workbench — 发布与维护指南

## 项目概览

**硬件调试工作台** (Hardware Workbench) 是一个跨平台的串口调试工具，使用 Rust + egui 构建，支持 Lua 插件系统、录制回放、多串口管理、自定义面板。

- 仓库: `Tydwdh/serial_tool`
- 版本号: `Cargo.toml` workspace `version` 字段 + `update.json`
- Rust edition: 2024
- 最低 Rust 版本: 1.96+ (与 CI 保持一致)

## 项目结构

```
crates/
  core/         — 基础类型 (Event, Payload, LogLevel, topics)
  databus/      — 发布/订阅事件总线 (DataBus)
  transport/    — 串口抽象层 (serialport, Windows native worker)
  extension/    — 插件管理器 (发现、启用、权限、生命周期)
  lua_host/     — Lua 5.4 运行时 (mlua, 沙箱, ctx.* API)
  panels/       — 所有 egui 面板 UI (terminal, log, chart, dock 等)
  recorder/     — JSONL 录制 + 回放引擎
  updater/      — 自更新系统 (HTTPS 下载 + SHA256 + zip 提取)
  marketplace/  — 插件市场客户端
  testing/      — 测试报告存储
  app/          — 应用壳 (eframe::App, 布局, 快捷键, 运行时)
plugins/        — 内置插件 (模板 + demo)
installer/      — Inno Setup 安装脚本
docs/           — 用户文档
assets/         — 字体、图标
```

## 本地开发

### 构建

```bash
# Debug 构建
cargo build

# Release 构建
cargo build --release

# 检查
cargo check --workspace
```

### 测试

```bash
# 运行所有测试（updater 测试需要管理员权限，在 CI 中跳过）
cargo test --workspace --lib

# 特定 crate
cargo test -p tool-panels --lib
cargo test -p tool-extension --lib
```

### 代码检查

```bash
# 格式化
cargo fmt --all

# Clippy
cargo clippy --all-targets -- -D warnings
```

## 发布流程

### 1. 准备工作

```bash
# 确保所有测试通过
cargo test --workspace --lib

# 确保格式正确
cargo fmt --all --check

# 确保 clippy 零警告
cargo clippy --all-targets -- -D warnings
```

### 2. 更新版本号

```powershell
# 编辑 Cargo.toml workspace.package.version
# 例如: version = "0.7.1"

# 提交版本号变更
git add Cargo.toml
git commit -m "release: v0.7.1"
```

### 3. 打包便携版

```bash
# Windows
.\package.bat

# 产出:
#   dist/hardware-workbench-app/    — 便携版目录
#   dist/hardware-workbench-app.zip — 便携版 zip
```

### 4. 构建安装器（可选，仅 Windows）

```powershell
# 需要安装 Inno Setup 6
choco install innosetup

# 构建安装器
.\installer\build-installer.ps1

# 产出:
#   dist/HardwareWorkbenchSetup.exe
```

### 5. 打标签发布

```bash
git tag v0.7.1
git push origin main
git push origin v0.7.1
```

### 6. CI 自动流程

推送 `v*` 标签后，CI 自动执行:

1. **Test & Lint** — `cargo fmt --check`, `cargo clippy`, `cargo test --all-targets`
2. **Package** — `package.bat` 生成便携版 zip
3. **Installer** — 安装 Inno Setup, 构建 `HardwareWorkbenchSetup.exe`
4. **Upload** — 上传两个文件到 artifact
5. **Release** — 使用 `softprops/action-gh-release@v2` 创建 GitHub Release，自动生成 release notes
6. **update.json** — 自动生成 `update.json` 并推回 `main` 分支

### 7. 发布插件（如有插件变更）

```powershell
.\plugin-marketplace\scripts\publish.ps1 `
  -PluginId gcode-sender `
  -Version 0.3.0
```

插件源码、市场索引和版本化 ZIP 均在本仓库维护。脚本默认从
`plugins/<plugin-id>/` 读取源码，将产物写入 `plugin-marketplace/`，并更新
`registry.json` 的版本、下载 URL、SHA256 和文件大小。

## update.json 格式

```json
{
  "version": "0.7.1",
  "date": "2026-07-06",
  "download_url": "https://github.com/Tydwdh/serial_tool/releases/download/v0.7.1/hardware-workbench-app.zip",
  "changelog": []
}
```

应用启动时检查 `https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json`，发现新版本后通过内置 updater 自动下载、SHA256 校验、替换 exe。

## 配置文件

- 路径: `%APPDATA%\HardwareWorkbench\workspace.json`
- 原子写入: 先写 `.tmp`，再重命名为 `.backup`，最后重命名为 `workspace.json`
- 包含: 面板布局、串口配置、快捷键映射、插件设置、最近工作区

## 插件系统

### 插件目录

- 开发期: `plugins/` 目录（通过 `build.rs` 同步到 `target/`）
- 安装期: 跟随 exe 的 `plugins/` 目录
- 市场安装: `app_dir()/plugins/`

### 插件清单

每个插件根目录下需要 `plugin.json`，格式见 `plugins/plugin.schema.json`。

支持两种清单格式:
- 旧格式: `main` + `permissions`
- 新格式: `live` + `replay` (结构化)

### 沙箱

Lua 插件运行在受限沙箱中:
- 禁止 `BASE`/`IO`/`OS`/`DEBUG`/`FFI` 标准库
- `package.preload` 只读
- `dofile`/`loadfile` 不可用
- 指令计数钩子防止无限循环

## 常见问题

### 测试失败: "请求的操作需要提升"

`tool-updater` 的某些测试需要管理员权限。跳过:

```bash
cargo test --workspace --lib --exclude tool-updater
```

### 构建失败: 找不到 `windows-sys`

确保在 Windows 上构建。transport crate 的 `windows_native.rs` 仅在 `cfg(windows)` 下编译。

### 插件同步失败

`build.rs` 负责将 `plugins/` 复制到 `target/<profile>/plugins/`。如果构建脚本报错，检查 `plugins/` 目录是否存在。
