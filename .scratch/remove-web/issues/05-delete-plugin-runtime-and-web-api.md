# 05: 删除 plugin_runtime 与 plugin_api 的 web-only 面

**What to build:** 第二套 Lua 引擎与差分测试消失；`tool-plugin-api` 只保留 native 插件契约；现有插件与场景测试不受影响。

**Blocked by:** 02.

**Status:** ready-for-agent

- [ ] `tool-plugin-runtime` crate 删除；`omnilua` workspace 依赖删除；app 的 plugin runtime 依赖删除。
- [ ] `lua_api_parity` 与 `web_lua_engine` 测试删除。
- [ ] `tool-plugin-api` 的 engine/host 模块与 web-only 类型删除；保留 `PluginValue`、权限、ctx、错误及 native 实际使用的类型。
- [ ] `cargo test -p tool-lua-host -p tool-extension -p tool-application` 通过。
