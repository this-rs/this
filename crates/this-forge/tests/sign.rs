//! `sign.rs` : chargement des clés, signature/vérification, JSON canonique, empreintes.

use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};
use this_forge::sign::{
    Attestation, ForgeKeys, b64url, canonical_json, sha256_hex, source_digest, unb64url, verify,
};

fn err(r: anyhow::Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", r.unwrap_err())
}

fn key_file(dir: &std::path::Path, name: &str, content: &str, mode: u32) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    p
}

fn fresh() -> (tempfile::TempDir, ForgeKeys) {
    let d = tempfile::tempdir().unwrap();
    let k = ForgeKeys::load_or_create(&d.path().join("k")).unwrap();
    (d, k)
}

// ---------- chargement de clé ----------

#[test]
fn absent_key_is_created_with_parent_dirs_hex_and_0600_for_both_keys() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a/b/forge.key");
    let k = ForgeKeys::load_or_create(&p).unwrap();
    let jwt = d.path().join("a/b/forge.key.jwt");
    for f in [&p, &jwt] {
        let text = std::fs::read_to_string(f).unwrap();
        assert_eq!(text.len(), 65);
        assert!(text.ends_with('\n'));
        assert!(
            text.trim()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        assert_eq!(
            std::fs::metadata(f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // les deux clés sont distinctes
    assert_ne!(
        std::fs::read_to_string(&p).unwrap(),
        std::fs::read_to_string(&jwt).unwrap()
    );
    assert_eq!(k.public_hex().len(), 64);
}

#[test]
fn existing_key_file_is_loaded_deterministically() {
    let d = tempfile::tempdir().unwrap();
    let seed = "01".repeat(32);
    let p = key_file(d.path(), "k", &format!("  {seed}  \n"), 0o600);
    let a = ForgeKeys::load_or_create(&p).unwrap();
    let b = ForgeKeys::load_or_create(&p).unwrap();
    assert_eq!(a.public_hex(), b.public_hex());
    assert_eq!(a.sign(b"x"), b.sign(b"x"));
    // une graine d'octets 0x01 donne une clé publique fixe (Ed25519 déterministe)
    let expected = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]).verifying_key();
    assert_eq!(a.verifying_key(), expected);
}

#[test]
fn key_file_with_loose_permissions_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let p = key_file(d.path(), "k", &"ab".repeat(32), 0o644);
    let e = err(ForgeKeys::load_or_create(&p).map(|_| ()));
    assert!(
        e.contains("est lisible par d'autres que son propriétaire (mode 644) : chmod 600"),
        "{e}"
    );
    let p = key_file(d.path(), "k2", &"ab".repeat(32), 0o640);
    assert!(err(ForgeKeys::load_or_create(&p).map(|_| ())).contains("mode 640"));
}

#[test]
fn key_file_with_wrong_length_is_unreadable() {
    let d = tempfile::tempdir().unwrap();
    for (n, content) in [
        ("empty", ""),
        ("short", "abcd"),
        ("long", &"a".repeat(66)[..]),
    ] {
        let p = key_file(d.path(), n, content, 0o600);
        let e = err(ForgeKeys::load_or_create(&p).map(|_| ()));
        assert!(
            e.contains("clé illisible (64 caractères hex attendus)"),
            "{n}: {e}"
        );
    }
}

#[test]
fn key_file_with_non_hex_content_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let p = key_file(d.path(), "k", &"zz".repeat(32), 0o600);
    let e = err(ForgeKeys::load_or_create(&p).map(|_| ()));
    assert!(e.contains("hex invalide"), "{e}");
}

#[test]
fn pem_header_instead_of_hex_is_not_accepted() {
    let d = tempfile::tempdir().unwrap();
    let p = key_file(
        d.path(),
        "k",
        "-----BEGIN PRIVATE KEY-----\nMC4CAQ\n-----END PRIVATE KEY-----\n",
        0o600,
    );
    assert!(err(ForgeKeys::load_or_create(&p).map(|_| ())).contains("clé illisible"));
}

#[test]
fn key_file_with_invalid_utf8_is_an_io_error() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("k");
    std::fs::write(&p, [0xffu8, 0xfe, 0xfd]).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    let e = err(ForgeKeys::load_or_create(&p).map(|_| ()));
    assert!(e.contains("valid UTF-8"), "{e}");
}

