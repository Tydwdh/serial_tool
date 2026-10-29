# 硬件调试工作台文档

本目录放长期维护的文档；历史变更见根目录 [CHANGELOG](../CHANGELOG.md) 与 GitHub Releases 页。安装、构建与功能总览见根目录 [README](../README.md)。

## 阅读顺序

**插件作者**

1. [插件开发总览](plugins/README.md)：目录结构、运行时角色、开发流程与设计建议。
2. [plugin.json 参考](plugins/manifest.md)：字段、权限、`contributes`、`live`/`replay` 配置。
3. [Lua API 参考](plugins/lua-api.md)：`ctx.*`、串口 patterns、任务/协程、UI 事件协议。
4. [场景测试 DSL](plugins/scenarios.md)：不接硬件的自动化验收。
5. [插件协议枚举（自动生成）](plugins/spec.generated.md)。

**维护者**

1. [架构](ARCHITECTURE.md)：分层、命令/数据流、并发模型、单一组合根、测试策略。
2. [已知差异与残余工作](known-gaps.md)：刻意保留的差异与未闭合缺口。
3. [性能诊断](performance.md)：运行时诊断行与固定压力用例。

**发布与打包**

1. [发布流程](RELEASE.md)：版本、tag、CI 与 update.json。
2. [发布包与安装器](INSTALLER.md)：便携包、Windows 安装器、Ubuntu `.deb`。
3. [发布说明模板](releases/TEMPLATE.md)。

## 仓库策略

主仓库只保留插件系统、开发文档和少量模板；正式发布包默认不预装测试脚本或个人脚本，避免用户安装后看到与自己设备无关的功能。具体设备脚本建议放在单独的脚本仓库（例如 `hardware-workbench-scripts`），随插件一起维护 README、截图与兼容版本说明。

用户安装插件的位置：

- Windows 便携包 / 安装器：`<安装目录>\plugins\<plugin-id>\`
- Ubuntu `.deb`：`~/.local/share/HardwareWorkbench/plugins/<plugin-id>/`

插件配置写在用户配置目录的 `HardwareWorkbench\plugin-config\`；用 Windows 安装器卸载主程序时该目录会一并删除。

## 编辑器与校验

- `plugin.json` 的 JSON Schema 位于 `plugins/plugin.schema.json`；模板插件已内置 `$schema`，复制后编辑器会提供字段提示与校验。
- Lua 的编辑器提示位于 `plugins/.lua/`（LuaLS stub），根目录 `.luarc.json` 会自动加载；只用于补全与消除假报错，不参与运行。

## 发布包内的示例

`package.bat` 会把模板插件、`plugin.schema.json` 与 LuaLS stub 复制到发布包的 `examples\plugins\`，并在运行目录的 `plugins\` 下放一份 schema 与 stub。模板不会自动加载，需要试用时手动复制到 `plugins\` 下。

插件市场与发布脚本见 [plugin-marketplace/README.md](../plugin-marketplace/README.md)。
