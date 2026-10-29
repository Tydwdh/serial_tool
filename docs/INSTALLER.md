# 发布包与安装器

本文说明三种发布形态的生成方式、包内布局、安装位置与卸载行为。发布流程与版本号约定见
[RELEASE.md](RELEASE.md)。

## 发布形态概览

| 形态 | 生成脚本 | 产物 |
|------|----------|------|
| 便携 zip | `package.bat`（Windows） | `dist/hardware-workbench-app\`、`dist/hardware-workbench-app.zip` |
| Windows 安装器 | `installer/build-installer.ps1`（Inno Setup 6） | `dist/HardwareWorkbenchSetup.exe` |
| Ubuntu `.deb` | `installer/linux/build-deb.sh`（Ubuntu/Debian） | `dist/hardware-workbench_<version>_amd64.deb` |

发布包默认不预装个人脚本或测试脚本；应用会创建空的插件目录，用户可从插件市场安装或手动放入。

## 便携包

```powershell
Set-Location <仓库目录>
.\package.bat
```

脚本先 `cargo build -p hardware-workbench-app --release`（`package.bat:15`），再组装
`dist/hardware-workbench-app\` 并压缩为 `dist/hardware-workbench-app.zip`（`package.bat:76`）。
目录布局：

| 路径 | 来源 / 说明 | 依据 |
|------|-------------|------|
| `hardware-workbench-app.exe` | `target\release\` 构建产物 | `package.bat:32` |
| `assets\` | 字体与图标：`JetBrainsMonoNerdFontMono-Regular.ttf`、`NotoSansSC-VF.ttf`、`app-icon.ico`、`FONT_LICENSES.md`、`OFL-1.1.txt`（存在才复制） | `package.bat:39` |
| `themes\` | 来自 `assets\themes\` | `package.bat:50` |
| `docs\` | 来自仓库 `docs\`（存在才复制） | `package.bat:49` |
| `README.md`、`CHANGELOG.md`、`LICENSE`、`THIRD_PARTY_NOTICES.md` | 仓库根文件 | `package.bat:52` |
| `licenses\` | `egui_tiles-LICENSE-MIT`、`ChineseSimplified-Translation-LICENSE-MIT` | `package.bat:56` |
| `plugins\plugin.schema.json`、`plugins\.lua\`、`.luarc.json` | 插件清单 schema 与 Lua 定义 | `package.bat:60` |
| `examples\plugins\` | `plugin.schema.json`、`.lua\` 与模板插件 `template.hello`、`template.serial-chart`、`template.file-tool` | `package.bat:64` |
| `logs\` | 空目录 | `package.bat:73` |

便携版没有安装步骤，解压即可运行；程序与数据目录都在解压目录内（见文末位置表）。

## Windows 安装器

前置：安装 Inno Setup 6，且 `ISCC.exe` 在 `PATH` 或默认安装目录中；也可用环境变量
`INNOSETUP_ISCC` 指定其完整路径（`installer/build-installer.ps1:8`）。

```powershell
Set-Location <仓库目录>
.\installer\build-installer.ps1
```

脚本流程（`installer/build-installer.ps1`）：

1. 校验简体中文语言文件 `installer\ChineseSimplified.isl` 存在。
2. 除非传入 `-SkipPackage`，先运行 `package.bat` 生成便携目录。
3. 通过 `cargo metadata` 读取 `hardware-workbench-app` 的版本，以 `/DMyAppVersion=<version>`
   传给 Inno Setup（`:33`、`:81`）。
4. 编译 `installer\hardware-workbench-app.iss`，产物为 `dist\HardwareWorkbenchSetup.exe`（`:54`）。

安装器要点（`installer/hardware-workbench-app.iss`）：

| 项目 | 值 |
|------|-----|
| AppId | `{9C06F7D9-4E3B-45CF-8C3A-4373D6F83C79}`（`:8`） |
| 默认安装目录 | `{localappdata}\Programs\HardwareWorkbench`（`:16`） |
| 权限 | `PrivilegesRequired=lowest`，**按当前用户安装，不需要管理员**（`:25`） |
| 架构 | `x64compatible`（`:26`） |
| 安装界面语言 | 英文 + `ChineseSimplified.isl`（`:33`） |
| 启动菜单快捷方式 | 始终创建（`:48`） |
| 桌面快捷方式 | 可选任务 `desktopicon`，默认不勾选（`:38`） |
| 安装内容 | `dist\hardware-workbench-app\` 全部文件复制进 `{app}`（`:44`） |
| 安装后 | 可选启动应用（`:51`） |

### 卸载清理范围

`[UninstallDelete]`（`installer/hardware-workbench-app.iss:54` 起）把程序文件与用户数据一并清除：

**安装目录（`{app}`）内：**

- 程序文件（安装时记录）。
- `logs\`、`plugins\`、`themes\`、`plugin-config\` 用户内容。
- 旧版残留的 `workspace.json`、`workspace.json.backup`、`plugin-config\`。
- 更新器残留：`*.exe.bak`（替换失败时只回滚不删）、`.hw_update_probe_*`（写权限探测文件）。
- 末尾兜底删除整个 `{app}` 目录。

**`%APPDATA%\HardwareWorkbench\` 内：**

- `workspace.json`、`workspace.json.backup`、`*.tmp`。
- 被隔离的损坏配置副本 `*.corrupt-*.backup`。
- `plugin-config\`、`update\`、`updater\`。
- 早期版本写在漫游目录下的遗留：`themes\`、`workspace-iced.json(+.backup)`。
- 最后 `dirifempty` 移除目录本身。

**另加：** `%APPDATA%\hardware-workbench\`——eframe 的窗口位置与 egui memory 持久化目录，目录名取自
`crates/app/src/main.rs:119` 的 `with_app_id("hardware-workbench")`，与上面的 `HardwareWorkbench`
是两个不同目录。

末尾对整个 `{app}` 的兜底删除是必需的：Inno 只删它记录过的文件、且只在目录已空时才移除目录，而
`copy_updated_resources` 会把更新包里的 `assets`/`docs`/`licenses`/`examples` 复制进来，这些新增
文件不在安装日志里，不兜底就会残留。

两个后果需要知道：

- Windows 的用户数据目录就是安装目录，因此卸载会连带删除 `logs\` 中的日志文件与用户插件配置。
- 该清单由 `crates/app/tests/installer_uninstall_scope.rs` 钉住：它从 `main.rs` 读出 `app_id`
  再在 `.iss` 中查找对应条目，因此改了 `with_app_id` 而未同步清单会直接测试失败。

## Ubuntu `.deb`

目标为 x86_64（Debian 架构名 `amd64`，`installer/linux/build-deb.sh:8`，其它架构直接报错退出）。
在 Ubuntu 或带 `dpkg-deb` 的 Debian 系统上执行：

```bash
bash ./installer/linux/build-deb.sh
```

产物 `dist/hardware-workbench_<version>_amd64.deb`（`build-deb.sh:46`）。安装需要一次性的系统
权限：

```bash
sudo apt install ./dist/hardware-workbench_<version>_amd64.deb
```

安装后可从应用菜单启动，或执行 `hardware-workbench-app`；两者都以当前普通用户运行，**不要**
用 `sudo hardware-workbench-app`。

### 包内布局与依赖

| 项 | 值 | 依据 |
|----|-----|------|
| 二进制 | `/usr/lib/hardware-workbench/hardware-workbench-app` | `build-deb.sh:51` |
| 命令 | `/usr/bin/hardware-workbench-app`（指向上述二进制的符号链接） | `build-deb.sh:67` |
| 字体资源 | `/usr/lib/hardware-workbench/assets/JetBrainsMonoNerdFontMono-Regular.ttf`、`NotoSansSC-VF.ttf` | `build-deb.sh:53` |
| 图标 | `/usr/share/icons/hicolor/256x256/apps/hardware-workbench.png` | `build-deb.sh:57` |
| 桌面项 | `/usr/share/applications/hardware-workbench.desktop` | `build-deb.sh:59` |
| 文档/版权 | `/usr/share/doc/hardware-workbench/{copyright,README.md}` | `build-deb.sh:61` |
| 运行时依赖 | `libgl1`、`libudev1`、`libx11-6`、`libxkbcommon0`、`libwayland-client0` | `installer/linux/control.in` |
| 包名 / 段 | `hardware-workbench` / `electronics` | `control.in` |
| 维护者 | `Tydwdh <tydwdh@users.noreply.github.com>` | `control.in` |

桌面项声明 `Exec=hardware-workbench-app`、`Icon=hardware-workbench`、分类
`Development;Electronics;`、`StartupWMClass=hardware-workbench`
（`installer/linux/hardware-workbench.desktop`）。

### 串口权限

打开 `/dev/ttyUSB0`、`/dev/ttyACM0` 等设备提示 `Permission denied` 时，应用按 Ubuntu 标准做法
提示把用户加入 `dialout` 组：

```bash
sudo usermod -aG dialout $USER
```

执行后注销并重新登录生效。`.deb` 不会放宽设备权限，也不会自动安装宽泛的 udev 规则。

### 卸载行为

`.deb` 不含 `prerm`/`postrm`/`conffiles`，因此 `dpkg -r` 与 `dpkg -P` 都**不会**删除用户数据：
`~/.config/HardwareWorkbench`、`~/.local/share/HardwareWorkbench`，以及 eframe 的
`~/.local/share/hardware-workbench` 都会保留。

## 版本号来源

版本号唯一权威来源是根 `Cargo.toml` 的 `[workspace.package].version`（`Cargo.toml:19`）。

- Windows 安装器：`build-installer.ps1` 用 `cargo metadata` 读取 `hardware-workbench-app` 版本，
  作为 `/DMyAppVersion` 传给 Inno Setup（`installer/build-installer.ps1:33`、`:81`）；`.iss` 中该宏
  缺失会直接编译报错（`installer/hardware-workbench-app.iss:3`）。
- Ubuntu `.deb`：`build-deb.sh` 默认用 `cargo pkgid -p hardware-workbench-app` 解析版本，也接受
  位置参数覆盖（`build-deb.sh:21`）；格式须匹配 `x.y.z` 语义版本（`build-deb.sh:27`）。

## 安装 / 配置 / 数据位置

| 用途 | Windows（便携 / 安装器） | Ubuntu `.deb` | 依据 |
|------|--------------------------|----------------|------|
| 程序与只读资源 | 便携：解压目录；安装器：`%LOCALAPPDATA%\Programs\HardwareWorkbench` | `/usr/lib/hardware-workbench/` | `hardware-workbench-app.iss:16` |
| 主配置 | `%APPDATA%\HardwareWorkbench\workspace.json` | `~/.config/HardwareWorkbench/workspace.json` | `crates/app/src/config.rs:238` |
| 插件配置 | `%APPDATA%\HardwareWorkbench\plugin-config\` | `~/.config/HardwareWorkbench/plugin-config/` | `crates/extension/src/manager.rs:71` |
| 用户数据根 | 安装目录（Windows 取 exe 所在目录） | `~/.local/share/HardwareWorkbench/` | `crates/app/src/bootstrap.rs:38` |
| 插件 | 用户数据根 `plugins\` | 用户数据根 `plugins/` | `bootstrap.rs:54` |
| 主题 | 用户数据根 `themes\` | 用户数据根 `themes/` | `bootstrap.rs:58` |
| 日志 | 用户数据根 `logs\` | 用户数据根 `logs/` | `bootstrap.rs:64` |
| 更新下载与清单 | `%APPDATA%\HardwareWorkbench\update\` | `~/.config/HardwareWorkbench/update/` | `crates/updater/src/lib.rs:415` |
| 更新 helper 日志 | `%APPDATA%\HardwareWorkbench\updater\` | `~/.config/HardwareWorkbench/updater/` | `crates/updater/src/lib.rs:437` |
| eframe 窗口/记忆 | `%APPDATA%\hardware-workbench\` | `~/.local/share/hardware-workbench/` | `crates/app/src/main.rs:119` |

Windows 上便携版与安装版的用户数据根都落在程序所在目录，因此便携包可整体搬移；Ubuntu 上程序
安装在只读的 `/usr/lib/hardware-workbench`，插件、主题与日志文件统一写入用户数据目录，无需 root。
