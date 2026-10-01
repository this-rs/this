//! this-gateway — LA passerelle IAM devant les stores obrain.
//!
//! Chantier PASSERELLE-THIS-RS, étendu par STS-PERSISTANT. Le relief
//! (`graph_relief`) a prouvé le squelette : un store obrain ouvert en lecture
//! seule, publié sur REST par `ServerBuilder`. La passerelle y ajoute ce qui
//! manquait aux surfaces actuelles (viz :8432, bridges, `--trusted-host`) :
//! **une identité par principal, pas par machine**, et **un refus par
//! défaut**.
//!
//! ```text
//! this-gateway --addr 127.0.0.1:8144 \
//!     --keys /chemin/cles \
//!     --identites /chemin/identites.obrain \
//!     --store kv=/tmp/kv.obrain,tenant=<uuid-A> \
//!     --store demo=/chemin/demo.obrain,tenant=public \
//!     --user alice:secret:<uuid-A>:lecteur          # SEED de bootstrap
//!
//! this-gateway user add bob:mdp:<uuid>:lecteur --identites /chemin/…
//! this-gateway user list --identites /chemin/…
//! this-gateway user passwd bob:nouveau --identites /chemin/…
//! ```
//!
//! La chaîne, de l'extérieur vers l'intérieur :
//!
//! 0. **le cookie devient un en-tête** ([`cookie_vers_en_tete`]) — un
//!    navigateur ne peut pas poser `Authorization` sur une navigation, et
//!    `POST /auth/token` pose donc un cookie `this_token` (`HttpOnly`,
//!    `SameSite=Lax`, `Secure` si `X-Forwarded-Proto: https`). Sans quoi
//!    rattacher un projet existant n'aurait que deux issues : le rendre
//!    public, ou réécrire son authentification ;
//! 1. **authentification** (`WamiAuthProvider`, Ed25519) — un jeton STS émis
//!    par la passerelle elle-même (`POST /auth/token`) devient un
//!    `AuthContext::User { user_id, tenant_id, roles }` ;
//! 2. **la porte** : hors chemins publics (`/health`, `/healthz`, `/auth/`),
//!    pas de principal ⇒ 401 — avec UNE exception délibérée : la lecture
//!    anonyme (GET/HEAD) d'un store déclaré `tenant=public` (voir [`porte`]) ;
//! 3. **la garde de store** : `/stores/{nom}/…` n'est servi que si le tenant
//!    du jeton est propriétaire du store (ou store déclaré `tenant=public`).
//!    Store inconnu ⇒ 404, tenant étranger ⇒ 403 ;
//! 4. **l'autorisation this-rs**, armée dans chaque routeur de store — avec
//!    l'abstention-refus : un chemin sans garde explicite (la faille
//!    4-segments) meurt en 403 ici aussi. Défense en profondeur.
//!
//! # Identités persistées (chantier STS-PERSISTANT)
//!
//! Les utilisateurs vivent dans un **store obrain dédié** (`--identites`,
//! défaut `~/.local/share/this-gateway/identites.obrain`), nœuds `User
//! {username, hash_argon2, tenant_id, roles, created_at}` — mots de passe
//! hachés **argon2id** (voir `this::storage::annuaire_identites`). Le flag
//! `--user` n'est plus qu'un **seed de bootstrap** : au premier démarrage,
//! l'utilisateur absent du store y est créé AVEC hash — le clair n'est
//! jamais persisté — puis **le store fait foi** (un `--user` dont le login
//! existe déjà est ignoré, mot de passe compris).
//!
//! Administration : à chaud par les routes `/auth/users` (rôle `admin`
//! requis), ou hors ligne par `this-gateway user add/list/passwd` — le
//! store d'identités est verrouillé en exclusif par la passerelle en marche,
//! donc la CLI s'utilise passerelle éteinte (le message d'erreur le dit).
//!
//! # Lecture anonyme des stores publics
//!
//! **Choix de sécurité assumé** (cible ffs.space) : un store `tenant=public`
//! est lisible SANS jeton, en GET/HEAD uniquement. Tout le reste — écriture
//! sur un store public, n'importe quel accès à un store à tenant, store
//! inconnu — reste 401 pour un anonyme : on ne renseigne même pas sur
//! l'existence des stores.
//!
//! **Lecture seule d'abord** : chaque store de données est ouvert par
//! `ObrainStore::open_read_only` (verrou partagé — coexiste avec le viz et le
//! relief). Seul le store d'IDENTITÉS est ouvert en écriture (verrou
//! exclusif : la passerelle est l'unique écrivain de ses identités).
//! L'écriture des stores de données attend le Principal natif (RFC
//! principal-natif-v2).
//!
//! # La vue native par tenant (chantier NATIF-J4)
//!
//! Depuis le jalon 4, `/stores/{nom}` ne sert plus LE store : il sert **la
//! vue du principal** — un handle `ObrainDB` confiné par
//! `avec_principal(u32)` (jalon 3 : toutes les primitives de lecture du
//! substrat filtrent, le moteur n'a aucune plomberie). La chaîne :
//!
//! ```text
//! jeton STS (tenant_id: Uuid)
//!   └─> substrate.tenantcat du store (Uuid -> u32, storage::tenantcat)
//!         └─> vue en cache, ou open RO + avec_principal(u32)   (~55 ms
//!             mesurés sur un store de 8 Go, UNE fois par (store, tenant))
//!               └─> le routeur ServerBuilder construit sur CE handle
//! ```
//!
//! * la garde par store (`tenant=` du CLI) reste la maille GROSSIÈRE ; la
//!   vue confinée est la maille FINE, intra-store ;
//! * anonyme, admin, service sans tenant, ou tenant NON interné dans le
//!   catalogue → vue 0 (public seul — `PrincipalSubstrat { 0 }` est légal
//!   et ne voit que `tenant_id == 0`, exactement la vue anonyme) ;
//! * la vue 0 de chaque store est construite AU DÉMARRAGE : elle valide le
//!   format (un store v1/v2 est refusé — le substrat ne migre jamais sous
//!   verrou partagé, la migration est un geste HORS LIGNE) et la passerelle
//!   refuse alors de démarrer : un gateway qui démarre sert tout ce qu'il
//!   déclare, ou rien ;
//! * le cache des vues est borné ([`PLAFOND_VUES`], éviction LRU, vues 0
//!   épinglées) : ~28 fds par vue sur un gros store, à tenir sous
//!   `LimitNOFILE`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::{Path as CheminUrl, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use this::config::auth::WamiConfig;
use this::config::server::GraphExposureConfig;
use this::config::{AuthConfig, ServerSecurityConfig};
use this::core::auth::sts::StsService;
use this::core::auth::wami_provider::WamiAuthProvider;
use this::core::auth::{AuthContext, AuthProvider};
use this::server::ServerBuilder;
use this::server::exposure::rest::auth_routes::{StsState, auth_router};
use this::server::middleware::auth::{AuthLayer, auth_middleware, cookie_vers_en_tete};
use this::storage::annuaire_identites::AnnuaireIdentites;
use this::storage::tenantcat::CatalogueTenants;
use this::storage::{ObrainLinkService, ObrainStore};
use tower::util::ServiceExt;

// ── Déclarations ────────────────────────────────────────────────────────

/// Qui possède un store. **Explicite** : un store sans propriétaire déclaré
/// n'est pas monté (refus par défaut, jusque dans la configuration).
#[derive(Clone)]
enum Propriete {
    /// Lisible par tout principal authentifié **et**, en GET/HEAD seulement,
    /// par l'anonyme (`tenant=public`, dit tel quel — voir [`porte`]).
    Public,
    /// Réservé au tenant propriétaire.
    Tenant(Uuid),
}

struct StoreDecl {
    nom: String,
    chemin: PathBuf,
    propriete: Propriete,
}

struct UserDecl {
    login: String,
    mot_de_passe: String,
    tenant: Uuid,
    roles: Vec<String>,
}

/// La **façade publique** : ce que le monde atteint, et par où.
///
/// Rien n'est inventé : sans `expose:`, le générateur n'écrit aucun artefact de
/// façade, et une exposition incomplète (pas d'`acme_email`, pas de `gateway`)
/// **refuse** la génération — un Caddyfile à moitié décrit est une panne qui
/// attend son heure.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExposeYaml {
    /// Contact ACME — obligatoire dès qu'un hôte est exposé.
    acme_email: String,
    /// Où la façade joint la passerelle (`192.168.1.103:8144`, ou
    /// `127.0.0.1:8144` si un tunnel ssh est généré).
    gateway: String,
    sites: Vec<SiteYaml>,
    /// Le plan DNS des hôtes exposés (aucun enregistrement n'est jamais écrit
    /// par le générateur : il produit le plan, l'exploitant l'applique).
    #[serde(default)]
    dns: Option<DnsYaml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteYaml {
    host: String,
    #[serde(default)]
    aliases: Vec<String>,
    /// Le projet rattaché qui sert la racine du site (il doit être déclaré
    /// dans `services:`).
    #[serde(default)]
    service: Option<String>,
    /// Comment la façade atteint le projet :
    /// * `via_gateway` (défaut) — tout passe par la passerelle, qui garde,
    ///   juge, refuse et transmet : **une seule route**, aucune décision de
    ///   sécurité dans Caddy ;
    /// * `forward_auth` — Caddy joint le projet en direct, mais interroge
    ///   d'abord `/auth/verify` : à réserver aux cas où le proxy de la
    ///   passerelle ne convient pas (gros volumes, protocoles particuliers).
    #[serde(default)]
    mode: Option<String>,
    /// Générer une unité de tunnel ssh vers la passerelle.
    #[serde(default)]
    tunnel: bool,
    /// La cible du tunnel (`triviere@192.168.1.103`).
    #[serde(default)]
    tunnel_host: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DnsYaml {
    /// La cible des enregistrements (l'adresse publique de la façade).
    target: String,
    #[serde(default = "dns_type_par_defaut")]
    record_type: String,
    #[serde(default = "dns_ttl_par_defaut")]
    ttl: u32,
}

fn dns_type_par_defaut() -> String {
    "A".to_string()
}

fn dns_ttl_par_defaut() -> u32 {
    300
}

/// Un site exposé, **résolu**.
struct SiteExpose {
    host: String,
    aliases: Vec<String>,
    service: Option<String>,
    mode: ModeFaçade,
    tunnel: bool,
    tunnel_host: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModeFaçade {
    /// Tout passe par la passerelle (défaut).
    ViaGateway,
    /// Caddy joint le projet en direct, après jugement par `/auth/verify`.
    ForwardAuth,
}

/// La façade résolue.
struct ExposeResolue {
    acme_email: String,
    gateway: String,
    sites: Vec<SiteExpose>,
    dns: Option<DnsYaml>,
}

/// **Le fichier de déclaration** de la passerelle.
///
/// Il remplace la ligne `ExecStart` interminable de l'unité systemd : c'est la
/// structure qu'on relit, qu'on versionne et qu'on fait évoluer — et non un
/// argumentaire qu'on ne peut ni commenter ni différencier.
///
/// ```yaml
/// addr: 192.168.1.103:8144
/// keys: ~/.config/this-gateway/cles
/// identites: ~/.local/share/this-gateway/identites.obrain
/// issuer: obrain-n3
///
/// stores:                      # rattacher une BASE (lecture seule, par tenant)
///   - name: runtime
///     path: ~/.local/share/obrain-harness/data/runtime-dev.obrain
///     tenant: c90599a3-f1b6-43f1-a5c8-03b7b645f378
///   - name: linux
///     path: /tmp/linux-full3.obrain
///     tenant: public           # lisible par l'anonyme, en GET/HEAD seulement
///
/// services:                    # rattacher un PROJET qui existe déjà (HTTP)
///   - name: viz
///     upstream: http://192.168.1.103:8432
///     deny: ["/api/query"]     # refus explicites, avant tout
/// ```
///
/// `deny_unknown_fields` : une clé mal orthographiée **refuse le démarrage** —
/// une faute de frappe ne doit pas produire une garde silencieusement absente.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    #[serde(default)]
    addr: Option<String>,
    #[serde(default)]
    keys: Option<PathBuf>,
    #[serde(default)]
    issuer: Option<String>,
    #[serde(default)]
    identites: Option<PathBuf>,
    #[serde(default)]
    stores: Vec<StoreYaml>,
    #[serde(default)]
    services: Vec<ServiceYaml>,
    /// La façade publique — absente : rien n'est exposé, et aucun artefact de
    /// façade n'est produit.
    #[serde(default)]
    expose: Option<ExposeYaml>,
}

impl Declaration {
    /// Un champ de **chemin** commence-t-il par `~` ?
    ///
    /// Sert à refuser une déclaration `~…` quand `HOME` est absent : garder le
    /// `~` littéral ferait chercher un répertoire nommé `~`, et l'échec serait
    /// attribué au store au lieu du chemin.
    fn cle_a_tilde(&self) -> bool {
        fn tilde(chemin: &PathBuf) -> bool {
            chemin.to_string_lossy().starts_with('~')
        }
        self.keys.as_ref().is_some_and(tilde)
            || self.identites.as_ref().is_some_and(tilde)
            || self.stores.iter().any(|s| tilde(&s.path))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreYaml {
    name: String,
    path: PathBuf,
    tenant: String,
}

/// Un service rattaché, tel qu'il est **déclaré**.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceYaml {
    name: String,
    upstream: String,
    /// **Sans jeton** — un choix qu'on écrit, jamais un défaut.
    #[serde(default)]
    anonymous: bool,
    /// Réserve le service à ce tenant.
    #[serde(default)]
    tenant: Option<String>,
    /// Chemins refusés à travers la passerelle (ex. `/api/query`, l'API de
    /// requête brute du viz — la sortie définitive est de passer par le
    /// gateway gardé).
    #[serde(default)]
    deny: Vec<String>,
    /// Préfixe monté. Défaut : `/services/{name}`.
    #[serde(default)]
    mount: Option<String>,
}

/// Un service rattaché, **résolu** (validé, montable).
struct ServiceDecl {
    nom: String,
    upstream: String,
    mount: String,
    garde: GardeService,
    deny: Vec<String>,
}

/// Qui peut atteindre un service rattaché. **Aucun défaut permissif.**
#[derive(Clone, PartialEq, Eq, Debug)]
enum GardeService {
    /// Jeton requis, n'importe quel principal authentifié.
    Authentifie,
    /// Jeton requis, et le tenant du jeton doit être celui-ci.
    Tenant(Uuid),
    /// Sans jeton, en lecture (GET/HEAD) seulement — `anonymous: true`.
    Anonyme,
}

struct Args {
    addr: String,
    keys_dir: PathBuf,
    issuer: String,
    identites: PathBuf,
    stores: Vec<StoreDecl>,
    services: Vec<ServiceDecl>,
    users: Vec<UserDecl>,
}

/// Le nom du service auquel appartient un chemin, et sa garde.
fn service_du_chemin<'a>(
    gardes: &'a HashMap<String, GardeService>,
    path: &str,
) -> Option<(&'a str, &'a GardeService)> {
    gardes.iter().find_map(|(mount, garde)| {
        (path == mount || path.starts_with(&format!("{mount}/"))).then_some((mount.as_str(), garde))
    })
}

/// Développe un `~` **initial** en `$HOME`, dans les champs de **chemin** de la
/// déclaration (`keys`, `identites`, `stores[].path`).
///
/// # Pourquoi c'est ici, et pas ailleurs
///
/// Le schéma documenté écrit `~/.config/…` (`docs/passerelle-rattachement.md`
/// §1). Le chargeur, lui, prenait le `~` **au pied de la lettre** : un
/// opérateur qui suivait la documentation obtenait cinq `MANQUE` au prévol —
/// et une passerelle qui **refuse de démarrer** (« store déclaré et absent »),
/// sans que rien ne dise que le `~` en était la cause.
///
/// Le développement se fait dans le **seul point de lecture** : le binaire et
/// le prévol (qui génère son script depuis la déclaration parsée) voient donc
/// les mêmes chemins, par construction — un prévol qui divergerait du binaire
/// ne constaterait rien.
///
/// **`~autre` n'est pas deviné** : on ne choisit pas le domicile de quelqu'un
/// d'autre. Sans `HOME`, on **refuse** plutôt que de garder un `~` littéral,
/// qui désignerait un répertoire nommé `~` — l'absence serait alors attribuée
/// au store au lieu du chemin.
fn developper_tilde(decl: &mut Declaration) -> anyhow::Result<()> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        if decl.cle_a_tilde() {
            anyhow::bail!(
                "déclaration : un chemin commence par `~` mais `HOME` n'est pas défini — \
                 écrivez un chemin absolu"
            );
        }
        return Ok(());
    };

    let developper = |chemin: &mut PathBuf| {
        let texte = chemin.to_string_lossy();
        if let Some(reste) = texte.strip_prefix("~/") {
            *chemin = home.join(reste);
        }
    };

    if let Some(keys) = decl.keys.as_mut() {
        developper(keys);
    }
    if let Some(identites) = decl.identites.as_mut() {
        developper(identites);
    }
    for store in &mut decl.stores {
        developper(&mut store.path);
    }
    Ok(())
}

