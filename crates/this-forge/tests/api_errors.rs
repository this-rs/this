//! Les REFUS de l'API : chaque test affirme le code HTTP **et** un bout du corps
//! (le code stable `error.code` ou le message), jamais seulement « ça répond ».
//!
//! Rien ici n'a besoin de docker ni du réseau : le runner est volontairement cassé
//! (`docker_bin` inexistant) et les lignes d'état sont posées directement dans le store,
//! ce qui rend les chemins d'erreur déterministes.
mod common;

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use chrono::Utc;
use common::*;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use this_forge::auth::{hash_password, issue_token, now_secs};
use this_forge::entities::{Account, Build, Session};
use this_forge::runner::Diagnostic;
use this_forge::sign::sha256_hex;
use tower::ServiceExt;
use uuid::Uuid;

// ── aides ───────────────────────────────────────────────────────────────────

/// Une ligne de build posée directement dans le store (aucun conteneur en jeu).
fn build_row(account_id: &str, name: &str, status: &str) -> Build {
    let mut b = Build::new(
        name.into(),
        status.into(),
        account_id.into(),
        "1.0.0".into(),
        "rustc unknown".into(),
        "source-digest".into(),
        None,
        None,
        0,
        "[]".into(),
        None,
        None,
        None,
        None,
    );
    b.created_at = Utc::now();
    b
}

fn signed_row(account_id: &str, name: &str) -> Build {
    let mut b = build_row(account_id, name, "signed");
    b.finished_at = Some(Utc::now().to_rfc3339());
    b.wasm_sha256 = Some("sha-du-wasm".into());
    b.wasm_size = Some(8);
    b.signature = Some("signature-ed25519".into());
    b.manifest_b64 = Some("manifeste-b64".into());
    b
}

fn diagnostics_json(message: &str) -> String {
    serde_json::to_string(&[Diagnostic {
        file: "src/lib.rs".into(),
        line: 7,
        column: 3,
        level: "error".into(),
        message: message.into(),
    }])
    .unwrap()
}

/// Envoie une requête brute (en-têtes libres) et rend le corps complet.
async fn send(h: &Harness, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = h.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, bytes)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn bearer(uri: &str, value: HeaderValue) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", value)
        .body(Body::empty())
        .unwrap()
}

/// Lit un flux SSE jusqu'à sa fin (ou jusqu'au délai), et rend le texte reçu.
async fn sse_text(h: &Harness, token: &str, uri: &str, limit: Duration) -> String {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"),
        "content-type SSE attendu, vu {:?}",
        res.headers()["content-type"]
    );
    let mut body = res.into_body();
    let mut out = String::new();
    // on s'arrête à la fin du flux, à une trame en erreur, ou au délai : dans les trois
    // cas le flux est considéré terminé et le texte reçu fait foi.
    while let Ok(Some(Ok(f))) = tokio::time::timeout(limit, body.frame()).await {
        if let Some(d) = f.data_ref() {
            out.push_str(&text(d));
        }
    }
    out
}