#[test]
fn key_path_that_is_a_directory_is_unreadable() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("dir");
    std::fs::create_dir(&p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(ForgeKeys::load_or_create(&p).is_err());
}

#[test]
fn creation_failure_names_the_path() {
    let d = tempfile::tempdir().unwrap();
    // le « répertoire parent » est un fichier : create_dir_all échoue
    let file = d.path().join("f");
    std::fs::write(&file, "x").unwrap();
    assert!(ForgeKeys::load_or_create(&file.join("sub/k")).is_err());
    // le parent existe mais n'est pas inscriptible : l'ouverture échoue avec le chemin
    let ro = d.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
    let r = ForgeKeys::load_or_create(&ro.join("k"));
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
    if libc_geteuid() != 0 {
        assert!(err(r.map(|_| ())).contains("création de "));
    }
}

fn libc_geteuid() -> u32 {
    // évite une dépendance : l'uid vient du propriétaire d'un fichier fraîchement créé
    use std::os::unix::fs::MetadataExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::metadata(d.path()).unwrap().uid()
}

#[test]
fn bad_jwt_key_file_fails_the_whole_load() {
    let d = tempfile::tempdir().unwrap();
    let p = key_file(d.path(), "k", &"11".repeat(32), 0o600);
    key_file(d.path(), "k.jwt", "nope", 0o600);
    assert!(err(ForgeKeys::load_or_create(&p).map(|_| ())).contains("k.jwt"));
}