/// Lit et **valide** le fichier de déclaration : une déclaration refusée ne
/// démarre pas, plutôt que de démarrer à moitié.
fn lit_declaration(chemin: &Path) -> anyhow::Result<Declaration> {
    let texte = std::fs::read_to_string(chemin)
        .map_err(|e| anyhow::anyhow!("déclaration illisible {} : {e}", chemin.display()))?;
    let mut decl: Declaration = serde_yaml::from_str(&texte)
        .map_err(|e| anyhow::anyhow!("déclaration invalide {} : {e}", chemin.display()))?;

    // **Avant toute validation** : les contrôles qui suivent parlent de chemins,
    // ils doivent voir ceux que le processus ouvrira vraiment.
    developper_tilde(&mut decl)?;

    let mut noms_stores = std::collections::HashSet::new();
    for store in &decl.stores {
        if store.name.is_empty() {
            anyhow::bail!("déclaration : un store a un `name` vide");
        }
        if !noms_stores.insert(store.name.clone()) {
            anyhow::bail!("déclaration : deux stores portent le nom `{}`", store.name);
        }
    }

    let mut montages = std::collections::HashSet::new();
    for service in &decl.services {
        if service.name.is_empty() {
            anyhow::bail!("déclaration : un service a un `name` vide");
        }
        if !service.upstream.starts_with("http://") && !service.upstream.starts_with("https://") {
            anyhow::bail!(
                "service `{}` : `upstream: {}` doit être une URL http(s)",
                service.name,
                service.upstream
            );
        }
        if service.anonymous && service.tenant.is_some() {
            anyhow::bail!(
                "service `{}` : `anonymous: true` et `tenant:` s'excluent — on ne peut pas être \
                 sans jeton ET réservé à un tenant",
                service.name
            );
        }
        let mount = service
            .mount
            .clone()
            .unwrap_or_else(|| format!("/services/{}", service.name));
        if !mount.starts_with('/') {
            anyhow::bail!(
                "service `{}` : `mount: {mount}` doit commencer par `/`",
                service.name
            );
        }
        if mount.len() > 1 && mount.ends_with('/') {
            anyhow::bail!(
                "service `{}` : `mount: {mount}` ne doit pas finir par `/`",
                service.name
            );
        }
        if ["/", "/health", "/healthz", "/auth", "/stores"].contains(&mount.as_str())
            || mount.starts_with("/auth/")
            || mount.starts_with("/stores/")
        {
            anyhow::bail!(
                "service `{}` : `mount: {mount}` est réservé à la passerelle (sondes, STS, \
                 stores) — choisissez un préfixe",
                service.name
            );
        }
        if !montages.insert(mount.clone()) {
            anyhow::bail!("déclaration : deux services sont montés sur `{mount}`");
        }
        for chemin in &service.deny {
            if !chemin.starts_with('/') {
                anyhow::bail!(
                    "service `{}` : `deny: {chemin}` doit commencer par `/` (chemin absolu du \
                     service rattaché)",
                    service.name
                );
            }
        }
    }

    // ── La façade : une exposition incomplète refuse la génération ──
    if let Some(expose) = &decl.expose {
        if expose.acme_email.trim().is_empty() {
            anyhow::bail!("`expose.acme_email` est vide : ACME a besoin d'un contact");
        }
        if expose.gateway.trim().is_empty() {
            anyhow::bail!(
                "`expose.gateway` est vide : la façade doit savoir où joindre la passerelle"
            );
        }
        if expose.sites.is_empty() {
            anyhow::bail!("`expose.sites` est vide : une façade sans site ne sert rien");
        }
        let mut hotes: std::collections::HashSet<String> = std::collections::HashSet::new();
        for site in &expose.sites {
            if site.host.trim().is_empty() {
                anyhow::bail!("`expose.sites` : un site a un `host` vide");
            }
            if site.host.contains('/') || site.host.contains(':') {
                anyhow::bail!(
                    "`expose.sites` : `host: {}` doit être un nom d'hôte, sans schéma ni chemin",
                    site.host
                );
            }
            for hote in std::iter::once(&site.host).chain(site.aliases.iter()) {
                let hote = hote.to_ascii_lowercase();
                if !hotes.insert(hote.clone()) {
                    anyhow::bail!("`expose.sites` : `{hote}` est déclaré deux fois");
                }
            }
            if let Some(mode) = &site.mode
                && !matches!(mode.as_str(), "via_gateway" | "forward_auth")
            {
                anyhow::bail!(
                    "site `{}` : `mode: {mode}` inconnu — `via_gateway` ou `forward_auth`",
                    site.host
                );
            }
            if let Some(service) = &site.service
                && !decl.services.iter().any(|s| &s.name == service)
            {
                anyhow::bail!(
                    "site `{}` : `service: {service}` n'est pas déclaré dans `services:` — une \
                     façade qui pointe un projet inexistant ne se découvre qu'en production",
                    site.host
                );
            }
            if site.tunnel && site.tunnel_host.is_none() {
                anyhow::bail!(
                    "site `{}` : `tunnel: true` exige `tunnel_host` (ex. `triviere@192.168.1.103`)",
                    site.host
                );
            }
        }
        if let Some(dns) = &expose.dns {
            if !this::dns::MANAGED_TYPES.contains(&dns.record_type.to_ascii_uppercase().as_str()) {
                anyhow::bail!(
                    "`expose.dns.record_type: {}` hors des types gérés ({}) — le générateur \
                     n'écrit jamais un enregistrement qu'il ne saurait pas retirer",
                    dns.record_type,
                    this::dns::MANAGED_TYPES.join(", ")
                );
            }
            if dns.target.trim().is_empty() {
                anyhow::bail!("`expose.dns.target` est vide");
            }
        }
    }

    #[cfg(not(feature = "proxy"))]
    if !decl.services.is_empty() {
        anyhow::bail!(
            "déclaration : {} service(s) rattaché(s), mais ce binaire a été construit sans la \
             feature `proxy` — reconstruisez avec `--features proxy` (le rattachement d'un \
             projet existant est un proxy, pas une lecture de store)",
            decl.services.len()
        );
    }

    Ok(decl)
}

fn decl_store(store: &StoreYaml) -> anyhow::Result<StoreDecl> {
    let propriete = if store.tenant == "public" {
        Propriete::Public
    } else {
        Propriete::Tenant(Uuid::parse_str(&store.tenant).map_err(|e| {
            anyhow::anyhow!(
                "store `{}` : tenant `{}` illisible ({e})",
                store.name,
                store.tenant
            )
        })?)
    };
    Ok(StoreDecl {
        nom: store.name.clone(),
        chemin: store.path.clone(),
        propriete,
    })
}

fn decl_service(service: &ServiceYaml) -> anyhow::Result<ServiceDecl> {
    let garde = if service.anonymous {
        GardeService::Anonyme
    } else if let Some(tenant) = &service.tenant {
        GardeService::Tenant(Uuid::parse_str(tenant).map_err(|e| {
            anyhow::anyhow!(
                "service `{}` : tenant `{tenant}` illisible ({e})",
                service.name
            )
        })?)
    } else {
        GardeService::Authentifie
    };
    Ok(ServiceDecl {
        nom: service.name.clone(),
        upstream: service.upstream.clone(),
        mount: service
            .mount
            .clone()
            .unwrap_or_else(|| format!("/services/{}", service.name)),
        garde,
        deny: service.deny.clone(),
    })
}

fn usage() -> ! {
    eprintln!(
        "usage: this-gateway [--addr HOTE:PORT] --keys DIR \\\n\
         \x20   [--identites CHEMIN.obrain] \\\n\
         \x20   --store nom=chemin,tenant=<uuid|public> [--store …] \\\n\
         \x20   [--user login:motdepasse:<uuid-tenant>[:role1|role2]] [--issuer NOM]\n\
         \n\
         \x20      this-gateway user add login:motdepasse:<uuid-tenant>[:roles] [--identites CHEMIN]\n\
         \x20      this-gateway user list [--identites CHEMIN]\n\
         \x20      this-gateway user passwd login:nouveaumotdepasse [--identites CHEMIN]\n\
         \x20      this-gateway tenantcat add REPERTOIRE-STORE <uuid-tenant> [libelle]\n\
         \x20      this-gateway tenantcat list REPERTOIRE-STORE\n\
         \n\
         --config FICHIER : la DÉCLARATION (stores + services rattachés).
\
         \x20   Elle remplace cette ligne de commande : `--addr`, `--keys`,
\
         \x20   `--issuer`, `--identites`, `stores:` et `services:` y vivent.
\
         \x20   Un `--store`/`--addr`/… donné en plus la complète (le CLI gagne
\
         \x20   sur un scalaire, les stores s'ajoutent).
\
         \x20   `services:` rattache un PROJET EXISTANT par HTTP :
\
         \x20     - name: viz
\
         \x20       upstream: http://192.168.1.103:8432
\
         \x20       deny: [\"/api/query\"]   # refus explicites, avant tout
\
         \x20   Gardé par défaut (jeton requis) ; `anonymous: true` l'ouvre en
\
         \x20   lecture seule, et c'est un choix qu'on écrit. Le projet rattaché
\
         \x20   reçoit x-this-subject / x-this-tenant / x-this-roles VÉRIFIÉS.
\
         --keys DIR : lit DIR/gateway.key.pem + DIR/gateway.pub.pem\n\
         \x20          (générés s'ils manquent — mode d'essai)\n\
         --identites : store obrain des utilisateurs (défaut\n\
         \x20          ~/.local/share/this-gateway/identites.obrain).\n\
         \x20          `--user` n'est qu'un SEED : absent du store il y est créé\n\
         \x20          haché argon2id, présent il est ignoré — le store fait foi.\n\
         Chaque store DOIT déclarer son propriétaire (tenant=…) : la\n\
         passerelle refuse par défaut, jusque dans la configuration.\n\
         tenant=public : lisible par tout authentifié, et par l'anonyme en\n\
         GET/HEAD seulement (choix documenté — cible ffs.space).\n\
         tenantcat : le catalogue Uuid<->u32 DU store (substrate.tenantcat,\n\
         NATIF-J4) — l'attribution est un geste d'exploitant, la passerelle\n\
         le lit au montage (redémarrer après un add)."
    );
    std::process::exit(2);
}

fn chemin_identites_par_defaut() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/this-gateway/identites.obrain")
}

fn parse_store(spec: &str) -> anyhow::Result<StoreDecl> {
    // `nom=chemin,tenant=<uuid|public>` — la virgule est le séparateur de
    // clauses ; le chemin ne doit donc pas en contenir (dit ici).
    let (nom, reste) = spec
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("--store attend nom=chemin,tenant=… : `{spec}`"))?;
    let (chemin, tenant) = reste
        .split_once(",tenant=")
        .ok_or_else(|| anyhow::anyhow!("--store `{nom}` : propriétaire manquant (tenant=<uuid|public>) — un store sans propriétaire n'est pas monté"))?;
    let propriete =
        if tenant == "public" {
            Propriete::Public
        } else {
            Propriete::Tenant(Uuid::parse_str(tenant).map_err(|e| {
                anyhow::anyhow!("--store `{nom}` : tenant `{tenant}` illisible ({e})")
            })?)
        };
    Ok(StoreDecl {
        nom: nom.to_string(),
        chemin: PathBuf::from(chemin),
        propriete,
    })
}

fn parse_user(spec: &str) -> anyhow::Result<UserDecl> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() < 3 {
        anyhow::bail!("--user attend login:motdepasse:<uuid-tenant>[:role1|role2] : `{spec}`");
    }
    Ok(UserDecl {
        login: parts[0].to_string(),
        mot_de_passe: parts[1].to_string(),
        tenant: Uuid::parse_str(parts[2])
            .map_err(|e| anyhow::anyhow!("--user `{}` : tenant illisible ({e})", parts[0]))?,
        roles: parts
            .get(3)
            .map(|r| r.split('|').map(str::to_string).collect())
            .unwrap_or_default(),
    })
}

fn parse_args() -> anyhow::Result<Args> {
    // Les scalaires restent `Option` jusqu'à la fusion : on doit pouvoir
    // distinguer « non donné » (la déclaration décide) de « donné » (le CLI
    // décide). Un défaut posé trop tôt rendrait le fichier inopérant.
    let mut chemin_declaration: Option<PathBuf> = None;
    let mut addr: Option<String> = None;
    let mut keys_dir: Option<PathBuf> = None;
    let mut issuer: Option<String> = None;
    let mut identites: Option<PathBuf> = None;
    let mut stores_cli: Vec<StoreDecl> = Vec::new();
    let mut users: Vec<UserDecl> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => {
                chemin_declaration = Some(PathBuf::from(it.next().unwrap_or_else(|| usage())))
            }
            "--addr" => addr = Some(it.next().unwrap_or_else(|| usage())),
            "--keys" => keys_dir = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--issuer" => issuer = Some(it.next().unwrap_or_else(|| usage())),
            "--identites" => identites = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--store" => stores_cli.push(parse_store(&it.next().unwrap_or_else(|| usage()))?),
            "--user" => users.push(parse_user(&it.next().unwrap_or_else(|| usage()))?),
            _ => usage(),
        }
    }

    let declaration = match &chemin_declaration {
        Some(chemin) => {
            let decl = lit_declaration(chemin)?;
            tracing::info!(declaration = %chemin.display(), "déclaration chargée");
            Some(decl)
        }
        None => None,
    };

    let mut stores: Vec<StoreDecl> = match &declaration {
        Some(decl) => decl
            .stores
            .iter()
            .map(decl_store)
            .collect::<anyhow::Result<_>>()?,
        None => Vec::new(),
    };
    stores.extend(stores_cli);

    let services: Vec<ServiceDecl> = match &declaration {
        Some(decl) => decl
            .services
            .iter()
            .map(decl_service)
            .collect::<anyhow::Result<_>>()?,
        None => Vec::new(),
    };

    let args = Args {
        addr: addr
            .or_else(|| declaration.as_ref().and_then(|d| d.addr.clone()))
            .unwrap_or_else(|| "127.0.0.1:8144".to_string()),
        keys_dir: keys_dir
            .or_else(|| declaration.as_ref().and_then(|d| d.keys.clone()))
            .unwrap_or_default(),
        issuer: issuer
            .or_else(|| declaration.as_ref().and_then(|d| d.issuer.clone()))
            .unwrap_or_else(|| "this-gateway".to_string()),
        identites: identites
            .or_else(|| declaration.as_ref().and_then(|d| d.identites.clone()))
            .unwrap_or_else(chemin_identites_par_defaut),
        stores,
        services,
        users,
    };

    if args.keys_dir.as_os_str().is_empty() {
        eprintln!("aucun répertoire de clés : `--keys DIR` ou `keys:` dans la déclaration");
        usage();
    }
    if args.stores.is_empty() && args.services.is_empty() {
        eprintln!(
            "rien à servir : ni `--store`, ni `stores:`, ni `services:` — une passerelle qui ne \
             sert rien est une erreur de configuration"
        );
        usage();
    }
    Ok(args)
}

