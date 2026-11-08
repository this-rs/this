//! Tests de l'API qui n'ont pas besoin de docker (le runner y est volontairement cassé :
//! un build soumis échoue vite, ce qui suffit pour les quotas et l'isolement).
mod common;

use axum::http::StatusCode;
use chrono::Utc;
use common::*;
use serde_json::json;
use this_forge::entities::Build;
use this_forge::sign;

#[tokio::test]
async fn signup_with_valid_invitation_gives_a_token_that_works() {
    let h = harness().await;
    let (token, refresh, id) = h.signup("Alice@Example.com").await;
    assert!(!refresh.is_empty());
    let me = h.call("GET", "/v1/me", Some(&token), None).await;
    assert_eq!(me.status, StatusCode::OK);
    let j = me.json();
    assert_eq!(j["account"]["id"], id);
    assert_eq!(j["account"]["email"], "alice@example.com");
    assert_eq!(j["quota"]["builds_today"], 0);
    assert_eq!(j["quota"]["builds_limit"], 20);
}

#[tokio::test]
async fn signup_with_invalid_code_is_refused_readably() {
    let h = harness().await;
    let r = h
        .call("POST", "/v1/auth/signup", None, Some(json!({"invitation": "forge-nope", "email": "a@b.co", "password": "correct horse battery"})))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "invalid_invitation");
    assert!(
        r.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invitation")
    );
}

#[tokio::test]
async fn invitation_is_single_use_and_email_unique_and_password_checked() {
    let h = harness().await;
    let code = h.forge.create_invitation(1, None).await.unwrap();
    let ok = |email: &str, pw: &str, code: &str| json!({"invitation": code, "email": email, "password": pw});
    let weak = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(ok("a@b.co", "short", &code)),
        )
        .await;
    assert_eq!(
        (weak.status, weak.code().as_str()),
        (StatusCode::BAD_REQUEST, "weak_password")
    );
    let first = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(ok("a@b.co", "correct horse battery", &code)),
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let again = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(ok("c@d.co", "correct horse battery", &code)),
        )
        .await;
    assert_eq!(
        again.code(),
        "invalid_invitation",
        "un code à usage unique ne sert qu'une fois"
    );
    let code2 = h.forge.create_invitation(1, None).await.unwrap();
    let dup = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(ok("A@B.co", "correct horse battery", &code2)),
        )
        .await;
    assert_eq!(
        (dup.status, dup.code().as_str()),
        (StatusCode::CONFLICT, "email_taken")
    );
}

