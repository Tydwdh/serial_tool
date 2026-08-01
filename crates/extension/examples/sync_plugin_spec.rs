fn main() -> Result<(), String> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    tool_extension::spec::synchronize_repository(&root)
}
