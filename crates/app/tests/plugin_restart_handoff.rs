//! 插件「重启」交接的契约：判据必须取自权威状态，不能取自错误文案。
//!
//! 为什么只能按形状测：这段交接写在 `WorkbenchApp` 的渲染分支里（`src/ui/tiles.rs`），
//! 无头环境跑不了 egui，而它又是一条用户可见路径 —— 判据一失效，点「重启」就静默
//! 什么都不发生（旧写法 `message.contains("shutting down")` 正是如此：把
//! `ExtensionError::Stopping` 翻译成中文的那一刻起它恒假，而四条 CI 门全绿）。
//!
//! 判定本身的行为已由
//! `crates/application/tests/headless.rs::plugin_restart_handoff_relies_on_the_stopping_predicate`
//! 在 `Workbench` 层真实驱动过。本文件只补最后一厘米：组合根**问的是那条判定**，
//! 并且**没有再去匹配错误文本**。它不复制实现细节，只钉「调了谁 + 不许出现什么」。

/// 切出重启交接那个 `for` 循环的循环体（含首尾行）。
///
/// 按缩进切而不是数花括号，理由与 `startup_order.rs` 相同：体内字符串里可能出现成对括号。
fn restart_handoff_loop() -> String {
    const HEAD: &str = "for plugin_id in self.plugins_panel.take_pending_restart() {";
    let src = include_str!("../src/ui/tiles.rs");
    assert_eq!(
        src.matches(HEAD).count(),
        1,
        "`{HEAD}` 在 tiles.rs 里应恰好出现一次：0 次说明交接被搬走或改名，\
         多次说明这条断言切不出唯一的那一段"
    );
    let start = src.find(HEAD).expect("上一步刚断言过存在");
    let body_indent = src[..start]
        .rsplit('\n')
        .next()
        .expect("循环头之前必有内容")
        .len();
    let tail = &src[start..];
    let closing = format!("\n{}", " ".repeat(body_indent)) + "}";
    let end = tail
        .find(&closing)
        .unwrap_or_else(|| panic!("找不到与 `{HEAD}` 同缩进的收尾大括号，循环体切不出来"));
    tail[..end + closing.len()].to_owned()
}

#[test]
fn restart_handoff_asks_the_typed_predicate_before_dispatching_enable() {
    let loop_body = restart_handoff_loop();

    let asked = loop_body.matches("plugin_is_stopping(").count();
    let dispatched = loop_body.matches("AppCommand::EnablePlugin").count();
    assert_eq!(
        asked, 1,
        "循环体里必须恰好问一次 `plugin_is_stopping`，实际 {asked} 次：\
         0 次=又退回匹配文案或干脆丢了交接，多次=判定被复制"
    );
    assert_eq!(
        dispatched, 1,
        "循环体里必须恰好派发一次 `EnablePlugin`，实际 {dispatched} 次",
    );
    assert_eq!(
        loop_body.matches("push_pending_restart(").count(),
        1,
        "留给下一帧的那次 `push_pending_restart` 必须唯一，否则交接被复制了一份"
    );
    // 先问再派发：反过来（先派发再问）就是旧写法，判定读的是已被消费的窗口。
    assert!(
        loop_body.find("plugin_is_stopping(") < loop_body.find("AppCommand::EnablePlugin"),
        "必须先问 `plugin_is_stopping` 再派发 EnablePlugin，实际次序反了"
    );
}

/// 这条才是本轮的要害：把 `Stopping` 的文案换成中文后，CI 四道门都不看这里，
/// 只有本断言会红 —— 于是「翻译文案」这件事再也不能顺手杀掉重启交接。
#[test]
fn restart_handoff_never_inspects_error_text() {
    let loop_body = restart_handoff_loop();
    for forbidden in [
        ".contains(",
        "to_string()",
        "shutting down",
        "AppError::Plugin",
    ] {
        assert!(
            !loop_body.contains(forbidden),
            "重启交接里不得出现 {forbidden:?}，实际这段就是它：\
             判据一旦来自错误文本，改文案就会静默让「重启」失效（本轮要消灭的形状）"
        );
    }
}
