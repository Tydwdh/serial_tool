#[cfg(windows)]
fn main() {
    let mut resource = winresource::WindowsResource::new();
    if let Some(rc_path) = find_rc_exe() {
        println!("cargo:rerun-if-env-changed=PATH");
        println!("cargo:rerun-if-env-changed=RC_PATH");
        unsafe {
            std::env::set_var("RC_PATH", rc_path);
        }
    }
    resource.set_icon("../../assets/app-icon.ico");
    resource.set("FileDescription", "Hardware Workbench");
    resource.set("ProductName", "Hardware Workbench");

    resource.compile().expect("failed to embed Windows icon");
}

#[cfg(windows)]
fn find_rc_exe() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("RC_PATH") {
        let path = std::path::PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(path) = find_on_path("rc.exe") {
        return Some(path);
    }

    let arch_dir = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86") {
        "x86"
    } else {
        "x64"
    };

    for root in windows_kit_roots() {
        let bin = root.join("bin");
        let direct = bin.join(arch_dir).join("rc.exe");
        if direct.exists() {
            return Some(direct);
        }

        let mut versioned = match std::fs::read_dir(&bin) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(arch_dir).join("rc.exe"))
                .filter(|path| path.exists())
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        versioned.sort();
        if let Some(path) = versioned.pop() {
            return Some(path);
        }
    }

    None
}

#[cfg(windows)]
fn find_on_path(file_name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(file_name))
        .find(|path| path.exists())
}

#[cfg(windows)]
fn windows_kit_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(std::path::PathBuf::from(program_files_x86).join("Windows Kits\\10"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(std::path::PathBuf::from(program_files).join("Windows Kits\\10"));
    }
    roots
}

#[cfg(not(windows))]
fn main() {}
