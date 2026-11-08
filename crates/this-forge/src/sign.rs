//! Clés du service, JSON canonique et signature de l'artefact.
//!
//! Deux clés Ed25519 distinctes, deux fichiers `0600` : celle qui SIGNE LES ARTEFACTS
//! (épinglée par les appareils) et celle qui signe les JETONS d'accès (jamais publiée).
//! Les mélanger ferait d'un jeton volé une preuve de provenance de code : non.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use jsonwebtoken::{DecodingKey, EncodingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64url(s: &str) -> Result<Vec<u8>> {
    Ok(URL_SAFE_NO_PAD.decode(s)?)
}

/// Les clés du service.
pub struct ForgeKeys {
    signing: SigningKey,
    jwt_enc: EncodingKey,
    jwt_dec: DecodingKey,
}

impl ForgeKeys {
    /// Charge les deux clés, ou les crée (mode 0600, `create_new`) au premier démarrage.
    pub fn load_or_create(signing_path: &Path) -> Result<Self> {
        let signing = load_or_create_key(signing_path)?;
        let jwt_path = jwt_key_path(signing_path);
        let jwt = load_or_create_key(&jwt_path)?;
        let der = jwt.to_pkcs8_der().context("pkcs8")?;
        Ok(Self {
            signing,
            jwt_enc: EncodingKey::from_ed_der(der.as_bytes()),
            jwt_dec: DecodingKey::from_ed_der(jwt.verifying_key().as_bytes()),
        })
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// Clé publique (hex) et empreinte `sha256:<hex>` — la forme servie par `GET /forge/key`.
    pub fn public_hex(&self) -> String {
        hex(self.verifying_key().as_bytes())
    }

    pub fn fingerprint(&self) -> String {
        format!("sha256:{}", sha256_hex(self.verifying_key().as_bytes()))
    }

    /// Signe des octets : la signature est rendue en base64url (sans remplissage).
    pub fn sign(&self, message: &[u8]) -> String {
        b64url(&self.signing.sign(message).to_bytes())
    }

    pub fn jwt_encoding(&self) -> &EncodingKey {
        &self.jwt_enc
    }

    pub fn jwt_decoding(&self) -> &DecodingKey {
        &self.jwt_dec
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn jwt_key_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".jwt");
    PathBuf::from(s)
}

fn load_or_create_key(path: &Path) -> Result<SigningKey> {
    if path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                bail!(
                    "{} est lisible par d'autres que son propriétaire (mode {:o}) : chmod 600",
                    path.display(),
                    mode
                );
            }
        }
        let text = std::fs::read_to_string(path)?;
        let raw = text.trim();
        if raw.len() != 64 {
            bail!(
                "{} : clé illisible (64 caractères hex attendus)",
                path.display()
            );
        }
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            *b = u8::from_str_radix(&raw[2 * i..2 * i + 2], 16)
                .with_context(|| format!("{} : hex invalide", path.display()))?;
        }
        return Ok(SigningKey::from_bytes(&seed));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("création de {}", path.display()))?;
    f.write_all(hex(&seed).as_bytes())?;
    f.write_all(b"\n")?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Vérifie une signature base64url contre une clé publique (hex) : ce que fait l'appareil.
pub fn verify(public_hex: &str, message: &[u8], signature_b64: &str) -> Result<()> {
    if public_hex.len() != 64 {
        bail!("clé publique : 64 caractères hex attendus");
    }
    let mut pk = [0u8; 32];
    for (i, b) in pk.iter_mut().enumerate() {
        *b = u8::from_str_radix(&public_hex[2 * i..2 * i + 2], 16)?;
    }
    let key = VerifyingKey::from_bytes(&pk)?;
    let sig = unb64url(signature_b64)?;
    let sig: [u8; 64] = sig
        .try_into()
        .map_err(|_| anyhow::anyhow!("signature : 64 octets"))?;
    key.verify(message, &ed25519_dalek::Signature::from_bytes(&sig))?;
    Ok(())
}

/// JSON canonique : clés triées (ordre des octets UTF-8), aucun espace, nombres et chaînes
/// comme `serde_json`. C'est la forme exacte qui est signée, et qui est rendue (base64url)
/// dans `X-Forge-Manifest`.
pub fn canonical_json(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                write_canonical(&map[*k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(it, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap()),
    }
}

/// Entrées de la charge signée (§10).
pub struct Attestation<'a> {
    pub account: &'a str,
    pub built_at: &'a str,
    pub forge_version: &'a str,
    pub module_name: &'a str,
    pub module_version: &'a str,
    pub source_sha256: &'a str,
    pub toolchain: &'a str,
    pub wasm_sha256: &'a str,
}

impl Attestation<'_> {
    /// La charge signée, sous sa forme canonique.
    pub fn canonical(&self) -> String {
        canonical_json(&serde_json::json!({
            "account": self.account,
            "built_at": self.built_at,
            "forge_version": self.forge_version,
            "module": {"name": self.module_name, "version": self.module_version},
            "source_sha256": self.source_sha256,
            "toolchain": self.toolchain,
            "wasm_sha256": self.wasm_sha256,
        }))
    }
}

/// Empreinte des sources : sha256 du JSON canonique de `[{content, path}]` trié par chemin.
pub fn source_digest(files: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let arr: Vec<Value> = sorted
        .iter()
        .map(|(p, c)| serde_json::json!({"path": p, "content": c}))
        .collect();
    sha256_hex(canonical_json(&Value::Array(arr)).as_bytes())
}
