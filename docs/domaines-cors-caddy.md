# Domaines, CORS et délégation à Caddy

Trois réglages qui répondent à la même question — *qui atteint cette
application, et par où* — et qui vivent donc dans le même fichier,
`config/server.yaml`.

```yaml
cors:
  enabled: true
  origins: ["https://app.exemple.fr", "https://admin.exemple.fr"]
  credentials: true          # cookies / Authorization
  max_age_secs: 600          # cache du pré-vol

domains:
  allowed: ["app.exemple.fr", "*.interne.exemple.fr"]
  trust_forwarded_host: true # derrière Caddy
  tenants:
    "app.exemple.fr": "11111111-1111-1111-1111-111111111111"
  subdomain_base: "multi.exemple.fr"   # <uuid>.multi.exemple.fr

proxy:
  caddy:
    enabled: true
    upstream: "127.0.0.1:3000"
    email: "ops@exemple.fr"
    tls: acme                # acme | internal | off
    security_headers: true
    extra_directives: ["encode zstd gzip"]
```

```rust
let host = ServerBuilder::new()
    .register_module(mon_module)?
    .with_server_config_file("config/server.yaml")?   // ou .with_cors(…), .with_domains(…), .delegate_to_caddy(…)
    .build_host()?;
```

**Rien n'est activé par défaut.** Sans ce fichier : aucune couche CORS, aucune
garde de domaine, aucune délégation. Le framework n'invente pas les domaines
d'un déploiement.

---

## 1. Domaines servis

| Réglage | Effet |
|---|---|
| `allowed` **vide** | **aucune garde** : tous les hôtes sont servis (comportement d'avant) |
| `allowed` **renseigné** | tout autre `Host` est refusé en **421 Misdirected Request** |
| `*.exemple.fr` | exactement **un** niveau de sous-domaine (pas `a.b.exemple.fr`) |
| `trust_forwarded_host` | l'hôte effectif vient de `X-Forwarded-Host` (première valeur de la chaîne) |

**Pourquoi une garde** : sans elle, un `Host` forgé fait résoudre des liens
absolus, des redirections et des cookies vers un domaine que l'application ne
sert pas — et un cache intermédiaire peut empoisonner sa clé. La garde est
fermée : un hôte non listé n'a pas de réponse.

**`trust_forwarded_host` n'est pas un confort.** Faire confiance à cet en-tête
sans proxy devant revient à laisser **le client** choisir l'hôte que
l'application croit servir. À n'activer que derrière un proxy qui **remplace**
l'en-tête (c'est ce que fait Caddy) — et c'est précisément ce que dit la
délégation.

### Un domaine peut désigner le locataire

```yaml
tenants: { "app.exemple.fr": "<uuid>" }
subdomain_base: "multi.exemple.fr"     # <uuid>.multi.exemple.fr → locataire
```

L'ordre de résolution du locataire devient : `X-Tenant-Id` (impersonation
explicite) → **domaine servi** → revendications du jeton → `?tenant_id=`.

Le **domaine passe avant le jeton** : une fois qu'un domaine a parlé, un jeton
d'un autre locataire ne peut pas changer de locataire par la parole.

