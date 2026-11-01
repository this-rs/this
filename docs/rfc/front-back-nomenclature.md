# RFC — Nomenclature front/back : un nom, un propriétaire, une dérivation

**Statut** : proposé, 1er novembre 2026. **Portée** : `this`, `this-cli`, `this-ui`, `this-leptos`.
**RFC lié** : [`module-descriptor.md`](module-descriptor.md). Celui-ci ne le remplace pas : il **nomme**
ce que l'autre **déclare**. Une divergence entre les deux textes est un défaut du second.

> **Marqueurs.** `LU` = constaté par lecture du code, fichier et ligne cités. `DÉDUIT` = conclusion non
> exécutée. `MESURÉ` = commande lancée, sortie visible. **Aucune ligne de ce document n'est `MESURÉ`** :
> la rédaction est un travail de lecture. Les commandes à lancer pour promouvoir chaque affirmation sont
> données au §8.

---

## 1. Problème

Le socle `ModuleDescriptor` a réglé la question « qui déclare quoi » pour la donnée. Il n'a pas réglé la
question « qui **nomme** quoi » pour ce qui traverse la frontière. Le résultat, constaté :

**1.1 Deux vérités pour le client** — `derive/mod.rs:32-34` écrit « TypeScript derivation is out of this
batch (no consumer: the front is Leptos) » (LU). Trois semaines plus tard,
`this-cli/src/codegen/typescript.rs:12` (`pub fn generate`) produit un client API TypeScript complet, appelé
depuis `commands/generate.rs:60`, à partir de `codegen/introspect.rs`, qui lit le code Rust **par regex**
(LU, `introspect.rs:1-5`). Le client n'est donc pas dérivé du descripteur mais d'une seconde lecture
du source. Deux producteurs pour un nom, c'est exactement la faute que `this.descriptor.declared_twice`
refuse dans le descripteur.

**1.2 L'espace d'URL n'a pas de propriétaire** — `server/exposure/descriptor.rs:35` déclare
`pub const PREFIX: &str = "/_this"` avec le commentaire « Prefix of the framework's own routes » (LU). Quatre
routes l'utilisent. Mais `/ui/state/{portee}` (`ui/routes.rs:34`), `/graph*` (`graph/routes.rs:104-111`),
`/notifications/*` (`exposure/rest/notifications.rs:56-59`), `/events/stream` (`exposure/rest/mod.rs:82`) et
`/health` (`exposure/rest/mod.rs:364`) sont **à la racine**, donc dans le même espace que `/{plural}` des
entités. `check.rs` ne connaît que `reserved_namespace` (pour `this.`) et `tag_reserved` : aucune liste de
pluriels réservés n'existe (LU). Une entité nommée `graph` ou `notification` produit une route en collision ;
axum refuse deux routes identiques, mais par un **panic au montage**, pas par un `ContractError` (DÉDUIT).

**1.3 Le nom `ui` est déjà pris deux fois** — `this/src/ui/` est un document d'état d'interface servi en
REST (`ui/mod.rs:1-20`), pas une vue. Le jour où `this-ui` existe, `this::ui` et `this-ui` désignent deux
choses différentes à une lettre près.

**1.4 Le gabarit webapp pointe une route qui n'existe pas** — `templates/webapp/App.tsx.tera` appelle
`fetch("/api/health")`, `vite.config.ts.tera` proxie `/api` **sans rewrite**, et le serveur sert `/health` à
la racine. La page d'accueil d'un projet généré affiche donc « error » face à un hôte sain (DÉDUIT : LU des
trois côtés, jamais exécuté).

**1.5 La chaîne Leptos n'est pas prouvée sur sa cible** — `ci.yml:78-85` lance
`cargo test -p this-descriptor --all-features` sur l'hôte (feature `leptos` incluse) puis
`cargo check -p this-descriptor --target wasm32-unknown-unknown --features yaml,pattern` : **`leptos` est
exclu du contrôle wasm32** (LU). Le CHANGELOG (`:355`) annonce pourtant « y compris en wasm32 ».

