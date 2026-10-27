use std::rc::Rc;

use crate::{PluginError, PluginHostApi, PluginPermissions, PluginResult, PluginValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct PluginInstanceId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginFunctionId(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct PluginLoadConfig {
    pub plugin_id: String,
    pub plugin_name: String,
    pub plugin_version: String,
    pub script_name: String,
    pub context: PluginValue,
    pub permissions: PluginPermissions,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginCallResult {
    Completed(PluginValue),
}

/// Engine boundary. No `mlua::Value` or VM-specific table/function handle may
/// cross this trait.
pub trait LuaEngine {
    fn load_plugin(
        &mut self,
        source: &str,
        config: PluginLoadConfig,
        host: Rc<dyn PluginHostApi>,
    ) -> PluginResult<PluginInstanceId>;

    fn call(
        &mut self,
        instance: PluginInstanceId,
        function: PluginFunctionId,
        args: &[PluginValue],
    ) -> PluginResult<PluginCallResult>;

    fn stop(&mut self, instance: PluginInstanceId) -> PluginResult<()>;

    fn dispatch_event(
        &mut self,
        _instance: PluginInstanceId,
        _event: PluginValue,
    ) -> PluginResult<()> {
        Err(PluginError::UnsupportedCapability(
            "bus.subscribe".to_owned(),
        ))
    }

    fn dispatch_command(
        &mut self,
        _instance: PluginInstanceId,
        _command: &str,
        _context: PluginValue,
    ) -> PluginResult<PluginCallResult> {
        Err(PluginError::UnsupportedCapability("commands".to_owned()))
    }

    fn update_settings(
        &mut self,
        _instance: PluginInstanceId,
        _settings: PluginValue,
    ) -> PluginResult<()> {
        Err(PluginError::UnsupportedCapability("config".to_owned()))
    }
}