> **Le mappage de locataire n'élargit jamais la liste des hôtes.** Pour servir
> `<uuid>.multi.exemple.fr`, il faut aussi l'autoriser : `allowed:
> ["*.multi.exemple.fr"]`. Un réglage de locataire ne doit pas ouvrir des hôtes
> par effet de bord.

## 2. CORS

| Réglage | Défaut | Remarque |
|---|---|---|
| `enabled` | `false` | sans lui, **aucun** en-tête CORS n'est posé |
| `origins` | — | une origine absente de la liste n'obtient aucun en-tête : le navigateur bloque |
| `methods` | `GET POST PUT PATCH DELETE OPTIONS` | `*` est refusé |
| `headers` | `authorization, content-type, x-tenant-id` | |
| `expose_headers` | vide | ce que le navigateur a le droit de **lire** dans la réponse |
| `credentials` | `false` | `origins: ["*"]` avec `credentials: true` est **refusé** : les navigateurs rejettent cette combinaison |
| `max_age_secs` | — | durée de cache du pré-vol |

**La couche CORS est la plus externe**, devant l'authentification — et ce n'est
pas un détail : un **pré-vol** (`OPTIONS`) ne porte pas d'en-tête
`Authorization`. S'il rencontrait l'authentification d'abord, il recevrait 401 et
le navigateur refuserait un appel pourtant légitime. Vérifié par test :
`a_preflight_passes_without_a_token_even_when_authentication_is_installed`.

Une configuration impossible (aucune origine alors que CORS est activé, `*` avec
identifiants) **fait échouer la construction**, pas le premier appel.

## 3. Délégation à Caddy

```bash
cargo run --example caddyfile -- config/server.yaml > /etc/caddy/Caddyfile
caddy validate --config /etc/caddy/Caddyfile && caddy reload
```

Le `Caddyfile` est rendu **depuis la même configuration** : mêmes domaines (un
bloc de site par hôte servi), même upstream. Il contient :

* un bloc global avec `email` (ACME) si renseigné ;
* `reverse_proxy <upstream>` ;
* `tls internal` / `tls off` si demandé (ACME ne s'écrit pas : c'est le défaut) ;
* les en-têtes de sécurité usuels (`security_headers: true`) ;
* `extra_directives` recopiées telles quelles — l'échappatoire pour un besoin
  que ce schéma ne couvre pas.

**Ce que la délégation veut dire, et ne veut pas dire.**

* this-rs **décrit** ; Caddy **applique**. Le fichier est du texte : on le relit.
* **Le CORS est posé par l'application, pas par Caddy** — et le `Caddyfile` le
  dit en commentaire. Une seule couche décide : sinon deux jeux d'en-têtes se
  contredisent (`Access-Control-Allow-Origin` en double est un refus de
  navigateur).
* this-rs **ne pilote pas** Caddy : pas de rechargement automatique, pas
  d'écriture de fichier de configuration, pas d'appel à son API.

## 3 bis. La chaîne complète, et le harnais

```text
config/server.yaml
   ├─ domaines servis ──► PowerDNS   (A / AAAA / CNAME)
   ├─ CORS / garde ─────► this-rs    (la couche d'entrée)
   └─ proxy.caddy ──────► Caddyfile  (TLS, routage, en-têtes)
```

```yaml
dns:
  api_url: "http://127.0.0.1:8081"
  api_key_env: "PDNS_API_KEY"     # le NOM de la variable — jamais la clé
  zone: "exemple.fr"
  target: "203.0.113.10"          # la cible des enregistrements dérivés
  record_type: A                  # A | AAAA | CNAME
  ttl: 300
  prune: false                    # supprimer ce qui n'est plus déclaré ?
```

```bash
cargo run --features powerdns --example domain_harness -- config/server.yaml
cargo run --features powerdns --example domain_harness -- config/server.yaml --plan-dns
PDNS_API_KEY=… cargo run --features powerdns --example domain_harness -- config/server.yaml --apply-dns
```

**Une seule déclaration, trois délégations** : la liste `domains.sites` alimente
le `Caddyfile` **et** les enregistrements DNS. Pas deux inventaires à tenir en
accord.

### Ce que le framework s'autorise à toucher — et jamais

| | |
|---|---|
| Types gérés | **A**, **AAAA**, **CNAME** uniquement. Jamais MX, TXT, SRV, NS, SOA |
| Zone | uniquement la zone déclarée (`dns.zone`) ; un nom d'une autre zone est signalé, jamais touché |
| Suppression | **seulement** si `prune: true` ; par défaut un enregistrement géré mais non déclaré est **signalé**, pas effacé |
| Zone absente | `ensure_zone` ne la crée que si des serveurs de noms sont fournis (sinon la zone ne serait pas servie) — et ne recrée **jamais** une zone existante |
| Clé d'API | lue dans l'environnement, **jamais** depuis le fichier (qui se versionne), **jamais** journalisée ni incluse dans une erreur |
| Écriture | `--apply-dns` explicite ; sans lui, le harnais n'écrit rien |
| Refus du serveur | c'est une **erreur**, jamais un succès silencieux : un domaine cru publié et non publié est pire qu'un échec |

Le **plan** ([`this::dns::plan`]) est une fonction pure : il se relit, il se
teste sans réseau, et il dit exactement ce qui serait créé, modifié ou supprimé.

## 4. Ce que l'application peut lire à l'exécution

```rust
let résumé = host.exposure();
// cors_enabled, cors_origins (nombre), domains_guarded, hosts_allowed,
// tenants_mapped, subdomain_tenants, forwarded_host_trusted, caddy_delegated

let caddyfile = host.caddyfile();   // Some(…) si la délégation est configurée
```

Le résumé donne des **comptes et des faits, jamais les listes** : la liste des
origines et celle des domaines appartiennent à l'application, et les publier
dirait à un inconnu quels hôtes viser. Un consommateur qui veut savoir *si* une
garde est active lit le résumé ; un opérateur qui veut les valeurs lit sa
configuration.

## 5. Ordre des couches (à connaître si vous montez votre propre routeur)

En axum, **la dernière couche appliquée est la plus externe**. L'ordre voulu est :

```
CORS → domaine servi → authentification → locataire → politiques → handler
```

donc on applique dans l'ordre inverse : politiques d'abord, CORS en dernier.
`RestExposure::build_router` le fait pour vous ; si vous composez votre propre
routeur, reproduisez cet ordre — et souvenez-vous qu'un **repli**
(`fallback_service`, c'est le cas du frontend) **remplace** ce que les couches
enveloppaient : montez le repli **avant** elles (voir
`tests/frontend_surface.rs`).
