//! One command plan, two platform adapters.
//!
//! Every `AppCommand` is resolved here exactly once — validation, routing,
//! outcome text and the task kind. The native composition root implements
//! [`CommandEffects`] with the operations that genuinely differ.

use std::path::PathBuf;

use crate::TaskId;
use crate::recording::RecordModeView;
use crate::replay::ReplayPolicyView;
use crate::send_plan::{PlannedSend, plan_send};
use tool_platform::storage::FileHandle;
use tool_platform::{NetworkSerialConfig, PortId, SerialSettings};

use super::{AppCommand, CommandError, CommandOutcome};

/// The platform operations the shared command plan delegates to.
///
/// Defaults cover the platform-only commands: the side that cannot support a
/// command reports it with the same Chinese body the old hand-written match
/// used, so both roots keep their documented behaviour.
pub trait CommandEffects {
    // ── ports / transport ────────────────────────────────────────────────
    fn refresh_ports(&mut self) -> Result<CommandOutcome, CommandError>;
    fn request_port(&mut self) -> Result<CommandOutcome, CommandError>;
    fn register_network_port(&mut self, config: NetworkSerialConfig) -> Result<(), CommandError>;
    fn remove_network_port(&mut self, port: PortId) -> Result<(), CommandError>;
    fn connect(
        &mut self,
        port: PortId,
        settings: SerialSettings,
    ) -> Result<CommandOutcome, CommandError>;
    fn apply_serial_settings(&mut self, settings: SerialSettings);
    fn disconnect(&mut self, port: PortId) -> Result<CommandOutcome, CommandError>;
    fn reconnect(&mut self, port: PortId) -> Result<CommandOutcome, CommandError>;
    fn cancel_reconnect(&mut self, port: PortId) -> Result<CommandOutcome, CommandError>;
    fn cancel_task(&mut self, task_id: TaskId) -> bool;
    fn is_network_port(&self, port: &PortId) -> bool;
    fn send_planned(
        &mut self,
        port: PortId,
        plan: PlannedSend,
    ) -> Result<CommandOutcome, CommandError>;
    fn set_signal(
        &mut self,
        port: PortId,
        value: bool,
        dtr: bool,
    ) -> Result<CommandOutcome, CommandError>;

    // ── recording ────────────────────────────────────────────────────────
    fn start_recording(
        &mut self,
        file: FileHandle,
        mode: RecordModeView,
    ) -> Result<CommandOutcome, CommandError>;
    fn set_recording_mode(&mut self, mode: RecordModeView);
    fn stop_recording(&mut self) -> Result<CommandOutcome, CommandError>;
    fn pause_recording(&mut self) -> Result<CommandOutcome, CommandError>;
    fn resume_recording(&mut self) -> Result<CommandOutcome, CommandError>;
    fn add_recording_bookmark(&mut self, name: Option<String>);

    // ── replay ───────────────────────────────────────────────────────────
    fn load_replay(&mut self, file: FileHandle) -> Result<CommandOutcome, CommandError> {
        let _ = file;
        Err(CommandError::InvalidCommand(
            "当前平台不支持按路径加载回放".to_owned(),
        ))
    }
    fn replay_play(&mut self) -> Result<(), CommandError>;
    fn replay_pause(&mut self) -> Result<(), CommandError>;
    fn replay_stop(&mut self) -> Result<(), CommandError>;
    fn replay_seek(&mut self, position_ms: u64) -> Result<(), CommandError>;
    fn replay_seek_by(&mut self, delta_ms: i64) -> Result<(), CommandError>;
    fn replay_step(&mut self, delta: i32) -> Result<(), CommandError>;
    fn replay_set_speed(&mut self, speed: f64);
    fn replay_set_policy(&mut self, policy: ReplayPolicyView) -> Result<(), CommandError>;
    fn replay_add_bookmark(&mut self, name: Option<String>);
    fn replay_remove_bookmark(&mut self, position_ms: u64);

    // ── plugins / marketplace / update ───────────────────────────────────
    fn enable_plugin(&mut self, plugin_id: String) -> Result<(), CommandError>;
    fn disable_plugin(&mut self, plugin_id: String) -> Result<(), CommandError>;
    fn reload_plugins(&mut self) -> Result<CommandOutcome, CommandError>;
    fn discover_plugins(&mut self, roots: Vec<PathBuf>) -> Result<CommandOutcome, CommandError> {
        let _ = roots;
        Err(CommandError::InvalidCommand(
            "当前平台不支持扫描插件目录".to_owned(),
        ))
    }
    fn execute_plugin_command(
        &mut self,
        plugin_id: String,
        command_id: String,
        context: serde_json::Value,
    ) -> Result<(), CommandError>;
    fn refresh_marketplace(&mut self, url: String) -> Result<CommandOutcome, CommandError> {
        let _ = url;
        Err(CommandError::InvalidCommand(
            "市场和更新由桌面组合根负责".to_owned(),
        ))
    }
    fn check_for_update(&mut self) -> Result<CommandOutcome, CommandError> {
        Err(CommandError::InvalidCommand(
            "市场和更新由桌面组合根负责".to_owned(),
        ))
    }

