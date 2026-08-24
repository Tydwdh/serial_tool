# 安装器与发布

本项目提供两个发布形态：

- Portable zip：由 `package.bat` 生成，解压即可运行。
- Windows installer：由 Inno Setup 脚本生成，适合正式发布给普通用户。
- Ubuntu 22.04+ `.deb`：由 `installer/linux/build-deb.sh` 生成，安装后从应用菜单直接运行。

## 生成 Ubuntu `.deb`

Ubuntu V1 目标为 x86_64（Debian 架构名 `amd64`）。在 Ubuntu 或其它带有
`dpkg-deb` 的 Debian 系统上执行：

```bash
bash ./installer/linux/build-deb.sh
```

输出文件：

```text
dist/hardware-workbench_<version>_amd64.deb
```

安装阶段需要系统管理员权限，这是一次性的系统安装权限：

```bash
sudo apt install ./dist/hardware-workbench_<version>_amd64.deb
```

安装完成后，应用菜单会出现 `Hardware Workbench`，也可以执行：

```bash
hardware-workbench-app
```

这两个启动方式都以当前普通用户运行，不要使用 `sudo hardware-workbench-app`。
程序、插件、主题和默认录制文件的用户可写数据分别位于：

```text
~/.config/HardwareWorkbench/
~/.local/share/HardwareWorkbench/
```

### 串口权限

如果打开 `/dev/ttyUSB0` 或 `/dev/ttyACM0` 时提示 `Permission denied`，应用会给出
Ubuntu 的标准处理方式：

```bash
sudo usermod -aG dialout $USER
```

执行后注销并重新登录即可。`.deb` 不会把所有串口设备开放给所有用户，也不会自动
修改 `dialout` 或安装宽泛的 udev 规则。

## 生成便携包

Windows PowerShell 5.1：

```powershell
Set-Location <仓库目录>
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

便携包不会默认带上个人脚本、测试脚本或 `gcode-sender`。

## 生成 Windows 安装器

先安装 Inno Setup 6，并确保 `ISCC.exe` 在 `PATH` 中，或安装在默认目录。

简体中文安装界面使用仓库内固定版本的
`installer\ChineseSimplified.isl`，不依赖 Inno Setup 安装目录中是否附带翻译文件。

Windows PowerShell 5.1：

```powershell
Set-Location <仓库目录>
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
Set-Location <仓库目录>
git tag -a v1.1.0 -m "v1.1.0"
git push origin v1.1.0
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
- 安装目录中的用户插件与自定义主题。
- `%APPDATA%\HardwareWorkbench\workspace.json`。
- `%APPDATA%\HardwareWorkbench\workspace.json.backup`。
- `%APPDATA%\HardwareWorkbench\plugin-config\`。
- `%APPDATA%\HardwareWorkbench\update\`（更新下载残留）。
- `%APPDATA%\HardwareWorkbench\updater\`（更新 helper 日志）。
- 安装目录中旧版残留的 `workspace.json` 和 `plugin-config\`。

这符合“卸载不残留配置”的目标，但也意味着用户插件配置不会保留。