#[tokio::test]
async fn login_refresh_logout() {
    let h = harness().await;
    let (_t, refresh, _id) = h.signup("a@b.co").await;
    let bad = h
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(json!({"email": "a@b.co", "password": "wrong password!"})),
        )
        .await;
    assert_eq!(
        (bad.status, bad.code().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    let unknown = h
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(json!({"email": "x@y.co", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    let login = h
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(json!({"email": "a@b.co", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(login.status, StatusCode::OK);
    let token = login.json()["token"].as_str().unwrap().to_string();

    // refresh : rotation, l'ancien jeton ne resservira pas
    let r1 = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": refresh})),
        )
        .await;
    assert_eq!(r1.status, StatusCode::OK);
    let new_refresh = r1.json()["refresh"].as_str().unwrap().to_string();
    let replay = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": refresh})),
        )
        .await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);

    // logout : 204 et le jeton de rafraîchissement est révoqué
    let out = h.call("POST", "/v1/auth/logout", Some(&token), None).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    let after = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": new_refresh})),
        )
        .await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn protected_routes_refuse_missing_or_forged_tokens() {
    let h = harness().await;
    assert_eq!(
        h.call("GET", "/v1/me", None, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call("GET", "/v1/me", Some("not.a.jwt"), None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    // un jeton signé par UNE AUTRE forge est refusé
    let other = harness().await;
    let (t, _, _) = other.signup("a@b.co").await;
    assert_eq!(
        h.call("GET", "/v1/me", Some(&t), None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn forge_key_is_public_and_fingerprinted() {
    let h = harness().await;
    let r = h.call("GET", "/v1/forge/key", None, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert_eq!(j["algorithm"], "ed25519");
    let pk = j["public_key"].as_str().unwrap();
    assert_eq!(pk.len(), 64);
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&pk[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    assert_eq!(
        j["fingerprint"],
        format!("sha256:{}", sign::sha256_hex(&bytes))
    );
}

#[tokio::test]
async fn signing_key_file_is_0600_and_stable_across_restarts() {
    let h = harness().await;
    let path = h.forge.cfg.signing_key_path.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let again = sign::ForgeKeys::load_or_create(&path).unwrap();
    assert_eq!(again.fingerprint(), h.forge.keys.fingerprint());
}

/// Critère d'acceptation : A ne voit NI les builds NI l'artefact de B.
#[tokio::test]
async fn account_a_cannot_see_builds_or_artifact_of_account_b() {
    let h = harness().await;
    let (ta, _, _) = h.signup("a@b.co").await;
    let (tb, _, idb) = h.signup("b@b.co").await;
    // un build SIGNÉ de B, posé directement (l'isolement ne dépend pas de docker)
    let mut b = Build::new(
        "secret".into(),
        "signed".into(),
        idb,
        "1.0.0".into(),
        "rustc 1.93.1".into(),
        "s".into(),
        None,
        None,
        0,
        "[]".into(),
        Some("h".into()),
        Some(3),
        Some("sig".into()),
        Some("m".into()),
    );
    b.finished_at = Some(Utc::now().to_rfc3339());
    let b = h.forge.store.builds.create(b).await.unwrap();
    std::fs::write(h.forge.artifact_path(&b.id), b"\0asm\x01\0\0\0").unwrap();
    let id = b.id.to_string();

    for path in [
        format!("/v1/builds/{id}"),
        format!("/v1/builds/{id}/artifact"),
        format!("/v1/builds/{id}/events"),
    ] {
        let a = h.call("GET", &path, Some(&ta), None).await;
        eprintln!("A  GET {path} -> {} {}", a.status, a.code());
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(a.code(), "not_found");
        assert!(!String::from_utf8_lossy(&a.bytes).contains("secret"));
    }
    // B, lui, voit son build et son artefact
    let own = h
        .call("GET", &format!("/v1/builds/{id}"), Some(&tb), None)
        .await;
    eprintln!("B  GET /v1/builds/{{id}} -> {}", own.status);
    assert_eq!(own.status, StatusCode::OK);
    let art = h
        .call("GET", &format!("/v1/builds/{id}/artifact"), Some(&tb), None)
        .await;
    assert_eq!(art.status, StatusCode::OK);
    assert_eq!(art.headers["content-type"], "application/wasm");
    assert_eq!(art.headers["x-forge-signature"], "sig");
    // et le /me de A ne compte pas le build de B
    assert_eq!(
        h.call("GET", "/v1/me", Some(&ta), None).await.json()["quota"]["builds_today"],
        0
    );
}

#[tokio::test]
async fn quota_exceeded_is_refused_with_a_readable_code() {
    let h = harness_with(|c| {
        c.builds_per_day = 1;
        c.docker_bin = "/nonexistent/docker".into();
    })
    .await;
    let (t, _, _) = h.signup("a@b.co").await;
    let first = h.submit(&t, "demo", good_files()).await;
    assert_eq!(
        first.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&first.bytes)
    );
    let second = h.submit(&t, "demo", good_files()).await;
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.code(), "quota_builds_exceeded");
    assert!(
        second.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("quota")
    );
    let me = h.call("GET", "/v1/me", Some(&t), None).await.json();
    assert_eq!(me["quota"]["builds_today"], 1);
}

#[tokio::test]
async fn submission_validation_refuses_what_would_escape_the_template() {
    let h = harness_with(|c| c.docker_bin = "/nonexistent/docker".into()).await;
    let (t, _, _) = h.signup("a@b.co").await;
    for (files, why) in [
        (
            json!([{"path": "src/lib.rs", "content": ""}, {"path": "build.rs", "content": "fn main(){}"}]),
            "build.rs",
        ),
        (
            json!([{"path": "src/lib.rs", "content": ""}, {"path": "Cargo.toml", "content": "[package]"}]),
            "Cargo.toml",
        ),
        (
            json!([{"path": "src/lib.rs", "content": ""}, {"path": "src/../../x.rs", "content": ""}]),
            "traversal",
        ),
        (
            json!([{"path": "/etc/passwd.rs", "content": ""}]),
            "absolute",
        ),
        (json!([{"path": "src/main.rs", "content": ""}]), "no lib.rs"),
        (json!([]), "empty"),
    ] {
        let r = h.submit(&t, "demo", files).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (StatusCode::BAD_REQUEST, "invalid_files"),
            "{why}"
        );
    }
    let r = h
        .call(
            "POST",
            "/v1/builds",
            Some(&t),
            Some(json!({"name": "Bad Name", "version": "1.0.0", "files": good_files()})),
        )
        .await;
    assert_eq!(r.code(), "invalid_name");
    let r = h
        .call(
            "POST",
            "/v1/builds",
            Some(&t),
            Some(json!({"name": "ok", "version": "one", "files": good_files()})),
        )
        .await;
    assert_eq!(r.code(), "invalid_version");
}

#[tokio::test]
async fn forge_descriptor_declares_the_entities_and_is_coherent() {
    let set = this_forge::entities::forge_descriptor_set().unwrap();
    let _ = set;
    let d = this_forge::entities::forge_descriptor();
    let names: Vec<&str> = d.entities.iter().map(|e| e.name.as_str()).collect();
    for n in ["account", "invitation", "session", "build"] {
        assert!(names.contains(&n), "entité {n} absente du descripteur");
    }
    assert_eq!(names.len(), 4);
}

#[test]
fn canonical_json_sorts_keys_and_is_compact() {
    let v = json!({"b": 1, "a": {"z": [1, {"y": 2, "x": 3}], "c": "é\"x"}});
    assert_eq!(
        sign::canonical_json(&v),
        r#"{"a":{"c":"é\"x","z":[1,{"x":3,"y":2}]},"b":1}"#
    );
}

#[test]
fn abi_check_refuses_imports_and_missing_exports() {
    // module avec un import `env.f` et AUCUN export
    let wasm: Vec<u8> = vec![
        0x00, 0x61, 0x73, 0x6d, 0x01, 0, 0, 0, // en-tête
        0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type () -> ()
        0x02, 0x09, 0x01, 0x03, b'e', b'n', b'v', 0x01, b'f', 0x00, 0x00, // import env.f
    ];
    let err = this_forge::wasm_check::check_abi(&wasm).unwrap_err();
    let all: String = err
        .iter()
        .map(|v| v.0.clone())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(all.contains("imports") && all.contains("env::f"), "{all}");
    assert!(
        all.contains("`memory` manquant") && all.contains("`handle` manquant"),
        "{all}"
    );
    assert!(this_forge::wasm_check::check_abi(b"not wasm").is_err());
}

#[test]
fn cargo_json_is_parsed_into_structured_diagnostics() {
    let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":9,"is_primary":true}]}}"#;
    let summary = r#"{"reason":"compiler-message","message":{"level":"error","message":"aborting due to 1 previous error","spans":[]}}"#;
    let d = this_forge::runner::parse_cargo_json(&format!("{line}\nnoise\n{summary}\n"));
    assert_eq!(d.len(), 1);
    assert_eq!(
        (
            d[0].file.as_str(),
            d[0].line,
            d[0].column,
            d[0].level.as_str()
        ),
        ("src/lib.rs", 3, 9, "error")
    );
    assert_eq!(d[0].message, "E0308: mismatched types");
}