    // ── terminal ─────────────────────────────────────────────────────────
    fn clear_terminal(&mut self) -> Result<(), CommandError> {
        Ok(())
    }
    fn set_terminal_merge_window(&mut self, ms: u64);
    fn set_terminal_max_entries(&mut self, max: usize);
}

/// Resolve and execute one command. This is the only `match` over
/// `AppCommand`; the native composition root calls it through its adapter.
pub fn execute<E: CommandEffects + ?Sized>(
    effects: &mut E,
    command: AppCommand,
) -> Result<CommandOutcome, CommandError> {
    match command {
        AppCommand::RefreshPorts => effects.refresh_ports(),
        AppCommand::RequestPort => effects.request_port(),
        AppCommand::RegisterNetworkPort { config } => {
            effects.register_network_port(config)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::RemoveNetworkPort { port } => {
            effects.remove_network_port(port)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::Connect { port, settings } => effects.connect(port, settings),
        AppCommand::SetSerialSettings { settings } => {
            effects.apply_serial_settings(settings);
            Ok(CommandOutcome::Done)
        }
        AppCommand::Disconnect { port } => effects.disconnect(port),
        AppCommand::Reconnect { port } => effects.reconnect(port),
        AppCommand::CancelReconnect { port } => effects.cancel_reconnect(port),
        AppCommand::CancelTask { task_id } => {
            if effects.cancel_task(task_id) {
                Ok(CommandOutcome::Done)
            } else {
                Err(CommandError::InvalidState(format!(
                    "后台任务 {task_id:?} 不存在或已经结束"
                )))
            }
        }
        command @ (AppCommand::SendText { .. }
        | AppCommand::SendHex { .. }
        | AppCommand::SendRaw { .. }) => execute_send(effects, command),
        AppCommand::SetDtr { port, value } => effects.set_signal(port, value, true),
        AppCommand::SetRts { port, value } => effects.set_signal(port, value, false),
        AppCommand::StartRecording { file, mode } => effects.start_recording(file, mode),
        AppCommand::SetRecordingMode { mode } => {
            effects.set_recording_mode(mode);
            Ok(CommandOutcome::Done)
        }
        AppCommand::StopRecording => effects.stop_recording(),
        AppCommand::PauseRecording => effects.pause_recording(),
        AppCommand::ResumeRecording => effects.resume_recording(),
        AppCommand::AddBookmark { name } => {
            effects.add_recording_bookmark(name);
            Ok(CommandOutcome::Done)
        }
        AppCommand::AddReplayBookmark { name } => {
            effects.replay_add_bookmark(name);
            Ok(CommandOutcome::Done)
        }
        AppCommand::LoadReplay { file } => effects.load_replay(file),
        AppCommand::ReplayPlay => {
            effects.replay_play()?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ReplayPause => {
            effects.replay_pause()?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ReplayStop => {
            effects.replay_stop()?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ReplaySeek { position_ms } => {
            effects.replay_seek(position_ms)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ReplaySeekBy { delta_ms } => {
            effects.replay_seek_by(delta_ms)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ReplayStep { delta } => {
            effects.replay_step(delta)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::SetReplaySpeed { speed } => {
            effects.replay_set_speed(speed);
            Ok(CommandOutcome::Done)
        }
        AppCommand::SetReplayPolicy { policy } => {
            effects.replay_set_policy(policy)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::RemoveReplayBookmark { position_ms } => {
            effects.replay_remove_bookmark(position_ms);
            Ok(CommandOutcome::Done)
        }
        AppCommand::EnablePlugin { plugin_id } => {
            effects.enable_plugin(plugin_id)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::DisablePlugin { plugin_id } => {
            effects.disable_plugin(plugin_id)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ReloadPlugins => effects.reload_plugins(),
        AppCommand::RefreshMarketplace { url } => effects.refresh_marketplace(url),
        AppCommand::CheckForUpdate => effects.check_for_update(),
        AppCommand::DiscoverPlugins { roots } => effects.discover_plugins(roots),
        AppCommand::ExecutePluginCommand {
            plugin_id,
            command_id,
            context,
        } => {
            effects.execute_plugin_command(plugin_id, command_id, context)?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::ClearTerminal => {
            effects.clear_terminal()?;
            Ok(CommandOutcome::Done)
        }
        AppCommand::SetTerminalMergeWindow { ms } => {
            effects.set_terminal_merge_window(ms);
            Ok(CommandOutcome::Done)
        }
        AppCommand::SetTerminalMaxEntries { max } => {
            effects.set_terminal_max_entries(max);
            Ok(CommandOutcome::Done)
        }
    }
}

fn execute_send<E: CommandEffects + ?Sized>(
    effects: &mut E,
    command: AppCommand,
) -> Result<CommandOutcome, CommandError> {
    let port = match &command {
        AppCommand::SendText { port, .. }
        | AppCommand::SendHex { port, .. }
        | AppCommand::SendRaw { port, .. } => port.clone(),
        _ => {
            return Err(CommandError::InvalidCommand(
                "该命令不是发送类命令".to_owned(),
            ));
        }
    };
    let is_network = effects.is_network_port(&port);
    let plan = plan_send(&command, is_network)
        .map_err(|error| CommandError::Transport(error.to_string()))?;
    effects.send_planned(port, plan)
}
