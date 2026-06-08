# 硬件调试工作台插件开发文档

这一组文档面向 `v0.1-preview` 版本，用于帮助插件作者通过 Lua 编写插件，不需要重新编译主程序。

## 推荐阅读顺序

1. [`PLUGIN_DEVELOPMENT.md`](./PLUGIN_DEVELOPMENT.md)：插件开发总览。
2. [`plugin-manifest.md`](./plugin-manifest.md)：`plugin.json` 字段说明。
3. [`lua-plugin-api.md`](./lua-plugin-api.md)：实时 Lua 插件可用的 `ctx.*` API。
4. [`replay-analyzer.md`](./replay-analyzer.md)：`replay.lua` 回放解析器。
5. [`recording-replay.md`](./recording-replay.md)：录制模式和回放策略。
6. [`release-checklist.md`](./release-checklist.md)：发布前检查清单。

## 示例插件

本压缩包包含两个模板插件：

```text
plugins/template.hello/
plugins/template.serial-chart/
```

复制模板目录，修改 `plugin.json` 的 `id/name/version`，再改 Lua 脚本即可开始开发。