**1.6 Le vocabulaire de rendu n'existe pas** — il n'y a aucun nom pour un formulaire, une table, une
cellule, une server function, un island. Le plan `4b8d885a` les emploie (`EntityForm`, `EntityTable`,
`this-ui`, `this-leptos`) sans qu'aucun texte normatif ne les fixe.

---

## 2. Philosophie

Quatre principes, hérités du descripteur, appliqués aux noms :

1. **Un propriétaire par nom.** Un nom produit à deux endroits est une redéclaration, quelle que soit la
   ressemblance des deux productions.
2. **Une dérivation est une fonction pure et testée**, jamais un `format!` dans un handler ni une regex sur
   du code source.
3. **La casse suit le protocole, pas le goût.** `snake_case` à la frontière ; `PascalCase` pour les types ;
   `camelCase` uniquement là où un protocole l'impose — et c'est alors la dérivation qui l'applique.
4. **Un crate = une couche, pas un consommateur.** `this-descriptor` porte le contrat, `this-ui` le rendu,
   `this-leptos` le transport. Aucun des trois ne connaît l'application qui les assemble.

---

## 3. Les espaces de noms

Chaque ligne est un espace **fermé** : un producteur, des consommateurs, un contrôle. Un nom nouveau qui
n'entre dans aucune ligne est un défaut de ce document, pas une exception.

| # | Espace | Forme | Propriétaire (producteur) | Contrôle |
|---|---|---|---|---|
| N1 | module | `snake_case`, `^[a-z][a-z0-9_]*$` | `ModuleDescriptor.name` | `this.module.identity_mismatch` |
| N2 | entité (singulier) | `snake_case`, même grammaire | `EntityDesc.name` | identité trait ↔ descripteur |
| N3 | **pluriel** | `Pluralizer::pluralize(name)` | `EntityDesc.plural` | `this.module.plural_mismatch`, `duplicate_plural` |
| N4 | champ | `snake_case` | `FieldDesc.name` | `this.validation.unknown_field` |
| N5 | tag proto | entier, `1..=15` réservés, `8` = `tenant_id`, `>=16` déclaré | `FieldDesc.proto_tag` + `descriptor.lock` | `this.descriptor.tag_reused` |
| N6 | type | `PascalCase` | dérivé | — |
| N7 | code d'erreur | `<ns>.<sujet>.<raison>`, `this.` réservé | `FRAMEWORK_ERRORS` (`framework.rs:92`) ou `ModuleDescriptor.errors` | `duplicate_error_code`, `reserved_namespace` |
| N8 | clé de message | **identique à N7** | `I18nDecl.keys` / `MessageBundle` | le catalogue refuse un bundle incohérent |
| N9 | route REST | `/{plural}`, `/{plural}/{id}` | `EntityDescriptor::build_routes` | aucun (voir §6.1) |
| N10 | champ GraphQL | `camelCase` | `derive::graphql` | SDL |
| N11 | message gRPC | `PascalCase` + `Create`/`Update` | `derive::proto` | `TagMode::Strict` |
| N12 | server fn | `/api/<module>/<fn><hash>` | macro `#[server]` de `this-leptos` | unicité du `PATH` à la compilation |
| N13 | composant | `Entity<X>`, `<Entity><X>Island` | `this-ui` | — |
| N14 | route framework | `/_this/<sujet>` | `exposure::descriptor::PREFIX` | — (voir §6.2) |
| N15 | feature Cargo | un nom par surface : `leptos`, `frontend`, `graphql`, `grpc` | `Cargo.toml` | revue |

**Notes de lecture.**

- N1 → N8 sont **déjà** tenus par le descripteur. Ce document ne les invente pas, il les inscrit pour
  qu'ils ne soient plus rediscutés à chaque nouvelle surface.
- N12 est le seul espace entièrement neuf. `server_fn` le rend mécanique : `PATH = prefix + module_path +
  fn_name + hash`, le préfixe étant `/api` par défaut, surchargeable par `SERVER_FN_PREFIX` ou
  `#[server(prefix = "...")]`, et le chemin de module activé par `SERVER_FN_MOD_PATH`
  (LU, `leptos_macro/src/lib.rs:1013-1020`, `server_fn_macro/src/lib.rs:489-500`). Le hash garantit
  l'unicité, donc **le nom de la fonction n'a pas besoin d'être unique globalement** — mais il doit rester
  lisible : c'est lui qui apparaît dans une trace réseau.
