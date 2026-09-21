//! 卸载清理范围守卫：安装器删的，必须覆盖程序运行时会写的。
//!
//! 只读文本，不启动 egui、不装 Inno、不碰安装目录，因此 headless/CI 可跑。
//! 目的：这三处残留（eframe 持久化目录、被隔离的损坏配置、更新器写在 `{app}` 里的
//! 文件）此前都是"卸载后目录还在"的成因，而 `.iss` 当时没有任何断言 —— 少一条模式
//! 不会有任何东西变红。本文件把它们钉成契约。
//!
//! 真正的失效模式不是"有人删了一行"，而是**跨文件悄悄失配**：例如改了 `main.rs` 的
//! `with_app_id(...)`，卸载清单里那个小写目录名就再也匹配不上，而两边各自看都合理。
//! 因此下面的断言从代码里**读出**那个字符串，再去 `.iss` 里找它，而不是把字面量抄两遍。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // crates/app/tests -> 仓库根
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/app 之上应当是仓库根")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()))
}

/// 取出 `[UninstallDelete]` 段（到下一个 `[Section]` 或文件结尾为止）。
fn uninstall_delete_section() -> String {
    let iss = read("installer/hardware-workbench-app.iss");
    let start = iss
        .find("[UninstallDelete]")
        .expect(".iss 里必须有 [UninstallDelete] 段");
    let rest = &iss[start + "[UninstallDelete]".len()..];
    let end = rest.find("\n[").unwrap_or(rest.len());
    let section = rest[..end].to_owned();
    // 防空转：段落被截断或写成空时，后面的 contains 断言会全部无意义地失败/通过。
    assert!(
        section.matches("Type:").count() >= 10,
        "[UninstallDelete] 条目数异常（{}），解析可能截断了：\n{section}",
        section.matches("Type:").count()
    );
    section
}

/// eframe 的存储目录来自 `NativeOptions.viewport` 的 `app_id`；`persistence_path`
/// 未设时，Windows 下落到 `%APPDATA%\<app_id>\data\app.ron`。
fn app_id_from_main_rs() -> String {
    let main = read("crates/app/src/main.rs");
    let at = main
        .find("with_app_id(\"")
        .expect("main.rs 必须经 with_app_id(...) 指定 app_id");
    main[at + "with_app_id(\"".len()..]
        .split('"')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("解析 with_app_id 的字面量失败"))
        .to_owned()
}

#[test]
fn uninstaller_removes_the_eframe_persistence_directory_named_by_app_id() {
    let app_id = app_id_from_main_rs();
    let section = uninstall_delete_section();
    let wanted = format!("Name: \"{{userappdata}}\\{app_id}\"");
    assert!(
        section.contains(wanted.as_str()),
        "main.rs 的 app_id 是 {app_id:?}，eframe 就会往 %APPDATA%\\{app_id}\\data\\app.ron \
         写窗口与 memory；但 .iss 的 [UninstallDelete] 里没有 `{wanted}` —— 卸载会留下这个目录。\
         它和 HardwareWorkbench 是两个不同目录（大小写与层级都不同），不会被别的条目顺带删掉。"
    );
}

#[test]
fn uninstaller_covers_quarantined_corrupt_backups() {
    // 损坏配置的隔离副本由 core 命名：`{name}.corrupt-<时间戳>[-<序号>].backup`。
    let naming = read("crates/core/src/lib.rs");
    assert!(
        naming.contains(".corrupt-") && naming.contains(".backup"),
        "tool-core 里 `.corrupt-….backup` 的命名方式变了，请同步更新 .iss 的通配与本草拟断言"
    );
    let section = uninstall_delete_section();
    assert!(
        section.contains("*.corrupt-*.backup"),
        "%APPDATA%\\HardwareWorkbench 根下那份 workspace.json.corrupt-*.backup 没有被任何一条 \
         精确或通配模式匹配到；留下它就让目录非空，连 dirifempty 都失效"
    );
}

#[test]
fn uninstaller_cleans_updater_files_inside_the_install_directory() {
    let updater = read("crates/updater/src/lib.rs");
    let section = uninstall_delete_section();
    // `.exe.bak` 只在替换成功时删；探测文件在进程被强杀时留在原地。
    assert!(
        updater.contains(".bak\""),
        "更新器不再写 .bak？同步检查 .iss 的清理项"
    );
    assert!(
        updater.contains(".hw_update_probe_"),
        "更新器不再写探测文件？同步检查 .iss 的清理项"
    );
    assert!(
        section.contains("{app}\\*.exe.bak"),
        "替换失败时 `{{app}}\\hardware-workbench-app.exe.bak` 会残留，.iss 必须删它"
    );
    assert!(
        section.contains("{app}\\.hw_update_probe_"),
        "崩溃/强杀后 `{{app}}\\.hw_update_probe_<纳秒>` 会残留，.iss 必须删它"
    );
}

#[test]
fn catch_all_app_removal_is_the_last_entry() {
    let section = uninstall_delete_section();
    let entries: Vec<&str> = section
        .lines()
        .filter(|line| line.starts_with("Type:"))
        .collect();
    let last = *entries
        .last()
        .expect("[UninstallDelete] 不应为空 —— 空段落会让本文件所有断言变成假绿");
    assert!(
        last.trim_end() == "Type: filesandordirs; Name: \"{app}\"",
        "Inno 只在安装目录**已空**时才移除它，而 `copy_updated_resources` 会把更新包里的 \
         assets/docs/licenses/examples 复制进来（这些文件不在安装日志里）。兜底删 `{{app}}` \
         必须是最后一条，否则会先清空目录再谈精确模式。当前最后一条是：{last:?}"
    );
}

#[test]
fn guard_is_not_vacuous() {
    // 反向自检：往一个不存在的模式上断言必须失败，否则说明 `contains` 的比法本身有问题。
    let section = uninstall_delete_section();
    assert!(
        !section.contains("{userappdata}\\NoSuchDirectory"),
        "守卫本身失真：一个从未写入的模式怎么会出现在 [UninstallDelete] 里"
    );
    // 且必须真能"红"：把 app_id 换成别的就该匹配不到。
    let app_id = app_id_from_main_rs();
    assert!(
        !uninstall_delete_section().contains(&format!("Name: \"{{userappdata}}\\{app_id}-x\"")),
        "守卫失真：带后缀的目录名不该被匹配"
    );
}
