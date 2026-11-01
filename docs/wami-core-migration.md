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

### D3 — Dépendance
**Décision : versions publiées sur crates.io** (`wami`, `wami-core`, `wami-condition`), derrière la
feature `wami`. Pas de `git`/`path` → `cargo package` reste possible. On bump à 0.19/0.20 quand
publiées. `wami` apporte `jsonwebtoken 10` ; `this` garde `9` le temps de la migration (deux piles
crypto coexistent), puis on retire la 9.

### D4 — Tokens (double-acceptation)
wami exige `aud` + `kid` ; les tokens `this` actuels n'en ont pas.
**Décision : fenêtre de double-acceptation** — accepter les tokens legacy (this-signed) et les
nouveaux (wami `KeyManager`) pendant la migration ; `TypePolicy::Lenient` d'abord, puis `Strict`.
Une valeur d'audience par déploiement. Contrôle `iss` fait **à la main** (wami ne le compare pas).

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
3. **Audience obligatoire** — tokens en circulation rejetés sans fenêtre de double-acceptation.
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