- N15 : une feature porte le nom de la **surface** qu'elle ouvre, jamais celui du consommateur. `frontend`
  ouvre l'hébergement SPA ; `leptos` ouvrira le pont Leptos. Les deux restent hors de `all` tant qu'elles
  dépendent d'un checkout local (LU, `Cargo.toml` : commentaire de `frontend`).

---

## 4. Les six règles

**R1 — Un nom, un propriétaire.** Deux producteurs = une redéclaration, même faute que
`this.descriptor.declared_twice`. Le remède est toujours le même : supprimer le second producteur, pas
comparer les deux sorties.

**R2 — Une dérivation est une fonction pure et testée.** Elle vit dans un module nommé pour ce qu'elle
produit (`derive::graphql`, `derive::proto`, `naming::Pluralizer`, `engine::validate`). Un `format!` dans un
handler, une regex sur du source ou un `HashMap` construit à la main dans une route sont des violations.

**R3 — La casse suit le protocole.** `snake_case` partout à la frontière (JSON, query string, identifiants) ;
`PascalCase` pour les types ; `camelCase` seulement pour les champs GraphQL, et par la dérivation. Aucun nom
n'est « traduit » à la main entre deux couches.

**R4 — `this.` et `/_this` sont réservés.** Un module ne les utilise jamais. Toute route que le framework
ajoute naît sous `/_this`. Le préfixe existe déjà : `pub const PREFIX: &str = "/_this"`
(`exposure/descriptor.rs:35`).

**R5 — Le front ne réécrit aucune règle.** Un composant qui écrit `required`, `max_length`, une liste de
valeurs ou un libellé est faux **par construction** : il lit `FieldDesc`, `derive::payload_required` et
`I18nCatalog`. C'est la règle d'or du descripteur transposée au rendu : Leptos *rend* le descripteur, il ne
le redécrit pas.

**R6 — Un crate = une couche.** `this-descriptor` (contrat, wasm32, sans tokio/axum/tonic), `this-ui`
(rendu, sans transport), `this-leptos` (transport + hôte). Un crate qui importe une couche supérieure est
une erreur d'architecture, pas une commodité.

---

## 5. Ce que la nomenclature interdit — cas observés

| Anti-pattern | Où il est aujourd'hui | Règle violée |
|---|---|---|
| Dériver un client depuis le source Rust plutôt que du descripteur | `this-cli/src/codegen/introspect.rs` | R2 |
| Deux textes normatifs sur le même sujet | `derive/mod.rs:32` vs `codegen/typescript.rs` | R1 |
| Route framework à la racine, dans l'espace des pluriels | `/ui`, `/graph`, `/notifications`, `/events` | R4 |
| Nom de module qui deviendra un nom de crate | `this::ui` vs `this-ui` | R1, R6 |
| Gabarit qui code une URL en dur | `App.tsx.tera` → `/api/health` | R3, R5 |
| Preuve annoncée plus large que le test | CHANGELOG `:355` vs `ci.yml:81-85` | — (honnêteté des marqueurs) |

---

## 6. Trois corrections que la nomenclature rend obligatoires

### 6.1 Réserver les pluriels (bloquant avant `this-ui`)

`EntityDesc::normalize` dérive le pluriel, `ServerBuilder` vérifie qu'il n'est pas dupliqué — mais pas qu'il
n'entre en collision avec une route du framework. Le contrôle doit précéder l'usage :

- `check.rs` refuse un pluriel appartenant à `["_this", "health", "healthz", "graph", "ui", "events",
  "notifications"]`, avec un code stable (`this.module.duplicate_plural` suffit : la raison est la même).
- Le refus arrive **au build**, pas au montage du routeur.
- Un hôte qui ajoute une route framework à la racine doit l'inscrire dans cette liste dans le même commit.

### 6.2 Rapatrier les routes framework sous `/_this`

`/_this/descriptors`, `/_this/schema/{entity}/{op}` et `/_this/openapi.json` y sont déjà. Les autres non.
Deux issues, à trancher dans la même PR :

