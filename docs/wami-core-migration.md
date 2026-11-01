# WAMI — migration de `this` vers l'original publié

Status: **draft (branche `feat/wami-core-migration`, non fusionnée)**
Réf. plan: `4ee15776-e317-4d80-8d05-003a373816cc`

## 1. Contexte

`this` implémente le protocole WAMI **nativement** (feature `wami` = `jsonwebtoken 9`,
`ed25519-dalek`, `argon2`) : `core/auth/wami_provider.rs` (vérification JWT),
`core/auth/sts.rs` (STS maison), et une authz à base de `PolicyExpr` (`descriptor_policy.rs`,
`surface.rs`, `middleware/authorization.rs`).

Ce code cible l'ancien **fork** `reversTeam/wami`, désormais **abandonné** : il a divergé de
l'original `Lsh0x/wami` (base `618b106`, ~0.12) ; il a 8 commits « en avance » mais **redondants**
(le STS JWT #98 a été fusionné upstream) et **43 commits de retard**. L'original est publié sur
crates.io (`wami`, `wami-core`, `wami-condition`) et activement développé (`wami-core` ~0.19-0.20).

**Décision : abandonner le fork, suivre l'original publié.** On ne rebase pas le fork.

## 2. Ce que l'original apporte (depuis ~0.12)

- **Moteur d'autorisation unifié** : `WamiAction` (~84 variantes + wildcard), deny-overrides,
  **ARN `**` globbing** + `${tenant}/${principal}`, **conditions** (`wami-condition` : IP/MFA/
  transport, 60+ opérateurs), **permissions boundaries** (intersection), `WamiContext` enrichi
  (`source_ip`, `mfa_present`, `secure_transport`), provenance (chaîne de délégation).
- **STS JWT Ed25519** : `StsClaims` (sub=ARN, `tenant_id`, `space_id`, `space_role`,
  `scoped_actions`, `scoped_resources`, `kid`, `aud`), `KeyManager` (kid RFC 7638, rotation,
  PKCS8/SPKI), vérification **audience obligatoire**.
- **OAuth 2.0 / OIDC** (RFC 9068, PKCE, M2M, rotation) — `wami::service::oauth`.
- **`wami-scope` / `wami-scope-gql`** — compilation de politiques en prédicats (périmètre de lecture).

## 3. Répartition des crates (important)

| Crate | Contenu utile |
|---|---|
| `wami-core` | `WamiAction`, `WamiContext`(+builder), `WamiArn`/`TenantPath`/`ScopedName`, `PolicyDocument`/`PolicyStatement`, `arn::matching` (`glob_match`) — **primitives seulement** |
| `wami` | `Authorizer` (async, lié à un store), `Decision` (fail-closed), `policy_evaluator` (fonctions **pures** `evaluate_policy_document`), STS (`KeyManager`, `StsClaims`), OAuth/OIDC |
| `wami-condition` | opérateurs/clés de condition |
| `wami-scope`(-gql) | scope en prédicats / fragment GQL |

`wami` est lourd (`tokio full`, `bcrypt`, `regex`, `roxmltree`, `jsonwebtoken 10`). MSRV 1.90.
`this` ne déclare pas de MSRV (rustc local ≥ 1.97) → compatible.

## 4. Décisions d'architecture (à figer)

### D1 — Identité `Uuid` → `(TenantPath u64, ScopedName)`
`this` utilise des `Uuid` (tenants, users) ; wami `TenantPath` = segments `u64`.
**Décision : registre de mapping `Uuid ↔ u64` avec contrôle de collision**, pas une dérivation
brute. **Pourquoi** : une collision de `u64` entre deux tenants = franchissement de tenant
(faille). Le registre garantit l'unicité ; il est posé à la première utilisation d'un tenant.
Fallback documentaire (dérivation déterministe) **interdit** pour des tenants réels.

### D2 — Seam `perm:` synchrone
`this` interroge l'IAM via `PermissionFn = Fn(&str) -> Option<bool>` (**sync**), alors que
`Authorizer::authorize` est **async** et lié à un store.
**Décision : option A — fonctions pures `evaluate_policy_document` (sync)** avec un cache de
`PolicyDocument` par principal. Pas de store wami, pas de changement de signature de `PolicyEnv`.
Le coût : écrire le chargeur/cache de documents et reproduire l'ordre deny-overrides + boundary.

### D3 — Dépendance (mise à jour 2026-11-01) : suivre `main`, pas les crates publiés
**Décision (utilisateur) : « reste sur le main, on aura de l'avance sur le crates ».** On suit le
`main` des crates frères, pas les versions publiées. `this` path-depe déjà `obrain-*` et `vite-rs`
depuis leurs checkouts locaux ; wami rejoint le même schéma via un **worktree local sur
`origin/main`** : `../../obrain/wami-main/crates/wami` (wami 0.19 / wami-core 0.20 /
wami-condition 0.18.1). Conséquence assumée : la publication crates.io de `this` reste bloquée par
les deps `path` (cf. plan `f33522d1`) — on est délibérément **en avance sur le registre**. `wami`
apporte `jsonwebtoken 10` ; `this` garde `9` le temps de la migration (deux piles crypto
coexistent), puis on retire la 9.

### D4 — Tokens (double-acceptation)
wami exige `aud` + `kid` ; les tokens `this` actuels n'en ont pas.
**Décision (telle qu'implémentée, T4)** : le STS émet via `KeyManager` (kid RFC 7638, `aud` =
`DEFAULT_AUDIENCE` = `"this-rs"`, configurable). Les tokens **sans `aud`** (legacy) restent acceptés
tant que `with_legacy_tokens(true)` (défaut) ; passer à `false` ferme la fenêtre. Le refresh et la
porte `verify_handler` contrôlent l'audience par `check_audience`, car jsonwebtoken refuse tout
`aud` présent quand aucune audience attendue n'est fixée.
**Écart assumé avec wami** : le `sub` reste l'UUID utilisateur (et non l'ARN), pour garder le
contrat des consommateurs `this`. Conséquence : les jetons de `this` ne sont **pas** interopérables
avec la struct `StsClaims` de wami (sub ARN, `scoped_*`) ; ils se vérifient avec `KeyManager`
+ une struct de claims propre. Voir §9, question ouverte Q1.

