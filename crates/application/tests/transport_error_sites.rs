//! 形状守卫：`Workbench` 里那三处**运行时驱动不出来**的传输错误出口。
//!
//! 第 10 轮把 `workbench.rs` 的 9 处 `.map_err(|error| error.to_string())` 接到两个中文
//! 映射器上。其中 6 处由 `tests/headless.rs` 那一节真的造出失败并逐字钉住文案，剩下 3 处
//! 造不出来（理由记在 `headless.rs` 那节抬头里，摘要是：native 的端口枚举不会失败；
//! `NoPortAvailable` 要一台一个串口都没有的机器；`open_network_serial` 从不因连不上而失败）。
//! 两个映射器包错了站点编译就不过（入参是不同名的类型），所以这里只钉编译器管不着的那半：
//! **这三处的错误有没有又在翻译之前被 `to_string()` 压成英文原文**。
//!
//! 运行时覆盖不到的地方只能钉形状：任何一处退回 `.to_string()`，对应函数的映射器计数
//! 就少 1，这里立刻变红。按函数体切而不是全文搜，是因为 `send`（`SendPlanError`，正文
//! 本来就是中文）、`dispatch_load_replay`（io 错误，中文框架由 `tool-recorder` 在生产处
//! 补）、插件扫描与导出落盘（第 25 轮起走 `tool_platform::storage::translate_error`）
//! 用的都**不是传输映射器** —— 那是另外的靶子，混进来只会让本用例在无关改动时假报警。

const SOURCE: &str = include_str!("../src/workbench.rs");

/// `(函数名, 该函数体内必须出现的传输调用, 该函数体内两个映射器各应出现的次数)`。
///
/// 中间那项是防空调的凭据：没有那次传输调用，映射器计数就是在数空气。
const UNREACHABLE_SITES: &[(&str, &str, (usize, usize))] = &[
    ("spawn_refresh_ports", "backend.list_known_ports(", (0, 1)),
    ("spawn_request_port", "backend.request_port(", (0, 1)),
    // 只有网络分支驱动不出来；串口分支的同一函数由 headless 那节逐字钉着。
    ("connect", ".open_network_serial(", (1, 1)),
];

/// 按缩进切出某个 `fn` 的函数体（同 `crates/app/tests/startup_order.rs` 的办法）：
/// 数花括号要在不懂词法的前提下跳过 `format!("…{x}…")` 里的成对括号，反而更容易错。
fn body_of(fn_name: &str) -> String {
    let head_variants = [
        format!("    fn {fn_name}("),
        format!("    pub fn {fn_name}("),
        format!("    pub(crate) fn {fn_name}("),
    ];
    let (at, head_len) = head_variants
        .iter()
        .filter_map(|h| SOURCE.find(h).map(|i| (i, h.len())))
        .min()
        .unwrap_or_else(|| {
            panic!(
                "找不到 `{fn_name}` 的定义行：函数被改名或移出 workbench.rs 时，\
                 请连同本用例的期望值一起改"
            )
        });
    let tail = &SOURCE[at + head_len..];
    let start = tail.find('{').expect("签名之后必有 `{`") + 1;
    let end = ["\n    fn ", "\n    pub fn ", "\n    pub(crate) fn "]
        .iter()
        .filter_map(|m| tail[start..].find(*m))
        .min()
        .unwrap_or(tail[start..].len());
    tail[start..start + end].to_owned()
}

/// 体内每一处 `map_err(…)` 的参数文本（按括号配对切；这三处闭包体里都没有字符串字面量，
/// 所以不必处理引号内的括号）。
fn map_err_args(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut from = 0;
    while let Some(rel) = body[from..].find("map_err(") {
        let open = from + rel + "map_err(".len();
        let mut depth = 1;
        let mut i = open;
        while i < bytes.len() && depth > 0 {
            match bytes[i] as char {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        out.push(body[open..i.saturating_sub(1)].to_owned());
        from = i;
    }
    out
}

#[test]
fn the_three_undrivable_transport_exits_route_through_a_chinese_mapper() {
    for (name, needle, (transport, platform)) in UNREACHABLE_SITES {
        let body = body_of(name);
        assert!(
            body.contains(needle),
            "`{name}` 体内找不到 `{needle}`：函数结构变了，本用例的期望值已失效"
        );
        let t = body.matches("tool_transport::translate_error(").count();
        let p = body.matches("tool_platform::translate_error(").count();
        assert_eq!(
            (t, p),
            (*transport, *platform),
            "`{name}` 里的映射器数量变了：应 (tool_transport, tool_platform) = \
             ({transport}, {platform})，实际 ({t}, {p})。少一个通常意味着这一处又退回 \
             `.to_string()` —— 那两个枚举的 `Display` 是英文的，用户会直接看到 \
             `transport operation failed: …` 这类句子"
        );
        for arg in map_err_args(&body) {
            assert!(
                !arg.contains(".to_string()"),
                "`{name}` 里有一处 map_err 又把错误拍成 `Display` 原文: {arg}"
            );
        }
    }
}
