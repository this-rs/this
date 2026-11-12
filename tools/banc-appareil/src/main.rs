// Rien sur le bureau : cette app n'existe que sur un appareil.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    banc_appareil_lib::run()
}
