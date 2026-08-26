//! Native compatibility module for the shared keyboard model.
//!
//! Keep this module so existing Native configuration and runtime code keeps
//! compiling while the persisted model is also available to Web.

pub(crate) use crate::shared_keymap::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_registry::CommandRegistry;

    const DEFAULT_BOUND: &[&str] = &[
        CMD_REFRESH_PORTS,
        CMD_OPEN_PORT,
        CMD_TOGGLE_BOTTOM_PANEL,
        CMD_TOGGLE_RIGHT_DOCK,
        CMD_SEND,
        CMD_COMMAND_PALETTE,
        CMD_CLEAR_TERMINAL,
    ];
    const DEFAULT_UNBOUND: &[&str] = &[CMD_START_RECORDING, CMD_RECONNECT_PORT, CMD_ADD_BOOKMARK];

    #[test]
    fn default_keymap_covers_all_builtin_commands() {
        let keymap = Keymap::default();
        for command in CommandRegistry::builtin().all() {
            let bound = keymap.bindings.contains_key(&command.id);
            assert!(
                bound || DEFAULT_UNBOUND.contains(&command.id.as_str()),
                "command {} should have a default binding or be explicitly unbound",
                command.id
            );
        }
    }

    #[test]
    fn default_bindings_match_expected_set() {
        let keymap = Keymap::default();
        for id in DEFAULT_BOUND {
            assert!(keymap.bindings.contains_key(*id));
        }
        for id in DEFAULT_UNBOUND {
            assert!(!keymap.bindings.contains_key(*id));
        }
    }

    #[test]
    fn keybinding_display_formats_correctly() {
        assert_eq!(
            KeyBinding::new("O", true, true, false).display(),
            "Ctrl+Shift+O"
        );
        assert_eq!(
            KeyBinding::new("Backtick", true, false, false).display(),
            "Ctrl+Backtick"
        );
        assert_eq!(
            KeyBinding::new("B", true, false, true).display(),
            "Ctrl+Alt+B"
        );
    }

    #[test]
    fn set_get_and_clear_bindings() {
        let mut keymap = Keymap::default();
        let binding = KeyBinding::new("F5", false, false, false);
        keymap.set_bindings(CMD_REFRESH_PORTS, vec![binding.clone()]);
        assert_eq!(keymap.get_bindings(CMD_REFRESH_PORTS), vec![binding]);
        keymap.set_bindings(CMD_REFRESH_PORTS, vec![]);
        assert!(keymap.get_bindings(CMD_REFRESH_PORTS).is_empty());
    }

    #[test]
    fn remove_binding_everywhere_clears_conflicts() {
        let mut keymap = Keymap::default();
        let binding = KeyBinding::new("F5", false, false, false);
        keymap.set_bindings(CMD_REFRESH_PORTS, vec![binding.clone()]);
        keymap.set_bindings(CMD_START_RECORDING, vec![binding.clone()]);
        keymap.remove_binding_everywhere(&binding);
        assert!(keymap.get_bindings(CMD_REFRESH_PORTS).is_empty());
        assert!(keymap.get_bindings(CMD_START_RECORDING).is_empty());
    }
}
