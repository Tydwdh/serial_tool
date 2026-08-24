# 硬件调试工作台文档

这些文档面向准备编写、分发或维护 Lua 插件的人。插件不需要重新编译主程序，只要放到应用目录下的 `plugins\<plugin-id>\` 并在插件页刷新即可发现。

## 阅读顺序

1. [插件开发总览](./PLUGIN_DEVELOPMENT.md)：插件目录结构、实时插件、回放解析器和开发流程。
2. [plugin.json 说明](./plugin-manifest.md)：插件 ID、入口脚本、权限和 UI 贡献项。
3. [Lua 实时插件 API](./lua-plugin-api.md)：`ctx.*` API、串口、事件总线、UI、文件选择和配置。
4. [安装器与发布](./INSTALLER.md)：便携包、Windows 安装器、Ubuntu `.deb` 和卸载清理范围。

`plugin.json` 的 JSON Schema 位于 `plugins\plugin.schema.json`。模板插件已经内置 `$schema`，复制模板后编辑器通常会自动提供字段提示和基础校验。

Lua 插件 API 的编辑器提示位于 `plugins\.lua\`，项目根目录的 `.luarc.json` 会让 LuaLS 自动加载这些 stub。它们只用于消除假报错和提供补全，不参与运行。

## 插件和脚本仓库策略

主程序仓库只保留插件系统、开发文档和少量模板。正式发布包默认不预装测试脚本或个人脚本，避免用户安装后看到与自己设备无关的功能。

建议另外创建一个脚本仓库，例如 `hardware-workbench-scripts`，专门放：

- `gcode-sender` 这类具体设备/工作流脚本。
- 社区贡献的协议解析器。
- 不同硬件项目的示例配置。
- 插件 README、截图、版本说明和兼容的主程序版本。

用户安装脚本时，只需要把某个插件目录复制到（Ubuntu `.deb`）：

```text
~/.local/share/HardwareWorkbench/plugins/<plugin-id>/
```

Windows 便携包或安装器仍使用：

```text
<Hardware Workbench 安装目录>\plugins\<plugin-id>\
```

插件配置会写到用户配置目录下的 `HardwareWorkbench\plugin-config\`。使用 Windows 安装器卸载主程序时，该目录会一起删除。

## 发布包内的示例

`package.bat` 会把模板插件、`plugin.schema.json` 和 LuaLS stub 复制到 `examples\plugins\`，并在运行目录的 `plugins\` 下放一份 schema 与 stub。模板不会自动加载；需要试用时，可以手动复制到应用目录的 `plugins\` 下。
