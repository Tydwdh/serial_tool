#[cfg(test)]
mod tests {
    use crate::bootstrap::{ACTIVITY_BAR_WIDTH, BOTTOM_PANEL_HEIGHT, BOTTOM_PANEL_MIN, app_dir};
    use crate::state::{
        BottomTab, DetachedPanelAction, LineEnding, SendUiState, StatusLevel, StatusState,
    };

    #[test]
    fn status_state_defaults() {
        let s = StatusState::default();
        assert_eq!(s.message, "就绪");
        assert_eq!(s.level, StatusLevel::Info);
        assert_eq!(s.deadline_ms, 0);
    }

    #[test]
    fn status_level_ttl() {
        assert_eq!(StatusLevel::Info.ttl_ms(), 5_000);
        assert_eq!(StatusLevel::Warn.ttl_ms(), 8_000);
        assert_eq!(StatusLevel::Error.ttl_ms(), 15_000);
    }

    #[test]
    fn status_level_ordering() {
        assert!(StatusLevel::Error > StatusLevel::Warn);
        assert!(StatusLevel::Warn > StatusLevel::Info);
    }

    #[test]
    fn send_ui_state_defaults() {
        let s = SendUiState::default();
        assert!(s.input.is_empty());
        assert!(!s.hex_mode);
        assert_eq!(s.line_ending, LineEnding::Lf);
        assert!(s.error.is_none());
        assert!(!s.popup_open);
    }

    #[test]
    fn bottom_tab_label() {
        assert_eq!(BottomTab::Terminal.label(), "接收");
        assert_eq!(BottomTab::Logs.label(), "日志");
    }

    #[test]
    fn bottom_tab_available() {
        assert!(BottomTab::Terminal.is_available(false));
        assert!(!BottomTab::Terminal.is_available(true));
        assert!(BottomTab::Logs.is_available(true));
    }

    #[test]
    fn detached_panel_action_variants() {
        let _ = DetachedPanelAction::None;
        let _ = DetachedPanelAction::Attach;
        let _ = DetachedPanelAction::Close;
    }

    #[test]
    fn app_dir_returns_path() {
        let dir = app_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn constants_are_positive() {
        assert!(ACTIVITY_BAR_WIDTH > 0.0);
        assert!(BOTTOM_PANEL_HEIGHT > 0.0);
        assert!(BOTTOM_PANEL_MIN > 0.0);
    }

    #[test]
    fn status_state_clone_works() {
        let s1 = StatusState::default();
        let s2 = s1.clone();
        assert_eq!(s1.message, s2.message);
    }
}
