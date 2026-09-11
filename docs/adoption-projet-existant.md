# Adopter this-rs + obrain + IAM dans un projet qui existe déjà

Ce document est le mode opératoire, pas une présentation. Il décrit ce qu'un
projet **déjà en production** doit faire pour gagner l'identité canonique,
l'IAM matérialisé et l'application des politiques — sans réécrire son
authentification, ses entités ni son stockage.

Le parcours tient en quatre étapes, et **l'ordre compte** : brancher, observer,
adopter ses identités, armer.

---

## 0. Ce que « natif » veut dire ici, et ce que ça ne veut pas dire

| | |
|---|---|
| **Natif** | un appel branche le store et tout suit (relief, identité, plan IAM, application des politiques, bail d'écriture) ; le projet nomme ses identités avec **sa** fonction ; sa base existante est lue telle quelle — y compris si un autre outil l'a écrite. |
| **Pas natif** | rien n'est deviné. Une politique illisible, une identité étrangère sans mappage, une décision absente : **refus**. Le framework ne comble jamais un trou par une supposition. |

---

## 1. Brancher (une fois)

La CLI prépare le terrain — elle ne décide rien :

```bash
this adopt obrain --report-only      # --dry-run pour voir sans écrire
```

Elle active la feature `obrain` sur votre dépendance `this-rs` (TOML édité sans
toucher à votre mise en forme ni à vos autres clés), insère la ligne de
branchement si `this init` a laissé un marqueur `[this:obrain_store]`, sinon
**affiche** le fragment à recopier, et rappelle l'ordre d'adoption. Elle n'écrit
**aucune** configuration d'authentification : écrire un `auth.yaml` dans un
projet qui tourne pourrait désarmer son authentification.

À la main, l'équivalent tient en une ligne :

```toml
# Cargo.toml
this-rs = { path = "../this-rs/this", features = ["obrain"] }
```

```rust
use std::sync::Arc;
use this::server::ServerBuilder;
use this::storage::ObrainStore;

let store = ObrainStore::open("./data/app.obrain")?;   // ou open_read_only(...)

let host = ServerBuilder::new()
    .register_module(mon_module)?          // vos entités, vos routes : inchangées
    .with_obrain_store(store)              // ← tout le reste suit
    .build_host()?;
```

Ce seul appel obtient :

* le **relief** `GET /graph` — ce que le moteur sait faire, et ce qu'il ne sait pas ;
* l'**identité canonique** : convention `_uuid`/`_data` d'abord, puis identité
  dérivée pour les nœuds à propriétés plates ;
* `GET /graph/identity` — la chaîne d'identité, et `?counts=true` pour un
  comptage **borné** ;
* le **plan IAM** matérialisé (décision indexée par `(principal, portée)`) ;
* l'**application des politiques** `auth:` — **armée par défaut** ;
* le **bail d'écriture** si le store est inscriptible (fail-closed : si le bail
  ne peut pas être pris, `build_host()` échoue en nommant le shard et
  l'échappatoire) ;
* le **service de liens** obrain, si vous n'en fournissez pas : attacher obrain
  ne réclame plus `.with_link_service()`.

---

## 2. Observer **avant** d'armer

C'est l'étape qui évite de casser une production. Vos politiques déclarées n'ont
jamais été vérifiées contre le trafic réel : les activer d'un coup est un pari.

```rust
let host = ServerBuilder::new()
    .register_module(mon_module)?
    .with_obrain_store(store)
    .authorization_report_only()   // décide, journalise, publie — ne bloque rien
    .build_host()?;
```

En mode observation :

* chaque refus calculé part dans les journaux :
  `autorisation : refus OBSERVÉ — mode observation, la requête passe` ;
* la réponse porte `x-this-rs-authz: observed-deny`, plus
  `x-this-rs-authz-policy` et `x-this-rs-authz-scope` — un reverse-proxy peut
  donc les **compter** par route et par politique ;
* **rien n'est bloqué**.

Quand les journaux sont propres, on retire l'appel : c'est le même hôte, armé.

---

## 3. Rendre l'IAM natif à **vos** identités

Par défaut, this-rs suppose son modèle : `User`/`Owner` → `(user_id, tenant_id)`,
`Service` → `(service_name, tenant_id)`, `Admin` → `(admin_id, "global")`.

Si votre projet nomme ses identités autrement — un compte et une organisation,
une clé d'API et un projet, un sujet et un espace de travail — **vous fournissez
la fonction**, et l'IAM s'applique à vos noms :

```rust
use this::core::auth::AuthContext;

let host = ServerBuilder::new()
    .register_module(mon_module)?
    .with_obrain_store(store)
    .with_principal_mapping(|ctx| match ctx {
        AuthContext::User { user_id, tenant_id, .. } =>
            Some((format!("account:{user_id}"), format!("org:{tenant_id}"))),
        _ => None,   // ce contexte n'a pas d'identité chez moi
    })
    .build_host()?;
```

Puis vos politiques s'écrivent avec vos permissions :

```yaml
entities:
  order:
    delete: "perm:orders:write"
    list:   "any:perm:orders:read,role:operator"
```

et la décision est matérialisée **sous vos noms** :

```rust
store.iam().materialize(
    "account:…", "org:…",
    &[RoleGrant { role: "operator".into(), permissions: vec!["orders:write".into()] }],
)?;
```

> **Sans mappage, une identité étrangère est refusée — jamais devinée.** Un
> mappage qui répond `None` rend la politique **non tranchable**, donc refusée :
> c'est fail-closed, et c'est mesurable (voir le test
> `without_a_mapping_a_foreign_identity_model_is_refused_not_guessed`).

---

## 4. Si votre base a été écrite par un autre outil

Rien à faire : branchez-la. Les nœuds sans `_uuid` sont résolus par **identité
dérivée** (uuid v8, reconnaissable), et les identifiants sont **recalculables par
un tiers** (`canonical_bytes_hex` publié par `GET /graph/identity/{id}`).

Mesures réelles obtenues avec ce chemin : **0 par convention / 39 par identité
dérivée** sur une base d'aletheia, **1 553 071** sur un graphe de noyau Linux.
L'outil `examples/adopt_obrain_base` fait le diagnostic hors ligne (balayage
complet), et `GET /graph/identity?counts=true` fait un comptage **borné** en
ligne — le balayage complet d'une grosse base prend des minutes et n'est
**jamais** servi par HTTP.

