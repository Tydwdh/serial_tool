# Hardware Workbench 插件市场

[Hardware Workbench](https://github.com/Tydwdh/serial_tool) 的插件市场目录。

本目录既是插件分发存储，也是市场索引来源。客户端通过拉取这里的
`registry.json` 浏览和安装插件；插件源码与市场产物现在都由 `serial_tool`
主仓库统一管理。

## 目录结构

```
plugin-marketplace/
├── registry.json                # 全局索引（客户端拉取此文件）
├── registry.schema.json         # registry.json 的 JSON Schema
├── plugins/
│   └── <plugin-id>/
│       └── <version>/
│           ├── plugin.json      # 该版本清单（从 zip 内抽出，便于在线浏览）
│           └── <plugin-id>-<version>.zip
└── scripts/
    └── publish.ps1              # 发布脚本：打包 + 算 SHA256 + 更新 registry.json
```

## 发布新插件 / 新版本

前置：插件源码位于仓库 `plugins/<plugin-id>/` 下，`plugin.json` 已填好元数据
（`description`/`author`/`license`/`category` 等字段）。

```powershell
# 在仓库根目录执行；SourcePath 默认是 plugins/<PluginId>
.\plugin-marketplace\scripts\publish.ps1 `
    -PluginId gcode-sender `
    -Version 0.7.0
```

脚本会：
1. 校验 `plugin.json` 存在且 `version` 与参数一致
2. 把插件目录打包成 `<plugin-id>-<version>.zip`（排除 `.git`、临时文件）
3. 计算 zip 的 SHA256
4. 复制 `plugin.json` 到版本目录
5. 更新 `registry.json`（新增或替换该插件条目，填入 `download_url`/`sha256`/`size`）

之后与主程序一起 `git commit`、`git push` 即可。GitHub raw URL 在提交推送后生效。

## 添加 / 更新插件源码

插件**源码**维护在 `plugins/` 下，便于随主程序一起测试；本目录只保存
**发布产物**（zip + 清单副本 + 索引）。

## 客户端安装流程

1. 应用从 `https://raw.githubusercontent.com/Tydwdh/serial_tool/main/plugin-marketplace/registry.json` 拉取索引
2. 用户选择插件 → 下载 `download_url` 指向的 zip
3. 校验 SHA256 → 解压到 `app_dir/plugins/<plugin-id>/`
4. 刷新插件发现 → 启用

安全模型：强制 https + 域白名单（`raw.githubusercontent.com` / `github.com` /
`objects.githubusercontent.com`）+ SHA256 校验 + 拒绝 zip 内的可执行扩展名
（dll/exe/sys/bat/ps1 等）。
