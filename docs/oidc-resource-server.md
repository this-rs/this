# OIDC resource server — what `this` verifies, and what it refuses

Status: **livré** (PR #92). Suite de [#87](wami-core-migration.md) (WAMI fusionné) et de la PR #90
(un provider configuré mais inutilisable refuse le démarrage).

## 1. La décision : resource server, pas serveur d'autorisation (D11)

`this` **vérifie** des jetons ; il n'en émet pas, sauf par son STS embarqué (modes `embedded` et
`bootstrap`, pour le développement et les petits déploiements).

Devenir un serveur d'autorisation OAuth — Authorization Code + PKCE, consentement, rotation de
refresh tokens, `client_credentials` M2M, métadonnées de discovery pour *ses propres* jetons — est
le métier de wami, qui l'implémente déjà. Le refaire ici créerait deux serveurs d'autorisation à
maintenir, dont un sans les tests de conformité de l'autre.

| Besoin | Réponse |
|---|---|
| Un IdP externe (Keycloak, Auth0, Entra, Dex) émet les jetons | `provider: oidc` — vérification par découverte + JWKS |
| Un émetteur local pour le développement ou un petit déploiement | `provider: wami`, mode `embedded`/`bootstrap` |
| Un vrai serveur OAuth (PKCE, consentement, M2M) | wami, déployé à part, et `this` le vérifie en `oidc` |

## 2. Ce que la vérification fait

1. **Découverte paresseuse** — `auth.oidc.discovery_url` → `{issuer, jwks_uri}` → jeu de clés.
   Le jeu est mis en cache 15 minutes (`OidcAuthProvider::with_ttl` pour changer).
2. **Sélection par `kid`** — un `kid` inconnu déclenche **un** refetch, sérialisé par un mutex : une
   rafale de requêtes coûte un aller-retour, pas un par requête. C'est ce qu'une rotation de clés
   produit vu d'ici. Un JWKS à clé unique sans `kid` répond aussi.
3. **Claims** — `iss` doit être l'issuer découvert, `aud` doit nommer ce resource server, `exp` est
   requis (60 s de tolérance d'horloge, la valeur par défaut de `jsonwebtoken`).

### Pourquoi la découverte est paresseuse

`ServerBuilder::build_host` est **synchrone** : il ne peut pas faire d'aller-retour HTTP. La
découverte se fait donc au premier jeton. Un échec est **fermé** : pas de clé, donc pas d'identité
vérifiée, donc requête non authentifiée. Un hôte qui veut l'aller-retour au démarrage appelle
`OidcAuthProvider::warm_up()` depuis son chemin async.

C'est une limite assumée, pas un oubli : rendre le boot asynchrone toucherait tous les binaires de
l'écosystème pour un contrôle que la première requête rend de toute façon.

## 3. Ce qui est refusé, et pourquoi

| Refus | Raison |
|---|---|
| `HS256`/`HS384`/`HS512`, `none` | Un resource server ne partage aucun secret avec l'émetteur ; un jeton symétrique ferait de chaque vérificateur un émetteur. |
| `aud` étrangère | Un jeton émis pour un autre service ouvrirait toutes les portes d'ici. `auth.oidc.audience` est **obligatoire** : un resource server qui ne sait pas nommer son audience n'accepte aucun jeton. |
| `iss` étranger | La clé publiée par A ne doit pas vérifier un jeton prétendant venir de B. |
| Jeton sans tenant, ou tenant non-UUID | Le mapper sur l'UUID nil le placerait dans un tenant qui existe vraiment. Même règle que pour WAMI. |
| Provider injoignable | Fermé : jamais « laissons passer en attendant ». |
| Provider configuré sans `oidc` compilé | Refus au démarrage (PR #90) au lieu d'un hôte sans authentification. |

`Jwk::is_supported` n'est **pas** appelé : il déballe `key_algorithm`, donc un JWKS dont les entrées
omettent `alg` — permis par la spec, publié par plusieurs providers — ferait paniquer le thread de
requête. La forme de la clé décide, et la liste d'algorithmes acceptés est vérifiée sur l'en-tête du
jeton, là où elle a du sens.

## 4. Le `scope` du jeton (RFC 9068)

Un jeton OIDC porte `scope` — une seule chaîne, séparée par des espaces (RFC 6749 §3.3). Elle est
lue dans `AuthContext::User::scopes`.

**Règle** : un scope accorde `perm:` **là où aucun plan IAM ne tranche**. Un plan IAM branché garde
le dernier mot, donc un scope ne peut jamais transformer un refus existant en autorisation :
`admin_only` refuse encore un jeton qui porte `*`. C'est un **repli**, pas une couche d'autorisation
parallèle.

Correspondance : `perm:orders:read` est accordé par `orders:read`, `orders.read`, `orders:*`,
`orders.*` et `*`. Une famille s'arrête au point (`order*` n'accorde rien), et un scope ne nomme que
ce qu'il accorde — aucune permission n'est élargie.

Les deux séparateurs sont acceptés parce qu'un descripteur écrit `perm:orders:read` tandis qu'un IdP
publie `orders:read` : c'est la même permission.

## 5. `sub` opaque → UUID stable

Les sujets OIDC sont opaques (`auth0|abc`, un email). `this` identifie par `Uuid`.

- un `sub` déjà UUID est utilisé tel quel ;
- sinon `user_id = UUIDv5(NAMESPACE_URL, sub)`, **dérivé du sujet seul**, donc stable entre
  redémarrages et entre instances, sans table de correspondance.

Limite honnête : deux émetteurs qui publieraient le même `sub` pour deux personnes distinctes
donneraient le même `user_id`. La dérivation ne mélange pas l'issuer dans le nom pour rester stable
si l'`issuer` change de forme (`https://idp/` contre `https://idp`). Un déploiement multi-IdP doit
donc préfixer les sujets, ou fournir des `sub` UUID.

## 6. Ce qui n'est pas fait (et pourquoi)

**`wami-scope` — restreindre les lignes d'une liste.** `perm:` tranche un appel entier ; il ne sait
pas dire « tu peux lister, mais seulement ces lignes ». `wami-scope` compile une portée en
`Predicate` (exact, préfixe, et, ou, non) **destiné à être poussé dans le store avant la
pagination** — c'est explicite dans sa documentation : un filtre appliqué après coup fait perdre à
un top-k les lignes qu'un appelant étroitement scopé devrait voir.

`this` a neuf backends (in-memory, DynamoDB, Postgres, MySQL, MongoDB, Neo4j, ScyllaDB, LMDB,
Obrain) sans capacité de filtre commune. Porter le prédicat demande **un traducteur par backend**,
comme wami le fait (`wami-scope-gql`, `wami-scope-sql`). C'est un chantier à part entière, pas une
étape de cette PR : il se planifie par backend, avec le même harnais de conformité que wami
(`wami-scope-tests` : un monde, l'évaluateur comme oracle).

En attendant, le `scope` agit comme un **filtre d'admission** (repli de `perm:`), pas comme un
restricteur de lignes. Un appelant scopé peut lister toutes les lignes que son tenant autorise.

## 7. Configuration

```yaml
auth:
  provider: oidc
  oidc:
    discovery_url: https://idp.example.com/.well-known/openid-configuration
    audience: https://api.example.com      # obligatoire
    user_claim: sub                        # défaut
    roles_claim: roles                     # défaut
  tenant:
    claim_field: tenant_id                 # défaut ; doit être un UUID dans le jeton
```

`warm_up()` (Rust) pour forcer la découverte au démarrage plutôt qu'à la première requête.
