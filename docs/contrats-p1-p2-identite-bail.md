# Contrats d'identité, de bail, d'IAM et de journal — document canonique

**Ce fichier vit dans le dépôt** (`this/docs/`) : il est donc versionné avec le
code qu'il décrit. Il a été consolidé depuis `this-rs/docs/` (hors dépôt) le
2026-09-11, parce qu'une documentation non versionnée n'est pas une
documentation collective.

---

# Contrats P1 (identité engine-agnostique) et P2 (bail d'écriture + fencing)

**Nature** : contrats d'implémentation issus de l'orchestration à 9 agents du 2026-09-11.
**Rattachement** : programme « même entité quel que soit le moteur » (§4, chantiers P1 et P2).
**Dépôt concerné** : `this-rs` — **sans filet git** (`.git` ne contient que `hooks/`).

> Ce fichier est l'**artefact durable**. Le graphe en reçoit l'index (ADR + liens).
> Leçon de l'orchestration : 9 agents ont rendu ~400 Ko ; 51 Ko ont survécu au
> passage par le résultat d'outil. **Tout agent de conception écrit son artefact
> dans un fichier et ne retourne qu'un index (chemin + résumé ≤ 2 Ko).**

---

## P1 — Identité engine-agnostique

### Principe
Séparer **la projection** (pure, par nœud, sans I/O : toute la sémantique
d'identité) de **la localisation** (I/O, par moteur). C'est ce qui rend
l'identité testable sans obrain, sans base, sans build.

### Trois règles non négociables
1. **La convention `_uuid`/`_data` reste prioritaire et non déplaçable** — à
   l'index 0 du registre ; `push()` insère après. Conséquence : sur une base
   écrite par this-rs, la résolution est **identique à aujourd'hui** ⇒ les
   842 `#[test]` + 523 `#[tokio::test]` de `this/src` restent verts **sans
   retouche**. Si un test existant doit être édité, le patch est faux.
2. **Un `_uuid` illisible ARRÊTE la chaîne** (`Malformed` ≠ `NotApplicable`) :
   re-cléer créerait une **seconde identité** pour une entité qui en a déjà une.
3. **L'index d'identité ne partage pas la `HashMap` d'`ObrainDataService`** —
   champ séparé, jamais lu par `get/list/update/delete/search` : c'est la
   décision qui rend le patch **additif** et non substitutif.

### Identité canonique
```rust
pub const ENTITY_ID_SCHEME: &str = "this-rs/entity-id/sha256-128/v1";   // GELÉ
pub struct EntityKey { space: Option<String>, label: String, qualifier: Qualifier }
pub enum Qualifier {
    Natural { property: String, value: String },
    Content { digest: String },
}
```
- **Encodage canonique** : domaine `b"this-rs/entity-id/v1\0"`, puis chaque champ
  `u32 LE len || octets` ⇒ **injectif** : `("a","bc")` et `("ab","c")` ne peuvent
  pas collisionner (une concaténation de chaînes, si).
- **`uuid()` = `sha256(canonical_bytes)[..16]`**, puis version **8** (RFC 9562) :
  un id dérivé est *reconnaissable* et ne peut pas être confondu avec un v4 écrit
  par la convention.
- **Le nom de la propriété fait partie de la clé** — sans lui, un nœud dont
  `name` vaut le `qualified_name` d'un autre reçoit son id : fusion silencieuse.
- **SHA-256, pas le FNV existant** (`examples/aletheia_server.rs:277` `synth_uuid`) :
  l'id est une clé *permanente* et un `qualified_name` vient d'un client (collision
  adverse). `sha2 0.10.9` est déjà dans `Cargo.lock` et extrait localement ⇒ une
  ligne de `Cargo.toml`, **non optionnelle** : un `sha2` optionnel produirait deux
  espaces d'identité selon les features de build — le pire défaut possible pour un
  identifiant. Repli possible (FNV-1a-128) mais le schéma doit être choisi **avant**
  toute publication d'id, jamais après.

### Projections
- `ConventionProjection` — index 0. `_uuid` absent ⇒ `NotApplicable` ; présent et
  non-UUID ⇒ `Malformed` (arrêt). `data` = `_data` désérialisé s'il existe (il fait
  autorité), sinon les propriétés plates. **Jamais de recalcul.**
