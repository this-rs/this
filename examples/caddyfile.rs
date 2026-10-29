//! **Rend le `Caddyfile` délégué** par la configuration de l'application.
//!
//! ```bash
//! cargo run --example caddyfile -- config/server.yaml > /etc/caddy/Caddyfile
//! caddy validate --config /etc/caddy/Caddyfile && caddy reload
//! ```
//!
//! this-rs **décrit**, l'exploitant **applique** : le fichier est du texte, on le
//! relit avant de le poser. C'est la différence entre une délégation et une
//! automatisation silencieuse — et c'est ce qui permet à Caddy de rester le seul
//! à terminer TLS et à décider des redirections.

use this::config::ServerSecurityConfig;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: caddyfile <chemin de config/server.yaml>");
        std::process::exit(2);
    });

    let config = ServerSecurityConfig::from_file(&path)?;
    config.cors.validate()?;
    config.domains.validate()?;

    match config.caddyfile() {
        Some(caddyfile) => {
            print!("{caddyfile}");
            eprintln!(
                "# {} domaine(s) servi(s), upstream {} — relisez puis appliquez.",
                config.domains.allowed.len(),
                config.proxy.caddy.upstream
            );
            Ok(())
        }
        None => {
            anyhow::bail!(
                "aucune délégation Caddy dans {path} : renseignez `proxy.caddy.enabled: true`"
            )
        }
    }
}