/// **Le script de bascule** : les gestes, avec retour arrière ÉCRIT.
///
/// C'est l'acte risqué d'un renommage — deux unités, un redémarrage, des jetons
/// en circulation — réduit à un fichier qu'on relit avant de l'exécuter. Le
/// retour arrière n'est pas décrit : il s'exécute tout seul si la nouvelle
/// unité ne devient pas saine.
///
/// Invariants que le script ne touche **jamais** : les chemins des clés et du
/// store d'identités. Les renommer ferait générer une nouvelle paire Ed25519 et
/// **déconnecterait tout le monde** — un renommage ne doit pas coûter une
/// reconnexion générale.
///
/// Le corps est écrit en **chaîne brute** avec des jetons (`@BIN@`, …) : un
/// script plein de guillemets et d'accolades (`${VAR}`, `%{http_code}`) n'a rien
/// à faire dans un `format!`.
fn script_bascule(
    decl: &Declaration,
    chemin_declaration: &Path,
    binaire: &str,
    unite: &str,
    ancienne: &str,
    sante: &str,
) -> String {
    let corps = r##"# Bascule de la passerelle vers sa déclaration :
#   1. l'unité est installée sous son nouveau nom ;
#   2. l'ANCIENNE EST ARRÊTÉE AVANT que la nouvelle démarre — contrainte mesurée,
#      pas préférence : le store d'identités s'ouvre en verrou EXCLUSIF, donc
#      deux passerelles ne peuvent pas coexister (la nouvelle échouerait à
#      l'ouverture, et le retour arrière se déclencherait à chaque bascule) ;
#   3. la santé de la nouvelle est attendue (jusqu'à @ESSAIS@ essais) ;
#   4. si elle ne répond pas, elle est arrêtée et l'ancienne RELANCÉE — l'état
#      d'avant est RENDU, pas seulement décrit.
#
# Ce script ne touche NI les clés (@CLES@) NI le store d'identités
# (@IDENTITES@) : les renommer ferait générer une nouvelle paire Ed25519 et
# déconnecterait tous les utilisateurs.
#
# Usage : sh basculer.sh          (exécute)
#         sh basculer.sh --sec    (n'exécute rien, affiche ce qu'il ferait)

set -eu

BIN="@BIN@"
DECLARATION="@DECL@"
UNITE="@UNITE@"
ANCIENNE="@ANCIENNE@"
SANTE="@SANTE@"
ESSAIS=@ESSAIS@
BASCULE=0
if [ "${1:-}" = "--sec" ]; then BASCULE=1; fi

dire() { printf '%s\n' "$*"; }
faire() {
  if [ "$BASCULE" = 1 ]; then dire "  [à sec] $*"; return 0; fi
  sh -c "$*"
}

dire "== bascule de la passerelle =="
dire "binaire     : $BIN"
dire "déclaration : $DECLARATION"
dire "unité       : $UNITE"
dire "ancienne    : $ANCIENNE (arrêtée AVANT le démarrage — verrou exclusif du store d'identités)"

if [ ! -x "$BIN" ]; then
  dire "REFUS : $BIN introuvable ou non exécutable — rien n'a été touché."
  exit 2
fi
if [ ! -f "$DECLARATION" ]; then
  dire "REFUS : $DECLARATION introuvable — rien n'a été touché."
  exit 2
fi

UNITES="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
mkdir -p "$UNITES"

# On garde l'unité existante de côté : un retour arrière sans la copie de
# l'ancien fichier n'est pas un retour arrière.
if [ -f "$UNITES/$UNITE" ]; then
  cp "$UNITES/$UNITE" "$UNITES/$UNITE.avant-bascule"
  dire "unité existante conservée dans $UNITE.avant-bascule"
fi

ANCIENNE_TOURNAIT=non
if systemctl --user is-active --quiet "$ANCIENNE" 2>/dev/null; then
  ANCIENNE_TOURNAIT=oui
  dire "l'ancienne unité ($ANCIENNE) tourne : elle sera arrêtée AVANT le démarrage"
fi

# 1. Installer la nouvelle unité (écrite à côté de ce script).
#
# Le répertoire du script est calculé ICI, sur une ligne à part, et non dans la
# commande passée à `faire` : `faire` ré-exécute son argument avec `sh -c`, donc
# une substitution imbriquée (`\"$(dirname \"$0\")\"`) y arrivait **mal
# échappée** — `sh -c` répondait « Unterminated quoted string » et la bascule
# échouait à son premier geste. Un artefact qu'on ne peut pas lancer n'est pas
# déployable. Le test `passerelle_rattachement` vérifie désormais ce script avec
# `sh -n`, ligne par ligne.
ICI=$(dirname "$0")
faire "cp -f \"$ICI/$UNITE\" \"$UNITES/$UNITE\""
faire "systemctl --user daemon-reload"

# 2. ARRÊTER L'ANCIENNE D'ABORD — et c'est une contrainte MESURÉE, pas un choix.
#
# Le store d'identités s'ouvre en verrou EXCLUSIF (obrain : `already open by
# another process … exclusive lock on substrate.meta refused`). Deux passerelles
# ne peuvent donc PAS coexister : démarrer la nouvelle en laissant l'ancienne
# tourner échoue à l'ouverture du store d'identités, la santé ne vient jamais,
# et le script déclenche un retour arrière — à chaque fois.
#
# L'ordre « démarrer, attendre la santé, arrêter ensuite » (celui qu'on voulait
# pour éviter toute coupure) est donc INATTEIGNABLE avec ce verrou. La coupure
# est réelle mais brève : mesuré sur cette machine, la nouvelle unité répond sur
# `/health` en **0,2 s quand les bases sont chaudes, jusqu'à 3,3 s à froid** (mesuré sur cette machine, 4 bases déclarées).
#
# Le retour arrière reste complet : l'ancienne unité est intacte (on n'écrit
# jamais dans SON fichier), et elle est relancée si la nouvelle ne devient pas
# saine.
if [ "$ANCIENNE_TOURNAIT" = oui ]; then
  faire "systemctl --user stop $ANCIENNE"
  dire "ancienne unité arrêtée (verrou exclusif du store d'identités) : bascule en cours"
fi

# 3. Démarrer et attendre qu'elle serve VRAIMENT.
faire "systemctl --user start $UNITE"
sain=non
if [ "$BASCULE" = 1 ]; then
  # À SEC : rien n'a démarré, donc attendre une santé qui ne peut pas venir
  # afficherait un ÉCHEC et un retour arrière qui n'ont pas eu lieu. Un mode
  # d'essai qui crie au loup apprend à ignorer les cris.
  #
  # Le préfixe est `(à sec)`, DISTINCT de `[à sec]` : `[à sec]` annonce une
  # COMMANDE qui serait exécutée (et le test de l'artefact la relit avec
  # `sh -n`), celui-ci annonce une décision. Un préfixe unique pour les deux
  # rendait la sortie impossible à vérifier automatiquement.
  dire "  (à sec) santé non attendue : rien n'a été démarré"
  sain=oui
else
  i=0
  while [ "$i" -lt "$ESSAIS" ]; do
    i=$((i + 1))
    if curl -sf -o /dev/null --max-time 2 "$SANTE/health"; then sain=oui; break; fi
    sleep 1
  done
fi

if [ "$sain" != oui ]; then
  dire "ÉCHEC : $UNITE ne répond pas sur $SANTE/health après $ESSAIS essais."
  faire "systemctl --user stop $UNITE || true"
  if [ "$ANCIENNE_TOURNAIT" = oui ]; then
    dire "retour arrière : l'ancienne unité ($ANCIENNE) est relancée"
    faire "systemctl --user restart $ANCIENNE || true"
  fi
  dire "l'ancienne configuration est intacte : rien n'a été perdu, rien n'a été renommé."
  exit 1
fi

# 4. La nouvelle sert. L'ancienne a déjà été arrêtée (verrou exclusif du store
#    d'identités) : il n'y a plus rien à arrêter ici, et c'est le seul endroit
#    où la bascule diffère de ce qu'on voudrait — une coupure de moins d'une
#    seconde, contre un verrou qui ne se partage pas.
dire "la nouvelle passerelle sert : la bascule est faite"

dire "vérifier maintenant, dans cet ordre :"
dire "  GET $SANTE/health                 -> 200"
dire "  GET $SANTE/services/viz/          -> 401 sans jeton"
dire "  POST $SANTE/auth/token            -> 200 + cookie"
dire "  GET $SANTE/services/viz/ (cookie) -> 200"
dire "retrait : systemctl --user disable --now $UNITE"
"##;

    let corps = corps
        .replace("@BIN@", binaire)
        .replace("@DECL@", &chemin_declaration.display().to_string())
        .replace("@UNITE@", unite)
        .replace("@ANCIENNE@", ancienne)
        .replace("@SANTE@", sante)
        .replace("@ESSAIS@", "15")
        .replace(
            "@CLES@",
            &decl
                .keys
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        )
        .replace(
            "@IDENTITES@",
            &decl
                .identites
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
    // **Le shebang est la PREMIÈRE ligne**, avant l'en-tête : un script dont la
    // ligne 1 est un commentaire ne s'exécute pas directement (`./basculer.sh`),
    // et un artefact qu'on ne peut pas lancer tel quel n'est pas déployable.
    format!("#!/bin/sh\n{ENTETE}{corps}")
}

/// Le runbook : l'ordre des gestes, la vérification, le retrait.
fn runbook(
    decl: &Declaration,
    chemin_declaration: &Path,
    facade: Option<&ExposeResolue>,
    artefacts: &[String],
) -> String {
    let mut texte = String::from(
        "# RUNBOOK — préparation de la passerelle\n\n\
         **État : PRÉPARÉ, RIEN DE DÉPLOYÉ.** Ces artefacts ont été générés ; aucun\n\
         n'est installé, aucun port n'est ouvert, aucun DNS n'est modifié.\n",
    );
    texte.push_str(&format!(
        "\nDéclaration : `{}`\n\n## Ce que la déclaration décrit\n\n| | |\n|---|---|\n| adresse | `{}` |\n",
        chemin_declaration.display(),
        decl.addr.clone().unwrap_or_else(|| "127.0.0.1:8144".to_string()),
    ));
    texte.push_str(&format!(
        "| bases rattachées | {} |\n",
        if decl.stores.is_empty() {
            "aucune".to_string()
        } else {
            decl.stores
                .iter()
                .map(|s| format!("`{}` ({})", s.name, s.tenant))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    texte.push_str(&format!(
        "| projets rattachés | {} |\n",
        if decl.services.is_empty() {
            "aucun".to_string()
        } else {
            decl.services
                .iter()
                .map(|s| {
                    let garde = if s.anonymous {
                        "sans jeton, lecture seule".to_string()
                    } else if let Some(tenant) = &s.tenant {
                        format!("tenant {tenant}")
                    } else {
                        "jeton requis".to_string()
                    };
                    let refus = if s.deny.is_empty() {
                        String::new()
                    } else {
                        format!(" — refus {}", s.deny.join(", "))
                    };
                    format!("`{}` → {garde}{refus}", s.name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    if let Some(facade) = facade {
        texte.push_str(&format!(
            "| façade | {} |\n",
            facade
                .sites
                .iter()
                .map(|s| format!(
                    "`{}` → {} ({}{})",
                    s.host,
                    s.service
                        .clone()
                        .unwrap_or_else(|| "passerelle seule".to_string()),
                    match s.mode {
                        ModeFaçade::ViaGateway => "via_gateway",
                        ModeFaçade::ForwardAuth => "forward_auth",
                    },
                    if s.tunnel { ", tunnel" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    texte.push_str("\n## Artefacts\n\n");
    for artefact in artefacts {
        texte.push_str(&format!("- `{artefact}`\n"));
    }

    texte.push_str(
        "\n## Ordre des gestes — l'ordre compte\n\n\
         1. **Le binaire et la déclaration.** Copier la déclaration sur la machine de\n\
         \x20  la passerelle, et vérifier le chemin du binaire dans `ExecStart`\n\
         \x20  (l'unité porte un chemin par défaut : il peut ne pas être le vôtre).\n\
         1b. **La bascule, si vous la voulez automatique** : `sh basculer.sh --sec` pour\n\
         \x20  relire ce qu'il ferait, puis `sh basculer.sh`. Il installe l'unité, **arrête\n\
         \x20  l'ancienne** (le store d'identités s'ouvre en verrou exclusif : deux\n\
         \x20  passerelles ne peuvent pas coexister), démarre la nouvelle, attend la santé,\n\
         \x20  et **relance l'ancienne si la nouvelle ne répond pas** — il ne touche ni les\n\
         \x20  clés ni le store d'identités. La coupure dure moins d'une seconde (la\n\
         \x20  nouvelle répond sur `/health` en 0,2 s (bases chaudes) à 3,3 s (froid) — mesuré).\n\
         \x20  Les étapes 2 à 5 ci-dessous restent la version manuelle.\n\
         2. **L'unité** : `this-gateway.service` dans `~/.config/systemd/user/`, puis\n\
         \x20  `systemctl --user daemon-reload && systemctl --user enable --now this-gateway`.\n\
         \x20  Prévoir `loginctl enable-linger <utilisateur>` pour la survie hors session.\n\
         3. **Le tunnel**, s'il est déclaré : même dépôt dans `~/.config/systemd/user/`.\n\
         \x20  La clé ssh doit être autorisée côté cible et testée en `BatchMode` *avant*\n\
         \x20  d'activer : un tunnel sans clé meurt en boucle.\n\
         4. **Le DNS** : appliquer `dns.txt` (types gérés uniquement).\n\
         5. **La façade** : ajouter le bloc au Caddyfile existant, `caddy validate`,\n\
         \x20  puis `caddy reload` (sans coupure).\n\
         6. **Vérifier** — section suivante, avec les codes attendus.\n",
    );

    texte.push_str(
        "\n## Vérification\n\n\
         ```sh\n\
         # 1. La passerelle vit, et ne sert que ce qu'elle déclare.\n\
         curl -s -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:8144/health        # 200\n\
         curl -s -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:8144/stores/inconnu/graph  # 401 (existence non révélée)\n\n\
         # 2. Un projet rattaché est GARDÉ : sans jeton, refus.\n\
         curl -s -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:8144/services/viz/  # 401\n\n\
         # 3. Avec un jeton — par cookie, comme le fait une page.\n\
         curl -s -c /tmp/c.txt -X POST http://127.0.0.1:8144/auth/token \\\n\
         \x20    -H 'content-type: application/json' \\\n\
         \x20    -d '{\"username\":\"…\",\"password\":\"…\"}' > /dev/null\n\
         curl -s -b /tmp/c.txt -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:8144/services/viz/  # 200\n\n\
         # 4. Les refus déclarés, et leur portée.\n\
         curl -s -b /tmp/c.txt -o /dev/null -w '%{http_code}\\n' \\\n\
         \x20    -X POST http://127.0.0.1:8144/services/viz/api/query  # 403\n\
         curl -s -b /tmp/c.txt -o /dev/null -w '%{http_code}\\n' \\\n\
         \x20    http://127.0.0.1:8144/services/viz/api/querylog       # 200\n\n\
         # 5. L'identité ARRIVE au projet rattaché (en-têtes vérifiés).\n\
         curl -s -b /tmp/c.txt -D - -o /dev/null http://127.0.0.1:8144/services/viz/ | grep -i x-this\n\n\
         # 6. La PORTE que la façade interroge (mode `forward_auth`) :\n\
         #    `/auth/verify` répond 401 sans jeton, 200 avec — c'est ce qui\n\
         #    permet à Caddy de garder un projet qu'on n'a pas réécrit.\n\
         curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8144/auth/verify                 # 401\n\
         curl -s -b /tmp/c.txt -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8144/auth/verify  # 200\n\
         ```\n",
    );

    texte.push_str(
        "\n## Retrait\n\n\
         1. **La façade d'abord** : retirer le bloc du Caddyfile, `caddy reload` — c'est\n\
         \x20  le geste qui coupe l'accès public, immédiatement. Le DNS, lui, n'agit\n\
         \x20  qu'après expiration du TTL.\n\
         2. `systemctl --user disable --now this-gateway` (et les tunnels déclarés).\n\
         3. Les enregistrements DNS, si nécessaire.\n\
         4. Aucun état n'est créé chez un projet rattaché par la façade : le retrait\n\
         \x20  ne laisse rien à nettoyer de son côté.\n",
    );

    texte.push_str(
        "\n## Ce que ces artefacts ne font pas\n\n\
         - **Ils n'appliquent rien** : ni unité installée, ni DNS écrit, ni Caddy\n\
         \x20 rechargé. Chaque geste est un acte d'exploitant, relu avant d'être fait.\n\
         - **Ils ne réécrivent pas les projets rattachés** : un service est joint par\n\
         \x20 HTTP et remis sous garde. En contrepartie, un projet rattaché ne doit\n\
         \x20 être joignable **que** par la passerelle — les en-têtes `x-this-*`\n\
         \x20 n'ont de valeur que parce qu'elle les a vérifiés.\n\
         - **Ils n'ouvrent aucune route d'entités** : les routes CRUD viennent d'un\n\
         \x20 descripteur Rust, pas d'une déclaration.\n",
    );
    texte
}

/// **Le prévol** : ce qui doit être VRAI pour que le déploiement réussisse.
///
/// Il ne modifie **rien** : il constate, sur la machine cible (ou en local), les
/// faits qui décident — et il rend ces faits avec la commande qui les prouve,
/// comme le veut la convention du dépôt. Deux constats méritent d'être lus
/// avant tous les autres :
///
/// * **les clés** : absentes, un redémarrage génère une nouvelle paire et
///   **invalide tous les jetons émis** (tout le monde est déconnecté) ;
/// * **le store d'identités** : absent, tous les comptes sont à recréer — les
///   utilisateurs ne peuvent plus se connecter.
///
/// Le script est **le même** en local et à distance : une seule vérité, et la
/// distance ne change pas ce qu'on vérifie.
fn script_prevol(decl: &Declaration, binaire: Option<&str>, unite: &str) -> String {
    let mut l = String::new();
    l.push_str("set -u\nligne() { printf '%s|%s|%s\\n' \"$1\" \"$2\" \"$3\"; }\n");

    let bin = binaire
        .map(str::to_string)
        .unwrap_or_else(binaire_par_defaut);
    l.push_str(&format!(
        "BIN={bin:?}\nif [ -x \"$BIN\" ]; then ligne binaire OK \"$BIN\"; else ligne binaire MANQUE \"$BIN introuvable ou non exécutable\"; fi\n"
    ));

    let cles = decl
        .keys
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    l.push_str(&format!(
        "CLES={cles:?}\n\
         if [ -f \"$CLES/gateway.key.pem\" ] && [ -f \"$CLES/gateway.pub.pem\" ]; then \
         ligne cles OK \"les deux PEM sont là : les jetons émis survivent au redémarrage\"; \
         else ligne cles MANQUE \"$CLES : clé absente — un redémarrage GÉNÉRERAIT une nouvelle paire et INVALIDERAIT tous les jetons émis\"; fi\n"
    ));

    let identites = decl
        .identites
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    l.push_str(&format!(
        "ID={identites:?}\n\
         if [ -d \"$ID\" ]; then ligne identites OK \"$ID\"; \
         else ligne identites MANQUE \"$ID : store d'identités absent — les comptes seraient à recréer (seed), plus personne ne se connecte\"; fi\n"
    ));

    for store in &decl.stores {
        let chemin = store.path.display().to_string();
        l.push_str(&format!(
            "S={chemin:?}\n\
             if [ -d \"$S\" ]; then ligne \"store:{nom}\" OK \"$S\"; \
             else ligne \"store:{nom}\" MANQUE \"$S : store déclaré et absent — la passerelle refuse de démarrer (elle sert tout ce qu'elle déclare, ou rien)\"; fi\n",
            nom = store.name
        ));
    }

    let addr = decl
        .addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:8144".to_string());
    let port = addr.rsplit(':').next().unwrap_or("8144").to_string();
    l.push_str(&format!(
        "PORT={port:?}\n\
         # Motif ancré en FIN d'adresse : un motif avec un espace ne matchait rien,\n\
         # et le prévol annonçait « port libre » devant un service en marche —\n\
         # un constat faux vaut moins que pas de constat du tout.\n\
         TENEUR=$(ss -tlnp 2>/dev/null | grep -E \":${{PORT}}[[:space:]]\" | head -1)\n\
         if [ -z \"$TENEUR\" ]; then ligne port OK \"$PORT libre : première installation\"; \
         elif echo \"$TENEUR\" | grep -qE 'this(-rs)?-gateway'; then \
         \x20 if echo \"$TENEUR\" | grep -q 'this-rs-gateway'; then \
         \x20\x20 ligne port OK \"$PORT tenu par la passerelle sous son ANCIEN nom (this-rs-gateway) : REDÉMARRAGE, interruption de quelques secondes\"; \
         \x20 else ligne port OK \"$PORT déjà tenu par la passerelle : REDÉMARRAGE, interruption de quelques secondes\"; fi; \
         else ligne port MANQUE \"$PORT tenu par autre chose : $(echo \"$TENEUR\" | head -c 120)\"; fi\n"
    ));

    // Le nom d'unité est **paramétrable** (`--unite`) et l'ancien nom est
    // détecté : pendant un renommage, un prévol qui ne regarde que le nouveau
    // nom annonce « première installation » devant un service en marche.
    l.push_str(&format!(
        "UNITE={unite:?}\n\
         if systemctl --user is-active --quiet \"$UNITE\" 2>/dev/null; then\n\
         \x20 ACTUELLE=$(systemctl --user show \"$UNITE\" -p ExecStart --value 2>/dev/null | head -c 300)\n\
         \x20 case \"$ACTUELLE\" in \
         *--config*) ligne unite OK \"$UNITE active, déjà pilotée par une déclaration\";; \
         *) ligne unite ATTENTION \"$UNITE active, encore pilotée par la ligne de commande — ce redémarrage bascule sur la déclaration\";; \
         esac\n\
         else\n\
         \x20 if systemctl --user is-active --quiet this-rs-gateway 2>/dev/null; then\n\
         \x20\x20 ligne unite ATTENTION \"l'unité ACTUELLE s'appelle encore this-rs-gateway (nom d'avant) : la bascule renomme, et l'ancienne doit être arrêtée après démarrage de la nouvelle\"\n\
         \x20 else ligne unite ATTENTION \"$UNITE absente et inactive : première installation\"; fi\n\
         fi\n"
    ));

    for service in &decl.services {
        l.push_str(&format!(
            "CODE=$(curl -s -o /dev/null -w '%{{http_code}}' --max-time 4 {upstream:?} 2>/dev/null || echo 000)\n\
             if [ \"$CODE\" = \"000\" ]; then ligne \"service:{nom}\" MANQUE \"{upstream} injoignable depuis cette machine\"; \
             else ligne \"service:{nom}\" OK \"{upstream} répond $CODE\"; fi\n",
            nom = service.name,
            upstream = service.upstream
        ));
    }

    l.push_str(
        "if command -v cargo >/dev/null 2>&1; then ligne chaine OK \"cargo présent : le binaire peut être construit ici\"; \
         else ligne chaine ATTENTION \"cargo absent : construire ailleurs puis copier le binaire\"; fi\n\
         OBRAIN=$(ls -d \"$HOME\"/projects/banc-obrain-j4 2>/dev/null | head -1)\n\
         if [ -n \"$OBRAIN\" ]; then ligne moteur OK \"$OBRAIN\"; \
         else ligne moteur ATTENTION \"checkout obrain-j4 absent : compiler à côté, ou adapter le chemin dans Cargo.toml\"; fi\n",
    );

    if let Some(expose) = &decl.expose {
        for site in &expose.sites {
            if let Some(cible) = &site.tunnel_host {
                l.push_str(&format!(
                    "if ssh -o BatchMode=yes -o ConnectTimeout=5 {cible:?} true 2>/dev/null; then \
                     ligne \"tunnel:{host}\" OK \"{cible} joignable sans mot de passe\"; \
                     else ligne \"tunnel:{host}\" MANQUE \"{cible} refuse la clé : un tunnel sans clé meurt en boucle\"; fi\n",
                    host = site.host
                ));
            }
        }
    }
    l
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Verdict {
    Ok,
    Attention,
    Manque,
}

/// **`this-gateway prevol`** — constater, sans rien modifier.
fn prevol(brut: &[String]) -> anyhow::Result<()> {
    let mut chemin_config: Option<PathBuf> = None;
    let mut cible: Option<String> = None;
    let mut binaire: Option<String> = None;
    let mut unite = "this-gateway".to_string();
    let mut en_json = false;

    let mut it = brut.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => chemin_config = it.next().map(PathBuf::from),
            "--ssh" => cible = it.next().cloned(),
            "--bin" => binaire = it.next().cloned(),
            "--unite" => unite = it.next().cloned().unwrap_or(unite),
            "--json" => en_json = true,
            _ => {
                eprintln!(
                    "usage: this-gateway prevol --config FICHIER [--ssh user@hote] [--bin CHEMIN] [--unite NOM] [--json]"
                );
                std::process::exit(2);
            }
        }
    }
    let Some(chemin_config) = chemin_config else {
        eprintln!("`prevol` exige `--config FICHIER`");
        std::process::exit(2);
    };
    let decl = lit_declaration(&chemin_config)?;
    let script = script_prevol(&decl, binaire.as_deref(), &unite);

    // Un seul chemin de code : le script tourne en local (`bash -s`) ou sur la
    // cible (`ssh … bash -s`). La distance ne change pas ce qu'on vérifie.
    let sortie = match &cible {
        Some(cible) => {
            let mut enfant = std::process::Command::new("ssh")
                .args([
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=6",
                    cible,
                    "bash",
                    "-s",
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| anyhow::anyhow!("ssh {cible} : {e}"))?;
            {
                use std::io::Write;
                let stdin = enfant
                    .stdin
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("entrée de ssh indisponible"))?;
                stdin.write_all(script.as_bytes())?;
            }
            enfant
                .wait_with_output()
                .map_err(|e| anyhow::anyhow!("ssh {cible} : {e}"))?
        }
        None => {
            let mut enfant = std::process::Command::new("bash")
                .arg("-s")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| anyhow::anyhow!("bash : {e}"))?;
            {
                use std::io::Write;
                let stdin = enfant
                    .stdin
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("entrée de bash indisponible"))?;
                stdin.write_all(script.as_bytes())?;
            }
            enfant
                .wait_with_output()
                .map_err(|e| anyhow::anyhow!("bash : {e}"))?
        }
    };

    let mut constats: Vec<(String, Verdict, String)> = Vec::new();
    for ligne in String::from_utf8_lossy(&sortie.stdout).lines() {
        let mut champs = ligne.splitn(3, '|');
        let (Some(nom), Some(verdict), Some(preuve)) =
            (champs.next(), champs.next(), champs.next())
        else {
            continue;
        };
        let verdict = match verdict {
            "OK" => Verdict::Ok,
            "ATTENTION" => Verdict::Attention,
            _ => Verdict::Manque,
        };
        constats.push((nom.to_string(), verdict, preuve.to_string()));
    }

    if constats.is_empty() {
        anyhow::bail!(
            "le prévol n'a rien constaté — {}",
            String::from_utf8_lossy(&sortie.stderr).trim()
        );
    }

    if en_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "cible": cible,
                "constats": constats.iter().map(|(nom, verdict, preuve)| json!({
                    "constat": nom,
                    "verdict": match verdict { Verdict::Ok => "ok", Verdict::Attention => "attention", Verdict::Manque => "manque" },
                    "preuve": preuve,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!(
            "prévol — {}",
            cible.clone().unwrap_or_else(|| "cette machine".to_string())
        );
        println!("{:<22} {:<10} preuve", "constat", "verdict");
        for (nom, verdict, preuve) in &constats {
            println!(
                "{nom:<22} {:<10} {preuve}",
                match verdict {
                    Verdict::Ok => "OK",
                    Verdict::Attention => "ATTENTION",
                    Verdict::Manque => "MANQUE",
                }
            );
        }
    }

    let manques = constats
        .iter()
        .filter(|(_, verdict, _)| matches!(verdict, Verdict::Manque))
        .count();
    if manques > 0 {
        eprintln!(
            "\n{manques} constat(s) MANQUE : déployer maintenant échouerait ou casserait un service en marche."
        );
        std::process::exit(1);
    }
    Ok(())
}

/// Le binaire de la passerelle, **par défaut**, tel qu'il sera écrit dans les
/// artefacts.
///
/// Il est **absolu** (résolu depuis `HOME`), et non un spécificateur systemd :
/// un `%h` n'est développé que par systemd — dans `basculer.sh` et dans le
/// script du prévol, qui sont du **shell**, il restait littéral. Le contrôle
/// `[ -x "$BIN" ]` portait donc sur un chemin nommé `%h/…`, qui n'existe pas :
/// la bascule refusait au premier geste (« rien n'a été touché »), et le prévol
/// annonçait `binaire MANQUE` sur une machine où le binaire était là.
///
/// Sans `HOME`, on retombe sur le littéral `%h` : le constat échouera, ce qui
/// est la vérité — sans `HOME`, ni le shell ni l'unité ne peuvent le résoudre.
fn binaire_par_defaut() -> String {
    match std::env::var("HOME") {
        Ok(home) => format!("{home}/projects/this-rs/target/release/this-gateway"),
        Err(_) => "%h/projects/this-rs/target/release/this-gateway".to_string(),
    }
}

/// **`this-gateway plan`** — le plan de déploiement et ses artefacts.
fn plan_deploiement(brut: &[String]) -> anyhow::Result<()> {
    let mut chemin_config: Option<PathBuf> = None;
    let mut sortie: Option<PathBuf> = None;
    let mut binaire = binaire_par_defaut();
    let mut en_json = false;

    let mut it = brut.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => chemin_config = it.next().map(PathBuf::from),
            "--out" => sortie = it.next().map(PathBuf::from),
            "--bin" => binaire = it.next().cloned().unwrap_or_else(|| binaire.clone()),
            "--json" => en_json = true,
            _ => {
                eprintln!(
                    "usage: this-gateway plan --config FICHIER [--out DIR] [--bin CHEMIN] [--json]"
                );
                std::process::exit(2);
            }
        }
    }
    let Some(chemin_config) = chemin_config else {
        eprintln!("`plan` exige `--config FICHIER`");
        std::process::exit(2);
    };

    let decl = lit_declaration(&chemin_config)?;
    let facade = decl.expose.as_ref().map(resout_facade);
    // Les services **résolus** : mêmes règles que celles appliquées au service
    // (garde, montage), pour que la façade et le runtime ne divergent pas.
    let services: Vec<ServiceDecl> = decl
        .services
        .iter()
        .map(decl_service)
        .collect::<anyhow::Result<_>>()?;

    // ── Le plan, lisible par un opérateur ──
    let mut plan = serde_json::Map::new();
    plan.insert(
        "declaration".into(),
        json!(chemin_config.display().to_string()),
    );
    plan.insert(
        "addr".into(),
        json!(decl.addr.clone().unwrap_or_else(|| "127.0.0.1:8144".into())),
    );
    plan.insert(
        "stores".into(),
        json!(decl
            .stores
            .iter()
            .map(|s| json!({"name": s.name, "path": s.path.display().to_string(), "tenant": s.tenant}))
            .collect::<Vec<_>>()),
    );
    plan.insert(
        "services".into(),
        json!(decl
            .services
            .iter()
            .map(|s| json!({
                "name": s.name,
                "upstream": s.upstream,
                "mount": s.mount.clone().unwrap_or_else(|| format!("/services/{}", s.name)),
                "guard": if s.anonymous { "anonymous" } else if s.tenant.is_some() { "tenant" } else { "authenticated" },
                "deny": s.deny,
            }))
            .collect::<Vec<_>>()),
    );
    if let Some(facade) = &facade {
        plan.insert(
            "expose".into(),
            json!(facade
                .sites
                .iter()
                .map(|s| json!({
                    "host": s.host,
                    "aliases": s.aliases,
                    "service": s.service,
                    "mode": match s.mode { ModeFaçade::ViaGateway => "via_gateway", ModeFaçade::ForwardAuth => "forward_auth" },
                    "tunnel": s.tunnel,
                }))
                .collect::<Vec<_>>()),
        );
    }
    plan.insert("deployed".into(), json!(false));

    // ── Les artefacts ──
    let mut artefacts: Vec<String> = Vec::new();
    if let Some(dir) = &sortie {
        std::fs::create_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("création de {} : {e}", dir.display()))?;
        let ecrit = |chemin: PathBuf, contenu: String| -> anyhow::Result<()> {
            if let Some(parent) = chemin.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&chemin, contenu)
                .map_err(|e| anyhow::anyhow!("écriture de {} : {e}", chemin.display()))?;
            Ok(())
        };

        let unite = dir.join("this-gateway.service");
        ecrit(unite.clone(), unite_systemd(&chemin_config, &binaire))?;
        artefacts.push("this-gateway.service".to_string());

        // Le script de bascule : les gestes, le retour arrière, et les mêmes
        // invariants que le reste (clés et identités intouchées).
        let bascule = dir.join("basculer.sh");
        ecrit(
            bascule.clone(),
            script_bascule(
                &decl,
                &chemin_config,
                &binaire,
                "this-gateway.service",
                "this-rs-gateway.service",
                &format!(
                    "http://{}",
                    decl.addr
                        .clone()
                        .unwrap_or_else(|| "127.0.0.1:8144".to_string())
                ),
            ),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bascule, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| anyhow::anyhow!("chmod de {} : {e}", bascule.display()))?;
        }
        artefacts.push("basculer.sh".to_string());

        if let Some(facade) = &facade {
            let caddyfile = dir.join("caddy").join("Caddyfile");
            ecrit(caddyfile, caddyfile_hote(facade, &services))?;
            artefacts.push("caddy/Caddyfile".to_string());

            for site in &facade.sites {
                let bloc = dir.join("caddy").join(format!("{}.caddy", site.host));
                ecrit(
                    bloc,
                    format!(
                        "{}# ── Bloc à AJOUTER au Caddyfile existant de la façade ──\n{}",
                        ENTETE,
                        bloc_caddy(site, facade, &services)
                    ),
                )?;
                artefacts.push(format!("caddy/{}.caddy", site.host));

                if site.tunnel {
                    let nom = nom_tunnel(&site.host);
                    ecrit(
                        dir.join("systemd").join(&nom),
                        unite_tunnel(site, &facade.gateway),
                    )?;
                    artefacts.push(format!("systemd/{nom}"));
                }
            }

            let dns = dir.join("dns.txt");
            ecrit(dns, plan_dns(facade))?;
            artefacts.push("dns.txt".to_string());
        }

        let texte_runbook = runbook(&decl, &chemin_config, facade.as_ref(), &artefacts);
        ecrit(dir.join("RUNBOOK.md"), texte_runbook)?;
        artefacts.push("RUNBOOK.md".to_string());
    }

    if en_json {
        plan.insert("artefacts".into(), json!(artefacts));
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!(
            "structure `this-gateway` — {}",
            plan["addr"].as_str().unwrap_or("")
        );
        println!("  déclaration : {}", chemin_config.display());
        println!(
            "  bases       : {}",
            plan["stores"].as_array().map(Vec::len).unwrap_or(0)
        );
        println!(
            "  projets     : {}",
            plan["services"].as_array().map(Vec::len).unwrap_or(0)
        );
        if let Some(facade) = &facade {
            for site in &facade.sites {
                println!(
                    "  façade      : {} → {} ({}{})",
                    site.host,
                    site.service.clone().unwrap_or_else(|| "passerelle".into()),
                    match site.mode {
                        ModeFaçade::ViaGateway => "via_gateway",
                        ModeFaçade::ForwardAuth => "forward_auth",
                    },
                    if site.tunnel { ", tunnel" } else { "" }
                );
            }
        }
        if !artefacts.is_empty() {
            println!("  artefacts   : {} fichiers", artefacts.len());
            for artefact in &artefacts {
                println!("    · {artefact}");
            }
        } else {
            println!("  artefacts   : aucun (`--out DIR` pour les écrire)");
        }
        println!("  état        : PRÉPARÉ, RIEN DE DÉPLOYÉ — voir RUNBOOK.md");
    }
    Ok(())
}

// ── Administration hors ligne : `this-gateway user …` ───────────────

/// Les sous-commandes `user add/list/passwd`, sur le store d'identités.
///
/// Le store est ouvert en **exclusif** : si la passerelle tourne, l'ouverture
/// échoue au verrou — c'est voulu (un seul écrivain d'identités). Pour gérer
/// sans arrêter la passerelle, passer par ses routes HTTP `/auth/users`
/// (rôle `admin` requis).
fn administre_utilisateurs(args: &[String]) -> anyhow::Result<()> {
    let mut chemin = chemin_identites_par_defaut();
    let mut positionnels: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--identites" {
            chemin = PathBuf::from(
                it.next()
                    .ok_or_else(|| anyhow::anyhow!("--identites attend un chemin"))?,
            );
        } else {
            positionnels.push(a);
        }
    }

    let annuaire = AnnuaireIdentites::ouvrir(&chemin).map_err(|e| {
        anyhow::anyhow!(
            "ouverture de l'annuaire {} : {e}\n\
             (la passerelle en marche détient le verrou exclusif — arrêtez-la,\n\
             ou administrez à chaud via POST /auth/users avec un jeton admin)",
            chemin.display()
        )
    })?;

    match positionnels.first().map(|s| s.as_str()) {
        Some("add") => {
            let spec = positionnels.get(1).ok_or_else(|| {
                anyhow::anyhow!("user add attend login:motdepasse:<uuid-tenant>[:roles]")
            })?;
            let decl = parse_user(spec)?;
            let cree =
                annuaire.ajouter(&decl.login, &decl.mot_de_passe, decl.tenant, decl.roles)?;
            println!(
                "cree: {} tenant={} roles={} (hash argon2id persiste, clair jamais)",
                cree.username,
                cree.tenant_id,
                cree.roles.join("|")
            );
        }
        Some("list") => {
            for u in annuaire.lister()? {
                println!(
                    "{}\ttenant={}\troles={}\tcree={}\t{}",
                    u.username,
                    u.tenant_id,
                    u.roles.join("|"),
                    u.created_at,
                    u.hash_argon2
                );
            }
        }
        Some("passwd") => {
            let spec = positionnels
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("user passwd attend login:nouveaumotdepasse"))?;
            let (login, nouveau) = spec
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("user passwd attend login:nouveaumotdepasse"))?;
            annuaire.changer_mot_de_passe(login, nouveau)?;
            println!("mot de passe de {login} remplace (hash argon2id)");
        }
        _ => usage(),
    }
    Ok(())
}

// ── Administration hors ligne : `this-gateway tenantcat …` ──────────

/// Les sous-commandes `tenantcat add/list` : l'attribution Uuid ↔ u32 d'un
/// store de données (NATIF-J4). Hors requête, hors passerelle — celle-ci lit
/// le catalogue AU MONTAGE : après un `add`, la redémarrer pour qu'elle le
/// voie. Le fichier est append-only, l'écrire ne dérange pas un lecteur RO
/// en marche, mais il ne le verra pas avant son redémarrage.
fn administre_tenantcat(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("add") => {
            let dir = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("tenantcat add REPERTOIRE-STORE <uuid> [libelle]")
            })?;
            let uuid: Uuid = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("tenantcat add REPERTOIRE-STORE <uuid> [libelle]"))?
                .parse()
                .map_err(|e| anyhow::anyhow!("uuid illisible : {e}"))?;
            let libelle = args.get(3).map(String::as_str).unwrap_or("");
            let entree = CatalogueTenants::attribuer(Path::new(dir), uuid, libelle)?;
            println!(
                "interne: {} -> {} dans {}/{} ({})",
                entree.uuid,
                entree.numero,
                dir,
                this::storage::tenantcat::FICHIER_TENANTCAT,
                if entree.libelle.is_empty() {
                    "sans libelle"
                } else {
                    &entree.libelle
                }
            );
            println!(
                "(la passerelle lit le catalogue au montage : la redemarrer pour qu'elle le voie)"
            );
        }
        Some("list") => {
            let dir = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("tenantcat list REPERTOIRE-STORE"))?;
            let catalogue = CatalogueTenants::charger(Path::new(dir))?;
            for e in catalogue.entrees() {
                println!("{}\t{}\t{}", e.uuid, e.numero, e.libelle);
            }
            eprintln!("{} tenant(s) interne(s)", catalogue.compte());
        }
        _ => usage(),
    }
    Ok(())
}

// ── Clés Ed25519 ────────────────────────────────────────────────────────

/// Charge la paire PEM depuis `dir`, ou la génère (mode d'essai : la clé
/// privée reste sur la machine, jamais dans un dépôt).
fn charge_ou_genere_cles(dir: &Path) -> anyhow::Result<(String, String)> {
    let priv_path = dir.join("gateway.key.pem");
    let pub_path = dir.join("gateway.pub.pem");
    if priv_path.exists() && pub_path.exists() {
        return Ok((
            std::fs::read_to_string(&priv_path)?,
            std::fs::read_to_string(&pub_path)?,
        ));
    }
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
    use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};

    // Secret 32 octets sans dépendance `rand` (même recette que sts.rs).
    let (u1, u2) = (Uuid::new_v4(), Uuid::new_v4());
    let mut secret = [0u8; 32];
    secret[..16].copy_from_slice(u1.as_bytes());
    secret[16..].copy_from_slice(u2.as_bytes());
    let signing = SigningKey::from_bytes(&secret);

    let priv_pem = signing
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| anyhow::anyhow!("encodage clé privée : {e}"))?
        .to_string();
    let pub_pem = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| anyhow::anyhow!("encodage clé publique : {e}"))?;

    std::fs::create_dir_all(dir)?;
    std::fs::write(&priv_path, &priv_pem)?;
    std::fs::write(&pub_path, &pub_pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::warn!(
        dir = %dir.display(),
        "paire Ed25519 GÉNÉRÉE (mode d'essai) — pour un déploiement, fournir des clés gérées"
    );
    Ok((priv_pem, pub_pem))
}

