//! # `tauri-plugin-this-device` — raw device primitives for iOS
//!
//! Three native primitives, answering with the wire contract of
//! [`this_platform::device_bridge`]:
//!
//! - `status(capability)` — the permission state (`granted`, `denied`, `prompt`,
//!   `unsupported`), read without prompting the person;
//! - `gyroscope_sample()` — one gyroscope reading, or a refusal after about 2 s;
//! - `bluetooth_scan(duration_ms, services)` — a one-shot scan, the devices seen, once each;
//! - [`answer`] with a `Request::Call` — any of the thirteen device families (contacts,
//!   photos, files, calendar, health, Face ID, speech, NFC, local network, motion, location,
//!   notifications, live activity, clipboard, share), each with its own typed wire shape in
//!   [`families`].
//!
//! ## Security: these are raw primitives, not an API for modules
//!
//! The commands are **not** a permission and must never be reachable by a module directly.
//! The access decision belongs to the device broker, [`this_platform::device_bridge::gate`]:
//! it asks the person's consent before a request is forwarded to [`perform`]. That wiring is
//! not done yet, so until it is:
//!
//! - this plugin declares **no default permission** (no `default.toml`): a capability file
//!   must grant `this-device:allow-*` explicitly to expose a command to a webview. Do not add
//!   such a grant, nor `default`, without the broker in front of it;
//! - a Rust caller must go through `gate` first; [`perform`] does not check consent itself.
//!
//! ## Use
//!
//! ```ignore
//! tauri::Builder::default().plugin(tauri_plugin_this_device::init())
//! ```
//!
//! Only iOS talks to the device. On every other target, each request is answered with
//! `Unsupported` (`status`) or `not_supported` (the others), without touching anything.

use std::marker::PhantomData;

use serde_json::{Value, json};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime};
use this_platform::capability::Capability;
#[cfg(not(target_os = "ios"))]
use this_platform::device_bridge::PermissionState;
use this_platform::device_bridge::{DeviceError, Reply, Request};

pub mod consent;
pub mod families;
pub mod wire;

#[cfg(target_os = "ios")]
use tauri::plugin::PluginHandle;

// The Swift entry point, defined in `ios/Sources/ThisDevicePlugin.swift`.
#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_this_device);

/// The plugin's state: on iOS, the handle to the Swift side.
pub struct ThisDevice<R: Runtime> {
    #[cfg(target_os = "ios")]
    handle: PluginHandle<R>,
    _runtime: PhantomData<fn() -> R>,
}

/// Build the plugin. Add it once, before the app runs. It declares no permission: see the
/// module documentation.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("this-device")
        .invoke_handler(tauri::generate_handler![
            status,
            gyroscope_sample,
            bluetooth_scan
        ])
        .setup(|app, api| {
            #[cfg(not(target_os = "ios"))]
            let _ = &api;
            #[cfg(target_os = "ios")]
            let handle = api.register_ios_plugin(init_plugin_this_device)?;
            app.manage(ThisDevice::<R> {
                #[cfg(target_os = "ios")]
                handle,
                _runtime: PhantomData,
            });
            Ok(())
        })
        .build()
}

/// Answer one request, as JSON.
///
/// This is the single entry point of the bridge, and the only one a shell should call. It is
/// **not** gated: the caller must check consent first with
/// `this_platform::device_bridge::gate` and only call this when the gate returned `Ok`.
///
/// A family answers in its own shape (`{"kind":"contacts",...}`), so the answer is a
/// [`Value`]; the caller decodes it with the family's own type. The three pilot primitives
/// are still checked against [`Reply`] on the way back, because their shape is fixed.
pub async fn answer<R: Runtime>(app: &AppHandle<R>, request: &Request) -> Value {
    match request {
        Request::Call { method, args } => call_native(app, method, args).await,
        other => match serde_json::to_value(perform(app, other).await) {
            Ok(value) => value,
            // A `Reply` always serializes: this arm keeps the signature honest without a panic.
            Err(_) => json!({ "kind": "error", "code": "internal" }),
        },
    }
}

