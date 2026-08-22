//! Architecture contract: tool-application 不依赖 UI。

use std::fs;

#[test]
fn application_cargo_does_not_depend_on_ui() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let text = fs::read_to_string(path).expect("read application Cargo.toml");
    for banned in [
        "egui",
        "eframe",
        "egui_tiles",
        "egui_extras",
        "egui_material_icons",
        "rfd",
        "tool-panels",
        "hardware-workbench-app",
    ] {
        assert!(
            !text.contains(banned),
            "tool-application Cargo.toml must not contain `{banned}`"
        );
    }
}

#[test]
fn core_and_databus_do_not_depend_on_ui() {
    for (label, rel) in [
        ("core", "../core/Cargo.toml"),
        ("databus", "../databus/Cargo.toml"),
        ("transport", "../transport/Cargo.toml"),
        ("recorder", "../recorder/Cargo.toml"),
    ] {
        let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
        let text = fs::read_to_string(&path).unwrap_or_default();
        for banned in ["egui", "eframe", "egui_tiles"] {
            assert!(
                !text.contains(banned),
                "{label} Cargo.toml must not contain `{banned}`"
            );
        }
    }
}