// ── Les couches de la passerelle ────────────────────────────────────────

/// Les chemins que la passerelle sert sans principal — la même liste
/// explicite que l'autorisation this-rs (liveness et émission de jetons).
/// `/auth/users` vit sous ce préfixe mais vérifie lui-même le rôle admin.
/// Ce que la porte et la garde consultent : les **attachements** déclarés.
///
/// Un seul objet pour les deux, parce que la question est la même — « ce
/// chemin appartient à quoi, et qui a le droit ? » — et que deux tables
/// divergeraient.
struct EtatPortes {
    stores: Arc<HashMap<String, Propriete>>,
    services: Arc<HashMap<String, GardeService>>,
}

fn est_public(path: &str) -> bool {
    path == "/health" || path == "/healthz" || path == "/auth" || path.starts_with("/auth/")
}

/// **La porte** : hors chemins publics, pas de principal ⇒ 401 — jeton
/// absent ou invalide, même réponse : on ne renseigne pas un attaquant.
///
/// # L'exception anonyme, et pourquoi elle est étroite (choix de sécurité)
///
/// Cible ffs.space : un store déclaré `tenant=public` est un bien commun en
/// lecture. L'anonyme passe donc — mais SEULEMENT si les trois conditions
/// tiennent à la fois :
///
/// 1. méthode **GET ou HEAD** (jamais d'écriture anonyme, même sur public —
///    et les stores publics sont de toute façon montés en lecture seule) ;
/// 2. chemin `/stores/{nom}/…` d'un store **déclaré `tenant=public`** ;
/// 3. le store existe dans la déclaration — un store inconnu répond 401 à
///    l'anonyme (pas 404 : l'existence des stores ne se sonde pas sans
///    jeton).
///
/// Les stores à tenant gardent leur comportement : 401 sans jeton, 403 pour
/// un tenant étranger (garde de store), inchangé.
async fn porte(State(portes): State<Arc<EtatPortes>>, request: Request, next: Next) -> Response {
    if est_public(request.uri().path()) {
        return next.run(request).await;
    }
    let anonyme = matches!(
        request.extensions().get::<AuthContext>(),
        Some(AuthContext::Anonymous) | None
    );
    if !anonyme {
        return next.run(request).await;
    }

    let lecture = matches!(*request.method(), Method::GET | Method::HEAD);
    let store_public = request
        .uri()
        .path()
        .strip_prefix("/stores/")
        .map(|reste| reste.split('/').next().unwrap_or(""))
        .and_then(|nom| portes.stores.get(nom))
        .is_some_and(|p| matches!(p, Propriete::Public));
    // Un service rattaché `anonymous: true` : c'est un choix ÉCRIT par
    // l'exploitant, et il reste borné à la lecture — jamais d'écriture
    // anonyme, même sur un service déclaré ouvert.
    let service_anonyme = service_du_chemin(&portes.services, request.uri().path())
        .is_some_and(|(_, garde)| matches!(garde, GardeService::Anonyme));
    if lecture && (store_public || service_anonyme) {
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Bearer")],
        axum::Json(json!({
            "error": "unauthorized",
            "reason": "jeton STS requis (POST /auth/token)",
        })),
    )
        .into_response()
}