/// Un champ du corps d'erreur : `message` par exemple.
fn message(r: &Reply) -> String {
    r.json()["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

// ── inscription ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn signup_refuses_malformed_emails_before_anything_else() {
    let h = harness().await;
    let code = h.forge.create_invitation(10, None).await.unwrap();
    // le mot de passe est valide : seul l'e-mail doit décider.
    for bad in [
        "sans-arobase",
        "a@b@c.co",
        "@b.co",
        "a@",
        "a b@c.co",
        "a@b .co",
        "",
    ] {
        let r = h
            .call(
                "POST",
                "/v1/auth/signup",
                None,
                Some(
                    json!({"invitation": code, "email": bad, "password": "correct horse battery"}),
                ),
            )
            .await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (StatusCode::BAD_REQUEST, "invalid_email"),
            "e-mail {bad:?} accepté"
        );
        assert_eq!(message(&r), "adresse e-mail invalide");
    }
    // 255 caractères : au-delà de la limite RFC (254), refusé pour la même raison.
    let long = format!("{}@b.co", "a".repeat(250));
    let r = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(json!({"invitation": code, "email": long, "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::BAD_REQUEST, "invalid_email")
    );
    // et l'e-mail retenu est bien normalisé (espaces autour, majuscules)
    let ok = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(json!({"invitation": code, "email": "  Bob@Example.COM  ", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", text(&ok.bytes));
    assert_eq!(ok.json()["account"]["email"], "bob@example.com");
}

#[tokio::test]
async fn signup_counts_password_characters_not_bytes() {
    let h = harness().await;
    // 9 caractères accentués = 18 octets : la borne est en CARACTÈRES, donc refusé pour
    // le nombre, pas pour la taille en octets.
    let nine: String = "é".repeat(9);
    let r = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(json!({"invitation": "forge-nope", "email": "a@b.co", "password": nine})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::BAD_REQUEST, "weak_password")
    );
    // 10 caractères accentués = 20 octets : assez long, et le code d'invitation prend le
    // relais — la preuve que le mot de passe n'est plus le motif du refus.
    let ten: String = "é".repeat(10);
    let r = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(json!({"invitation": "forge-nope", "email": "a@b.co", "password": ten})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::FORBIDDEN, "invalid_invitation")
    );
}

#[tokio::test]
async fn signup_refuses_a_password_that_is_too_long() {
    let h = harness().await;
    let code = h.forge.create_invitation(1, None).await.unwrap();
    let r = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(json!({"invitation": code, "email": "a@b.co", "password": "x".repeat(257)})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::BAD_REQUEST, "weak_password")
    );
    assert!(
        message(&r).contains("au moins 10 caractères"),
        "{}",
        message(&r)
    );
}

#[tokio::test]
async fn auth_bodies_are_parsed_before_the_handler_runs() {
    let h = harness().await;
    // JSON invalide, puis champ requis absent : deux refus distincts, ni l'un ni l'autre
    // n'atteint le code de la forge.
    let (status, _h, bytes) = send(
        &h,
        Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(Body::from("{ceci n'est pas du json"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", text(&bytes));

    let r = h
        .call(
            "POST",
            "/v1/auth/signup",
            None,
            Some(json!({"email": "a@b.co", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(&r.bytes).contains("invitation"), "{}", text(&r.bytes));
}

// ── jeton d'accès ───────────────────────────────────────────────────────────

#[tokio::test]
async fn access_token_is_checked_for_scheme_signature_subject_and_account_state() {
    let h = harness().await;
    let (_t, _r, id) = h.signup("alice@b.co").await;
    let keys = &h.forge.keys;

    // schéma d'autorisation autre que Bearer, ou valeur non-ASCII : « jeton manquant »
    for value in [
        HeaderValue::from_static("Basic YWxpY2U6c2VjcmV0"),
        HeaderValue::from_static("Bearer"),
        HeaderValue::from_static("bearer minuscule"),
        HeaderValue::from_bytes(b"Bearer \xff\xfe").unwrap(),
    ] {
        let (status, _hd, bytes) = send(&h, bearer("/v1/me", value)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{}", text(&bytes));
        let j: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["error"]["code"], "unauthorized");
        assert_eq!(
            j["error"]["message"], "jeton manquant : Authorization: Bearer <jwt>",
            "valeur acceptée à tort"
        );
    }

    // « Bearer » suivi de rien : l'en-tête est PRÉSENT (le préfixe est reconnu), donc ce
    // n'est plus « jeton manquant » mais un jeton vide, que la vérification refuse.
    let (status, _hd, bytes) =
        send(&h, bearer("/v1/me", HeaderValue::from_static("Bearer "))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let j: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(j["error"]["message"], "jeton invalide ou expiré");

    // signé par la bonne clé mais `sub` n'est pas un uuid
    let bad_sub = issue_token(keys, "pas-un-uuid", 600).unwrap();
    let r = h.call("GET", "/v1/me", Some(&bad_sub), None).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    assert_eq!(message(&r), "jeton invalide");

    // signé par la bonne clé, `sub` est un uuid, mais aucun compte
    let ghost = issue_token(keys, &Uuid::new_v4().to_string(), 600).unwrap();
    let r = h.call("GET", "/v1/me", Some(&ghost), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(message(&r), "compte inconnu");

    // expiré (ttl nul, puis on laisse passer la seconde)
    let expired = issue_token(keys, &id, 0).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let r = h.call("GET", "/v1/me", Some(&expired), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(message(&r), "jeton invalide ou expiré");

    // compte désactivé APRÈS émission du jeton : le jeton reste vérifiable, le compte non
    let gone = h
        .forge
        .store
        .accounts
        .create(Account::new(
            "off@b.co".into(),
            "suspended".into(),
            h.forge.dummy_hash.clone(),
        ))
        .await
        .unwrap();
    let tg = issue_token(keys, &gone.id.to_string(), 600).unwrap();
    let r = h.call("GET", "/v1/me", Some(&tg), None).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    assert_eq!(message(&r), "compte désactivé");
    // et un compte désactivé ne peut pas se connecter non plus
    let pw = hash_password("correct horse battery").unwrap();
    h.forge
        .store
        .accounts
        .create(Account::new("off2@b.co".into(), "suspended".into(), pw))
        .await
        .unwrap();
    let r = h
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(json!({"email": "off2@b.co", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(message(&r), "e-mail ou mot de passe incorrect");
}

#[tokio::test]
async fn login_does_not_say_which_of_the_two_was_wrong() {
    let h = harness().await;
    h.signup("alice@b.co").await;
    let attempt = |email: &'static str, pw: &'static str| {
        let h = &h;
        async move {
            let start = Instant::now();
            let r = h
                .call(
                    "POST",
                    "/v1/auth/login",
                    None,
                    Some(json!({"email": email, "password": pw})),
                )
                .await;
            (r, start.elapsed())
        }
    };
    let (unknown, took_unknown) = attempt("inconnu@b.co", "correct horse battery").await;
    let (wrong, took_wrong) = attempt("alice@b.co", "mauvais mot de passe").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.code(), "unauthorized");
    // mêmes octets : rien ne dit lequel des deux a échoué
    assert_eq!(
        text(&unknown.bytes),
        text(&wrong.bytes),
        "la réponse distingue e-mail inconnu et mot de passe faux"
    );
    // et le coût est du même ordre : l'e-mail inconnu vérifie un hachage factice, il ne
    // répond pas « tout de suite » (un retour immédiat serait < 1 ms).
    assert!(
        took_unknown >= Duration::from_millis(2),
        "e-mail inconnu répondu en {took_unknown:?} : aucun hachage vérifié, donc le temps \
         de réponse trahit l'existence du compte"
    );
    assert!(
        took_unknown <= took_wrong * 20,
        "temps trop divergents : inconnu {took_unknown:?} contre mot de passe faux {took_wrong:?}"
    );
}

// ── jeton de rafraîchissement ───────────────────────────────────────────────

#[tokio::test]
async fn refresh_refuses_expired_orphaned_and_corrupted_sessions() {
    let h = harness().await;
    let (_t, refresh, id) = h.signup("alice@b.co").await;

    // inconnu : le jeton ne correspond à aucune session
    let r = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": "jeton-invente"})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    assert_eq!(message(&r), "jeton de rafraîchissement inconnu");

    // expiré : la session existe mais sa date est passée
    let expired = "forge-refresh-expire";
    h.forge
        .store
        .sessions
        .create(Session::new(
            sha256_hex(expired.as_bytes()),
            "active".into(),
            id.clone(),
            now_secs() as i64 - 60,
        ))
        .await
        .unwrap();
    let r = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": expired})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    assert!(
        message(&r).contains("expiré"),
        "message attendu sur l'expiration : {}",
        message(&r)
    );

    // compte disparu entre-temps : la session est valide, le compte n'existe plus
    let orphan = "forge-refresh-orphelin";
    h.forge
        .store
        .sessions
        .create(Session::new(
            sha256_hex(orphan.as_bytes()),
            "active".into(),
            Uuid::new_v4().to_string(),
            now_secs() as i64 + 600,
        ))
        .await
        .unwrap();
    let r = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": orphan})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(message(&r), "compte inconnu");

    // session corrompue (`account_id` qui n'est pas un uuid) : 500 propre, pas de panique
    let broken = "forge-refresh-corrompu";
    h.forge
        .store
        .sessions
        .create(Session::new(
            sha256_hex(broken.as_bytes()),
            "active".into(),
            "compte-sans-uuid".into(),
            now_secs() as i64 + 600,
        ))
        .await
        .unwrap();
    let r = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": broken})),
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal")
    );
    assert_eq!(message(&r), "erreur interne de la forge");
    assert!(
        !message(&r).contains("uuid"),
        "le détail interne ne doit pas fuir"
    );

    // le jeton de rafraîchissement d'origine, lui, marche toujours
    let ok = h
        .call(
            "POST",
            "/v1/auth/refresh",
            None,
            Some(json!({"refresh": refresh})),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
}

#[tokio::test]
async fn logout_needs_a_token_and_revokes_every_session_of_the_account() {
    let h = harness().await;
    let (t, _refresh, _id) = h.signup("alice@b.co").await;
    let r = h.call("POST", "/v1/auth/logout", None, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(message(&r), "jeton manquant : Authorization: Bearer <jwt>");

    // deux connexions : le logout en révoque deux
    let two = |pw: &'static str| json!({"email": "alice@b.co", "password": pw});
    let _second = h
        .call(
            "POST",
            "/v1/auth/login",
            None,
            Some(two("correct horse battery")),
        )
        .await;
    assert!(h.forge.store.sessions.list().await.unwrap().len() >= 2);
    let out = h.call("POST", "/v1/auth/logout", Some(&t), None).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert_eq!(out.bytes.len(), 0, "un 204 n'a pas de corps");
    assert!(
        h.forge.store.sessions.list().await.unwrap().is_empty(),
        "des jetons de rafraîchissement survivent au logout"
    );
}

// ── builds : existence et propriété ─────────────────────────────────────────

#[tokio::test]
async fn a_build_id_that_is_unknown_and_one_that_is_someone_elses_are_indistinguishable() {
    let h = harness().await;
    let (ta, _, _) = h.signup("a@b.co").await;
    let (_tb, _, idb) = h.signup("b@b.co").await;
    let theirs = h
        .forge
        .store
        .builds
        .create(signed_row(&idb, "secret"))
        .await
        .unwrap();
    std::fs::write(h.forge.artifact_path(&theirs.id), b"\0asm\x01\0\0\0").unwrap();
    let paths = |id: Uuid| {
        [
            format!("/v1/builds/{id}"),
            format!("/v1/builds/{id}/artifact"),
            format!("/v1/builds/{id}/events"),
        ]
    };
    for (other, ghost) in paths(theirs.id).iter().zip(paths(Uuid::new_v4()).iter()) {
        let a_other = h.call("GET", other, Some(&ta), None).await;
        let a_ghost = h.call("GET", ghost, Some(&ta), None).await;
        assert_eq!(a_other.status, StatusCode::NOT_FOUND, "{other}");
        assert_eq!(a_ghost.status, StatusCode::NOT_FOUND, "{ghost}");
        assert_eq!(
            text(&a_other.bytes),
            text(&a_ghost.bytes),
            "le corps d'un build d'autrui diffère de celui d'un build inexistant"
        );
        assert_eq!(a_other.code(), "not_found");
        assert_eq!(message(&a_other), "build inconnu");
        assert!(!text(&a_other.bytes).contains("secret"));
    }

    // un identifiant qui n'est même pas un uuid : même refus, même corps
    for path in [
        "/v1/builds/pas-un-uuid",
        "/v1/builds/pas-un-uuid/artifact",
        "/v1/builds/pas-un-uuid/events",
    ] {
        let r = h.call("GET", path, Some(&ta), None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.code(), "not_found");
        assert_eq!(message(&r), "build inconnu");
    }
}

// ── builds : la vue JSON ────────────────────────────────────────────────────

#[tokio::test]
async fn build_view_hides_the_artifact_until_it_is_signed_and_survives_broken_diagnostics() {
    let h = harness().await;
    let (t, _, id) = h.signup("a@b.co").await;

    // en file : pas d'artefact, pas de diagnostic
    let queued = h
        .forge
        .store
        .builds
        .create(build_row(&id, "demo", "queued"))
        .await
        .unwrap();
    let j = h
        .call("GET", &format!("/v1/builds/{}", queued.id), Some(&t), None)
        .await
        .json();
    assert_eq!(j["status"], "queued");
    assert_eq!(
        j["artifact"],
        Value::Null,
        "artefact annoncé avant signature : {j}"
    );
    assert_eq!(j["diagnostics"], json!([]));
    assert_eq!(j["finished_at"], Value::Null);

    // `diagnostics_json` illisible : on rend une liste vide, jamais un 500
    let mut broken = build_row(&id, "demo2", "failed");
    broken.diagnostics_json = "{pas du json".into();
    let broken = h.forge.store.builds.create(broken).await.unwrap();
    let r = h
        .call("GET", &format!("/v1/builds/{}", broken.id), Some(&t), None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["diagnostics"], json!([]));
    assert_eq!(r.json()["artifact"], Value::Null);

    // un échec réel : les diagnostics sortent tels quels
    let mut failed = build_row(&id, "demo3", "failed");
    failed.diagnostics_json = diagnostics_json("cannot find value `x`");
    failed.finished_at = Some(Utc::now().to_rfc3339());
    failed.duration_ms = 4200;
    let failed = h.forge.store.builds.create(failed).await.unwrap();
    let j = h
        .call("GET", &format!("/v1/builds/{}", failed.id), Some(&t), None)
        .await
        .json();
    assert_eq!(j["diagnostics"][0]["message"], "cannot find value `x`");
    assert_eq!(j["diagnostics"][0]["file"], "src/lib.rs");
    assert_eq!(j["status"], "failed");

    // et le succès porte l'artefact complet
    let signed = h
        .forge
        .store
        .builds
        .create(signed_row(&id, "demo4"))
        .await
        .unwrap();
    let j = h
        .call("GET", &format!("/v1/builds/{}", signed.id), Some(&t), None)
        .await
        .json();
    assert_eq!(j["artifact"]["sha256"], "sha-du-wasm");
    assert_eq!(j["artifact"]["size"], 8);
    assert_eq!(j["artifact"]["signature"], "signature-ed25519");
}

// ── builds : l'artefact ─────────────────────────────────────────────────────

#[tokio::test]
async fn artifact_refuses_everything_that_is_not_a_signed_build_with_a_readable_file() {
    let h = harness().await;
    let (t, _, id) = h.signup("a@b.co").await;

    // en cours : 409 `not_ready`
    let queued = h
        .forge
        .store
        .builds
        .create(build_row(&id, "demo", "queued"))
        .await
        .unwrap();
    let r = h
        .call(
            "GET",
            &format!("/v1/builds/{}/artifact", queued.id),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::CONFLICT, "not_ready")
    );
    assert_eq!(message(&r), "le build n'est pas terminé");

    // échoué : 409 `build_failed`, qui renvoie aux diagnostics
    let mut failed = build_row(&id, "demo2", "failed");
    failed.diagnostics_json = diagnostics_json("erreur");
    let failed = h.forge.store.builds.create(failed).await.unwrap();
    let r = h
        .call(
            "GET",
            &format!("/v1/builds/{}/artifact", failed.id),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::CONFLICT, "build_failed")
    );
    assert!(message(&r).contains("diagnostics"), "{}", message(&r));

    // signé mais le fichier a disparu du disque : 500, pas un corps vide
    let lost = h
        .forge
        .store
        .builds
        .create(signed_row(&id, "demo3"))
        .await
        .unwrap();
    let r = h
        .call(
            "GET",
            &format!("/v1/builds/{}/artifact", lost.id),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal")
    );
    assert_eq!(message(&r), "erreur interne de la forge");

    // signé, fichier présent, mais une signature qui n'est pas un en-tête HTTP valide
    let mut hostile = signed_row(&id, "demo4");
    hostile.signature = Some("signature\r\nx-injecte: 1".into());
    let hostile = h.forge.store.builds.create(hostile).await.unwrap();
    std::fs::write(h.forge.artifact_path(&hostile.id), b"\0asm\x01\0\0\0").unwrap();
    let r = h
        .call(
            "GET",
            &format!("/v1/builds/{}/artifact", hostile.id),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal")
    );

    // signé sans signature ni manifeste : l'artefact sort quand même, sans ces en-têtes
    let mut bare = signed_row(&id, "demo5");
    bare.signature = None;
    bare.manifest_b64 = None;
    let bare = h.forge.store.builds.create(bare).await.unwrap();
    std::fs::write(h.forge.artifact_path(&bare.id), b"\0asm\x01\0\0\0").unwrap();
    let r = h
        .call(
            "GET",
            &format!("/v1/builds/{}/artifact", bare.id),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "application/wasm");
    assert!(!r.headers.contains_key("x-forge-signature"));
    assert!(!r.headers.contains_key("x-forge-manifest"));
    assert_eq!(r.bytes, b"\0asm\x01\0\0\0");
}

// ── builds : le flux d'événements ───────────────────────────────────────────

#[tokio::test]
async fn build_events_ends_with_the_terminal_state() {
    let h = harness().await;
    let (t, _, id) = h.signup("a@b.co").await;

    let mut signed = signed_row(&id, "demo");
    signed.diagnostics_json = diagnostics_json("avertissement de build");
    let signed = h.forge.store.builds.create(signed).await.unwrap();
    let sse = sse_text(
        &h,
        &t,
        &format!("/v1/builds/{}/events", signed.id),
        Duration::from_secs(10),
    )
    .await;
    assert!(sse.contains("event: status"), "{sse}");
    assert!(sse.contains(r#"data: {"status":"signed"}"#), "{sse}");
    assert!(sse.contains("event: diagnostic"), "{sse}");
    assert!(sse.contains("avertissement de build"), "{sse}");
    assert!(sse.contains("event: artifact"), "{sse}");
    assert!(sse.contains("sha-du-wasm"), "{sse}");
    assert!(sse.contains("event: done"), "{sse}");

    // un échec : des diagnostics et un `done`, aucun artefact
    let mut failed = build_row(&id, "demo2", "failed");
    failed.diagnostics_json = diagnostics_json("impossible de compiler");
    let failed = h.forge.store.builds.create(failed).await.unwrap();
    let sse = sse_text(
        &h,
        &t,
        &format!("/v1/builds/{}/events", failed.id),
        Duration::from_secs(10),
    )
    .await;
    assert!(sse.contains(r#"data: {"status":"failed"}"#), "{sse}");
    assert!(sse.contains("impossible de compiler"), "{sse}");
    assert!(sse.contains("event: done"), "{sse}");
    assert!(
        !sse.contains("event: artifact"),
        "un échec n'a pas d'artefact : {sse}"
    );
}

#[tokio::test]
async fn build_events_follows_the_build_and_stops_when_it_disappears() {
    let h = harness().await;
    let (t, _, id) = h.signup("a@b.co").await;

    // 1. le build change d'état PENDANT le flux : l'événement arrive, puis `done`
    let moving = h
        .forge
        .store
        .builds
        .create(build_row(&id, "demo", "queued"))
        .await
        .unwrap();
    let forge = h.forge.clone();
    let bid = moving.id;
    let mover = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let Ok(Some(mut row)) = forge.store.builds.get(&bid).await else {
            return;
        };
        row.status = "signed".into();
        row.wasm_sha256 = Some("sha-du-wasm".into());
        row.wasm_size = Some(8);
        row.signature = Some("signature-ed25519".into());
        row.manifest_b64 = Some("manifeste-b64".into());
        row.finished_at = Some(Utc::now().to_rfc3339());
        forge.store.builds.update(&bid, row).await.unwrap();
    });
    let sse = sse_text(
        &h,
        &t,
        &format!("/v1/builds/{}/events", moving.id),
        Duration::from_secs(10),
    )
    .await;
    mover.await.unwrap();
    assert!(sse.contains(r#"data: {"status":"queued"}"#), "{sse}");
    assert!(sse.contains(r#"data: {"status":"signed"}"#), "{sse}");
    assert!(sse.contains("event: done"), "{sse}");

    // 2. le build disparaît du store : le flux se ferme au lieu de tourner sans fin
    let doomed = h
        .forge
        .store
        .builds
        .create(build_row(&id, "demo2", "queued"))
        .await
        .unwrap();
    let forge = h.forge.clone();
    let did = doomed.id;
    let remover = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        forge.store.builds.delete(&did).await.unwrap();
    });
    let started = Instant::now();
    let sse = sse_text(
        &h,
        &t,
        &format!("/v1/builds/{}/events", doomed.id),
        Duration::from_secs(10),
    )
    .await;
    remover.await.unwrap();
    assert!(sse.contains(r#"data: {"status":"queued"}"#), "{sse}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "le flux n'a pas vu la disparition du build et a continué"
    );
}

// ── builds : chaîne d'outils et quotas ──────────────────────────────────────

#[tokio::test]
async fn toolchain_must_be_the_one_the_forge_compiles_with() {
    let h = harness_with(|c| c.docker_bin = "/nonexistent/docker".into()).await;
    let (t, _, _) = h.signup("a@b.co").await;
    let submit = |toolchain: &'static str| {
        let h = &h;
        let t = t.clone();
        async move {
            h.call(
                "POST",
                "/v1/builds",
                Some(&t),
                Some(json!({
                    "name": "demo",
                    "version": "1.0.0",
                    "files": good_files(),
                    "toolchain": toolchain,
                })),
            )
            .await
        }
    };
    let r = submit("nightly").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::BAD_REQUEST, "unsupported_toolchain")
    );
    assert!(
        message(&r).contains("nightly"),
        "le refus doit nommer la chaîne demandée : {}",
        message(&r)
    );
    // `stable` et la chaîne réelle (inconnue ici : docker est cassé) sont acceptées
    assert_eq!(submit("stable").await.status, StatusCode::ACCEPTED);
    assert_eq!(
        submit("unknown").await.status,
        StatusCode::ACCEPTED,
        "la chaîne annoncée par la forge doit être acceptée"
    );
}

#[tokio::test]
async fn cpu_budget_and_pending_builds_are_capped() {
    let h = harness_with(|c| {
        c.docker_bin = "/nonexistent/docker".into();
        c.cpu_seconds_per_day = 1;
        c.max_pending_per_account = 1;
    })
    .await;
    let (t, _, id) = h.signup("a@b.co").await;

    // un build d'hier ne compte pas dans le budget du jour
    let mut yesterday = build_row(&id, "hier", "signed");
    yesterday.created_at = Utc::now() - chrono::Duration::days(1);
    yesterday.duration_ms = 3_600_000;
    h.forge.store.builds.create(yesterday).await.unwrap();
    let me = h.call("GET", "/v1/me", Some(&t), None).await.json();
    assert_eq!(me["quota"]["builds_today"], 0, "{me}");
    assert_eq!(me["quota"]["cpu_seconds_left"], 1, "{me}");

    // une durée négative ne consomme rien et ne fait pas paniquer le décompte
    let mut negative = build_row(&id, "negatif", "failed");
    negative.duration_ms = -5_000;
    h.forge.store.builds.create(negative).await.unwrap();
    let me = h.call("GET", "/v1/me", Some(&t), None).await.json();
    assert_eq!(me["quota"]["cpu_seconds_left"], 1, "{me}");

    // budget de calcul dépassé : 429 `quota_cpu_exceeded`
    let mut hogs = build_row(&id, "gourmand", "signed");
    hogs.duration_ms = 2_000;
    h.forge.store.builds.create(hogs).await.unwrap();
    let r = h.submit(&t, "demo", good_files()).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "quota_cpu_exceeded")
    );
    assert!(message(&r).contains("budget de calcul"), "{}", message(&r));
    let me = h.call("GET", "/v1/me", Some(&t), None).await.json();
    assert_eq!(
        me["quota"]["cpu_seconds_left"], 0,
        "le reste ne passe pas sous zéro : {me}"
    );

    // et un build en attente compte : `max_pending_per_account` = 1
    let h2 = harness_with(|c| {
        c.docker_bin = "/nonexistent/docker".into();
        c.max_pending_per_account = 1;
    })
    .await;
    let (t2, _, id2) = h2.signup("a@b.co").await;
    h2.forge
        .store
        .builds
        .create(build_row(&id2, "attente", "queued"))
        .await
        .unwrap();
    let r = h2.submit(&t2, "demo", good_files()).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "too_many_pending")
    );
    assert!(message(&r).contains("1 builds"), "{}", message(&r));
    // un build en cours compte aussi
    h2.forge
        .store
        .builds
        .create(build_row(&id2, "encours", "building"))
        .await
        .unwrap();
    let r = h2.submit(&t2, "demo", good_files()).await;
    assert_eq!(r.code(), "too_many_pending");
}

#[tokio::test]
async fn sources_are_bounded_in_count_and_bytes() {
    let h = harness_with(|c| c.docker_bin = "/nonexistent/docker".into()).await;
    let (t, _, _) = h.signup("a@b.co").await;

    let mut many: Vec<Value> = (0..65)
        .map(|i| {
            let path = if i == 0 {
                "src/lib.rs".to_string()
            } else {
                format!("src/m{i}.rs")
            };
            json!({"path": path, "content": ""})
        })
        .collect();
    let r = h.submit(&t, "demo", json!(many.clone())).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::BAD_REQUEST, "invalid_files")
    );
    assert!(
        message(&r).contains("65 fichiers, maximum 64"),
        "{}",
        message(&r)
    );

    // 64 fichiers passent la borne du nombre (et butent plus loin, pas sur `invalid_files`)
    many.truncate(64);
    let r = h.submit(&t, "demo", json!(many)).await;
    assert_ne!(r.code(), "invalid_files", "{}", message(&r));

    // au-delà d'un mébioctet de sources : refus explicite, avec les deux nombres
    let big = json!([{"path": "src/lib.rs", "content": "x".repeat(1024 * 1024 + 1)}]);
    let r = h.submit(&t, "demo", big).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::BAD_REQUEST, "invalid_files")
    );
    assert!(
        message(&r).contains("sources trop volumineuses")
            && message(&r).contains("maximum 1048576"),
        "{}",
        message(&r)
    );

    // chemin en double
    let dup = json!([
        {"path": "src/lib.rs", "content": ""},
        {"path": "src/lib.rs", "content": ""}
    ]);
    let r = h.submit(&t, "demo", dup).await;
    assert_eq!(r.code(), "invalid_files");
    assert!(message(&r).contains("chemin en double"), "{}", message(&r));
}

#[tokio::test]
async fn a_body_over_four_megabytes_is_refused_before_the_handler() {
    let h = harness().await;
    let huge = json!({
        "invitation": "forge-nope",
        "email": "a@b.co",
        "password": "x".repeat(5 * 1024 * 1024),
    });
    let (status, _headers, bytes) = send(
        &h,
        Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(Body::from(huge.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "corps de 5 Mio accepté : {}",
        text(&bytes)
    );
    let j: Value = serde_json::from_slice(&bytes).unwrap();
    // le refus vient de la couche transport du framework (forme `{code, detail, status}`),
    // pas d'une règle métier de la forge (forme `{error:{code,message}}`)
    assert_eq!(j["status"], 413, "{j}");
    assert_eq!(j["code"], "this.transport.bad_request", "{j}");
    assert!(
        j["detail"]
            .as_str()
            .unwrap_or("")
            .contains("length limit exceeded"),
        "ce n'est pas la limite de corps qui a refusé : {j}"
    );
    assert!(j["error"].is_string(), "forme d'erreur inattendue : {j}");
}

// ── authentification avant lecture du corps, routes inconnues, méthodes ─────

#[tokio::test]
async fn authentication_comes_before_the_body_is_looked_at() {
    let h = harness().await;
    // corps illisible ET jeton absent : c'est le jeton qui décide
    let (status, _headers, bytes) = send(
        &h,
        Request::builder()
            .method("POST")
            .uri("/v1/builds")
            .header("content-type", "application/json")
            .body(Body::from("pas du json du tout"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{}", text(&bytes));
    let j: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(j["error"]["code"], "unauthorized");

    // et avec un jeton valide mais un corps invalide : 400 (JSON) — jamais 500
    let (t, _, _) = h.signup("a@b.co").await;
    let (status, _headers, bytes) = send(
        &h,
        Request::builder()
            .method("POST")
            .uri("/v1/builds")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {t}"))
            .body(Body::from("{oops"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", text(&bytes));
}

#[tokio::test]
async fn unknown_route_and_wrong_method_are_refused_plainly() {
    let h = harness().await;
    // Le framework répond AVANT les routes de la forge pour ce qu'il ne sait pas router :
    // une route inconnue n'est donc pas un 404 muet d'axum mais un refus explicite.
    for uri in [
        "/v1/nexiste-pas",
        "/v1/builds/abc/artifact/extra",
        "/v1/forge/key/",
    ] {
        let r = h.call("GET", uri, None, None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{uri}");
        let j = r.json();
        assert_eq!(j["code"], "this.transport.bad_request", "{uri}");
        assert!(
            j["detail"].as_str().unwrap_or("").contains("path"),
            "le refus doit dire que le chemin est en cause : {j}"
        );
        assert_eq!(
            r.code(),
            "",
            "la forge ne rend pas son JSON sur une route inconnue"
        );
    }

    // la bonne route, la mauvaise méthode
    let r = h.call("GET", "/v1/auth/signup", None, None).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.json()["code"], "this.transport.method_not_allowed");
    assert!(
        r.json()["detail"].as_str().unwrap_or("").contains("GET"),
        "le refus doit nommer la méthode : {}",
        r.json()
    );

    for (method, uri) in [
        ("DELETE", "/v1/me"),
        ("POST", "/v1/me"),
        ("GET", "/v1/builds"),
    ] {
        let r = h.call(method, uri, None, None).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
        assert_eq!(r.json()["code"], "this.transport.method_not_allowed");
    }

    // et la liveness du framework reste joignable pendant ce temps
    let live = h.call("GET", "/health", None, None).await;
    assert_eq!(live.status, StatusCode::OK);
    assert_eq!(live.json()["status"], "ok");
}
