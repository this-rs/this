//! A browser `Vfs`: paths, etags and preconditions over IndexedDB.
//!
//! This is what lets a page **store** what it just compiled. Without it the lab
//! compiles a module and throws it away; with it, the artifact lands somewhere a
//! harness can find again, and a reload does not erase it.
//!
//! ## Why IndexedDB and not localStorage
//!
//! Measured (Chrome, 05/11/2026): `localStorage` tops out around **4.9 MB** and
//! refuses a 5.4 MB artifact once base64 has inflated it by a third. `IndexedDB`
//! stores the same artifact byte for byte, with a declared quota in the gigabytes.
//! `localStorage` stays fine for settings; it is not a place for a binary.
//!
//! ## Why every handle is a `JsValue`
//!
//! `Vfs` asks for `Send` futures (`#[async_trait]` on a `Send + Sync` trait). The
//! `web-sys` types — `IdbDatabase`, `IdbTransaction`, `IdbObjectStore`,
//! `IdbRequest` — are `!Send`, so holding one across an `await` makes the future
//! `!Send` and the impl is rejected. `JsValue` *is* `Send` on `wasm32`.
//!
//! So this backend never names those types: it calls the DOM through
//! `Reflect`, and every value that lives across an `await` is a `JsValue`. The
//! alternative — relaxing the trait to `?Send` — would break axum handlers on the
//! server, which is a regression traded for a client's convenience.
//!
//! ## What it honours
//!
//! The etag is `etag_of(content)` — the same function the server uses — and the
//! precondition is checked against the current state **before** writing. A backend
//! that skips either produces a store a server would reject, or worse, silently
//! overwrites what it was told not to.

use async_trait::async_trait;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use this_vfs::{
    Precondition, Vfs, VfsCaps, VfsEntry, VfsError, VfsMeta, VfsPath, check_precondition, etag_of,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const STORE: &str = "files";

/// A `Vfs` stored in the browser, keyed by path.
pub struct IndexedDbVfs {
    db: JsValue,
}

impl IndexedDbVfs {
    /// Open (or create) the database `name`. The object store is created on the
    /// first open; later opens reuse it, so a reload finds what was written.
    pub async fn open(name: &str) -> Result<Self, VfsError> {
        let factory =
            Reflect::get(&js_sys::global(), &JsValue::from_str("indexedDB")).map_err(backend)?;
        if factory.is_undefined() || factory.is_null() {
            return Err(backend("IndexedDB indisponible (navigation privée ?)"));
        }

        let open = call(&factory, "open", &[JsValue::from_str(name)])?;

        // Créé à la première ouverture seulement : `onupgradeneeded` ne se
        // déclenche que sur un changement de version.
        let upgrade = {
            let request = open.clone();
            Closure::once_into_js(move || {
                if let Ok(db) = Reflect::get(&request, &JsValue::from_str("result")) {
                    let _ = call(&db, "createObjectStore", &[JsValue::from_str(STORE)]);
                }
            })
        };
        Reflect::set(&open, &JsValue::from_str("onupgradeneeded"), &upgrade).map_err(backend)?;

        Ok(Self {
            db: await_request(&open).await?,
        })
    }

    fn store(&self, mode: &str) -> Result<JsValue, VfsError> {
        let transaction = call(
            &self.db,
            "transaction",
            &[JsValue::from_str(STORE), JsValue::from_str(mode)],
        )?;
        call(&transaction, "objectStore", &[JsValue::from_str(STORE)])
    }

    async fn get_record(&self, path: &VfsPath) -> Result<Option<(Vec<u8>, f64)>, VfsError> {
        let store = self.store("readonly")?;
        let request = call(&store, "get", &[JsValue::from_str(path.as_str())])?;
        let value = await_request(&request).await?;
        if value.is_undefined() || value.is_null() {
            return Ok(None);
        }
        let bytes = Reflect::get(&value, &JsValue::from_str("b")).map_err(backend)?;
        let bytes: Uint8Array = bytes
            .dyn_into()
            .map_err(|_| backend("enregistrement corrompu : `b` absent"))?;
        let modified = Reflect::get(&value, &JsValue::from_str("m"))
            .map_err(backend)?
            .as_f64()
            .unwrap_or(0.0);
        Ok(Some((bytes.to_vec(), modified)))
    }

    async fn put_record(
        &self,
        path: &VfsPath,
        bytes: &[u8],
        modified: f64,
    ) -> Result<(), VfsError> {
        let store = self.store("readwrite")?;
        let record = Object::new();
        Reflect::set(&record, &JsValue::from_str("b"), &Uint8Array::from(bytes))
            .map_err(backend)?;
        Reflect::set(
            &record,
            &JsValue::from_str("m"),
            &JsValue::from_f64(modified),
        )
        .map_err(backend)?;
        let request = call(
            &store,
            "put",
            &[record.into(), JsValue::from_str(path.as_str())],
        )?;
        await_request(&request).await?;
        Ok(())
    }
}

// Même vérité par cible que la définition du trait : sur wasm, les futures
// d'un backend navigateur ne sont pas `Send`.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Vfs for IndexedDbVfs {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        match self.get_record(path).await? {
            Some((bytes, _)) => Ok(bytes),
            None => Err(VfsError::NotFound(path.clone())),
        }
    }

    async fn write(
        &self,
        path: &VfsPath,
        bytes: &[u8],
        pre: Precondition,
    ) -> Result<VfsMeta, VfsError> {
        // La précondition se juge sur l'état courant, AVANT d'écrire : un
        // `IfMatch` périmé doit échouer, pas écraser en silence.
        let current = self.get_record(path).await?;
        let current_etag = current.as_ref().map(|(existing, _)| etag_of(existing));
        check_precondition(path, current_etag.as_deref(), &pre)?;

        let modified = js_sys::Date::now();
        self.put_record(path, bytes, modified).await?;

        Ok(VfsMeta {
            size: bytes.len() as u64,
            etag: etag_of(bytes),
            modified: to_datetime(modified),
        })
    }

    async fn stat(&self, path: &VfsPath) -> Result<Option<VfsMeta>, VfsError> {
        Ok(self
            .get_record(path)
            .await?
            .map(|(bytes, modified)| VfsMeta {
                size: bytes.len() as u64,
                etag: etag_of(&bytes),
                modified: to_datetime(modified),
            }))
    }

    async fn list(&self, dir: &VfsPath) -> Result<Vec<VfsEntry>, VfsError> {
        let store = self.store("readonly")?;
        let request = call(&store, "getAllKeys", &[])?;
        let keys = await_request(&request).await?;
        let keys: Array = keys.dyn_into().map_err(|_| backend("clés illisibles"))?;

        let prefix = format!("{}/", dir.as_str().trim_end_matches('/'));
        let mut files: Vec<String> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();

        for key in keys.iter() {
            let Some(key) = key.as_string() else { continue };
            let Some(rest) = key.strip_prefix(&prefix) else {
                continue;
            };
            match rest.split_once('/') {
                None => files.push(key),
                Some((child, _)) => {
                    let child_path = format!("{prefix}{child}");
                    if !dirs.contains(&child_path) {
                        dirs.push(child_path);
                    }
                }
            }
        }

        dirs.sort();
        let mut out: Vec<VfsEntry> = dirs
            .into_iter()
            .filter_map(|p| VfsPath::parse(&p).ok().map(|path| VfsEntry::Dir { path }))
            .collect();

        files.sort();
        for key in files {
            let Ok(path) = VfsPath::parse(&key) else {
                continue;
            };
            if let Some(meta) = self.stat(&path).await? {
                out.push(VfsEntry::File { path, meta });
            }
        }
        Ok(out)
    }

    async fn delete(&self, path: &VfsPath, pre: Precondition) -> Result<(), VfsError> {
        let current = self.get_record(path).await?;
        let current_etag = current.as_ref().map(|(existing, _)| etag_of(existing));
        check_precondition(path, current_etag.as_deref(), &pre)?;
        if current.is_none() {
            return Err(VfsError::NotFound(path.clone()));
        }
        let store = self.store("readwrite")?;
        let request = call(&store, "delete", &[JsValue::from_str(path.as_str())])?;
        await_request(&request).await?;
        Ok(())
    }

    fn caps(&self) -> VfsCaps {
        VfsCaps {
            read_only: false,
            versioned: false,
        }
    }
}

