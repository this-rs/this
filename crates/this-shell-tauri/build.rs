// Generates one `allow-<command>` permission per command of the plugin.
const COMMANDS: &[&str] = &["env", "quit"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
