# Plugin API v2 边界

Plugin API v2 的目标是保持一个 `plugin.json`、一个 `main.lua` 和一套
`ctx.*` 语义，Native/Web 只替换 Lua VM 与平台 capability，不维护
`plugin-web.lua` 或另一套插件格式。

当前仓库已经落地 Rust 边界：`tool-plugin-api` 提供
`PluginValue`、`PluginHostApi`、`LuaEngine`、`PluginCapability`、
`PluginSerialDevice` 和 opaque `FileHandle`。Native `mlua` 兼容运行时与 Web
纯 Rust VM 都通过这些类型接入；不得把
`mlua::Value`、`PathBuf` 或桌面端 `COM3` 语义暴露到协议层。

## 串口语义

新协议使用设备句柄：

```lua
for _, device in ipairs(ctx.serial.devices()) do
  print(device.id, device.label)
end

ctx.serial.open({
  port_id = device.id,
  baud_rate = 115200,
  data_bits = 8,
  stop_bits = 1,
  parity = "none",
})

ctx.serial.send_to(device.id, "M105\n")
```

`port_id` 是 opaque identifier。Native 可以把它映射到 `COM3`，Web 映射
到浏览器授权的 `SerialPort`，插件不得解析其内容。

## 文件语义

`ctx.dialog.open_file()` 返回 `FileHandle`，`ctx.fs.read_text(file)` 接受
这个句柄。真实路径只存在于 Native capability 内部，Web 使用浏览器
`File`/OPFS 映射。

## Capability 计算

manifest 仍声明权限，例如 `serial`、`bus`、`ui`、`config`。宿主将它们
映射为 `PluginCapability`，再与当前平台提供的 capability 求差集。市场
状态应由这个结果自动计算，而不是由插件作者手填 `web: true`。

## 迁移顺序

1. 保持现有 v1 行为测试作为 conformance baseline。
2. Native `mlua` 与 Web 纯 Rust VM 都通过 `LuaEngine` 和 `PluginValue` 适配。
3. 统一 coroutine yield/scheduler 语义；浏览器文件选择等异步 capability
   通过 `PluginHostApi` 的 pending/completion 通道恢复 Lua task。

在第 2 步完成前，不会改变现有 Native 插件的启停和 `ctx.*` 行为。