/// **La garde de store** : la maille d'isolation applicable AVANT le tenant
/// par nœud (RFC principal-natif-v2) — un store appartient à un tenant, et
/// seul ce tenant le lit à travers la passerelle.
async fn garde_store(
    State(portes): State<Arc<EtatPortes>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();

    // ── Un service rattaché : la garde est celle de sa déclaration ──
    if let Some((mount, garde)) = service_du_chemin(&portes.services, path) {
        match garde {
            GardeService::Authentifie | GardeService::Anonyme => {}
            GardeService::Tenant(proprietaire) => {
                let tenant_du_jeton = request
                    .extensions()
                    .get::<AuthContext>()
                    .and_then(|c| c.tenant_id());
                if tenant_du_jeton != Some(*proprietaire) {
                    return (
                        StatusCode::FORBIDDEN,
                        axum::Json(json!({
                            "error": "forbidden",
                            "reason": format!(
                                "le service `{mount}` est réservé à son tenant"
                            ),
                        })),
                    )
                        .into_response();
                }
            }
        }
    }
    if let Some(reste) = path.strip_prefix("/stores/") {
        let nom = reste.split('/').next().unwrap_or("");
        match portes.stores.get(nom) {
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    axum::Json(json!({"error": "not_found", "reason": "store inconnu"})),
                )
                    .into_response();
            }
            Some(Propriete::Public) => {}
            Some(Propriete::Tenant(proprietaire)) => {
                let tenant_du_jeton = request
                    .extensions()
                    .get::<AuthContext>()
                    .and_then(|c| c.tenant_id());
                if tenant_du_jeton != Some(*proprietaire) {
                    return (
                        StatusCode::FORBIDDEN,
                        axum::Json(json!({
                            "error": "forbidden",
                            "reason": "ce store n'appartient pas au tenant du jeton",
                        })),
                    )
                        .into_response();
                }
            }
        }
    }
    next.run(request).await
}

