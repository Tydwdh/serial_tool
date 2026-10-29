# 08: 最终验证与发布

**What to build:** 全量门禁与 native 冒烟通过；`v2.0.0` 发布；GitHub Pages 停用。

**Blocked by:** 07.

**Status:** ready-for-agent

- [ ] `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace --all-targets` 通过。
- [ ] native 应用启动冒烟：设备/终端/发送器面板渲染，串口列表可刷新；截图留存。
- [ ] 打 `v2.0.0` tag 并触发发布；确认发布产物与 `update.json` 更新。
- [ ] `gh api -X DELETE /repos/Tydwdh/serial_tool/pages` 停用 Pages（执行前再次确认）。
