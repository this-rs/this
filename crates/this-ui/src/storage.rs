//! Persistance des réglages dans `localStorage`.
//!
//! `this-app` définit le trait [`this_app::Store`] ; c'est ici, et seulement ici, qu'on
//! décide que le navigateur/webview est le stockage.

use this_app::Store;

fn handle() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub struct LocalStorage;

impl Store for LocalStorage {
    fn read(&self, key: &str) -> Option<String> {
        handle()?.get_item(key).ok().flatten()
    }

    fn write(&mut self, key: &str, value: String) {
        if let Some(store) = handle() {
            let _ = store.set_item(key, &value);
        }
    }
}

/// L'horloge monotone de l'app, en millisecondes (pour `this_app::Boot`).
pub fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}
