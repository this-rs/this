#![allow(dead_code)]
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use this_forge::{Forge, ForgeConfig, StoreKind};
use tower::ServiceExt;

pub struct Harness {
    pub forge: Arc<Forge>,
    pub app: Router,
    pub _dir: tempfile::TempDir,
}

pub async fn harness_with(f: impl FnOnce(&mut ForgeConfig)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ForgeConfig::new(dir.path());
    cfg.store = StoreKind::Memory;
    f(&mut cfg);
    let forge = Forge::start(cfg).await.unwrap();
    let app = this_forge::app(forge.clone()).unwrap();
    Harness {
        forge,
        app,
        _dir: dir,
    }
}

pub async fn harness() -> Harness {
    harness_with(|_| {}).await
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub bytes: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }
    pub fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
}

impl Harness {
    pub async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        Reply {
            status,
            headers,
            bytes,
        }
    }

    /// Crée une invitation et inscrit un compte ; rend `(token, refresh, account_id)`.
    pub async fn signup(&self, email: &str) -> (String, String, String) {
        let code = self.forge.create_invitation(1, None).await.unwrap();
        let r = self
            .call("POST", "/v1/auth/signup", None, Some(json!({"invitation": code, "email": email, "password": "correct horse battery"})))
            .await;
        assert_eq!(
            r.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&r.bytes)
        );
        let j = r.json();
        (
            j["token"].as_str().unwrap().into(),
            j["refresh"].as_str().unwrap().into(),
            j["account"]["id"].as_str().unwrap().into(),
        )
    }

    pub async fn submit(&self, token: &str, name: &str, files: Value) -> Reply {
        self.call(
            "POST",
            "/v1/builds",
            Some(token),
            Some(json!({"name": name, "version": "1.0.0", "files": files})),
        )
        .await
    }

    /// Attend l'état terminal d'un build ; rend sa vue JSON.
    pub async fn wait_build(&self, token: &str, id: &str) -> Value {
        for _ in 0..800 {
            let j = self
                .call("GET", &format!("/v1/builds/{id}"), Some(token), None)
                .await
                .json();
            if matches!(j["status"].as_str(), Some("signed" | "failed")) {
                return j;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        panic!("build {id} jamais terminé");
    }
}

/// Un module conforme à l'ABI v1, en Rust pur (utilise serde_json : dépendance de la liste blanche).
pub const GOOD_LIB: &str = r#"
use std::alloc::{Layout, alloc as raw_alloc};

#[unsafe(no_mangle)]
pub extern "C" fn alloc(n: i32) -> i32 {
    unsafe { raw_alloc(Layout::from_size_align(n.max(1) as usize, 8).unwrap()) as i32 }
}

fn pack(s: String) -> i64 {
    let b = s.into_bytes().leak();
    ((b.as_ptr() as u32 as i64) << 32) | b.len() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn manifest() -> i64 {
    pack(serde_json::json!({"name": "demo", "version": "1.0.0", "entities": []}).to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn handle(_op_ptr: i32, _op_len: i32, _req_ptr: i32, _req_len: i32) -> i64 {
    pack(serde_json::json!({"ok": true, "output": {}, "effects": []}).to_string())
}
"#;

pub fn good_files() -> Value {
    json!([{"path": "src/lib.rs", "content": GOOD_LIB}])
}