/// One call of a device family. The method is checked against the contract's table before it
/// is sent, so a typo is a refusal here rather than an unknown selector on the native side.
#[cfg(target_os = "ios")]
async fn call_native<R: Runtime>(app: &AppHandle<R>, method: &str, args: &Value) -> Value {
    if this_platform::device_bridge::capability_of(method, args).is_none() {
        return json!({ "kind": "error", "code": "not_supported" });
    }
    let plugin = app.state::<ThisDevice<R>>();
    plugin
        .handle
        .run_mobile_plugin_async::<Value>(method, args.clone())
        .await
        .unwrap_or_else(|_| json!({ "kind": "error", "code": "internal" }))
}

/// Off iOS there is no device: a family call is refused as unsupported.
#[cfg(not(target_os = "ios"))]
async fn call_native<R: Runtime>(_app: &AppHandle<R>, _method: &str, _args: &Value) -> Value {
    json!({ "kind": "error", "code": "not_supported" })
}

/// Answer one request, natively, in the shape of the pilot's [`Reply`]. Not gated: see
/// [`answer`].
#[cfg(target_os = "ios")]
pub async fn perform<R: Runtime>(app: &AppHandle<R>, request: &Request) -> Reply {
    let plugin = app.state::<ThisDevice<R>>();
    let native = match request {
        Request::Status { capability } => {
            plugin
                .handle
                .run_mobile_plugin_async::<Reply>("status", json!({ "capability": capability }))
                .await
        }
        Request::GyroscopeSample => {
            plugin
                .handle
                .run_mobile_plugin_async::<Reply>("gyroscopeSample", json!({}))
                .await
        }
        Request::BluetoothScan {
            duration_ms,
            services,
        } => {
            let (duration_ms, services) = match wire::scan_args(*duration_ms, services) {
                Ok(args) => args,
                Err(refusal) => return refusal,
            };
            plugin
                .handle
                .run_mobile_plugin_async::<Reply>(
                    "bluetoothScan",
                    json!({ "duration_ms": duration_ms, "services": services }),
                )
                .await
        }
        // Reached only if a caller bypasses `answer`. A family answer is not a `Reply`, so
        // answering it here would be a lie: it is refused instead.
        Request::Call { .. } => {
            return Reply::Error {
                code: DeviceError::NotSupported,
            };
        }
    };
    // A failed call into the plugin is not a device answer: it is an internal error.
    native.unwrap_or(Reply::Error {
        code: DeviceError::Internal,
    })
}

/// Answer one request. Off iOS there is no device: every request is refused as unsupported.
#[cfg(not(target_os = "ios"))]
pub async fn perform<R: Runtime>(_app: &AppHandle<R>, request: &Request) -> Reply {
    match request {
        Request::Status { .. } => Reply::Status {
            state: PermissionState::Unsupported,
        },
        Request::GyroscopeSample | Request::BluetoothScan { .. } | Request::Call { .. } => {
            Reply::Error {
                code: DeviceError::NotSupported,
            }
        }
    }
}

/// The permission state of a capability. Raw: see the module documentation.
#[tauri::command]
async fn status<R: Runtime>(app: AppHandle<R>, capability: Capability) -> Reply {
    perform(&app, &Request::Status { capability }).await
}

/// One gyroscope sample. Raw: see the module documentation.
#[tauri::command]
async fn gyroscope_sample<R: Runtime>(app: AppHandle<R>) -> Reply {
    perform(&app, &Request::GyroscopeSample).await
}

/// A one-shot Bluetooth scan. Raw: see the module documentation.
#[tauri::command]
async fn bluetooth_scan<R: Runtime>(
    app: AppHandle<R>,
    duration_ms: u32,
    services: Vec<String>,
) -> Reply {
    perform(
        &app,
        &Request::BluetoothScan {
            duration_ms,
            services,
        },
    )
    .await
}