- `FlatPropertyProjection` — clé = 1ʳᵉ propriété candidate présente et non vide parmi
  `["qualified_name","qn","id","_id","key","name","title"]` ; **label canonique =
  plus petit label non réservé**, jamais `labels[0]` (qui est en ordre d'insertion).
- `ContentDigestProjection` — **désactivé par défaut**, avec liste de refus publiée
  (`updated_at`, `created_at`, `ts`, `epoch`, `_rev`, `_version`, `deleted_at`) :
  sans elle l'identité changerait à chaque écriture. C'est une clé de
  **déduplication à la lecture**, pas une clé primaire persistante.
- `ApiProjection` / `ChainProjection` — **déclarées** (`motive`, `key_properties`)
  mais `Unresolvable { reason: "non implémenté" }`. Un motif non branché se
  **déclare** ; il ne rend ni silence ni zéro (doctrine du relief).

### Index espace-aware — la garde est DANS le type
```rust
pub struct IdentityIndex { space: Option<String>, by_uuid: HashMap<Uuid, u64> }
impl IdentityIndex { pub fn insert(&mut self, id: Uuid, handle: u64) -> bool; /* false si l'espace diffère */ }
```
On ne peut donc pas l'oublier à un site d'appel : c'est ce qui neutralise le piège
« deux espaces sont deux substrats qui allouent chacun leurs `NodeId` à partir de 1 ».
`use_graph`/`reset_graph` le purgent déjà via `invalidate_caches`.

### Insertion additive
| Fichier | Nature |
|---|---|
| `this/src/core/identity.rs` | **neuf**, non gaté (pur, testable sans moteur) |
| `this/src/storage/obrain_identity.rs` | **neuf**, gate `obrain` (`node_view`, `discover_labels`, `rebuild_identity_index[_capped]`) |
| `this/src/server/host.rs` / `builder.rs` | + champ `identity`, accesseurs, **auto-câblage** `obrain_default()` quand un `graph_store` est attaché |
| `this/src/storage/obrain.rs` | **~14 lignes AJOUTÉES, 0 modifiée** (champ + 4 init + 1 ligne dans `invalidate_caches`) |
| `this/src/graph/routes.rs` | + `GET /graph/identity`, `GET /graph/identity/{id}` (`spawn_blocking`, jamais d'erreur métier) |
| `this/Cargo.toml` | + `sha2` (non optionnel) |
| `this/tests/identity_conformance.rs` | **neuf** : vecteurs d'or, injectivité, indépendance à l'ordre, séparation d'espaces, non-régression |

**`Relief` reste intact en v1** : aucun champ ajouté, `RELIEF_SCHEMA` inchangé.

### Preuve exigée
- `entity.obrain` (lecture seule) : **0 par convention ET 38 par identité dérivée**.
  Deux chiffres sur un même label = le chemin neuf est **additif**, pas substitutif.
- Même mesure sur `/tmp/linux-full.obrain` (`Function`) : de 0 à non nul.
- Aucun test existant édité (compte de tests avant/après).
- `grep -c foldhash this/src/core/identity.rs` = 0 (aucun hachage instable entre
  exécutions — `obrain-common/src/utils/hash.rs:33-40`).

### Risques
- **Dépôt sans filet git** ⇒ copie de sauvegarde horodatée dans `/tmp` (jamais dans
  le dépôt) avant toute réécriture ; pas de reformatage global.
- **Collision** avec le travail parallèle sur `this/src/graph/` : relire le fichier
  juste avant de le patcher, refuser si l'empreinte diffère.
- **Mémoire** : `HashMap<Uuid,u64>` ≈ **300 Mo** pour 6,3 M entités, sur une machine
  à 2 Go libres avec un obrain à 10 Go de RSS ⇒ plafond d'entrées + `truncated: true`,
  et l'index complet **jamais** construit depuis une requête HTTP.
- **`all_labels()` vide** ⇒ `labels_error` obligatoire : publier `[]` se lirait
  « aucune entité », exactement le mensonge que la doctrine du relief interdit.
- **Rupture du consommateur aletheia** : remplacer `synth_uuid` change les ids
  exposés ⇒ `--identity-scheme v1|legacy` avec **legacy par défaut**.

---

## P2 — Bail d'écriture + jeton de fencing

### Le constat qui change la conception
**`commit_wal` n'est pas un point de passage suffisant.**
`ObrainStore::commit_wal` (`this/src/storage/obrain.rs:149-157`) est le point unique
de la **durabilité** (7 sorties de mutation l'appellent), mais **pas des mutations** —
et côté obrain **le WAL est écrit AVANT la mutation mmap**
(`obrain-substrate/src/writer.rs:287-309` : « (1) WAL first » puis « (2) mmap mutation »).

⇒ Une vérification placée au seul commit laisse le record du writer périmé **déjà
dans le WAL**. Et il n'existe **aucun marqueur de commit annulable** : les records de
mutation partent avec `flags: 0` ; `FLAG_COMMIT` n'est posé que par le record de
checkpoint et ne sert qu'à déclencher le fsync.

**Conséquence : le verrou doit être en ENTRÉE d'opération ; la vérification au commit
n'est qu'un filet.**

### Le trou d'unicité est intra-processus
`obrain-substrate/src/file.rs:179-207` : si une part existe déjà, elle est
**retournée** (`:193-195`). Donc **deux `ObrainDB::open()` du même répertoire dans le
même processus réussissent tous les deux**, chacun avec son mmap et son WAL.
L'exclusion inter-processus tient ; l'intra-processus non — c'est ce que le bail couvre.

### Où l'époque peut vivre (inventaire vérifié)
| Emplacement | Verdict |
|---|---|
| En-tête substrat | **non** — `MetaHeader` gelé (96 o, `SUBSTRATE_FORMAT_VERSION = 1`) ; un second `SubstrateFile` se heurte au flock |
| Colonne blob | **non exposée** — aucune méthode publique `*blob*` sur `SubstrateStore` |
| Propriété de nœud | **non** — elle vit dans l'état **rejoué** : un rejeu, un checkpoint ou une restauration ferait **reculer l'époque**, cassant le jeton |
| **Fichier possédé par this-rs dans le répertoire du store** | **OUI** — le chemin est accessible (`ObrainDB::config()` public) et tous les consommateurs de `read_dir` filtrent par préfixe ⇒ une entrée `<shard>.lock` est tolérée |

### Invariants gelés (chacun un test)
- **P2-1 exclusion** — tant que W détient le `flock` exclusif, personne d'autre
  n'acquiert le bail ni n'incrémente l'époque de ce shard.
- **P2-2 fencing** — `begin_write()` refuse **avant** le premier octet muté ;
  `commit_wal()` re-vérifie (atome + tripwire disque).
- **P2-3 époque hors du rejeu** — ni WAL, ni zones, ni `substrate.meta`.
- **P2-4 coût à chaud** — 1 load atomique par écriture ; 1 `pread` de 128 o toutes
  les 256 commits (défaut) ⇒ ≈1 ns/commit amorti, négligeable devant les
  ~13-15 µs d'un CREATE.

### Forme
`this/src/storage/writer_lease.rs` (non gaté ; std + serde seulement) :
`StoreId` (chemin canonique | volatile) · `ShardId { store, space }` ·
`OwnerId { instance: Uuid, process, host, started_at_ms }` (le pid seul est
réutilisable) · `WriterLease { shard, owner, epoch, issued_at, expires_at, heartbeat }` ·
`LeaseRecord` — trame **128 octets**, `MAGIC = b"THISLSE1"`, version, **CRC32**, epoch,
owner ; `decode_lossy_epoch` réservé à la **réparation explicite** · `LeaseInner`
(atomes `state`/`epoch`/`expires_at`/compteur de tripwire) · `LeaseAuthority`
(`File` | `Memory` | `Consensus`) avec `shard_map()` **synchrone et local**.

**Aucune dépendance nouvelle** : `rustc 1.93` fournit `try_lock`, `try_lock_shared`,
`lock_shared` (vérifié dans la rlib) — `fs2` inutile.

### Compatibilité
Les tests de réouverture **ferment le store avant de rouvrir** ⇒ un bail pris par
défaut à l'ouverture ne les casse pas. `new_in_memory()` n'a pas de chemin de fichier
⇒ mode `Volatile`. `open_read_only` refuse déjà les mutations.

---

## Maintenance
Ces contrats sont **gelés** dès qu'un id ou une époque est publié : changer
`ENTITY_ID_SCHEME` ou l'algorithme **re-clé toutes les entités** ; changer
`MAGIC`/la taille de la trame **invalide les baux existants**. Toute évolution ⇒
**nouveau schéma versionné**, jamais une modification en place.

---

## Contrat de latence par surface — MESURÉ (ronde 12)

`cargo run -p this-rs --features obrain --release --example latency_contract`

Médianes, 300 itérations in-process, 200 requêtes HTTP, machine partagée (load ~20).

### in-process

| surface | coût | ce que ça inclut |
|---|---|---|
| bail · état publié (chemin chaud) | **230 ns** | lecture du verrou + époque — appelé à **chaque** écriture |
| identité · recherche indexée | **1,19 µs** | `find_nodes_by_property` (index) |
| identité · projection d'un nœud | **2,22 µs** | lecture du nœud + chaîne de motifs + SHA-256 |
| IAM · lecture d'un nœud de décision | **4,28 µs** | 1 lecture de nœud (table de propriétés incluse) |
| IAM · décision matérialisée | **5,77 µs** | recherche indexée + 1 lecture de nœud — appelée à **chaque** requête autorisée |
| journal · relecture depuis 0 | **1,52 ms** | lecture de zone + tri + dédup (64 entrées) |
| journal · append d'une entrée | **2,90 ms** | attribution du `seq` + écriture de zone — **coût de durabilité**, pas un chemin chaud |

### HTTP — socket réelle, connexion réutilisée (le `connect` n'est pas inclus)

| surface | coût | ce que ça inclut |
|---|---|---|
| `GET /graph/identity/{id}` (200, résolution) | **63,75 µs** | pile complète + résolution indexée |
| `GET /graph/identity` | **84,66 µs** | pile complète + introspection des labels + notes + pièges |
| `GET /graph` | **184,94 µs** | pile complète + 28 procédures + 14 pièges + notes |

**Lecture** : HTTP coûte ~11× à ~32× les briques in-process qu'il compose — l'ordre de grandeur attendu pour une surface réseau dont le coût est dominé par la **sérialisation du document**, pas par l'autorisation.

### Une erreur de mesure attrapée avant publication

La première version lisait la réponse avec un `read()` simple : sur une socket locale il rend **dès les en-têtes**, pendant que le handler écrit encore le corps. Elle annonçait **4,3 µs** pour `GET /graph` — invraisemblable pour un document de 28 procédures. Après lecture **complète** (jusqu'au dernier octet annoncé par `Content-Length`), le chiffre réel est **184,94 µs** : un facteur **43**.

Deux autres corrections d'honnêteté dans ce banc :
- la ligne « traversée des rôles » a été **retirée** : ce store ne porte que les décisions matérialisées, pas les arêtes — la mesurer aurait donné un chiffre flatteur et faux (la comparaison vit dans `iam_bench`, où le graphe existe) ;
- la requête `GET /graph/identity/{id}` sert un identifiant **réellement indexé** (200), pas un 404 : mesurer le chemin d'erreur et le présenter comme le chemin nominal aurait été un abus.

---

---

## Mode par défaut, branchement clé en main, bascule d'interface

### 1. L'application des politiques est ARMÉE PAR DÉFAUT

`ServerBuilder::new()` ⇒ `host.authorize == true`. Une entité enregistrée dont la
configuration déclare `auth:` voit ses politiques **appliquées** sans aucun appel
supplémentaire, en **403** si le contexte ne satisfait pas.

La politique par défaut d'une entité est `"authenticated"` : une entité sans
configuration explicite **exige un contexte authentifié**. C'est voulu — un
framework qui laisse passer par défaut n'est pas sûr par défaut.

**Sortie explicite** : `ServerBuilder::without_authorization()`. L'appel est
**journalisé en avertissement** (« les politiques `auth:` déclarées ne seront PAS
appliquées ») pour que la décision reste visible en exploitation. Une fois
désarmées, les sections `auth:` sont **décoratives** : lues, jamais appliquées.

### 2. Brancher obrain suffit — `with_obrain_store`

```rust
let store = ObrainStore::open("./data/app.obrain")?;
let host = ServerBuilder::new()
    .with_link_service(ObrainLinkService::new(store.clone()))
    .with_obrain_store(store)   // ← tout le reste suit
    .build_host()?;
```

Un seul appel donne : le relief (`GET /graph`), l'identité canonique (convention
`_uuid`/`_data` **puis** propriétés plates), le plan IAM matérialisé, l'application
des politiques (armée), et le **bail d'écriture** si le store est inscriptible.

**Le bail est fail-closed à la construction** : s'il ne peut pas être pris (un
autre écrivain le détient — autre processus **ou ce processus**), `build_host()`
**échoue** avec un message qui nomme le shard et l'échappatoire. Continuer sans
bail serait exactement le trou qu'il comble. Un store ouvert en **lecture seule**
n'arme jamais de bail : il n'écrit pas, donc il n'a rien à arbitrer.
Opt-out assumé : `ServerBuilder::without_write_fencing()` (journalisé).

### 3. Bascule d'interface — `GraphBackend`

Tout ce qui utilise obrain peut pointer ses interfaces vers this-rs : le trait
**`this::storage::GraphBackend`** (object-safe) est implémenté **par les deux**,
`obrain_engine::ObrainDB` et `this::storage::ObrainStore`.

```rust
fn pages<B: GraphBackend>(backend: &B) -> usize { backend.nodes_by_label("Page").len() }

let n = pages(store.db());       // obrain brut
let n = pages(&obrain_store);    // this-rs par-dessus obrain  ← MÊME appel
let backends: Vec<&dyn GraphBackend> = vec![&raw_db, wrapped.as_ref()];
```

Le consommateur qui bascule **ne change aucun appel** de lecture ou d'écriture ;
il gagne l'identité, l'IAM, le bail et le journal à côté.

| couvert par le contrat commun | **hors** contrat, et pourquoi |
|---|---|
| nœuds, propriétés, arêtes, voisinage, degrés, labels, comptage | **espaces nommés** : obrain ne les a pas — mis dans le trait, un défaut silencieux laisserait croire qu'`ObrainDB` sait les faire |
| index de propriété (poser/retirer/tester) | **bail d'écriture** : arbitrage propre à this-rs |
| `find_nodes_by_property`, `commit` | identité canonique, décision IAM, journal |

Les opérations du trait passent par l'**espace actif** sur `ObrainStore` : un
consommateur qui bascule n'a rien à savoir des espaces pour que ses lectures
restent correctes.

---

## Contrat de latence — **REFRAÎCHI** (mesure du 2026-09-11, après le mode par défaut et l'IAM v2)

`examples/latency_contract` (400 itérations in-process, 250 requêtes HTTP) et
`examples/iam_bench` (200 principaux × 4 rôles × 20 permissions). Machine
**partagée**, load ≈ 29 au moment de la mesure.

### Ce qui a changé dans la mesure elle-même (et pourquoi c'était nécessaire)

1. **Minimum ET médiane.** Une même opération mesurée à deux minutes
   d'intervalle donnait 6 µs puis 16 µs : la médiane ondule d'un facteur 3 sous
   charge. Le **minimum** est le coût sans interférence — c'est lui qu'on cite.
2. **La politique `perm:` mesurait un chemin qui n'existe pas.** La décision
   était matérialisée sous `("user:1", "tenant-a")` alors que le contexte se
   mappe en `(user_id, tenant_id)` : la politique ne trouvait **aucune décision**
   et affichait 1,08 µs — le coût d'une recherche qui ne trouve rien. Corrigé :
   elle mesure désormais le **chemin nominal** (4,4 µs, cohérent avec la décision
   IAM elle-même).
3. **Une clé d'ACL codée en dur** dans le banc (`"tenant-a/user:1"`) datait du
   schéma v1 : elle a fait **paniquer** le banc après le passage à v2. Le banc
   passe maintenant par le contrat (`Iam::acl_key`, `GraphBackend`), jamais par
   une valeur recopiée.

### in-process (minimum / médiane)

| surface | minimum | médiane | ce que ça inclut |
|---|---|---|---|
| bail · état publié | **140 ns** | 150 ns | lecture du verrou + époque — appelé à **chaque** écriture |
| autorisation · politique du vocabulaire | **140 ns** | 141 ns | résolution de route + évaluation (`public`) |
| identité · recherche indexée | **992 ns** | 1,11 µs | `find_nodes_by_property` (index) |
| identité · projection d'un nœud | **2,21 µs** | 2,47 µs | lecture + chaîne de motifs + SHA-256 |
| IAM · décision matérialisée | **4,12 µs** | 6,22 µs | recherche indexée + 1 lecture de nœud |
| IAM · lecture d'un nœud de décision | **4,52 µs** | 4,98 µs | 1 lecture de nœud |
| autorisation · politique `perm:` | **4,42 µs** | 4,65 µs | résolution + mappage d'identité + décision IAM |
| journal · relecture depuis 0 | **1,37 ms** | 2,35 ms | lecture de zone + tri + dédup (64 entrées) |
| journal · append d'une entrée | **1,96 ms** | 3,24 ms | attribution du `seq` + écriture de zone |

**Ce que le mode par défaut coûte par requête autorisée** : la résolution de
route et l'évaluation d'une politique du vocabulaire coûtent **140 ns** ; une
politique `perm:` coûte **4,4 µs**, dominée par la décision IAM indexée.

### HTTP (socket réelle, connexion réutilisée — le `connect` n'est pas inclus)

| surface | minimum | médiane |
|---|---|---|
| `GET /graph/identity` | **47,51 µs** | 68,11 µs |
| `GET /graph/identity/{id}` (200) | **59,19 µs** | 87,14 µs |
| `GET /graph` | **109,52 µs** | 166,45 µs |

Le document et le réseau dominent : `GET /graph/identity` coûte **×12** la
décision IAM qu'il contient.

### IAM : le contrat **robuste** n'est pas un rapport, c'est une **forme**

Un rapport unique (index ÷ traversée) a été mesuré de **×0,5 à ×18** sur la même
machine selon la taille du graphe et la charge. Ce qui tient, c'est la forme —
mesurée en faisant varier le graphe d'administration (minimums) :

| rôles × permissions | index | traversée | rapport |
|---|---|---|---|
| 1 × 5 | 7,46 µs | 4,03 µs | ×0,5 |
| 4 × 20 | 7,20 µs | 41,43 µs | ×5,8 |
| 8 × 40 | 8,92 µs | 161,79 µs | ×18,1 |

**La colonne `index` est plate** : la décision matérialisée ne dépend pas du
nombre de rôles, parce qu'elle ne les parcourt pas. La traversée, elle, croît
avec le graphe. *Honnêteté* : sur un graphe minuscule (1 rôle × 5 permissions),
la traversée est **moins chère** que l'index — le seuil est autour d'une
poignée d'arêtes.

**Écriture** (le prix payé une fois, à l'écriture IAM) : **21,16 ms par
principal** avec un commit par principal, **1,36 ms par principal** en un seul
lot — **×16**. C'est le chiffre qui justifie `materialize_batch`.