fn to_datetime(ms: f64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(ms as i64).unwrap_or_else(chrono::Utc::now)
}

fn backend(error: impl std::fmt::Debug) -> VfsError {
    VfsError::Backend(anyhow::anyhow!("{error:?}"))
}

/// Appelle une méthode de l'objet JS et rend son résultat brut.
fn call(target: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, VfsError> {
    let function = Reflect::get(target, &JsValue::from_str(name)).map_err(backend)?;
    let function: Function = function
        .dyn_into()
        .map_err(|_| backend(format!("`{name}` n'est pas une fonction")))?;
    let arguments = Array::new();
    for argument in args {
        arguments.push(argument);
    }
    function.apply(target, &arguments).map_err(backend)
}

/// Attend une requête IndexedDB en la transformant en `Promise`.
///
/// Seul le `JsValue` de la promesse traverse l'`await` : la requête elle-même
/// reste dans la fermeture, côté JS.
async fn await_request(request: &JsValue) -> Result<JsValue, VfsError> {
    let request = request.clone();
    let promise = Promise::new(&mut |resolve, reject| {
        let on_success = {
            let resolve = resolve.clone();
            let request = request.clone();
            Closure::once_into_js(move || {
                let value =
                    Reflect::get(&request, &JsValue::from_str("result")).unwrap_or(JsValue::NULL);
                let _ = resolve.call1(&JsValue::NULL, &value);
            })
        };
        let on_error = {
            let reject = reject.clone();
            let request = request.clone();
            Closure::once_into_js(move || {
                let error =
                    Reflect::get(&request, &JsValue::from_str("error")).unwrap_or(JsValue::NULL);
                let _ = reject.call1(&JsValue::NULL, &error);
            })
        };
        let _ = Reflect::set(&request, &JsValue::from_str("onsuccess"), &on_success);
        let _ = Reflect::set(&request, &JsValue::from_str("onerror"), &on_error);
    });
    JsFuture::from(promise).await.map_err(backend)
}