#[test]
fn accessors_expose_consistent_key_material() {
    let (_d, k) = fresh();
    assert_eq!(
        k.public_hex(),
        k.verifying_key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert_eq!(
        k.fingerprint(),
        format!("sha256:{}", sha256_hex(k.verifying_key().as_bytes()))
    );
    // jetons : ce qu'encode l'une se décode avec l'autre (clé jwt distincte de la clé d'artefact)
    let _ = (k.jwt_encoding(), k.jwt_decoding());
    use jsonwebtoken::{Algorithm, Header, Validation, decode, encode};
    let claims = json!({"sub": "x", "exp": 4_102_444_800u64});
    let tok = encode(&Header::new(Algorithm::EdDSA), &claims, k.jwt_encoding()).unwrap();
    let mut v = Validation::new(Algorithm::EdDSA);
    v.required_spec_claims.clear();
    let out = decode::<Value>(&tok, k.jwt_decoding(), &v).unwrap();
    assert_eq!(out.claims["sub"], "x");
}

// ---------- signature / vérification ----------

#[test]
fn sign_then_verify_roundtrip_and_signature_shape() {
    let (_d, k) = fresh();
    let sig = k.sign(b"hello");
    assert_eq!(sig.len(), 86); // 64 octets en base64url sans remplissage
    assert!(!sig.contains(['+', '/', '=']));
    verify(&k.public_hex(), b"hello", &sig).unwrap();
    // message vide
    verify(&k.public_hex(), b"", &k.sign(b"")).unwrap();
}

#[test]
fn verify_rejects_altered_message_by_one_byte() {
    let (_d, k) = fresh();
    let sig = k.sign(b"hello");
    let e = err(verify(&k.public_hex(), b"hellp", &sig));
    assert!(e.to_lowercase().contains("signature"), "{e}");
    assert!(verify(&k.public_hex(), b"hello!", &sig).is_err());
}

#[test]
fn verify_rejects_signature_from_another_key() {
    let (_d1, a) = fresh();
    let (_d2, b) = fresh();
    let sig = b.sign(b"m");
    assert!(verify(&a.public_hex(), b"m", &sig).is_err());
    verify(&b.public_hex(), b"m", &sig).unwrap();
}

#[test]
fn verify_rejects_bad_public_key_forms() {
    let (_d, k) = fresh();
    let sig = k.sign(b"m");
    assert_eq!(
        err(verify("abcd", b"m", &sig)),
        "clé publique : 64 caractères hex attendus"
    );
    assert_eq!(
        err(verify("", b"m", &sig)),
        "clé publique : 64 caractères hex attendus"
    );
    // 64 caractères mais pas de l'hexadécimal
    let e = err(verify(&"zz".repeat(32), b"m", &sig));
    assert!(e.contains("invalid digit"), "{e}");
    // 32 octets qui ne décodent pas en point de la courbe
    let bad = "ff".repeat(32);
    let r = verify(&bad, b"m", &sig);
    assert!(r.is_err());
}

#[test]
fn verify_rejects_bad_signature_encodings() {
    let (_d, k) = fresh();
    let pk = k.public_hex();
    // base64url invalide (caractères standard « + » / « / » et remplissage « = »)
    assert!(verify(&pk, b"m", "ab+/cd==").is_err());
    assert!(verify(&pk, b"m", "!!!").is_err());
    // base64url valide mais mauvaise longueur
    assert_eq!(
        err(verify(&pk, b"m", &b64url(&[0u8; 63]))),
        "signature : 64 octets"
    );
    assert_eq!(
        err(verify(&pk, b"m", &b64url(&[0u8; 65]))),
        "signature : 64 octets"
    );
    assert_eq!(err(verify(&pk, b"m", "")), "signature : 64 octets");
    // 64 octets nuls : bonne longueur, signature fausse
    assert!(verify(&pk, b"m", &b64url(&[0u8; 64])).is_err());
}

#[test]
fn manifest_rebuilt_with_keys_in_another_order_verifies() {
    let (_d, k) = fresh();
    let a = json!({"z": 1, "a": {"y": [1, 2], "b": null}});
    let b: Value = serde_json::from_str(r#"{"a":{"b":null,"y":[1,2]},"z":1}"#).unwrap();
    let sig = k.sign(canonical_json(&a).as_bytes());
    verify(&k.public_hex(), canonical_json(&b).as_bytes(), &sig).unwrap();
    // mais la forme non canonique (autre ordre brut) ne vérifie pas
    let raw = r#"{"z":1,"a":{"y":[1,2],"b":null}}"#;
    assert!(verify(&k.public_hex(), raw.as_bytes(), &sig).is_err());
}

// ---------- canonical_json ----------

fn canon(s: &str) -> String {
    canonical_json(&serde_json::from_str::<Value>(s).unwrap())
}

#[test]
fn canonical_scalars() {
    assert_eq!(canon("null"), "null");
    assert_eq!(canon("true"), "true");
    assert_eq!(canon("false"), "false");
    assert_eq!(canon("0"), "0");
    assert_eq!(canon("-12"), "-12");
    assert_eq!(canon("1.5"), "1.5");
    assert_eq!(canon("1e3"), "1000.0");
    assert_eq!(canon(r#""""#), r#""""#);
    assert_eq!(canon(r#""a\"b\\c\n""#), r#""a\"b\\c\n""#);
}

#[test]
fn canonical_empty_containers() {
    assert_eq!(canon("{}"), "{}");
    assert_eq!(canon("[]"), "[]");
    assert_eq!(canon(r#"{"a":{},"b":[]}"#), r#"{"a":{},"b":[]}"#);
}

#[test]
fn canonical_nested_objects_sorted_at_every_level_arrays_keep_order() {
    assert_eq!(
        canon(r#"{"b":{"d":1,"c":[3,{"y":0,"x":null}]},"a":[2,1]}"#),
        r#"{"a":[2,1],"b":{"c":[3,{"x":null,"y":0}],"d":1}}"#
    );
    assert_eq!(
        canon(r#"[ {"b":1,"a":2} , [ ] , "x" ]"#),
        r#"[{"a":2,"b":1},[],"x"]"#
    );
}

#[test]
fn canonical_sorts_keys_by_utf8_bytes_not_by_locale_or_case() {
    // 'B' (0x42) < 'a' (0x61) < 'é' (0xC3A9) < '€' (0xE282AC)
    assert_eq!(
        canon(r#"{"€":4,"é":3,"a":2,"B":1}"#),
        r#"{"B":1,"a":2,"é":3,"€":4}"#
    );
    // clé vide en premier, préfixe avant ce qu'il préfixe
    assert_eq!(canon(r#"{"ab":2,"a":1,"":0}"#), r#"{"":0,"a":1,"ab":2}"#);
}

#[test]
fn canonical_keeps_unicode_values_unescaped_and_escapes_controls() {
    assert_eq!(canon(r#"{"k":"héllo ✓ 日本"}"#), r#"{"k":"héllo ✓ 日本"}"#);
    assert_eq!(
        canonical_json(&json!({"k": "\u{1}\t"})),
        "{\"k\":\"\\u0001\\t\"}"
    );
    assert_eq!(canonical_json(&json!({"é\n": 1})), "{\"é\\n\":1}");
}

#[test]
fn canonical_is_idempotent_and_order_independent() {
    let a = canon(r#"{"x":[{"b":1,"a":2}],"a":"é"}"#);
    let b = canon(r#"{"a":"é","x":[{"a":2,"b":1}]}"#);
    assert_eq!(a, b);
    assert_eq!(canon(&a), a);
}

// ---------- sha256_hex / b64url / unb64url ----------

#[test]
fn sha256_hex_known_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    // octets non ASCII : « é » en UTF-8
    assert_eq!(sha256_hex("é".as_bytes()), sha256_hex(&[0xc3, 0xa9]));
    assert_ne!(sha256_hex(&[0xff]), sha256_hex(&[0xfe]));
    assert_eq!(sha256_hex(&[0xff; 100]).len(), 64);
}

#[test]
fn b64url_alphabet_and_padding() {
    assert_eq!(b64url(b""), "");
    assert_eq!(b64url(b"f"), "Zg");
    assert_eq!(b64url(b"fo"), "Zm8");
    assert_eq!(b64url(b"foo"), "Zm9v");
    // 0xfb 0xff -> « +/8= » en base64 standard
    assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    let all: Vec<u8> = (0..=255).collect();
    let s = b64url(&all);
    assert!(!s.contains(['+', '/', '=']));
    assert!(s.contains('-') && s.contains('_'));
}

#[test]
fn unb64url_roundtrip_and_refusals() {
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(unb64url(&b64url(&all)).unwrap(), all);
    assert_eq!(unb64url("").unwrap(), Vec::<u8>::new());
    assert!(unb64url("Zg==").is_err()); // le remplissage est refusé
    assert!(unb64url("-_8=").is_err());
    assert!(unb64url("+/8").is_err()); // alphabet standard refusé
    assert!(unb64url("Z").is_err()); // longueur impossible
    assert!(unb64url("é").is_err());
}

// ---------- Attestation / source_digest ----------

fn att<'a>() -> Attestation<'a> {
    Attestation {
        account: "acc",
        built_at: "2026-01-01T00:00:00Z",
        forge_version: "0.1.0",
        module_name: "courses",
        module_version: "1.2.3",
        source_sha256: "aa",
        toolchain: "rustc 1.90",
        wasm_sha256: "bb",
    }
}

#[test]
fn attestation_canonical_is_sorted_compact_and_nests_module() {
    assert_eq!(
        att().canonical(),
        r#"{"account":"acc","built_at":"2026-01-01T00:00:00Z","forge_version":"0.1.0","module":{"name":"courses","version":"1.2.3"},"source_sha256":"aa","toolchain":"rustc 1.90","wasm_sha256":"bb"}"#
    );
}

#[test]
fn attestation_signature_verifies_and_breaks_on_any_field_change() {
    let (_d, k) = fresh();
    let sig = k.sign(att().canonical().as_bytes());
    verify(&k.public_hex(), att().canonical().as_bytes(), &sig).unwrap();
    let mut other = att();
    other.wasm_sha256 = "bc";
    assert!(verify(&k.public_hex(), other.canonical().as_bytes(), &sig).is_err());
}

#[test]
fn source_digest_is_order_independent_and_content_sensitive() {
    let f = |p: &str, c: &str| (p.to_string(), c.to_string());
    let one = source_digest(&[f("b.rs", "2"), f("a.rs", "1")]);
    let two = source_digest(&[f("a.rs", "1"), f("b.rs", "2")]);
    assert_eq!(one, two);
    assert_eq!(
        one,
        sha256_hex(br#"[{"content":"1","path":"a.rs"},{"content":"2","path":"b.rs"}]"#)
    );
    assert_ne!(one, source_digest(&[f("a.rs", "1"), f("b.rs", "3")]));
    assert_ne!(one, source_digest(&[f("a.rs", "1"), f("c.rs", "2")]));
    assert_eq!(source_digest(&[]), sha256_hex(b"[]"));
}
