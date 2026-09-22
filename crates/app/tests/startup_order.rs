//! 启动顺序是 composition root 的契约，而它没法用真正的启动来测（`WorkbenchApp::new`
//! 要 `eframe::CreationContext`，无头环境拿不到）。所以这里改成把 `new` 函数体内
//! 各步的**先后次序**按字面钉住：拆函数时若把某一步提前到另一步之前，本用例会变红。
//!
//! 只断言次序与"每步在 `new` 体内恰好出现一次"，不复制实现细节；步骤与 `new` 的
//! 文档注释里那条顺序一一对应。

/// `new` 体内必须按这个顺序各出现一次的片段。
const STEPS: &[&str] = &[
    // 主题与字体最早：否则 eframe 可能已用默认主题、未注册中文字体就画了首帧。
    "apply_theme(&cc.egui_ctx, theme::AppTheme::default());",
    "setup_fonts(cc);",
    // bus 早于任何往它发日志的分支（配置损坏 / 未来版本 / 缺失三条都发）。
    "let bus = DataBus::new();",
    // 重绘 waker 先于 `startup_workbench`：`Workbench::new` 不收这个参数，只能构造后注入。
    "let waker: std::sync::Arc<dyn RepaintWaker>",
    "startup_config(&bus)",
    "let theme_dir = ensure_theme_dir();",
    // 主题 JSON 的读取先于它内部那次 `apply_theme`（取色按 `ACTIVE_THEME`）。
    "activate_startup_theme(&cc.egui_ctx",
    "let panels = startup_panel_manager(",
    "let (send, serial) = startup_ui_state(",
    "startup_workbench(&bus, waker,",
    // `Self` 字面量之后才是"回填面板设置 → 刷新端口 → 迁移回写配置"。
    "let mut app = Self {",
    "apply_persisted_panel_settings(&mut app,",
    "app.refresh_ports();",
    "rewrite_config_if_migrated(",
];

const NEW_HEAD: &str = "pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {";

/// 取 `new` 的函数体：从签名之后开始，到下一个同缩进的 `fn` 为止。
/// 用缩进切而不是数花括号，是因为体内 `format!("…{version}…")` 这类字符串里有成对的括号，
/// 数括号要在不懂词法的前提下跳过字符串，反而更容易错。
fn new_body() -> String {
    let src = include_str!("../src/app/mod.rs");
    let start = src
        .find(NEW_HEAD)
        .unwrap_or_else(|| panic!("找不到 `WorkbenchApp::new` 的签名：{NEW_HEAD}"));
    let tail = &src[start + NEW_HEAD.len()..];
    // 任何一条都可能是 `new` 之后的第一个条目；取最早出现的那条，否则会把后面的
    // 方法甚至私有函数的定义行一起算进"体内"，计数断言就会误报。
    let end = [
        "\n    fn ",
        "\n    pub(crate) fn ",
        "\n    pub fn ",
        "\n}",
        "\nfn ",
    ]
    .iter()
    .filter_map(|marker| tail.find(*marker))
    .min()
    .unwrap_or_else(|| panic!("`new` 之后找不到任何条目边界，函数体切不出来"));
    tail[..end].to_owned()
}

#[test]
fn startup_side_effects_keep_their_order() {
    let body = new_body();
    let mut previous = "<首步>";
    let mut last = 0usize;
    for step in STEPS {
        let hits = body.matches(*step).count();
        assert_eq!(
            hits, 1,
            "`new` 体内 `{step}` 应恰好出现一次，实际 {hits} 次：\
             0 次说明被删走或改名，多次说明步骤被复制"
        );
        let at = body.find(*step).expect("上一步刚断言过存在");
        assert!(
            at >= last,
            "启动步骤次序被改：`{step}` 排到了 `{previous}` 之前，\
             而 `new` 的文档注释规定它必须更晚"
        );
        last = at;
        previous = step;
    }
}

#[test]
fn startup_steps_actually_live_inside_new() {
    // 上一条只比次序，这条防空转：若整段启动逻辑被搬出 `new`，`new` 体里就只剩寥寥几行，
    // 次序断言会在一段无关文本上成立。
    let body = new_body();
    let top_level = body
        .lines()
        .filter(|line| line.starts_with("        ") && !line.starts_with("         "))
        .count();
    assert!(
        top_level >= STEPS.len(),
        "`new` 体内只有 {top_level} 行八空格缩进的顶层语句，少于约定的 {} 个启动步骤：\
         步骤大概被搬到了 `new` 之外，次序断言就失去意义了",
        STEPS.len()
    );
}