## 5. Carte de migration

| Élément `this` | Équivalent wami | Action |
|---|---|---|
| `wami_provider::StsClaims` | `wami::sts::jwt::StsClaims` | Remplacer (sub=ARN, aud requis, scoped_actions/resources) |
| `sts::StsService::new(PEM)` | `KeyManager::from_bytes([u8;32])` | Adapter (kid dérivé) |
| `issue_token_pair` | `KeyManager::sign_claims_as` + aud | Adapter |
| `jwks_response` | `KeyManager::jwks()` (kid + rotation) | Remplacer |
| `revoke`/`is_revoked` (HashSet) | `OAuthService::revoke_token` (store) **ou** garder HashSet + `jti` | Décider |
| `login` + argon2 + `AnnuaireVerification` | pas d'équivalent (wami = bcrypt/LoginProfile) | **Garder** (conserver argon2) |
| `claims_to_context` → `AuthContext` | `WamiContext` (+ ARN) | Garder `AuthContext` + **pont** vers `WamiContext` |
| `registry_env.permission` (`perm:`) | `policy_evaluator::evaluate_policy_document` (sync) | Brancher (option A) |
| `check_iam` / `Iam::allows` (obrain) | évaluation wami ; `Err(Unreadable*)` → `Unevaluable` | Adapter (repli obrain conservé) |
| `middleware::Decision` / `surface::PolicySource` | collision de noms avec wami | Aliaser à l'import |

### Points de rupture
1. **Principal `(tenant,name)` vs `Uuid`** (D1) — le plus dur.
2. **`Decision` fail-closed** — `Err(UnreadablePolicy/Condition)` ≠ `Deny` ; traiter comme refus.
   `#[serde(deny_unknown_fields)]` sur `PolicyStatement` : un champ inconnu fait refuser tout le doc.
3. **Audience** — l'ADR visait « obligatoire ». Non appliqué : sans `wami.audience`, le vérifieur
   accepte tout `aud` et journalise un `warn` au démarrage. Le rendre obligatoire est une
   décision de déploiement (Q2), car cela casse les configs existantes.
