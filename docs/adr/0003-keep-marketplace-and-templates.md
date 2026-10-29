# 保留插件市场与模板插件

市场索引 `plugin-marketplace/registry.json` 的 `download_url` / `manifest_url` / `main_url` 直接指向 `main` 分支上的版本化 zip 与 `plugins/` 模板，且这些插件由维护者本人实际使用。删除它们会断掉已发布的安装路径，因此「清理无用部分」不包含市场条目、版本化 zip 与 `plugins/template.*`。

**Consequences**: 上述目录继续作为发布资产维护；清理范围只覆盖无生产/测试调用者的代码与失效文档引用。