- **(a)** déplacer `/ui/state/*`, `/graph/*`, `/notifications/*`, `/events/stream` sous `/_this`. Casse les
  clients existants — acceptable si aucun hôte public n'est encore en production, à vérifier.
- **(b)** garder les chemins et écrire la liste réservée au §6.1, en documentant que la racine est
  **partagée** entre entités et framework.

Recommandation : (a) pour `/ui` et `/graph` (jeunes, peu consommés), (b) pour `/health`, `/notifications` et
`/events/stream` (déjà documentés dans `integration-consommateurs.md`).

### 6.3 Trancher le client TypeScript

`codegen/typescript.rs` est aujourd'hui un client complet produit par regex. Deux sorties, pas trois :

- **le garder** → `derive::ts_client(entity, op)` dans `this-descriptor` (pure, wasm32, testée), et
  `codegen/introspect.rs` devient un simple porteur de `ProjectIntrospection` alimenté par le descripteur ;
- **le supprimer** → `derive/mod.rs:32` redevient vrai, et le gabarit webapp ne génère plus de client.

Recommandation : le garder et le dériver. Le front React existe, il ne disparaîtra pas parce qu'on l'ignore.

---

## 7. Impact fichier par fichier

| Fichier | Nature |
|---|---|
| `crates/this-descriptor/src/check.rs` | + liste de pluriels réservés (§6.1) |
| `crates/this-descriptor/src/derive/ts_client.rs` | nouveau, si §6.3 « garder » |
| `crates/this-descriptor/src/derive/mod.rs` | + `ts_client`, correction du commentaire `:32` |
| `this-cli/src/codegen/typescript.rs` | réécrit sur le descripteur, ou supprimé |
| `this/src/ui/routes.rs`, `src/graph/routes.rs` | préfixe `/_this` (§6.2 a) |
| `this-cli/src/templates/webapp/App.tsx.tera` | `/api/health` → `/health` (ou montage `/api`) |
| `.github/workflows/ci.yml` | + `cargo check --target wasm32 --features leptos` (§1.5) |
| `CHANGELOG.md` | corriger la portée du « y compris en wasm32 » |
| `docs/atlas/` | diagramme de la chaîne de noms (mermaid.ffs.dev, hors dépôt) |

---

## 8. Alternatives rejetées

**Un crate `this-front` unique.** Regrouper rendu et transport dans un seul crate simplifierait l'ajout,
mais interdirait `this-ui` dans un front non-Leptos et forcerait `server_fn` dans toute application qui veut
seulement un formulaire. Rejeté : la couche de rendu n'a aucune raison de connaître un protocole.

**Des indices de rendu dans `FieldDesc` (`ui: { widget, hidden }`).** Plus puissant pour rendre un module
*stocké* sans code, mais ajoute une facette `ui` au modèle de données et couple le contrat wasm à un moteur
de rendu. Rejeté — déjà acté par la décision `0b8a60a3` du plan `4b8d885a`.

**Nommer les server functions à la main.** `server_fn` hash le chemin, donc l'unicité est gratuite ; imposer
un nom unique global serait une contrainte sans bénéfice. Rejeté : seul le **préfixe** est normé (`/api`),
pas le nom de la fonction.

**Supprimer le client TypeScript.** Cohérent avec `derive/mod.rs`, mais laisse le gabarit webapp sans accès
typé à l'API. Rejeté tant que le front React est le front réel.

---

## 9. État et mesures à faire

Rien dans ce document n'est `MESURÉ`. Pour promouvoir chaque affirmation :

| Affirmation | Commande |
|---|---|
| §1.5 (leptos hors wasm32) | `cargo check -p this-descriptor --target wasm32-unknown-unknown --features leptos` |
| §1.2 (collision de routes) | déclarer une entité `graph` dans un module, démarrer l'hôte, observer |
| §1.4 (gabarit cassé) | `this init` + `this dev`, ouvrir la page, lire le statut |
| §6.3 (client TS identique au descripteur) | générer sur `examples/rest`, comparer les deux sorties |

**Suite** : plan `4b8d885a` (this-ui, this-leptos, hébergement, modules). Ce document en fixe le vocabulaire ;
les tâches `85cb0520` et `c22851bc` en sont la première preuve.
