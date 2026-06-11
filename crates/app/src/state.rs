use crate::app::StatusLevel;

#[derive(Clone)]
pub(crate) struct StatusState {
    pub(crate) message: String,
    pub(crate) level: StatusLevel,
    pub(crate) deadline_ms: u64,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            message: "就绪".into(),
            level: StatusLevel::Info,
            deadline_ms: 0,
        }
    }
}