// ── Le plan de déploiement : les artefacts, générés depuis la déclaration ─
//
// Le générateur n'exécute **rien** : il écrit des fichiers relus par un
// opérateur, chacun portant son état (« préparé, non déployé »). C'est le
// partage du dépôt : le framework décrit, l'exploitant applique.

/// L'en-tête que porte **chaque** artefact.
const ENTETE: &str = "\
# =============================================================================
# PRÉPARÉ, NON DÉPLOYÉ — fichier GÉNÉRÉ depuis la déclaration de la passerelle.
# Rien n'a été installé, aucun port ouvert, aucun DNS modifié.
# Voir RUNBOOK.md (même dossier) : l'ordre des gestes, la vérification, le retrait.
# =============================================================================
";

/// La façade résolue, prête à être rendue.
fn resout_facade(expose: &ExposeYaml) -> ExposeResolue {
    ExposeResolue {
        acme_email: expose.acme_email.clone(),
        gateway: expose.gateway.clone(),
        sites: expose
            .sites
            .iter()
            .map(|site| SiteExpose {
                host: site.host.clone(),
                aliases: site.aliases.clone(),
                service: site.service.clone(),
                mode: match site.mode.as_deref() {
                    Some("forward_auth") => ModeFaçade::ForwardAuth,
                    _ => ModeFaçade::ViaGateway,
                },
                tunnel: site.tunnel,
                tunnel_host: site.tunnel_host.clone(),
            })
            .collect(),
        dns: expose.dns.as_ref().map(|dns| DnsYaml {
            target: dns.target.clone(),
            record_type: dns.record_type.to_ascii_uppercase(),
            ttl: dns.ttl,
        }),
    }
}

/// L'unité systemd de la passerelle : la déclaration remplace la ligne de
/// commande.
fn unite_systemd(chemin_declaration: &Path, binaire: &str) -> String {
    format!(
        "{ENTETE}
[Unit]
Description=Passerelle IAM this-rs devant les stores et les projets rattachés
After=network-online.target

[Service]
Type=simple
# La DÉCLARATION remplace la ligne de commande : un seul fichier à relire, et
# `--user` n'y figure pas (un mot de passe en clair dans une unité reste dans
# l'historique du shell). Les identités vivent dans le store d'identités.
ExecStart={binaire} --config {declaration}
Restart=always
RestartSec=5
# Une vue confinée par (store, tenant) tient ~28 descripteurs ; le cache est
# borné, mais la limite se pose ici plutôt que de la découvrir en production.
LimitNOFILE=65536

[Install]
WantedBy=default.target
",
        declaration = chemin_declaration.display(),
    )
}

/// Le nom de l'unité de tunnel d'un site.
fn nom_tunnel(host: &str) -> String {
    format!("this-rs-tunnel-{}.service", host.replace('.', "-"))
}

/// L'unité de tunnel ssh vers la passerelle.
///
/// Le tunnel joint **la passerelle**, jamais chaque projet : la façade n'a
/// qu'un chemin à connaître, et atteindre un projet ne demande plus d'ouvrir
/// un second tunnel vers lui.
fn unite_tunnel(site: &SiteExpose, gateway: &str) -> String {
    let cible = site.tunnel_host.clone().unwrap_or_default();
    let port = gateway.rsplit(':').next().unwrap_or("8144");
    let hote = gateway.rsplit_once(':').map(|(h, _)| h).unwrap_or(gateway);
    format!(
        "{ENTETE}
[Unit]
Description=Tunnel ssh vers la passerelle this-rs ({gateway}) pour {site}
After=network-online.target

[Service]
# `ExitOnForwardFailure` : un tunnel qui n'a pas pu prendre le port meurt au
# lieu de rester vivant sans rien transmettre — un tunnel « up » qui ne passe
# rien est la panne la plus longue à diagnostiquer.
ExecStart=/usr/bin/ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes -o ServerAliveInterval=60 -o ServerAliveCountMax=5 -L 127.0.0.1:{port}:{hote}:{port} {cible}
Restart=always
RestartSec=30

[Install]
WantedBy=default.target
",
        site = site.host,
    )
}

/// Le bloc Caddy d'un site — la variante **à ajouter** au Caddyfile existant.
fn bloc_caddy(site: &SiteExpose, expose: &ExposeResolue, services: &[ServiceDecl]) -> String {
    let mut hotes = vec![site.host.clone()];
    hotes.extend(site.aliases.iter().cloned());
    let hote_caddy = expose.gateway.clone();
    let mut bloc = format!("{} {{\n\tencode zstd gzip\n\n", hotes.join(", "));
    bloc.push_str(&format!(
        "\tlog {{\n\t\toutput file /var/log/caddy/{}-access.log {{\n\t\t\troll_size 20MiB\n\t\t\troll_keep 10\n\t\t}}\n\t}}\n\n",
        site.host
    ));
    bloc.push_str(
        "\theader {\n\
         \t\tStrict-Transport-Security \"max-age=31536000; includeSubDomains\"\n\
         \t\tX-Content-Type-Options \"nosniff\"\n\
         \t\tX-Frame-Options \"DENY\"\n\
         \t\tReferrer-Policy \"no-referrer\"\n\
         \t\t-Server\n\
         \t}\n\n",
    );

    match site.mode {
        ModeFaçade::ViaGateway => {
            let service = site
                .service
                .as_ref()
                .and_then(|nom| services.iter().find(|s| &s.nom == nom));
            match service {
                // Un site qui **est** le projet rattaché : la racine du site ne
                // correspond à aucune route de la passerelle (`/` y répond 404).
                // Il faut donc préfixer vers le montage du service — sans quoi
                // la page d'accueil d'un site exposé serait un 404, la garde
                // fonctionnant par ailleurs. C'est la répétition avec un vrai
                // Caddy qui l'a montré, pas la relecture.
                Some(service) => {
                    bloc.push_str(&format!(
                        "\troute {{\n\
                         \t\t# 1. Les chemins que la passerelle sert elle-même : login, sondes,\n\
                         \t\t#    stores, et le montage du service.\n\
                         \t\t@passerelle path /auth/* /auth /health /healthz /stores/* /services/*\n\
                         \t\thandle @passerelle {{\n\
                         \t\t\treverse_proxy {hote_caddy}\n\
                         \t\t}}\n\n\
                         \t\t# 2. Tout le reste du site EST le projet rattaché : on préfixe\n\
                         \t\t#    vers son montage, et la passerelle décide (jeton, tenant,\n\
                         \t\t#    refus déclarés). Aucune décision de sécurité ici.\n\
                         \t\thandle {{\n\
                         \t\t\trewrite * {mount}{{uri}}\n\
                         \t\t\treverse_proxy {hote_caddy}\n\
                         \t\t}}\n\
                         \t}}\n",
                        mount = service.mount
                    ));
                }
                // Un site sans projet rattaché : il **est** la passerelle.
                None => {
                    bloc.push_str(&format!(
                        "\t# Tout passe par la passerelle : elle garde (jeton), juge (tenant),\n\
                         \t# refuse (chemins déclarés) et transmet au projet rattaché.\n\
                         \treverse_proxy {hote_caddy}\n"
                    ));
                }
            }
        }
        ModeFaçade::ForwardAuth => {
            // **`route` impose l'ordre écrit** : les directives nues sont
            // réordonnées par Caddy selon son ordre interne, et ici l'ordre est
            // porteur de sens (le login d'abord, le refus ensuite, la porte
            // après, le projet en dernier).
            //
            // La route du **login** n'est pas un détail : sans elle, un
            // navigateur n'obtient JAMAIS de cookie de session — une garde sans
            // porte d'entrée n'est pas une garde, c'est une fermeture. C'est la
            // répétition avec un vrai Caddy qui l'a montré, pas la relecture.
            bloc.push_str(&format!(
                "\troute {{\n\
                 \t\t# 1. Le login (et les sondes) : la passerelle y répond elle-même,\n\
                 \t\t#    sinon le cookie de session est hors d'atteinte.\n\
                 \t\t@passerelle path /auth/* /auth /health /healthz\n\
                 \t\thandle @passerelle {{\n\
                 \t\t\treverse_proxy {hote_caddy}\n\
                 \t\t}}\n\n"
            ));
            if let Some(service) = site
                .service
                .as_ref()
                .and_then(|nom| services.iter().find(|s| &s.nom == nom))
                && !service.deny.is_empty()
            {
                let mut conditions: Vec<String> = Vec::new();
                for regle in &service.deny {
                    conditions.push(regle.clone());
                    conditions.push(format!("{regle}/*"));
                }
                bloc.push_str(&format!(
                    "\t\t# 2. Les refus déclarés, AVANT la porte : un chemin refusé l'est\n\
                     \t\t#    avec ou sans jeton, exactement comme dans la passerelle.\n\
                     \t\t@interdit path {}\n\
                     \t\trespond @interdit 403\n\n",
                    conditions.join(" ")
                ));
            }
            bloc.push_str(&format!(
                "\t\t# 3. La PORTE ({hote_caddy}/auth/verify) : signature, échéance,\n\
                 \t\t#    émetteur et révocation vérifiés par la passerelle, et l'identité\n\
                 \t\t#    qui revient en en-têtes.\n\
                 \t\tforward_auth {hote_caddy} {{\n\
                 \t\t\turi /auth/verify\n\
                 \t\t\tcopy_headers x-this-subject x-this-tenant x-this-roles\n\
                 \t\t}}\n\n"
            ));
            if let Some(service) = site
                .service
                .as_ref()
                .and_then(|nom| services.iter().find(|s| &s.nom == nom))
            {
                bloc.push_str(&format!(
                    "\t\t# 4. Le projet rattaché, joint en direct.\n\
                     \t\treverse_proxy {}\n",
                    service.upstream
                ));
            }
            bloc.push_str("\t}\n");
        }
    }
    bloc.push_str("}\n");
    bloc
}

/// Le Caddyfile complet d'une façade **hôte** (chemin B) : options globales +
/// un bloc par site.
fn caddyfile_hote(expose: &ExposeResolue, services: &[ServiceDecl]) -> String {
    let mut fichier = String::from(ENTETE);
    fichier.push_str(&format!(
        "{{\n\temail {}\n\n\
         \t# Le port 80 peut être tenu par autre chose : ACME passe alors par\n\
         \t# TLS-ALPN-01 (port 443), comme le fait déjà le Caddy existant.\n}}\n\n",
        expose.acme_email
    ));
    for site in &expose.sites {
        fichier.push_str(&bloc_caddy(site, expose, services));
        fichier.push('\n');
    }
    // Un artefact exactement tel que `caddy fmt` le laisserait : un fichier
    // généré qui commence par un avertissement de formatage apprend à ignorer
    // les avertissements.
    format!("{}\n", fichier.trim_end())
}

/// Le plan DNS : les enregistrements **désirés**, et rien d'autre.
fn plan_dns(expose: &ExposeResolue) -> String {
    let Some(dns) = &expose.dns else {
        return format!(
            "{ENTETE}\n# Aucun `expose.dns` déclaré : le générateur ne propose aucun\n# enregistrement, et n'en écrit jamais aucun.\n"
        );
    };
    let hotes: Vec<String> = expose
        .sites
        .iter()
        .flat_map(|site| std::iter::once(site.host.clone()).chain(site.aliases.iter().cloned()))
        .collect();
    let enregistrements =
        this::dns::desired_records(&hotes, &dns.record_type, &dns.target, dns.ttl);

    let mut texte = format!(
        "{ENTETE}\n# Enregistrements DÉSIRÉS (type {} → {}), TTL {} s.\n\
         # Aucun n'est écrit par ce fichier : les types gérés sont {} — jamais MX,\n\
         # TXT, NS ni SRV, pour qu'une réconciliation ne puisse pas effacer le mail\n\
         # d'un domaine. L'application est un geste d'exploitant (PowerDNS ou\n\
         # registrar), et le retrait suit le même chemin.\n\n",
        dns.record_type,
        dns.target,
        dns.ttl,
        this::dns::MANAGED_TYPES.join(", ")
    );
    for enregistrement in &enregistrements {
        texte.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            enregistrement.name,
            enregistrement.ttl,
            enregistrement.record_type,
            enregistrement.content
        ));
    }
    texte.push_str(&format!(
        "\n# Vérifier la propagation avant d'exposer :\n#   dig +short {} {}\n",
        dns.record_type,
        expose.sites.first().map(|s| s.host.as_str()).unwrap_or("")
    ));
    texte
}

// ── Services rattachés : le proxy gardé (feature `proxy`) ───────────────

/// Taille maximale d'un corps transmis à un service rattaché.
///
/// Un proxy qui accepte n'importe quelle taille est un amplificateur : la
/// limite est **écrite**, pas devinée.
#[cfg(feature = "proxy")]
const CORPS_MAX: usize = 8 * 1024 * 1024;

/// Un **service rattaché** : un projet qui existe déjà, joint par HTTP.
#[cfg(feature = "proxy")]
struct EtatService {
    nom: String,
    mount: String,
    upstream: String,
    deny: Vec<String>,
    client: reqwest::Client,
}

#[cfg(feature = "proxy")]
impl EtatService {
    /// Ce chemin est-il refusé par la déclaration ?
    ///
    /// La correspondance est **exacte ou par préfixe de segment** : `deny:
    /// ["/api/query"]` refuse `/api/query` et `/api/query/…`, jamais
    /// `/api/querylog`. Une règle qui refuserait plus large que ce qu'elle
    /// écrit serait un piège.
    fn refuse(&self, chemin: &str) -> bool {
        self.deny.iter().any(|regle| {
            chemin == regle
                || chemin
                    .strip_prefix(regle)
                    .is_some_and(|reste| reste.starts_with('/'))
        })
    }
}

/// Les en-têtes qu'un proxy ne transmet **jamais**.
///
/// Hop-by-hop (RFC 9110) : ils appartiennent à UNE connexion, pas à la
/// requête. `host` et `content-length` : l'amont a les siens, recalculés.
/// Et **tout `x-this-*`** : l'identité ne doit venir que de la vérification,
/// jamais du client — sinon n'importe qui s'attribue le tenant qu'il veut.
#[cfg(feature = "proxy")]
fn en_tete_interdit(nom: &str) -> bool {
    let nom = nom.to_ascii_lowercase();
    matches!(
        nom.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    ) || nom.starts_with("x-this-")
}