4. **Vocabulaire d'actions** obrain → les CRUD de `this` deviennent des chaînes `this:<entity>:<op>`.

## 6. Risques
- **Sécurité** : collision d'identité (D1) — mitigation par registre.
- **Disponibilité** : tokens rejetés pendant la bascule — mitigation par double-acceptation (D4).
- **Perf** : `wami` lourd ; le chemin `perm:` reste sync (pas d'await dans le middleware).
- **Packaging** : git/path interdits (D3) pour préserver `cargo package`.

## 7. Ordre de travail
1. Dépendance `wami` (feature) + `cargo check` (portillon).
2. Registre d'identité + pont `WamiContext`.
3. STS → `KeyManager` (aud, kid, jwks, double-acceptation).
4. Authz `perm:` → `evaluate_policy_document` (conditions, boundaries).
5. Docs + tests + `this.rs` ; auto-review avant PR.
6. (déféré) OAuth/OIDC + `wami-scope`.

## 8. État de livraison (lot WAMI, branche `feat/wami-core-migration`)

| Tâche | Commit | État | Ce qui est vrai |
|---|---|---|---|
| T1 ADR | `7bca269` | fait | ce document |
| T2 dépendance `wami` main + portillon | `39ce7ad` | fait | `cargo check --features wami` vert |
| T3 registre d'identité + pont | `af1c87d` | fait | `IdentityRegistry` alloue et refuse toute réutilisation ; `wami_context_for` ; `RequestMeta` (X-Forwarded-* ignorés par défaut) |
| T4 STS → `KeyManager` | `1e20d9e` | fait | kid + aud émis ; porte et refresh corrigés (sans cela, tous les jetons étaient refusés) ; test croisé avec le vérifieur `wami_provider` |
| T5 authz `perm:` → wami | `d1b3275` | fait (non-REST) | `wami_policy::decide` ; `WamiEnforcement` porté par l'hôte ; branché dans le guard (gRPC/GraphQL/WS) |
| T5b authz REST → wami | — | **non fait** | le middleware REST (`authorization.rs`) n'appelle pas encore wami |
| T6 docs + revue | ce commit | en cours | voir §9 |
| T7 OAuth/OIDC + `wami-scope` | — | déféré | — |

### Limites connues (à lever avant la mise en production)
- **Registre d'identité en mémoire** : les segments ne survivent pas à un redémarrage. `load()`
  existe pour restaurer une table persistée, mais aucune persistance n'est branchée.
- **Permissions tenant-wide** : le seam `perm:` ne transporte pas la ressource. Une statement
  dont `Resource` nomme une entité précise ne peut pas matcher.
- **Pas de `RequestMeta` sur le guard** : les conditions IP/TLS échouent fermées sur les surfaces
  non-REST.
- **Policies en mémoire** : `PolicyStore` n'est pas persisté.
- **Rotation de clé** : le vérifieur n'a qu'une clé publique ; une rotation réelle demande plusieurs
  clés publiques côté vérifieur.
- **REST** : le middleware d'autorisation REST reste sur l'ancien chemin (T5b).

## 9. Points à trancher (utilisateur)
- **Q1 — `sub` UUID ou ARN ?** Garder l'UUID préserve les consommateurs `this` mais rend les jetons
  non interopérables avec wami. Passer à l'ARN est possible, mais casse le contrat actuel.
- **Q2 — `wami.audience` obligatoire ?** Recommandé en production ; casse les configs sans audience.
- **Q3 — T5b (REST)** : à faire dans ce lot ou dans un lot suivant ? Le REST est la surface principale.

## 10. Guide de migration (ancien → wami)
- Jetons émis avant ce lot : acceptés (pas d'`aud`) tant que `with_legacy_tokens(true)`.
- Jetons émis après : `aud = "this-rs"` et `kid` dans le header. Configurer `wami.audience` pour
  les vérifier strictement.
- `builder.with_wami_enforcement(WamiEnforcement::new(registry, store, instance))` arme wami pour
  les surfaces non-REST. Sans cet appel, le comportement est inchangé.
