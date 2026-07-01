# 安装器与发布

本项目提供两个发布形态：

- Portable zip：由 `package.bat` 生成，解压即可运行。
- Windows installer：由 Inno Setup 脚本生成，适合正式发布给普通用户。

## 生成便携包

Windows PowerShell 5.1：

```powershell
Set-Location "C:\Users\tyd27\Desktop\tool"
.\package.bat
```

输出：

```text
dist\hardware-workbench-app\
dist\hardware-workbench-app.zip
```

便携包会包含：

- 主程序 exe。
- `assets\` 字体资源。
- `docs\` 文档。
- 空的 `plugins\` 目录。
- `examples\plugins\` 中的模板插件。
- 空的 `logs\` 目录。

便携包不会默认带上个人脚本、测试脚本或 `demo.gcode-sender`。

## 生成 Windows 安装器

先安装 Inno Setup 6，并确保 `ISCC.exe` 在 `PATH` 中，或安装在默认目录。

Windows PowerShell 5.1：

```powershell
Set-Location "C:\Users\tyd27\Desktop\tool"
.\installer\build-installer.ps1
```

脚本会先运行 `package.bat`，再调用 Inno Setup 编译：

```text
installer\hardware-workbench-app.iss
```

输出：

```text
dist\HardwareWorkbenchSetup.exe
```

## 版本号来源

项目版本以根目录 `Cargo.toml` 的 `[workspace.package].version` 为准。安装器构建脚本会通过 `cargo metadata` 读取 `hardware-workbench-app` 的版本，并传给 Inno Setup。

发布新版本时只需要：

1. 修改 `Cargo.toml` 中的版本号。
2. 提交版本变更。
3. 打同名 tag，例如版本 `0.1.1` 对应 `v0.1.1`。

Windows PowerShell 5.1：

```powershell
Set-Location "C:\Users\tyd27\Desktop\tool"
git tag -a v0.1.1 -m "v0.1.1"
git push origin v0.1.1
```

## 安装位置

安装器默认安装到当前用户目录：

```text
%LOCALAPPDATA%\Programs\HardwareWorkbench
```

这样不需要管理员权限，也允许应用在安装目录下创建 `logs\`。

## 配置位置

主程序配置：

```text
%APPDATA%\HardwareWorkbench\workspace.json
%APPDATA%\HardwareWorkbench\workspace.json.backup
```

插件配置：

```text
%APPDATA%\HardwareWorkbench\plugin-config\
```

旧版或便携模式可能产生的配置：

```text
<安装目录>\workspace.json
<安装目录>\plugin-config\
```

## 卸载清理范围

安装器卸载时会删除：

- 安装目录中的程序文件。
- 安装目录中的 `logs\`。
- `%APPDATA%\HardwareWorkbench\workspace.json`。
- `%APPDATA%\HardwareWorkbench\workspace.json.backup`。
- `%APPDATA%\HardwareWorkbench\plugin-config\`。
- `%APPDATA%\HardwareWorkbench\update\`（更新下载残留）。
- `%APPDATA%\HardwareWorkbench\updater\`（更新 helper 日志）。
- 安装目录中旧版残留的 `workspace.json` 和 `plugin-config\`。

这符合“卸载不残留配置”的目标，但也意味着用户插件配置不会保留。