/// L'identité **vérifiée** du principal, en en-têtes pour l'amont.
///
/// Ce sont les mêmes noms que `/auth/verify` : une seule convention, donc un
/// projet rattaché n'a qu'une chose à connaître.
#[cfg(feature = "proxy")]
fn identite_en_entetes(ctx: &AuthContext) -> Vec<(&'static str, String)> {
    match ctx {
        AuthContext::User {
            user_id,
            tenant_id,
            roles,
        } => {
            let mut entetes = vec![
                ("x-this-subject", user_id.to_string()),
                ("x-this-tenant", tenant_id.to_string()),
            ];
            if !roles.is_empty() {
                entetes.push(("x-this-roles", roles.join(",")));
            }
            entetes
        }
        AuthContext::Owner {
            user_id, tenant_id, ..
        } => vec![
            ("x-this-subject", user_id.to_string()),
            ("x-this-tenant", tenant_id.to_string()),
        ],
        AuthContext::Service {
            service_name,
            tenant_id,
        } => {
            let mut t = vec![("x-this-subject", service_name.clone())];
            if let Some(tenant) = tenant_id {
                t.push(("x-this-tenant", tenant.to_string()));
            }
            t
        }
        AuthContext::Admin { admin_id } => vec![
            ("x-this-subject", admin_id.to_string()),
            ("x-this-roles", "admin".to_string()),
        ],
        AuthContext::Anonymous => Vec::new(),
    }
}

/// **Le proxy gardé** : tout chemin du service est transmis à l'amont.
///
/// L'ordre compte : les **refus déclarés** d'abord (`/api/query` du viz — sa
/// requête brute), puis la transmission. La garde, elle, est déjà passée : ce
/// handler n'est atteint que par un appelant que la [porte](porte) et la
/// [garde de store](garde_store) ont laissé passer.
#[cfg(feature = "proxy")]
async fn sert_le_service(State(etat): State<Arc<EtatService>>, request: Request) -> Response {
    let chemin = request.uri().path().to_string();
    let suffixe = chemin.strip_prefix(&etat.mount).unwrap_or(&chemin);
    let suffixe = if suffixe.is_empty() { "/" } else { suffixe };

    if etat.refuse(suffixe) {
        tracing::warn!(service = %etat.nom, chemin = %suffixe, "chemin refusé par la déclaration");
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({
                "error": "forbidden",
                "reason": "ce chemin est refusé par la déclaration de rattachement",
                "path": suffixe,
            })),
        )
            .into_response();
    }

    let mut url = format!("{}{}", etat.upstream.trim_end_matches('/'), suffixe);
    if let Some(requete) = request.uri().query() {
        url.push('?');
        url.push_str(requete);
    }

    let (parts, corps) = request.into_parts();
    let methode = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
        Ok(methode) => methode,
        Err(_) => {
            return (
                StatusCode::METHOD_NOT_ALLOWED,
                axum::Json(json!({"error": "method_not_allowed"})),
            )
                .into_response();
        }
    };

    let mut en_tetes = reqwest::header::HeaderMap::new();
    for (nom, valeur) in parts.headers.iter() {
        if !en_tete_interdit(nom.as_str()) {
            en_tetes.append(nom.clone(), valeur.clone());
        }
    }
    if let Some(ctx) = parts.extensions.get::<AuthContext>() {
        for (nom, valeur) in identite_en_entetes(ctx) {
            if let (Ok(nom), Ok(valeur)) = (
                reqwest::header::HeaderName::from_bytes(nom.as_bytes()),
                reqwest::header::HeaderValue::from_str(&valeur),
            ) {
                en_tetes.insert(nom, valeur);
            }
        }
    }

    let corps = match axum::body::to_bytes(corps, CORPS_MAX).await {
        Ok(octets) => octets,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                axum::Json(json!({
                    "error": "payload_too_large",
                    "reason": format!("corps au-delà de {CORPS_MAX} octets"),
                })),
            )
                .into_response();
        }
    };

    let reponse = match etat
        .client
        .request(methode, &url)
        .headers(en_tetes)
        .body(corps)
        .send()
        .await
    {
        Ok(reponse) => reponse,
        Err(erreur) => {
            tracing::warn!(service = %etat.nom, amont = %etat.upstream, error = %erreur, "amont injoignable");
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({
                    "error": "bad_gateway",
                    "reason": "le service rattaché est injoignable",
                })),
            )
                .into_response();
        }
    };

    let statut = StatusCode::from_u16(reponse.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut en_tetes_reponse = axum::http::HeaderMap::new();
    for (nom, valeur) in reponse.headers().iter() {
        if !en_tete_interdit(nom.as_str()) {
            en_tetes_reponse.append(nom.clone(), valeur.clone());
        }
    }
    // **Le corps est TRANSMIS, pas tamponné.** `reponse.bytes()` attendait la fin
    // du flux : un projet qui parle SSE (ou tout flux long) se serait figé, et le
    // rattachement aurait paru cassé sans qu'aucune erreur ne soit levée. Une
    // fois les en-têtes partis, une coupure de l'amont ne peut plus changer le
    // statut — elle tronque le flux, et c'est une propriété du streaming, pas un
    // défaut : le client voit une fin de flux, pas un 502 inventé après coup.
    (
        statut,
        en_tetes_reponse,
        axum::body::Body::from_stream(reponse.bytes_stream()),
    )
        .into_response()
}

// ── Administration à chaud : /auth/users (rôle admin requis) ────────────

/// L'état des routes d'administration : l'annuaire persisté, partagé avec le
/// STS (même `Arc` — une création à chaud est immédiatement connectable).
struct EtatAdmin {
    annuaire: Arc<AnnuaireIdentites>,
}

#[derive(Deserialize)]
struct CreationUtilisateur {
    username: String,
    password: String,
    tenant_id: Uuid,
    #[serde(default)]
    roles: Vec<String>,
}

#[derive(Deserialize)]
struct NouveauMotDePasse {
    password: String,
}

/// Le rôle `admin` (ou un `AuthContext::Admin`) ouvre l'administration.
/// Ces routes vivent sous `/auth/` (donc passent la porte sans jeton) :
/// la vérification ICI est la seule barrière — elle est fail-closed.
fn est_admin(ctx: &AuthContext) -> bool {
    match ctx {
        AuthContext::Admin { .. } => true,
        AuthContext::User { roles, .. } => roles.iter().any(|r| r == "admin"),
        _ => false,
    }
}

fn refus_admin() -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(json!({
            "error": "forbidden",
            "reason": "l'administration des utilisateurs exige le rôle admin",
        })),
    )
        .into_response()
}

/// GET /auth/users — la liste, SANS les hashes (un jeton admin n'est pas une
/// raison de faire circuler des hashes argon2 sur le réseau).
async fn admin_liste(
    State(etat): State<Arc<EtatAdmin>>,
    axum::Extension(ctx): axum::Extension<AuthContext>,
) -> Response {
    if !est_admin(&ctx) {
        return refus_admin();
    }
    match etat.annuaire.lister() {
        Ok(tous) => axum::Json(json!({
            "users": tous
                .iter()
                .map(|u| json!({
                    "username": u.username,
                    "user_id": u.user_id,
                    "tenant_id": u.tenant_id,
                    "roles": u.roles,
                    "created_at": u.created_at,
                }))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => erreur_annuaire(e),
    }
}

/// POST /auth/users — création à chaud (hash argon2id calculé hors du
/// runtime async : argon2 coûte ~20 ms de CPU, c'est son travail).
async fn admin_cree(
    State(etat): State<Arc<EtatAdmin>>,
    axum::Extension(ctx): axum::Extension<AuthContext>,
    axum::Json(corps): axum::Json<CreationUtilisateur>,
) -> Response {
    if !est_admin(&ctx) {
        return refus_admin();
    }
    let annuaire = etat.annuaire.clone();
    let resultat = tokio::task::spawn_blocking(move || {
        annuaire.ajouter(
            &corps.username,
            &corps.password,
            corps.tenant_id,
            corps.roles,
        )
    })
    .await;
    match resultat {
        Ok(Ok(cree)) => (
            StatusCode::CREATED,
            axum::Json(json!({
                "status": "created",
                "username": cree.username,
                "tenant_id": cree.tenant_id,
                "roles": cree.roles,
            })),
        )
            .into_response(),
        Ok(Err(e)) if e.to_string().contains("existe déjà") => (
            StatusCode::CONFLICT,
            axum::Json(json!({"error": "conflict", "reason": "ce login existe déjà"})),
        )
            .into_response(),
        Ok(Err(e)) => erreur_annuaire(e),
        Err(e) => erreur_annuaire(anyhow::anyhow!("tâche de hachage interrompue : {e}")),
    }
}

/// PUT /auth/users/{username}/password — remplacement du mot de passe.
async fn admin_passwd(
    State(etat): State<Arc<EtatAdmin>>,
    axum::Extension(ctx): axum::Extension<AuthContext>,
    CheminUrl(username): CheminUrl<String>,
    axum::Json(corps): axum::Json<NouveauMotDePasse>,
) -> Response {
    if !est_admin(&ctx) {
        return refus_admin();
    }
    let annuaire = etat.annuaire.clone();
    let resultat = tokio::task::spawn_blocking(move || {
        annuaire.changer_mot_de_passe(&username, &corps.password)
    })
    .await;
    match resultat {
        Ok(Ok(())) => (
            StatusCode::OK,
            axum::Json(json!({"status": "password_changed"})),
        )
            .into_response(),
        Ok(Err(e)) if e.to_string().contains("inconnu") => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": "not_found", "reason": "utilisateur inconnu"})),
        )
            .into_response(),
        Ok(Err(e)) => erreur_annuaire(e),
        Err(e) => erreur_annuaire(anyhow::anyhow!("tâche de hachage interrompue : {e}")),
    }
}

fn erreur_annuaire(e: anyhow::Error) -> Response {
    tracing::error!("annuaire d'identités : {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(json!({"error": "identity_store_error"})),
    )
        .into_response()
}

fn routes_admin(etat: Arc<EtatAdmin>) -> Router {
    Router::new()
        .route(
            "/auth/users",
            axum::routing::get(admin_liste).post(admin_cree),
        )
        .route(
            "/auth/users/{username}/password",
            axum::routing::put(admin_passwd),
        )
        .with_state(etat)
}

// ── La vue native par tenant (NATIF-J4) ─────────────────────────────────

/// Le plafond GLOBAL de vues en cache, tous stores confondus. Chiffré au
/// banc (plan J4) : ~28 fds par vue sur un store de 8 Go, service souvent
/// borné à `LimitNOFILE=1024` — 32 vues ≈ 900 fds au pire. Une vue évincée
/// se reconstruit à la prochaine requête du tenant (~55 ms mesurés, une
/// fois). Les vues publiques (tenant 0) sont épinglées : jamais évincées.
const PLAFOND_VUES: usize = 32;

/// Une vue en cache : le routeur this-rs complet construit sur UN handle
/// RO confiné. Cloner un `Router` est bon marché (état en `Arc`).
struct VueEnCache {
    routeur: Router,
    /// Vue publique de démarrage — hors éviction (elle sert l'anonyme et
    /// tout principal non interné : la perdre pénaliserait tout le monde).
    epinglee: bool,
    dernier_usage: Instant,
}

/// Le cache global des vues, clé `(nom du store, tenant u32)`.
struct CacheVues {
    vues: tokio::sync::Mutex<HashMap<(String, u32), VueEnCache>>,
}

/// L'état du dispatcher d'UN store monté : de quoi résoudre le tenant et
/// construire (ou retrouver) sa vue.
struct EtatVueStore {
    nom: String,
    chemin: PathBuf,
    propriete: Propriete,
    /// `substrate.tenantcat` du store, lu AU MONTAGE (un tenant interné
    /// ensuite exige un redémarrage — limitation documentée du plan J4).
    catalogue: CatalogueTenants,
    cache: Arc<CacheVues>,
}

/// Construit la vue `tenant` d'un store : ouverture RO (verrou partagé —
/// les vues d'un même fichier coexistent, vérifié au banc), confinement
/// `avec_principal` (immuable pour la vie du handle, jalon 3), puis le
/// routeur this-rs complet sur CE handle. BLOQUANT (~3-55 ms selon la
/// taille du store) : à appeler sous `spawn_blocking` en vol.
///
/// C'est ICI qu'un store v1/v2 meurt : le substrat refuse toute ouverture
/// non exclusive d'un format ancien (`UnsupportedVersion` — on ne migre
/// jamais sous verrou partagé). Le message le dit à l'exploitant.
fn construit_vue(
    nom: &str,
    chemin: &Path,
    propriete: &Propriete,
    tenant: u32,
) -> anyhow::Result<Router> {
    let store = ObrainStore::open_read_only(chemin).map_err(|e| {
        anyhow::anyhow!(
            "store `{nom}` ({}) : {e}\n\
             — si l'erreur est une version de format : un store v1/v2 se migre\n\
             HORS LIGNE (ouverture exclusive par un binaire v3, service arrêté) ;\n\
             la passerelle ne migre jamais sous verrou partagé",
            chemin.display()
        )
    })?;
    store
        .db()
        .avec_principal(tenant)
        .map_err(|e| anyhow::anyhow!("store `{nom}` : pose du principal {tenant} : {e}"))?;
    let links = ObrainLinkService::new(store.clone());
    let politique_graph = match propriete {
        Propriete::Public => "public",
        Propriete::Tenant(_) => "authenticated",
    };
    let securite = ServerSecurityConfig {
        graph: GraphExposureConfig {
            policy: Some(politique_graph.to_string()),
        },
        ..Default::default()
    };
    ServerBuilder::new()
        .with_link_service(links)
        .with_obrain_store(store)
        .with_server_security(securite)
        .build()
        .map_err(|e| anyhow::anyhow!("store `{nom}` : construction du routeur : {e}"))
}

impl EtatVueStore {
    /// La vue du tenant : cache d'abord, construction (`spawn_blocking`)
    /// sinon. Deux premières requêtes simultanées d'un même tenant peuvent
    /// construire deux fois — l'entrée du premier arrivé fait foi, l'autre
    /// vue est jetée (inefficacité bénigne, jamais deux vues servies).
    async fn vue(&self, tenant: u32) -> anyhow::Result<Router> {
        let cle = (self.nom.clone(), tenant);
        {
            let mut vues = self.cache.vues.lock().await;
            if let Some(v) = vues.get_mut(&cle) {
                v.dernier_usage = Instant::now();
                return Ok(v.routeur.clone());
            }
        }

        let (nom, chemin, propriete) = (
            self.nom.clone(),
            self.chemin.clone(),
            self.propriete.clone(),
        );
        let construit =
            tokio::task::spawn_blocking(move || construit_vue(&nom, &chemin, &propriete, tenant))
                .await
                .map_err(|e| anyhow::anyhow!("tâche de construction de vue interrompue : {e}"))??;

        let mut vues = self.cache.vues.lock().await;
        let entree = vues.entry(cle).or_insert(VueEnCache {
            routeur: construit,
            epinglee: false,
            dernier_usage: Instant::now(),
        });
        entree.dernier_usage = Instant::now();
        let routeur = entree.routeur.clone();

        // Éviction LRU au-delà du plafond — jamais une vue épinglée.
        while vues.len() > PLAFOND_VUES {
            let victime = vues
                .iter()
                .filter(|(_, v)| !v.epinglee)
                .min_by_key(|(_, v)| v.dernier_usage)
                .map(|(k, _)| k.clone());
            match victime {
                Some(k) => {
                    vues.remove(&k);
                    tracing::info!(store = %k.0, tenant = k.1, "vue évincée (plafond {PLAFOND_VUES})");
                }
                None => break, // tout est épinglé : on tolère le dépassement
            }
        }
        Ok(routeur)
    }
}

/// **Le dispatcher de vue** : toute requête `/stores/{nom}/…` passe ici
/// (fallback du routeur monté — le préfixe est déjà retiré par `nest`).
/// AuthContext → Uuid tenant → u32 via le catalogue DU store → la vue.
/// Sans tenant résoluble (anonyme, admin, service sans tenant, Uuid non
/// interné) : vue 0 — le public seul, rien d'autre.
async fn sert_la_vue(State(etat): State<Arc<EtatVueStore>>, request: Request) -> Response {
    let tenant = request
        .extensions()
        .get::<AuthContext>()
        .and_then(|c| c.tenant_id())
        .and_then(|uuid| etat.catalogue.resoudre(&uuid))
        .unwrap_or(0);
    match etat.vue(tenant).await {
        Ok(routeur) => match routeur.oneshot(request).await {
            Ok(reponse) => reponse,
            Err(jamais) => match jamais {}, // Infallible
        },
        Err(e) => {
            tracing::error!(store = %etat.nom, tenant, "vue inconstructible : {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": "store_view_error"})),
            )
                .into_response()
        }
    }
}

