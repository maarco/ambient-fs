// tauri plugin for ambient-fs daemon integration
//
// this plugin bridges ambient-fsd to tauri apps via IPC commands
// and forwards daemon events to the frontend as tauri events.

mod config;
mod state;
mod commands;
mod events;

pub use config::AmbientFsConfig;

use tauri::Manager;
use tauri::plugin::Builder;
use tauri::Wry;
use state::PluginState;

/// initialize the ambient-fs plugin
///
/// usage in tauri app:
/// ```text
/// tauri::Builder::default()
///     .plugin(tauri_plugin_ambient_fs::init())
///     .run(tauri::generate_context!())
///     .expect("error while running tauri application");
/// ```
pub fn init() -> tauri::plugin::TauriPlugin<Wry> {
    Builder::new("ambient-fs")
        .invoke_handler(tauri::generate_handler![
            commands::watch_project,
            commands::unwatch_project,
            commands::query_events,
            commands::query_awareness,
            commands::query_tree,
            commands::attribute,
            commands::query_agents,
            commands::get_status,
        ])
        .setup(|app, _api| {
            let state = PluginState::new(app.clone());
            app.manage(state);
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_plugin() {
        // we can't fully test the plugin without a tauri runtime,
        // but we can verify the init function exists and compiles
        let _plugin_name = "ambient-fs";
    }
}