Trois choses à savoir sur l'identité :

* l'identifiant dérivé est **permanent** : renommer la propriété qui le porte
  change l'identité — et donc les références qui pointaient dessus ;
* l'index d'identité vit **en mémoire** : il repart de zéro à l'ouverture et à
  chaque bascule d'espace ;
* il est **cloisonné par espace** : deux espaces du même store sont deux
  ensembles d'identités.

---

## 5. Si vous voulez basculer vos lectures/écritures de code

Le trait `this::storage::GraphBackend` est implémenté **par les deux** —
`obrain_engine::ObrainDB` (obrain brut) et `this::storage::ObrainStore` :

```rust
fn pages<B: GraphBackend>(backend: &B) -> usize { backend.nodes_by_label("Page").len() }

let n = pages(store.db());      // obrain brut
let n = pages(&obrain_store);   // this-rs par-dessus obrain — MÊME appel
let backends: Vec<&dyn GraphBackend> = vec![&raw_db, wrapped.as_ref()];
```

Le contrat commun couvre : nœuds, propriétés, arêtes, voisinage, degrés, labels,
comptage, recherche par propriété indexée, commit, index de propriété. **Hors
contrat, volontairement** : les espaces nommés et le bail (obrain ne les a pas —
les mettre avec un défaut silencieux laisserait croire qu'il sait les faire).

---

## 6. Revenir en arrière, à tout moment

| Besoin | Appel | Ce que ça coûte |
|---|---|---|
| Ne plus appliquer les politiques | `.without_authorization()` | journalisé en avertissement ; les `auth:` deviennent **décoratives** |
| Ne plus arbitrer les écritures | `.without_write_fencing()` | journalisé ; un second écrivain n'est plus refusé |
| Voir sans bloquer | `.authorization_report_only()` | aucun coût : les décisions sont publiées, pas imposées |

Aucun de ces appels n'est silencieux : chacun écrit une ligne d'avertissement.

---

## 7. Les pièges, tels qu'ils se présentent

1. **Le vocabulaire est clos.** `delete: "admin"` n'est pas `admin_only` : c'est
   refusé, jamais replié sur « authenticated ». Une faute de frappe **ferme** une
   route au lieu de l'ouvrir — c'est le sens du choix.
2. **Fail-closed partout** : politique illisible, décision absente, résolveur
   inconnu, invalidation — refus.
3. **`not:perm:X` ne peut pas transformer un « non tranchable » en
   autorisation** : une liste de refus reste une liste de refus, pas une liste
   d'admission.
4. **`HEAD` est gouverné comme `GET`** — axum sert `HEAD` par le handler `GET` :
   sans cela, une requête `HEAD` atteindrait la route sans sa politique.
5. **Une route inconnue s'abstient** au lieu d'être refusée : c'est la couche
   d'authentification globale qui reste juge. Vérifiez que vous en avez une.
6. **Une décision IAM a un digest de contenu** et une époque monotone ;
   l'invalidation vide les permissions **sans supprimer le nœud**, pour que
   l'époque ne recule jamais.

---

## 8. Checklist de migration

- [ ] `features = ["obrain"]` et un store ouvert (écriture ou **lecture seule**).
- [ ] `.with_obrain_store(store)` — le relief et l'identité doivent répondre.
- [ ] `GET /graph/identity?counts=true` : combien d'entités sont résolues, et par
      quel motif ? Si la convention ne voit rien et la dérivation tout, c'est le
      cas « base écrite par un autre outil ».
- [ ] `.authorization_report_only()` en préproduction **et** en production, une
      journée de trafic : quelles routes, quelles politiques seraient refusées ?
- [ ] Corriger les politiques qui refusaient à tort, **pas** l'inverse.
- [ ] Si vos identités ne sont pas celles de this-rs :
      `.with_principal_mapping(...)`, puis vérifier qu'une décision matérialisée
      sous vos noms est bien trouvée.
- [ ] Retirer `authorization_report_only()` — l'hôte est armé.
- [ ] Vérifier le bail : un second processus doit être **refusé**.
- [ ] `cargo fmt --check` et `cargo clippy --all-features -- -D warnings` (le
      dépôt les exige en pre-push).