// ── Assemblage ──────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Sous-commandes d'administration : `this-gateway user|tenantcat …`.
    let brut: Vec<String> = std::env::args().skip(1).collect();
    if brut.first().map(String::as_str) == Some("user") {
        return administre_utilisateurs(&brut[1..]);
    }
    if brut.first().map(String::as_str) == Some("tenantcat") {
        return administre_tenantcat(&brut[1..]);
    }
    if brut.first().map(String::as_str) == Some("plan") {
        return plan_deploiement(&brut[1..]);
    }
    if brut.first().map(String::as_str) == Some("prevol") {
        return prevol(&brut[1..]);
    }

    let args = parse_args()?;
    let (priv_pem, pub_pem) = charge_ou_genere_cles(&args.keys_dir)?;

    // ── L'annuaire persisté : LE registre des identités (écriture) ──
    let annuaire = Arc::new(AnnuaireIdentites::ouvrir(&args.identites)?);
    tracing::info!(
        chemin = %args.identites.display(),
        identites = annuaire.compte(),
        "annuaire d'identités ouvert (store obrain dédié, hashes argon2id)"
    );

    // `--user` = SEED de bootstrap : créé haché s'il manque, ignoré sinon.
    // Le clair fourni ici ne quitte jamais la mémoire de ce processus.
    for user in &args.users {
        match annuaire.chercher(&user.login)? {
            Some(_) => tracing::info!(
                login = %user.login,
                "seed --user ignoré : ce login existe dans l'annuaire, le store fait foi"
            ),
            None => {
                annuaire.ajouter(
                    &user.login,
                    &user.mot_de_passe,
                    user.tenant,
                    user.roles.clone(),
                )?;
                tracing::info!(
                    login = %user.login,
                    tenant = %user.tenant,
                    "seed --user créé dans l'annuaire (hash argon2id — le clair n'est pas persisté)"
                );
            }
        }
    }

    // ── STS : émission, adossée à l'annuaire (le store fait foi) ──
    let mut sts = StsService::new(&priv_pem, args.issuer.clone())?;
    sts.set_access_ttl_secs(900);
    sts.set_annuaire(annuaire.clone());
    let sts_state = Arc::new(StsState {
        sts: Arc::new(sts),
        public_key_pem: pub_pem.clone(),
        decoding_key: jsonwebtoken::DecodingKey::from_ed_pem(pub_pem.as_bytes())?,
    });

    // ── Validation : WamiAuthProvider sur la même clé publique ──
    let auth_config = AuthConfig {
        provider: this::config::auth::AuthProviderType::Wami,
        wami: Some(WamiConfig {
            public_key: Some(pub_pem.clone()),
            private_key: None,
            issuer: Some(args.issuer.clone()),
            audience: None,
            mode: this::config::auth::AuthMode::Sts,
            token_ttl_secs: 900,
            refresh_ttl_secs: 86400,
        }),
        ..Default::default()
    };
    let provider: Arc<dyn AuthProvider> = Arc::new(WamiAuthProvider::from_config(&auth_config)?);

    // ── Un routeur this-rs complet PAR store, en lecture seule ──
    let mut app = Router::new()
        .route("/health", axum::routing::get(sante))
        .route("/healthz", axum::routing::get(sante))
        .merge(auth_router(sts_state))
        .merge(routes_admin(Arc::new(EtatAdmin {
            annuaire: annuaire.clone(),
        })));

    // ── NATIF-J4 : un dispatcher de VUE par store, plus un routeur figé ──
    // La vue 0 (publique) de chaque store est construite ICI : elle valide
    // le format (v3 exigé — un store v1/v2 fait échouer le démarrage avec
    // le remède en clair) et servira l'anonyme comme les non-internés.
    let cache = Arc::new(CacheVues {
        vues: tokio::sync::Mutex::new(HashMap::new()),
    });
    let mut proprietes: HashMap<String, Propriete> = HashMap::new();
    for decl in &args.stores {
        let catalogue = CatalogueTenants::charger(&decl.chemin).map_err(|e| {
            anyhow::anyhow!("store `{}` : catalogue tenant illisible : {e}", decl.nom)
        })?;
        let vue_publique = construit_vue(&decl.nom, &decl.chemin, &decl.propriete, 0)?;
        cache.vues.lock().await.insert(
            (decl.nom.clone(), 0),
            VueEnCache {
                routeur: vue_publique,
                epinglee: true,
                dernier_usage: Instant::now(),
            },
        );
        let etat = Arc::new(EtatVueStore {
            nom: decl.nom.clone(),
            chemin: decl.chemin.clone(),
            propriete: decl.propriete.clone(),
            catalogue,
            cache: cache.clone(),
        });
        let tenants_internes = etat.catalogue.compte();
        let dispatcher = Router::new().fallback(sert_la_vue).with_state(etat);
        app = app.nest(&format!("/stores/{}", decl.nom), dispatcher);
        proprietes.insert(decl.nom.clone(), decl.propriete.clone());
        tracing::info!(
            store = %decl.nom,
            chemin = %decl.chemin.display(),
            politique = match &decl.propriete {
                Propriete::Public => "public",
                Propriete::Tenant(_) => "authenticated",
            },
            tenants_internes,
            "store monté en LECTURE SEULE sous /stores/{} — vues natives par tenant (v3)",
            decl.nom
        );
    }
    // ── Les services rattachés : un projet qui existe déjà, joint par HTTP ──
    #[cfg(feature = "proxy")]
    let mut gardes_services: HashMap<String, GardeService> = HashMap::new();
    #[cfg(feature = "proxy")]
    {
        let client = reqwest::Client::builder()
            // Le client suit ses redirections, pas le proxy : une redirection
            // traversante ferait sortir la requête du périmètre gardé.
            .redirect(reqwest::redirect::Policy::none())
            // **Délai de CONNEXION seulement** : un amont injoignable doit
            // échouer en cinq secondes au lieu de retenir la connexion du client
            // indéfiniment. Aucun délai TOTAL, et c'est délibéré : il tuerait un
            // flux légitime (SSE) au bout de N secondes — la passerelle est
            // transparente, elle ne décide pas combien de temps un flux vit.
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow::anyhow!("client HTTP du proxy : {e}"))?;
        for service in &args.services {
            let etat = Arc::new(EtatService {
                nom: service.nom.clone(),
                mount: service.mount.clone(),
                upstream: service.upstream.clone(),
                deny: service.deny.clone(),
                client: client.clone(),
            });
            let routeur = Router::new()
                .fallback(sert_le_service)
                .with_state(etat.clone());
            app = app.nest(&service.mount, routeur);
            // **La racine du service, montée explicitement — avec ET sans le
            // slash final.** Un `nest` ne fait pas correspondre `/prefixe/`
            // (slash final) à son fallback, et c'est pourtant l'URL qu'un
            // navigateur demande pour un site : sans ces deux routes, un
            // service rattaché répond 404 sur sa propre page d'accueil alors
            // que tout le reste fonctionne.
            app = app.route(
                &service.mount,
                axum::routing::any(sert_le_service).with_state(etat.clone()),
            );
            app = app.route(
                &format!("{}/", service.mount),
                axum::routing::any(sert_le_service).with_state(etat),
            );
            gardes_services.insert(service.mount.clone(), service.garde.clone());
            tracing::info!(
                service = %service.nom,
                mount = %service.mount,
                upstream = %service.upstream,
                garde = ?service.garde,
                refus = ?service.deny,
                "service rattaché sous {} — la garde est celle de sa déclaration",
                service.mount
            );
        }
    }

    let proprietes = Arc::new(proprietes);
    #[cfg(not(feature = "proxy"))]
    let gardes_services: HashMap<String, GardeService> = HashMap::new();
    let portes = Arc::new(EtatPortes {
        stores: proprietes,
        services: Arc::new(gardes_services),
    });

    // ── Les couches (axum : la dernière appliquée est la plus externe) ──
    // Exécution voulue : traduction du cookie → authentification → porte →
    // garde (store ou service) → routes.
    let app = app
        .layer(middleware::from_fn_with_state(portes.clone(), garde_store))
        .layer(middleware::from_fn_with_state(portes, porte))
        .layer(middleware::from_fn_with_state(
            AuthLayer::new(provider, "public".to_string()),
            auth_middleware,
        ))
        // La plus EXTERNE (appliquée en dernier, exécutée en premier) : le
        // cookie de session devient l'en-tête que la couche d'authentification
        // sait lire. Elle ne vérifie rien — la vérification reste unique.
        .layer(middleware::from_fn(cookie_vers_en_tete));

    println!("passerelle IAM sur http://{}", args.addr);
    println!(
        "  POST /auth/token                 émission STS (login/password) + cookie de session"
    );
    println!(
        "       curl -c c.txt -X POST http://{}/auth/token \\",
        args.addr
    );
    println!("            -H 'content-type: application/json' \\");
    println!("            -d '{{\"username\":\"…\",\"password\":\"…\"}}'");
    println!(
        "       curl -b c.txt http://{}/stores/{{nom}}/graph",
        args.addr
    );
    println!("  GET  /auth/keys                  JWKS (clé publique)");
    println!("  GET  /auth/users                 administration (rôle admin)");
    println!(
        "  GET  /stores/{{nom}}/graph         relief (jeton requis — ou GET anonyme si tenant=public)"
    );
    println!("  chaque réponse de /stores/… est LA VUE du principal (NATIF-J4) :");
    println!("    tenant interné (substrate.tenantcat) = ses nœuds + le public ;");
    println!("    anonyme ou non-interné = le public seul");
    for service in &args.services {
        println!(
            "  {} → {} ({}){}",
            service.mount,
            service.upstream,
            match &service.garde {
                GardeService::Anonyme => "sans jeton, lecture seule",
                GardeService::Authentifie => "jeton requis",
                GardeService::Tenant(_) => "jeton du tenant propriétaire",
            },
            if service.deny.is_empty() {
                String::new()
            } else {
                format!(" — refus : {}", service.deny.join(", "))
            }
        );
    }
    println!("  refus par défaut partout ailleurs — abstention = 403");

    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn sante() -> axum::Json<serde_json::Value> {
    axum::Json(json!({"status": "ok", "service": "this-gateway"}))
}

// ---------------------------------------------------------------------------
// Le chargeur de déclaration — ce qu'une déclaration fait VRAIMENT des chemins
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_declaration {
    use super::*;

    /// Écrit une déclaration minimale mais **valide** dans un fichier temporaire.
    fn declaration(contenu: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("répertoire temporaire");
        let chemin = dir.path().join("deploy.yaml");
        std::fs::write(&chemin, contenu).expect("écriture de la déclaration");
        (dir, chemin)
    }

    /// **Le défaut constaté** : le schéma documenté écrit `~/.config/…`, et le
    /// chargeur prenait le `~` au pied de la lettre. Un opérateur qui suivait la
    /// documentation obtenait cinq `MANQUE` au prévol et une passerelle qui
    /// refuse de démarrer — sans que rien ne nomme le `~`.
    #[test]
    fn un_tilde_initial_est_developpe_dans_les_chemins() {
        let home = std::env::var("HOME").expect("HOME défini pour ce test");
        let (_dir, chemin) = declaration(
            "addr: 127.0.0.1:9999\n\
             keys: ~/.config/this-rs-gateway/cles\n\
             identites: ~/.local/share/this-rs-gateway/identites.obrain\n\
             issuer: test\n\
             stores:\n\
             \x20 - name: s\n\
             \x20   path: ~/data/s.obrain\n\
             \x20   tenant: public\n",
        );

        let decl = lit_declaration(&chemin).expect("la déclaration doit être acceptée");

        assert_eq!(
            decl.keys.as_deref(),
            Some(Path::new(&format!("{home}/.config/this-rs-gateway/cles"))),
            "`keys` doit être développé, pas gardé littéral"
        );
        assert_eq!(
            decl.identites.as_deref(),
            Some(Path::new(&format!(
                "{home}/.local/share/this-rs-gateway/identites.obrain"
            )))
        );
        assert_eq!(
            decl.stores[0].path,
            PathBuf::from(format!("{home}/data/s.obrain")),
            "le chemin d'un store aussi : c'est celui dont l'absence fait refuser le démarrage"
        );
    }

    /// Les chemins **absolus** traversent le chargeur sans être touchés — la
    /// configuration en service les écrit ainsi.
    #[test]
    fn un_chemin_absolu_n_est_pas_touche() {
        let (_dir, chemin) = declaration(
            "addr: 127.0.0.1:9999\n\
             keys: /etc/cles\n\
             stores:\n\
             \x20 - name: s\n\
             \x20   path: /srv/data/s.obrain\n\
             \x20   tenant: public\n",
        );

        let decl = lit_declaration(&chemin).expect("déclaration acceptée");
        assert_eq!(decl.keys.as_deref(), Some(Path::new("/etc/cles")));
        assert_eq!(decl.stores[0].path, PathBuf::from("/srv/data/s.obrain"));
    }

    /// **`~autre` n'est pas deviné** : on ne choisit pas le domicile d'un autre.
    /// Le chemin reste littéral — donc « absent » au constat, ce qui est la
    /// vérité — et il n'est pas silencieusement réécrit vers `$HOME`.
    #[test]
    fn un_tilde_d_un_autre_utilisateur_n_est_pas_devine() {
        let (_dir, chemin) = declaration(
            "addr: 127.0.0.1:9999\n\
             keys: /etc/cles\n\
             stores:\n\
             \x20 - name: s\n\
             \x20   path: ~quelqu_un/data/s.obrain\n\
             \x20   tenant: public\n",
        );

        let decl = lit_declaration(&chemin).expect("déclaration acceptée");
        assert_eq!(
            decl.stores[0].path,
            PathBuf::from("~quelqu_un/data/s.obrain"),
            "seul un `~/` initial est développé"
        );
    }

    /// Une clé inconnue refuse toujours le chargement : le développement du `~`
    /// n'a pas ouvert la porte à une faute de frappe silencieuse.
    #[test]
    fn une_cle_inconnue_refuse_toujours() {
        let (_dir, chemin) = declaration(
            "addr: 127.0.0.1:9999\n\
             cles: ~/.config/cles\n",
        );
        let erreur = lit_declaration(&chemin).expect_err("`cles` n'existe pas");
        assert!(
            erreur.to_string().contains("déclaration invalide"),
            "une faute de frappe doit refuser, pas être ignorée : {erreur}"
        );
    }
}
