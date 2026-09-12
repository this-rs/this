//! **Micro-harnais de domaines et d'expositions.**
//!
//! Un seul programme pour la chaîne complète :
//!
//! ```text
//! config/server.yaml
//!    ├─ domaines servis ──► PowerDNS   (enregistrements A / AAAA / CNAME)
//!    ├─ CORS / garde ─────► this-rs    (la couche d'entrée)
//!    └─ proxy.caddy ──────► Caddyfile  (TLS, routage, en-têtes)
//! ```
//!
//! # Trois usages
//!
//! ```bash
//! # 1. LIRE : ce qui est déclaré, le plan DNS, le Caddyfile — aucun réseau.
//! cargo run --features powerdns --example domain_harness -- config/server.yaml
//!
//! # 2. PLANIFIER : interroger PowerDNS et montrer l'écart, sans rien écrire.
//! cargo run --features powerdns --example domain_harness -- config/server.yaml --plan-dns
//!
//! # 3. APPLIQUER : écrire l'écart (la clé vient de l'environnement).
//! PDNS_API_KEY=… cargo run --features powerdns --example domain_harness -- \
//!     config/server.yaml --apply-dns
//! ```
//!
//! # Ce que ce harnais ne fait jamais
//!
//! * il n'applique **aucun** changement sans `--apply-dns` ;
//! * il ne touche **aucun** enregistrement hors A/AAAA/CNAME, ni hors de la zone
//!   déclarée ;
//! * il ne supprime rien sans `prune: true` dans la configuration ;
//! * il ne pilote pas Caddy : il rend un `Caddyfile` que l'exploitant relit.

use this::config::ServerSecurityConfig;
use this::dns::{desired_records, plan};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: domain_harness <config/server.yaml> [--plan-dns] [--apply-dns] [--json]");
        std::process::exit(2);
    });
    let flags: Vec<String> = args.collect();
    let plan_dns = flags
        .iter()
        .any(|f| f == "--plan-dns" || f == "--apply-dns");
    let apply_dns = flags.iter().any(|f| f == "--apply-dns");
    let as_json = flags.iter().any(|f| f == "--json");

    let config = ServerSecurityConfig::from_file(&path)?;
    config.cors.validate()?;
    config.domains.validate()?;

    // ── 1. Ce que l'application déclare ──────────────────────────────────
    let sites = &config.domains.sites;
    if !as_json {
        println!("── exposition déclarée ──");
        println!("  hôtes servis      : {}", sites.len());
        for host in sites {
            println!("    - {host}");
        }
        if config.domains.allowed.is_empty() {
            println!("  garde de domaine  : AUCUNE (aucun hôte n'est refusé)");
        } else {
            println!("  garde de domaine  : {:?}", config.domains.allowed);
        }
        println!(
            "  CORS              : {}",
            if config.cors.enabled {
                format!("{} origine(s)", config.cors.origins.len())
            } else {
                "non configuré".to_string()
            }
        );
        println!();
    }

    // ── 2. La délégation à Caddy (texte, jamais appliquée ici) ──────────
    let caddyfile = config.caddyfile();
    if !as_json {
        match &caddyfile {
            Some(text) => {
                println!("── Caddyfile délégué (relisez-le, puis `caddy reload`) ──");
                print!("{text}");
                println!();
            }
            None => println!("── Caddyfile : non demandé (`proxy.caddy.enabled: false`) ──\n"),
        }
    }

    // ── 3. La délégation DNS ────────────────────────────────────────────
    let Some(dns) = &config.dns else {
        if as_json {
            println!(
                "{}",
                serde_json::json!({
                    "sites": sites,
                    "caddyfile": caddyfile,
                    "dns": null,
                })
            );
        } else {
            println!("── DNS : non délégué (pas de section `dns:` dans la configuration) ──");
        }
        return Ok(());
    };

    let desired = desired_records(sites, &dns.record_type, &dns.target, dns.ttl);

    if !plan_dns {
        // Sans accès au serveur : on montre ce qui SERAIT publié.
        if as_json {
            println!(
                "{}",
                serde_json::json!({
                    "sites": sites,
                    "caddyfile": caddyfile,
                    "dns": {
                        "zone": dns.zone_id(),
                        "desired": desired,
                        "applied": false,
                    },
                })
            );
        } else {
            println!(
                "── DNS : {} enregistrement(s) {} vers {} dans {} ──",
                desired.len(),
                dns.record_type,
                dns.target,
                dns.zone_id()
            );
            println!("  (plan non calculé : ajoutez `--plan-dns` pour interroger le serveur)");
        }
        return Ok(());
    }

    // ── 4. Le plan, puis — seulement si demandé — l'application ─────────
    let runtime = tokio::runtime::Runtime::new()?;
    let client = this::dns::PdnsClient::new(dns.clone())?;

    runtime.block_on(async {
        let current = client.current_records().await?;
        let dns_plan = plan(&dns.zone, &desired, &current, dns.prune);

        if !as_json {
            println!("── plan DNS ──");
            println!("  {}", dns_plan.summary());
            for record in &dns_plan.creates {
                println!(
                    "    + {} {} {}",
                    record.name, record.record_type, record.content
                );
            }
            for record in &dns_plan.updates {
                println!(
                    "    ~ {} {} {}",
                    record.name, record.record_type, record.content
                );
            }
            for record in &dns_plan.deletes {
                println!(
                    "    - {} {} {}",
                    record.name, record.record_type, record.content
                );
            }
            for record in &dns_plan.untouched {
                println!(
                    "    = {} {} {} (hors périmètre : jamais touché)",
                    record.name, record.record_type, record.content
                );
            }
            println!();
        }

        if !apply_dns {
            if as_json {
                println!(
                    "{}",
                    serde_json::json!({
                        "caddyfile": caddyfile,
                        "dns": { "zone": dns_plan.zone, "plan": dns_plan, "applied": false },
                    })
                );
            } else {
                println!("  (rien n'a été écrit : ajoutez `--apply-dns` pour appliquer)");
            }
            return Ok::<(), anyhow::Error>(());
        }

        let applied = client.apply(&dns_plan).await?;
        if as_json {
            println!(
                "{}",
                serde_json::json!({
                    "caddyfile": caddyfile,
                    "dns": { "zone": dns_plan.zone, "plan": dns_plan, "applied": applied },
                })
            );
        } else {
            println!(
                "  appliqué : {} remplacé(s), {} supprimé(s), rectifié : {}",
                applied.replaced, applied.deleted, applied.rectified
            );
        }
        Ok(())
    })
}
