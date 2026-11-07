// Generates one `allow-<command>` permission per command of the plugin, and the Swift
// package wiring from `ios/`. No default permission is declared: see the note in `lib.rs`.
const COMMANDS: &[&str] = &["status", "gyroscope_sample", "bluetooth_scan"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
