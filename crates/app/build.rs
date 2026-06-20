#[cfg(windows)]
fn main() {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../../assets/app-icon.ico");
    resource.set("FileDescription", "Hardware Workbench");
    resource.set("ProductName", "Hardware Workbench");

    resource.compile().expect("failed to embed Windows icon");
}

#[cfg(not(windows))]
fn main() {}
