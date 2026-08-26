//! Platform-neutral update presentation DTOs.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfoView {
    pub version: String,
    pub date: String,
    pub download_url: String,
    pub changelog: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateStatusView {
    pub checking: bool,
    pub info: Option<UpdateInfoView>,
    pub error: Option<String>,
}
