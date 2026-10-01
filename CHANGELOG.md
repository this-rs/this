# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

> ### ✅ LES DEUX LIGNES SONT RÉUNIFIÉES ICI — `this-rs-passerelle` n'est plus la ligne
>
> **2026-10-01.** L'avertissement qui précédait celui-ci (« cet arbre n'est plus
> la ligne de la passerelle ») datait du 15/09 et **n'est plus vrai** : les
> apports de `this-rs-passerelle` sont rentrés **dans ce dépôt** — binaire
> `this-gateway`, annuaire d'identités persisté (argon2id), catalogue tenant,
> rattachement déclaratif (`--config`), `prevol`, `plan`, `basculer.sh`,
> `/auth/verify`, cookie de session, exposition Caddy — et ses suites y sont
> vertes : `cargo test --features obrain,wami,proxy`, dont
> `passerelle_identites` et `passerelle_rattachement` (gateway **réel** en
> sous-processus).
>
> Ce qui reste vrai de l'avertissement d'origine, et qu'il faut garder en tête :
> **la production n'exécute pas le binaire de cet arbre.** Mesuré le 2026-10-01
> (lecture seule, `192.168.1.103:8144` — voir
> [`docs/atlas/rattachement-passerelle.mmd`](docs/atlas/rattachement-passerelle.mmd)) :
> l'unité `this-rs-gateway.service` tourne un binaire construit hors de cet
> arbre, **pilotée par la ligne de commande** et non par `--config`, et **aucun
> service n'est monté** (l'`ExecStart` ne porte que des `--store`). Basculer
> reste donc un geste d'exploitation : `plan`, puis `basculer.sh`, qui rend
> l'état d'avant si la nouvelle ne devient pas saine.

### Fixed
- **`deleted_at` : le contrat n'était pas « faux », il était INCONNU — et le voici mesuré, verrouillé pour les neuf backends.** L'atlas relevait « écrit mais jamais filtré, et `delete` est un DELETE dur ». Vrai, et pire que ça : **trois backends nomment la colonne** (postgres, mysql, dynamodb), **six ne la nomment jamais** (lmdb, mongodb, neo4j, scylladb, obrain…) — pour ceux-là, le champ ne survit que si leur sérialisation le transporte. Autrement dit, personne ne savait si la même requête rendait le même résultat selon le moteur.
  - Le banc partagé porte désormais le contrat : `test_soft_deleted_entity_is_still_listed_the_current_contract` (`tests/storage_harness/data_service_tests.rs`) — `deleted_at` est **persisté et rendu**, et `get`/`list` **ne le filtrent pas**. Le framework ne supprime jamais en douceur (`delete` est dur) : `deleted_at` est une convention de l'**application**, informative, et cacher ce qui est marqué revient à l'appelant (`Entity::is_deleted`). Le contrat des neuf backends est écrit noir sur blanc dans `Entity::deleted_at` (`src/core/entity.rs`).
  - **Ce test n'est pas un blanc-seing, c'est un garde** : il fait tomber tout backend qui se mettrait à filtrer **seul**. Pour de vraies suppressions douces, il faut **retourner ce test** et changer les backends **ensemble** — décision qui appartient au produit, pas à un correctif glissé.
  - **Ce qui a été mesuré, et ce qui ne l'a pas été** : in-memory, lmdb et obrain (les trois backends qui tournent **sans Docker**) passent ; postgres, mysql, mongodb, neo4j, dynamodb et scylladb **n'ont pas pu être exercés ici** (pas de Docker dans cet environnement). Le test est justement la garde qui le dira à leur prochaine exécution — et il nomme ce qu'il vérifie.
- **Le flux d'événements gRPC était fail-OPEN : une requête sans portée recevait les événements de TOUS les locataires.** `extract_tenant_id` rendait `Option<Uuid>` : l'`AuthContext` d'abord (qu'aucun interceptor ne pose côté gRPC), puis l'en-tête `x-tenant-id`. Et `None` n'était **pas** un refus : la garde de filtrage (`if let Some(conn_tenant) = tenant_id && …`) ne s'appliquait simplement pas, si bien que le flux non scopé voyait tout — pour qui savait ouvrir un canal HTTP/2. L'atlas le disait « fail-closed lu dans le code, exercé par aucun test » ; il n'était, en vérité, **pas appliqué du tout**. La portée est désormais décidée **avant** d'ouvrir quoi que ce soit (`portee_du_flux`, `src/server/exposure/grpc/event_service.rs`), et l'absence de portée est un `PermissionDenied` qui nomme la raison **et** le remède.
  - Une identité avec locataire scope le flux, et elle **prime** sur l'en-tête — un client ne choisit pas son locataire quand un jeton en désigne un. Un `Admin` authentifié (sans locataire) voit tout, **explicitement** : c'est toute la différence entre « tout le monde voit tout » et « l'administrateur voit tout ». L'en-tête `x-tenant-id` bien formé reste le geste déclaré ; mal formé, il ne vaut rien (donc refus, jamais « tout »).
  - Le refus est jugé **avant** la disponibilité de l'`EventBus` : un serveur mal configuré ne doit pas faire passer un refus pour une panne, sinon on apprend à relire les refus comme des incidents.
  - **Exercé, pas seulement écrit** : six tests unitaires sur la résolution (dont « anonyme sans en-tête → refus », « l'identité prime sur l'en-tête », « en-tête mal formé → refus »), un test au niveau du **service** (le refus est câblé, et il précède la vérification du bus), et un test d'**intégration sur un serveur gRPC réel** (`un_flux_d_evenements_sans_portee_est_refuse`, `tests/grpc_integration.rs` — 27/27). Trois tests existants qui s'abonnaient **sans portée** mesuraient le comportement fail-open : ils présentent maintenant une identité (utilisateur pour un flux scopé, administrateur pour le « tout »), et le disent.
  - **Ce qui reste ouvert, et qui est écrit au point qui décide** : l'en-tête `x-tenant-id` est un **choix du client**, pas une preuve — le middleware HTTP (`server/middleware/tenant.rs`) lui accorde la même confiance **et la lui accorde avant l'identité**, sans jamais confronter la valeur réclamée au jeton. Un utilisateur du locataire A peut donc y désigner le locataire B. Le fermer demande de trancher une politique d'ensemble (en-tête honoré seulement pour un administrateur, ou seulement derrière une option déclarée) : le constat est posé dans `tenant.rs`, pas glissé sous le tapis.
- **`LinkAuthConfig` — la politique par TYPE DE LIEN — était déclarée, documentée, testée… et appliquée par PERSONNE.** Elle permet d'écrire « lire les factures d'une commande » là où l'entité ne sait dire que « lire les commandes » ; la décision se prenait pourtant sur la **forme du chemin**, avec l'`EntityAuthConfig` de l'entité source. `AppState::get_link_auth_policy` calculait bien la politique par opération… et n'avait **aucun appelant de production**. Une politique déclarée et jamais appliquée est pire qu'une politique absente : elle donne l'assurance d'un réglage qui n'a pas eu lieu. Le middleware consulte désormais le type de lien **avant** l'entité source, quand le chemin nomme une route de lien déclarée (`declared_link_policy`, `src/server/middleware/authorization.rs`).
  - La résolution est celle du **routeur lui-même** (`LinkRouteRegistry` : le couple `(type d'entité, nom de route)`, dans un sens ou dans l'autre) — donc on ne devine rien, et une route de lien déclarée pour **une autre entité source** ne décide pas ici. Le `3ᵉ` segment n'est lu comme nom de route que pour les opérations de lien : `POST /{entité}/{id}/{cible}` crée une entité liée, son `3ᵉ` segment est un pluriel d'entité.
  - **Sans `auth` sur le lien, rien ne change** : la politique de l'entité reste seule, puis le défaut du framework. C'est ce qui rend ce correctif sans effet de bord sur les configurations existantes — et un test le verrouille (`sans_auth_sur_le_lien_la_politique_de_l_entite_reste_seule`), comme il verrouille qu'une politique de lien **ne déborde pas** sur les autres routes de la même entité.
  - La traduction « opération → politique » n'existe plus qu'à **un** endroit (`LinkAuthConfig::policy_for`), et `AppState::get_link_auth_policy` y délègue. Au passage, une opération **hors vocabulaire** rendait `"authenticated"` — une politique **inventée**, indistinguable d'un `authenticated` délibéré ; elle rend maintenant `None` (le test qui affirmait l'ancien contrat a été corrigé avec sa raison).
  - Ce qui **reste** non couvert, et qui est dit ici plutôt que tu : `LinkAuthConfig::get` vise « obtenir un lien par identifiant » (`GET /links/{id}`), une route qui **ne porte pas le type de lien** dans son chemin — aucune politique par type n'y est donc résoluble. Six tests verrouillent l'ensemble dans `authorization.rs`.
- **Le schéma Postgres n'était appliqué par AUCUN code de production.** Les deux migrations (`migrations/001_create_entities.up.sql`, `002_create_links.up.sql`) vivaient dans le dépôt comme un **document** : le seul `sqlx::migrate!` du dépôt était dans un test (`tests/postgres_tests.rs`), pendant que MySQL et ScyllaDB réécrivaient le même schéma en dur, chacun de son côté. Autrement dit, trois définitions du même contrat et **aucun lien entre elles** : la première divergence se serait découverte en production, une colonne absente à la fois. Le paquet fournit désormais **le geste**, comme il le faisait déjà pour MySQL et ScyllaDB — `this::storage::postgres::ensure_schema(&pool)` — avec le même nom, le même contrat (idempotent, rejouable à chaque démarrage) et le même refus du silence : l'échec est **rendu**, pas journalisé, parce qu'une base au schéma incomplet doit arrêter le démarrage plutôt que produire des requêtes qui échouent une par une. **Rien n'est créé au démarrage pour autant** : ouvrir une base est une décision d'application, la modifier en est une seconde, et un framework qui migre la base de son hôte lui retire cette décision. Voir [`docs/schema-et-migrations.md`](docs/schema-et-migrations.md).
  - `tests/postgres_tests.rs` passe désormais par **le geste du paquet** et non par un `sqlx::migrate!` recopié : recopier mesurait le SQL, pas le code qui l'applique.
  - **`tests/schema_migrations.rs` (neuf) tient le lien sans Docker** — donc dans n'importe quelle CI : toute colonne écrite par `postgres.rs` existe dans les migrations ; MySQL déclare les mêmes colonnes que la référence Postgres ; toute colonne écrite par `mysql.rs` existe dans son DDL. Ces tests **nomment ce qu'ils ne couvrent pas** : les types ne sont pas comparés (`UUID` contre `CHAR(36)`, `JSONB` contre `JSON`), et ScyllaDB n'est pas aligné — son DDL ne porte ni `tenant_id` ni `deleted_at`, donc ni confinement par locataire ni effacement doux. Cet écart est **consigné par un test qui tombera le jour où il sera comblé**, pour forcer la mise à jour au lieu de laisser une exception silencieuse.
- **Le portail de poussée (`clippy --lib --tests --all-features -D warnings`) refusait l'arbre : sept constats.** Le pre-push lance `cargo fmt --check` ET ce clippy : un arbre qui ne les passe pas **n'est pas poussable**, et un correctif qui ne peut pas partir ne vaut pas mieux qu'un correctif absent. `cargo fmt` a d'abord été repassé (17 emplacements, 7 fichiers), puis les sept constats traités — dont deux qui n'étaient pas de la forme mais du **sens** :
  - `storage/permission.rs` : la règle « `held == "*"` couvre la demande » liait `wanted` sans jamais le lire. La corriger en `Some(_)` **changeait la règle** — une permission sans cible aurait cessé de couvrir une demande sans cible ; le test `the_allocating_and_allocating_free_paths_agree` l'a refusé, et c'est lui qui a raison. Le motif est `_`, et la raison est écrite sur place.
  - `core/auth/sts.rs` + `bin/this-gateway.rs` : `set_annuaire` pose l'annuaire **une fois** et rend `false` si un autre était déjà posé — auquel cas le nouveau serait ignoré **en silence**, et la passerelle authentifierait contre un annuaire qui n'est pas le sien. Le test **exige** désormais le constat (`assert!`), et le binaire **s'arrête** (`anyhow::ensure!`) au lieu d'ignorer la réponse.
  - Les cinq autres : `push.rs` (`mem::take` au lieu de `drain(..).collect()`), `dynamodb.rs` (`into_iter()` redondant), `storage_harness/integration/mod.rs` (`sort_by_key`), et deux chemins `async-graphql` **non branchés** (`dynamic_schema.rs`, `schema.rs`) marqués morts **en le disant** — l'`allow` est un constat sur un chemin que personne ne monte, pas une excuse ; il tombera avec le `vec![]` du stub `list_entities`.
- **La bascule ne pouvait pas réussir : son ORDRE était impossible.** `basculer.sh` démarrait la nouvelle unité **en laissant l'ancienne tourner**, puis n'arrêtait l'ancienne qu'après la santé — pour éviter toute coupure. Mesuré avec deux passerelles réelles et **un seul** store d'identités : la seconde ne démarre pas — `substrate … is already open by another process (exclusive lock on substrate.meta refused)`. Le store d'identités s'ouvre en **verrou exclusif**, donc deux passerelles ne peuvent pas coexister : la santé ne venait jamais, et le script déclenchait un **retour arrière à chaque bascule**. L'ordre est désormais **arrêter l'ancienne, puis démarrer la nouvelle**, et la raison est écrite dans l'artefact (sans elle, la prochaine relecture le « corrigera » en ordre sans coupure). La coupure est réelle mais brève : **0,2 s** jusqu'à `/health` quand les bases sont chaudes, **jusqu'à 3,3 s à froid**, mesuré avec la déclaration de production (4 bases + viz). Deux gardes : `deux_passerelles_ne_partagent_pas_le_store_didentites` (deux passerelles réelles, `tests/passerelle_identites.rs` — il tombera le jour où la coexistence devient possible, et il faudra alors *reposer* l'ordre sans coupure) et, dans le test de l'artefact, l'ordre des gestes **et** la présence de la raison.
- **`basculer.sh` n'était pas exécutable — deux défauts dans le même artefact, et le test qui manquait.** Le générateur produisait un script que le shell **refuse** :
  - **La commande d'installation n'était pas du shell valide.** La ligne était `faire "cp -f \"$(dirname \"$0\")/$UNITE\" …"` — or `faire` **ré-exécute son argument avec `sh -c`** : la substitution imbriquée y arrivait mal échappée, et `sh` répondait `Syntax error: Unterminated quoted string`. La bascule échouait à son **premier** geste. Le répertoire du script est maintenant calculé sur une ligne à part (`ICI=$(dirname "$0")`), et la commande reçoit ses deux chemins entre guillemets — vérifié : `sh -n -c` passe.
  - **Le binaire par défaut s'écrivait `%h/projects/…`** — un spécificateur **systemd**, que le **shell** ne développe pas. `[ -x "$BIN" ]` portait donc sur un chemin littéralement nommé `%h/…` : `basculer.sh` répondait `REFUS … rien n'a été touché` sur **toute** machine, et `prevol` annonçait `binaire MANQUE` là où le binaire était installé. Le défaut est désormais **absolu** (résolu depuis `HOME`).
  - **Le test ne pouvait pas les voir** : il lisait le **texte** du script (shebang, mots-clés, absence de `rm -rf`) sans jamais demander au shell s'il était lisible — le shebang avait déjà été pris en défaut une fois, la syntaxe l'a été une seconde. Il exécute maintenant `sh -n basculer.sh`, puis relit **chaque commande** du mode à sec avec `sh -n -c` : c'est là que l'échappement se joue. Pour que la sortie soit analysable, les commandes du mode à sec portent `[à sec]` et les **décisions** `(à sec)` — un préfixe unique pour les deux rendait la vérification automatique impossible.
  - **Conséquence sur ce qui a été rapporté** : les répétitions de bascule (« nominal » et « retour arrière forcé ») n'ont pas pu être produites par **ce** générateur avec ses options par défaut — `plan --config … --out DIR`, la commande documentée, écrivait un script qui refusait de démarrer. Elles ont été obtenues avec un `--bin` explicite ou par une version antérieure du générateur.
- **Une déclaration écrite comme la documentation ne démarrait pas : le `~` n'était pas développé.** Le schéma documenté écrit `keys: ~/.config/…` (`docs/passerelle-rattachement.md` §1) ; le chargeur prenait le `~` **au pied de la lettre**. Un opérateur qui suivait la documentation obtenait cinq `MANQUE` au prévol **et** une passerelle qui refuse de démarrer (« store déclaré et absent »), sans que rien ne nomme le `~`. Le développement se fait maintenant dans le **seul point de lecture** — donc le binaire *et* le prévol (qui génère son script depuis la déclaration parsée) voient la même chose, par construction. `~/` **initial seulement** : `~autre` n'est pas deviné, et sans `HOME` la déclaration est refusée plutôt que lue avec un `~` littéral qui désignerait un répertoire nommé `~`. Quatre tests dans `src/bin/this-gateway.rs` (le binaire n'en avait **aucun**). La documentation dit désormais, dans le même souffle, qu'un `~` est accepté **et** qu'on écrit des chemins **absolus** en production : une déclaration en `~` rendrait le **retour arrière** impossible, puisque l'ancien binaire la refuse.
- **Déploiement préparé, RIEN de déployé.** Déclaration `~/.config/this-rs-gateway/deploy.yaml` (les 4 bases en service + le viz rattaché), plan et artefacts dans `~/sauvegardes-bascule-v3/plan-2026-09-14/` (`this-gateway.service`, `basculer.sh`, `RUNBOOK.md`), binaire stagé `~/sauvegardes-bascule-v3/this-gateway.v4-j4-20260914` (reconstruit depuis ce dépôt, `obrain,wami,proxy`). `prevol` : **exit 0, aucun `MANQUE`**, une seule `ATTENTION` — le nom d'unité, attendu (la bascule renomme). Rien n'a été installé, rien n'a été redémarré : la bascule reste un geste d'exploitation, avec son retour arrière écrit.
- **Un refus de lecture de graphe dit maintenant POURQUOI, et nomme le remède.** La raison était la générique — « `graph` n'est le pluriel d'aucune entité enregistrée » — qui envoie chercher une **entité** manquante là où il manque une **politique**. Un opérateur qui cherche au mauvais endroit finit par désarmer la couche pour faire taire le symptôme. Le motif porte désormais la cause et l'issue : « lectures de graphe (`/graph…`) : aucune `graph.policy` déclarée — l'abstention est un refus. Pour les ouvrir délibérément, déclarez `security.graph.policy` (`public`, `authenticated`, `perm:graph:read`, `role:…`) ; pour les ouvrir sans garde, désarmez l'autorisation. » Deux tests exigent que le corps du 403 contienne `graph.policy` (unité : `src/graph/routes.rs` ; HTTP : `tests/iam_http_perimeter.rs`) — un motif qu'on ne peut pas réécrire en silence.
- **Constat de production (RIEN n'a été modifié) : la passerelle qui tourne n'est pas celle que ce CHANGELOG décrit.** L'unité `this-rs-gateway.service` (PID 3560386, démarrée le 2026-09-14 à 12:31, seule instance, port 8144) exécute `~/sauvegardes-bascule-v3/this-rs-gateway.v3-j4`, écrit à **10:55**. Ce binaire ne contient **aucune** des briques du rattachement : `strings` y trouve `/auth/verify` **0**, `x-this-subject` **0**, `/services/` **0**, `anonymous` **0**, `--config` **0**, `prevol` **0**, `plan` **0** — seul `/stores/` y est (`10`). Le binaire fraîchement construit depuis ce dépôt (`target/debug/this-gateway`, 14:48) les porte tous (`6 / 4 / 9 / 6 / 8 / 93 / 5`). Vérifié en ligne : `GET http://192.168.1.103:8144/auth/verify` rend **404**, alors que le RUNBOOK que le binaire courant génère exige **401** sans jeton. **Conséquence** : la production sert les stores obrain en lecture seule (STS, IAM, `/stores/{nom}`) — et **rien** du rattachement de projet. Les mesures de `docs/passerelle-rattachement.md` §8 (401 sans jeton, 200 avec, 403 sur le refus déclaré) ont donc été prises sur un binaire **local**, jamais sur celui qui tourne. Aucun redémarrage, aucune reconstruction n'a été tenté : remplacer le binaire en service est un geste d'exploitation (reconstruire depuis ce dépôt puis `plan` + `basculer.sh`, qui attend la santé et **rend** l'état d'avant en cas d'échec).
- **Vérification étendue à la configuration passerelle.** `--features obrain` seul ne construit pas les suites de la passerelle (elles exigent `wami`, et `proxy` pour le rattachement) : elles n'avaient donc **jamais été exécutées** dans cette passe. `cargo test --features obrain,wami,proxy` → **1 443 verts, 0 rouge**, dont `passerelle_identites` (2), `passerelle_rattachement` (11, gateway réel en sous-processus) et les 4 tests neufs du chargeur de déclaration dans le binaire. Trois configurations vérifiées : `obrain` (1 376), `obrain,wami,proxy` (1 443), défaut (1 005).
- **Un test de la passerelle échouait selon la CHARGE, pas selon le code.** `le_prevol_reconnait_la_passerelle_sous_son_ancien_nom` attendait que son propre stub tienne le port, mais sortait de sa boucle d'attente **au bout de 10 s sans regarder** si le stub l'avait pris : en suite complète, le prévol voyait un port **libre** et le test accusait le prévol d'un défaut inexistant. Il vérifie maintenant que le port est tenu (20 s) et **nomme l'environnement** comme cause s'il ne l'est pas — un échec doit dire sa cause, sinon il apprend à ignorer les rouges. Vérifié : 3 passes consécutives des deux suites passerelle, 13/13 vertes à chaque fois.
- **`cargo doc -D warnings` repasse — deux liens fautifs, préexistants.** `src/dns/powerdns.rs` liait `DnsPlan` par un **lien court** alors que l'import n'existe que sous la feature `powerdns` : introuvable sans elle, **redondant** avec elle — un lien qui ne tient que dans une configuration de features ne tient pas, il s'écrit maintenant en **chemin complet** (`super::DnsPlan`, en parallèle de `super::plan` juste à côté). `src/deploy/mod.rs` portait une cible explicite identique à son label. `RUSTDOCFLAGS="-D warnings" cargo doc -p this-rs --no-deps` passe **avec `--features powerdns,obrain`** et **sans feature** (le `--all-features` n'a pas été rejoué : il demande le checkout `vite-rs`).
- **La suite `--features obrain` repasse à zéro rouge : 1 376 verts, 0 échec.** Les 14 rouges n'étaient pas des défauts de code mais **trois causes distinctes**, dont treize assertions écrites **avant** le correctif `abstention = refus` (2026-09-13) et jamais mises à jour. Elles attendaient `200` là où la couche rend désormais `403 deny_abstain`.
  - **`src/graph/routes.rs`** — les deux tests de **montage** désarment la couche, parce que le statut ne prouve plus rien : armée, une route *montée* sans `graph.policy` et une route *absente* rendent le même 403. Deux tests neufs verrouillent le contrat à cet endroit : `graph_reads_are_refused_when_no_policy_is_declared` (`/graph`, `/graph/identity`, `/graph/spaces` → 403 + `deny_abstain`) et `declaring_a_graph_policy_serves_the_identity_contract` — **le remède est réel**, pas seulement documenté.
  - **`tests/iam_http_perimeter.rs`** — `an_unknown_route_is_never_refused_by_the_authorization_layer` affirmait l'ancien contrat **dans son nom** ; il devient `an_abstention_is_a_refusal_on_the_real_router` (`/health` 200 comme seul chemin public déclaré ; `/graph`, `/custom/ping`, `/graph/identity`, pluriel inconnu → 403, chacun **nommé**). Les tests qui **lisent** le relief et le contrat d'identité le font désormais **sous la couche armée**, avec `graph.policy: authenticated` et un contexte (`with_graph_policy`) : c'est le parcours réel d'un consommateur, et un anonyme reste refusé. `no_graph_surface_exists_when_no_store_is_attached` observe l'absence de route avec la couche **retirée** — le repli du routeur de liens répond `400 Invalid path format`, ce qui est nommé dans le test plutôt que confondu avec un 404.
  - **`tests/native_integration.rs`** — `one_call_wires_obrain_identity_iam_and_enforcement` vérifie les **deux temps** du chemin clé en main : sans politique déclarée, 403 ; politique déclarée + identité, 200.
  - **`tests/origin_surface.rs`** — le banc de la surface *origine* (domaines, CORS, `X-Forwarded-Host`) **désarme** l'autorisation : armée, elle refusait `/whoami` avant le garde de domaine — ces quatre tests mesuraient l'autorisation au lieu du domaine.
  - **Deux commentaires faux, corrigés à la source** : `ServerSecurityConfig::graph` annonçait « sans politique, elles restent descriptives et publiques » ; `RestExposure::build_router` annonçait « ARMÉE EXPLICITEMENT, jamais par défaut » alors que `ServerHost::authorize` vaut `true`. Un commentaire qui contredit le champ coûte des tests écrits sur le mauvais contrat. `docs/integration-consommateurs.md` §3 ter dit maintenant le contrat réel **et le remède déclaratif** : `graph.policy: "public"` pour un relief observable sans droits — public **par déclaration**, plus par accident.
- **Un `cargo test` par défaut ne compilait plus.** `tests/scope_budgets.rs` nommait `this::graph::*` sans `#![cfg(feature = "obrain")]`, et `examples/weave_pilot.rs` / `examples/weave_bench.rs` parlaient obrain sans `required-features` : la cible ne se construisait pas du tout, donc l'affirmation « 788 verts sans obrain » était devenue fausse — elle ne mesurait rien. Les deux exemples sont déclarés dans `Cargo.toml` pour porter la contrainte, le garde-fou de budgets est gaté. **1 001 tests verts sans la feature.**
- **`tests/obrain_in_process.rs` visait une base que le moteur ne peut plus ouvrir.** `/mnt/obrain/mig/runtime-dev.obrain` est en **format substrat 1**, le moteur (`banc-obrain-j4`) exige **3** — et une ouverture **lecture seule** ne migre jamais : la migration n'a lieu qu'à l'ouverture **exclusive** (`obrain-substrate/src/file.rs`). Le test choisit désormais sa base (`THIS_OBRAIN_TEST_STORE`, sinon la base vivante du harnais en format 3, sinon la copie de migration) et **nomme chaque candidat écarté avec sa raison** : un test qui saute en silence ne vaut pas mieux qu'un test absent. Re-mesuré sur la base vivante (`429 717` nœuds, format 3) : ouverture **692,8 ms**, lecture de propriété **5 664 ns** (2 000/2 000 nœuds avec la clé `name`), `MATCH (n) RETURN count(n)` **1 356 µs** — la borne (< 50 µs) tient, et la leçon d'origine reste consignée.

### Added
- **`basculer.sh` — la bascule générée, avec retour arrière ÉCRIT.** Le plan produit désormais le script de la migration : il installe l'unité, la démarre, **attend la santé**, n'arrête l'ancienne (`this-rs-gateway.service`) qu'**après** que la nouvelle ait répondu, et **relance l'ancienne si la nouvelle ne devient pas saine** — l'état d'avant est *rendu*, pas seulement décrit. Il conserve l'unité existante (`*.avant-bascule`), refuse de démarrer si le binaire ou la déclaration manquent (« rien n'a été touché »), et **ne touche jamais** les chemins des clés ni du store d'identités : les renommer ferait générer une nouvelle paire Ed25519 et déconnecterait tout le monde. `--sec` affiche ce qu'il ferait sans rien exécuter.
  - **Répété les deux chemins** sur la machine de production : nominal (200 sur `/health`, ancienne arrêtée après, service rattaché en 401) et **retour arrière forcé** (port occupé → `ÉCHEC` nommé, nouvelle unité arrêtée, ancienne relancée, code 1).
  - **Corrigé (artefact) : l'en-tête passait avant le `#!/bin/sh`** — le script ne s'exécutait donc pas directement. Un artefact qu'on ne peut pas lancer tel quel n'est pas déployable ; le shebang est la première ligne, et un test l'exige.
  - **Corrigé (mode à sec) : il annonçait un `ÉCHEC` et un retour arrière qui n'avaient pas eu lieu.** Un mode d'essai qui crie au loup apprend à ignorer les cris : il n'attend plus une santé que rien n'a démarré.
- **Le client du proxy a un délai de CONNEXION (5 s).** Un amont injoignable échoue en cinq secondes au lieu de retenir la connexion du client indéfiniment. Aucun délai **total**, délibérément : il tuerait un flux légitime (SSE) au bout de N secondes — la passerelle est transparente, elle ne décide pas combien de temps un flux vit.
- **Constat de terrain corrigé** : `192.168.1.103` — que je désignais comme « n3 », une autre machine — **est la machine de travail elle-même** (`hostname` = `n3`, `enp2s0` porte cette adresse, `MainPID` identique en local et par `ssh`). La production tourne donc en unité utilisateur **locale** : la bascule est un geste local, sans étape distante. Les mesures « à distance » restent valides (elles étaient en lecture seule), mais la distinction poste/n3 était fausse.
- **Le proxy transmet le corps en flux, il ne le tamponne plus.** `reponse.bytes()` attendait la fin du corps : un projet qui parle **SSE** (ou tout flux long) se serait **figé** à travers la passerelle, sans erreur levée — le rattachement aurait paru cassé et rien ne l'aurait dit. Mesuré : TTFB **0,005 s** à travers la passerelle contre **0,701 s** au total, avec un amont SSE réel qui n'écrit son second événement qu'à 700 ms. `reqwest/stream` ajouté, verrouillé par un test qui lit la réponse en **TCP brut** (donc incrémentalement, comme un navigateur).
  - Ce test a d'ailleurs corrigé **mon propre test** : il lisait en bloquant dans un `#[tokio::test]` mono-thread, donc il affamait l'amont qu'il avait lui-même lancé — la passerelle attendait une réponse que le test empêchait de produire. `flavor = "multi_thread"` est nécessaire, et c'est écrit dans le fichier.
- **Limite mesurée et documentée : le proxy ne porte pas WebSocket.** Une demande d'`Upgrade` traverse et rend un `200` ordinaire, jamais un `101` : le proxy est un client HTTP, il ne peut pas tunneliser une connexion mise à niveau. Un projet qui parle WebSocket se rattache en `forward_auth` (Caddy joint le projet en direct et porte le WebSocket nativement), pas en `via_gateway`.
- **Le binaire s'appelle `this-gateway`** (unité `this-gateway.service`, `src/bin/this-gateway.rs`) — nom court, cohérent avec `this-host`. L'unité déployée sur n3 s'appelle encore `this-rs-gateway.service` : le **prévol le signale** (`unite` → `ATTENTION`, et `port` → « tenu par la passerelle sous son ANCIEN nom : c'est un redémarrage ») au lieu d'annoncer une première installation. Les chemins qui portent des **données** ne changent pas (`~/.config/this-rs-gateway/cles`, `~/.local/share/this-rs-gateway/identites.obrain`) : les renommer ferait générer une nouvelle paire Ed25519 et **déconnecterait tout le monde**. `prevol --unite NOM` permet de viser un autre nom.
- **Deux défauts trouvés par une répétition avec un vrai Caddy** — et que `caddy adapt` ne pouvait pas voir, puisqu'ils sont *exécutables*, pas syntaxiques :
  - **`forward_auth` : aucune route ne menait au login.** `forward_auth` s'appliquait à tout, `/auth/token` compris : un navigateur n'obtenait **jamais** son cookie de session — une garde sans porte d'entrée n'est pas une garde, c'est une fermeture. Le bloc porte désormais `@passerelle path /auth/* /auth /health /healthz` → passerelle, **avant** la porte.
  - **`via_gateway` : la racine du site allait au 404 de la passerelle.** `/` n'est pas une route de la passerelle (elle sert `/stores/…`, `/services/…`, `/auth/…`, `/health`) : la page d'accueil d'un site exposé était un 404, la garde fonctionnant par ailleurs. Le bloc **préfixe** maintenant vers le montage du service (`rewrite * /services/<nom>{uri}`).
  - Les directives sont enveloppées dans un `route { }` : Caddy réordonne les directives nues, et ici **l'ordre est porteur de sens**. Les deux défauts sont verrouillés par des tests, et la répétition est mesurée dans `docs/passerelle-rattachement.md` §8 (401 sans jeton, login 200, 200 avec cookie, 403 sur le refus déclaré, 401 sur un cookie forgé, dans **les deux modes**).
- **Rattachement déclaratif : `this-gateway --config deploy.yaml`** — un fichier remplace la ligne `ExecStart` interminable de l'unité systemd. Il déclare les **bases** (`stores:`) et les **projets existants** (`services:`), avec `addr`, `keys`, `issuer`, `identites`. Les options de ligne de commande restent acceptées en complément (un scalaire du CLI gagne, les `--store` s'ajoutent). `deny_unknown_fields` : une faute de frappe **refuse le démarrage**, comme un `upstream` sans `http(s)`, un `mount` réservé (`/auth/…`, `/stores/…`), `anonymous: true` **et** `tenant:` ensemble, ou un `deny` sans `/`.
- **`this-gateway prevol` — constater avant de déployer, sans rien modifier.** Le **même** script tourne en local (`bash -s`) ou sur la cible (`ssh … bash -s`) : la distance ne change pas ce qu'on vérifie. Deux constats se lisent avant tous les autres — **`cles`** (une paire Ed25519 absente ferait qu'un redémarrage en génère une nouvelle et **invalide tous les jetons émis**) et **`identites`** (store absent : les comptes seraient à recréer, plus personne ne se connecte). Puis `binaire`, `store:<nom>`, `port` (libre / tenu par la passerelle = redémarrage / tenu par autre chose = `MANQUE`), `unite` (encore pilotée par la ligne de commande ?), `service:<nom>` (code HTTP de l'amont vu depuis la cible), `chaine` (`cargo`), `moteur` (checkout obrain), `tunnel:<hôte>` (clé acceptée en `BatchMode`). Sortie **1** dès qu'un constat est `MANQUE` — un prévol vert veut dire quelque chose. `--json` pour une machine.
- **Corrigé (prévol) : « port libre » annoncé devant un service en marche.** Le motif de détection du port était ancré avec un espace, donc ne matchait rien — un constat faux vaut moins que pas de constat. Le motif est désormais ancré en fin d'adresse, et deux tests locaux verrouillent les deux cas : port déjà tenu (`MANQUE`, sortie non nulle) et prévol tout-vrai (sortie 0, aucun `MANQUE`).
- **`this-gateway plan` — le plan de déploiement et ses artefacts.** Le générateur **n'exécute rien** : il écrit des fichiers relus par un opérateur, chacun portant son état (« préparé, non déployé »), et un `RUNBOOK.md` avec l'ordre des gestes, les vérifications (codes attendus : 401 sans jeton, 200 avec, 403 sur un refus déclaré) et le retrait. `plan --config … --out DIR` écrit l'unité systemd utilisateur (`ExecStart=… --config <déclaration>` — la ligne interminable disparaît, aucun mot de passe n'y entre, `LimitNOFILE` y est posé), les blocs Caddy des deux chemins (`<hôte>.caddy` à ajouter au Caddyfile existant, et la variante hôte complète), les unités de tunnel, le plan DNS (réutilise `this::dns::desired_records` : A/AAAA/CNAME seulement, jamais MX/TXT/NS/SRV) ; `--json` publie le plan (`deployed: false`) sans rien écrire.
- **Deux façons d'exposer un site rattaché, explicites.** `via_gateway` (défaut) : tout passe par la passerelle — **une seule route**, aucune décision de sécurité dans Caddy. `forward_auth` : Caddy joint le projet en direct mais interroge d'abord `/auth/verify` (`copy_headers x-this-subject x-this-tenant x-this-roles`), **et** repose les refus déclarés en `respond … 403`. Dans les deux cas **le tunnel joint la passerelle**, jamais chaque projet : un seul chemin à connaître, et atteindre un projet n'ouvre plus un second tunnel.
- **Les artefacts Caddy sont validés par un vrai binaire Caddy** (v2.11.4) : `caddy adapt` sort en 0 **sans aucun avertissement** et `caddy fmt --diff` ne trouve aucun écart — un fichier généré qui commence par un avertissement de formatage apprend à ignorer les avertissements. La configuration adaptée contient bien `forward_auth` → `/auth/verify`, les deux `reverse_proxy` et le refus 403.
- **Une exposition incomplète refuse la génération** : `expose` sans `acme_email`, sans `gateway` ou sans site, site pointant un `service` non déclaré, `mode` inconnu, hôte avec un schéma, `tunnel: true` sans `tunnel_host`, `record_type` hors des types gérés, deux sites pour le même hôte.
- **Un projet existant se rattache par HTTP, sous la garde** (`services:`, feature `proxy`). Le projet n'est pas réécrit : la passerelle le joint et le remet sous sa garde — **gardé par défaut** (jeton requis), `tenant:` pour le réserver à un tenant, `anonymous: true` pour l'ouvrir en **lecture seule** (jamais d'écriture anonyme, même déclaré ouvert). Il reçoit l'identité **vérifiée** en en-têtes (`x-this-subject`, `x-this-tenant`, `x-this-roles` — les mêmes noms que `/auth/verify`), et un en-tête d'identité **envoyé par le client est écrasé** : sinon n'importe qui choisirait son tenant. Les en-têtes hop-by-hop, `host` et `content-length` ne traversent pas ; corps borné à 8 Mio ; les redirections de l'amont ne sont pas suivies (une redirection traversante sortirait du périmètre gardé).
- **Le refus déclaré, borné à ce qu'il écrit** : `deny: ["/api/query"]` refuse `/api/query` et `/api/query/plan`, **jamais** `/api/querylog`. C'est la sortie prévue pour le viz, dont `/api/query` accepte aujourd'hui des requêtes **sans authentification**.
- **Corrigé (rattachement) : la racine d'un service monté répondait 404.** Un `nest` axum ne fait pas correspondre `/préfixe/` (slash final) à son fallback — or c'est l'URL qu'un navigateur demande pour un site : la page d'accueil d'un service rattaché répondait 404 alors que tout le reste fonctionnait. Les deux formes (`/services/viz` et `/services/viz/`) sont désormais montées explicitement.
- `docs/passerelle-rattachement.md` : le fichier de déclaration, le modèle de garde, la portée des refus, les en-têtes d'identité et leurs conditions de confiance, et les commandes de vérification. Mesure du 2026-09-14 contre le viz de n3 : **401 sans jeton, 200 avec** (le tableau de bord, 986 octets), **403 sur `/api/query`** — alors qu'en direct il est public et accepte un `POST /api/query` sans authentification.
- `tests/passerelle_rattachement.rs` (2 tests, gateway **réel** en sous-processus, amont sur port éphémère) : transmission à l'amont avec chemin dépréfixé et requête préservée, identité vérifiée reçue, en-tête forgé écrasé, refus borné, service `anonymous: true` lisible en GET mais jamais en POST, et sept déclarations invalides qui doivent refuser le démarrage.
- **`proxy` (feature)** : `[dep:reqwest]`. Au passage, `push` et `powerdns` utilisaient la feature implicite `reqwest`, qui disparaît dès qu'on écrit `dep:reqwest` — corrigés en `dep:reqwest`. (`GET|POST`, `src/server/exposure/rest/auth_routes.rs`). Sans elle, un projet qui existe déjà ne peut pas être mis sous la garde du gateway : `forward_auth` de Caddy n'a **rien** à qui demander, et il ne reste que deux issues — rendre le projet public, ou réécrire son authentification. Ce qu'elle vérifie, dans cet ordre : signature EdDSA, échéance, **émetteur** (`iss` — une signature valide ne dit pas *qui* a signé), puis **révocation** par `jti` (sinon `POST /auth/revoke` serait décoratif sur le seul chemin qui garde un projet non réécrit). Elle lit le jeton dans `Authorization: Bearer` **ou** dans le cookie `this_token`, et rend l'identité en en-têtes (`x-this-subject`, `x-this-tenant`, `x-this-roles`) que `forward_auth` recopie au projet rattaché. 5 tests, dont l'émetteur étranger et le jeton révoqué.
- **`/auth/verify` — la porte qu'un proxy interroge** (`GET|POST`, `src/server/exposure/rest/auth_routes.rs`). Sans elle, un projet qui existe déjà ne peut pas être mis sous la garde du gateway : `forward_auth` de Caddy n'a **rien** à qui demander, et il ne reste que deux issues — rendre le projet public, ou réécrire son authentification. Ce qu'elle vérifie, dans cet ordre : signature EdDSA, échéance, **émetteur** (`iss` — une signature valide ne dit pas *qui* a signé), puis **révocation** par `jti` (sinon `POST /auth/revoke` serait décoratif sur le seul chemin qui garde un projet non réécrit). Elle lit le jeton dans `Authorization: Bearer` **ou** dans le cookie `this_token`, et rend l'identité en en-têtes (`x-this-subject`, `x-this-tenant`, `x-this-roles`) que `forward_auth` recopie au projet rattaché. 5 tests, dont l'émetteur étranger et le jeton révoqué.
- **Le cookie de session rend la structure utilisable depuis un navigateur** (`this::server::middleware::auth`). `POST /auth/token` pose `this_token` (`HttpOnly`, `SameSite=Lax`, `Max-Age` = durée du jeton d'accès, `Secure` **seulement** si `X-Forwarded-Proto: https`), et `cookie_vers_en_tete` — monté **le plus à l'extérieur**, donc avant l'authentification — traduit le cookie en en-tête. Elle **ne vérifie rien** : la confiance reste [`AuthProvider`], une seule implémentation, jamais deux. Un cookie forgé est refusé comme un jeton forgé (verrouillé par la matrice HTTP de `tests/passerelle_identites.rs`, sur un gateway réel).
- **Les deux lignes d'évolution sont réunifiées.** `this-rs-passerelle` (le checkout déployé sur n3) était une **copie** du crate : 169 fichiers identiques, 4 ajoutés (`src/bin/gateway.rs`, `src/storage/annuaire_identites.rs`, `src/storage/tenantcat.rs`, `tests/passerelle_identites.rs`) et 5 modifiés. Ces apports sont désormais **dans ce dépôt** : la passerelle IAM est un binaire de this-rs (`[[bin]] this-gateway`, `required-features = ["obrain", "wami"]`), avec l'annuaire d'identités persisté (argon2id, `dep:argon2`) et le catalogue tenant (`substrate.tenantcat`). Les dépendances obrain pointent vers `banc-obrain-j4` — le moteur que la production utilise. Ses 2 tests d'intégration passent ici.
- **`this-host` — a deployment host driven by configuration, on which existing projects are attached.** One binary, one `deploy.yaml`, and N attached projects — each with its own base, identity mapping, policies and exposure, mounted under its own prefix:
  ```sh
  this-host --config deploy.yaml --dry-run   # the plan, without opening a single base
  this-host --config deploy.yaml             # the structure, in service
  curl -s localhost:8080/ | jq               # the plan the structure publishes
  ```
  - **`read_only: true` is the default** for an attached base: attaching a live base does not open it for writing. `read_only: false` must be written, and the plan warns that the write lease is then taken (a second writer is refused).
  - **The plan (`GET /`, `--dry-run`) publishes what is attached AND what is not.** `inspected: false` means no base was opened. It publishes paths and facts — never the contents of a declared file (a policy string cannot leak through the plan), and it names the three things the structure deliberately does not do: entity CRUD routes (they need a Rust `EntityDescriptor`), reverse proxying (the framework *describes* the proxy and renders a `Caddyfile`; it does not drive it), and IAM materialisation in a read-only base.
  - **Fail-closed at load, not at the first request**: `bind` that is not `IP:port`, no attachment, name/mount duplicated or reserved (`/`, `/health`, `/healthz`), two attachments on one base, an `exposure`/`policies`/`auth` file declared and absent, an unreadable `server.yaml` (parsed at load), an unknown identity placeholder, `auth:` without the `wami` feature, and **any unknown key** (`deny_unknown_fields`: `authorisation:` does not pass).
  - **Identities are mapped in the project's own vocabulary** (`principal: "account:{user_id}"`, `scope: "org:{tenant_id}"`), with the known placeholders being exactly what `AuthContext` carries; a value missing for the current context leaves the policy **unevaluable — therefore refused**.
  - **A coupling made visible instead of discovered**: since abstention became a refusal, `enforce` without a declared `graph.policy` answers **403 on `/graph*`** — the plan says so, names the remedy, and a test locks both the 403 and the explanation. And a base opened read-only cannot receive materialised IAM decisions (`perm:` is settled by a decision *in that base*), so `enforce` + `graph.policy` + `read_only: true` is not satisfiable by this host alone — published as a warning.
  - `this::deploy` (`DeploymentConfig`, `Attachment`, `Plan`) plus `src/bin/this-host.rs`; `tests/deploy_host.rs` (8 tests, two of them gated on `obrain` and attaching a base written by another tool, read-only — the node count does not move) and 12 unit tests for validation and the plan. See `docs/hote-de-deploiement.md`.

### Integration
- **Consumer contract** — three ways to integrate the stack into another program (the obrain Dashboard, a harness, an application), each independent of the others: as a **library** (`this::storage::{Iam, AclStamp, GraphBackend, ObrainStore}`, re-exported so no deep paths are needed), over **HTTP** (`GET /graph`), or through **obrain alone**. See `docs/integration-consommateurs.md`.
- **The relief now publishes the IAM data contract**: schema version, decision count (active space), the property the enforcement actually reads (`acl_perms_blob`), and the labels, properties and edges with their meaning and a `written` flag — `true` when the framework writes it, `false` when it only reads it if present. A consumer no longer has to read framework source to know what it may rely on.
- **`ObrainStore::check_write` is public**: a program that writes through its own path (a dashboard in write mode, a migration tool, a job) must be able to honour the write lease. It was `pub(crate)`, so the fence could not be respected by anyone outside the crate.
- **Documentation is warning-free**: `RUSTDOCFLAGS="-D warnings" cargo doc -p this-rs --no-deps` passes with and without `--all-features` — 14 defects fixed (links broken by module-declaration site resolution, links from public documentation to private items, a bare URL). The module docs no longer depend on where they are declared.
- `tests/consumer_contract.rs` (8 tests) plays the role of an external program: it reads a decision through the public API, asks the hot-path question, opens a store **read-only** without taking the write lease, **reads the decisions with obrain alone** (no `Iam` type — the proof the data is not captive), discovers decisions by label, checks the published contract over HTTP, and enforces its own policies on its own router.

### Performance
- **IAM hot path: 6.15 µs → 2.37 µs per authorised request (×2.6)**, measured. The hot path no longer builds a full `AclStamp` — roles, scope, principal and a **SHA-256 digest** — to answer a membership question: `Iam::allows(principal, scope, permission)` does one index lookup, **one** property read and a string scan. Permissions are additionally stored in a hot-path form (`acl_perms_blob`), and permissions containing a control character are refused at materialisation, so one permission cannot impersonate another.
- `stamp_of` reads its seven fields through the engine's `get_node_property` ("without loading all properties") and `node_has_label`, instead of materialising the node and rebuilding a B-tree with allocated `String` keys: **4.52 µs → 2.88 µs**. The full decision follows: 4.12 → 3.23 µs.
- **Fixed (security):** `stamp_of` read through the **root** session, so in a named space a node could be missing (fail-closed but wrong) or the id could designate a *different node of the same number* in the root. It now reads through the active space — the same scoping as `acl_node` — and a security test locks it.
- `GraphBackend::node_property` no longer reads every property to return one (it called `node_properties().get(key)`); it uses the engine's direct `Node::get_property`. A generic win for every facade consumer.
- **Not optimised, on purpose**: route resolution plus a vocabulary policy costs ~240 ns, i.e. 0.5% of a ~45 µs HTTP request. A pre-parsed route table would add a second referential to maintain for half a percent.
- **Write path**: the IAM is never consulted on writes (verified: no `iam()` reference in `src/storage/obrain.rs` or `src/core/service.rs`); the only added cost is the write lease — **0.01%** of a journal append (210 ns vs 1.91 ms), now printed by the latency contract.

### Added
- **A benchmark suite for the granular-rights hot path, and the three optimisations it found.** `cargo bench --features obrain --bench scope_bench` measures permission matching (by number of permissions), scope construction, the multi-scale descent with and without a scope, the store shape, and the per-request chain (context → decision → scope). No `criterion`: the harness is explicit (warm-up, N rounds, median **and** minimum, rounds published), because this repo builds offline and an order of magnitude is enough to see an `O(n)` where a direct comparison belongs.
  - **What it fixed**: `may_read_node` allocated three `String` per question (called once per node and per relation) — now compares `&str` directly, with a **differential test** proving the allocating and allocation-free paths decide identically (an optimisation that changed an authorisation would be the worst possible trade). Scope construction was re-done per request (7.7 µs for 67 permissions) — now cached, keyed by `(principal, scope, decision epoch)`, because the epoch is monotone and a cache ignoring it would serve a **revoked** right: a test requires exactly that. And each node was read **twice** (once to check its label, once to describe it) — the labels are now reused. Measured: refusal matching ×1.5, granted matching ×2, unrestricted short-circuit ×2.5, `scope_for` −16%, filtered descent −8% (ratio ×1.65 → ×1.44).
  - **Two hypotheses the measurement refuted**, kept in the docs: the cycle check in `Vec::contains` looked like the hotspot (replacing it with a set changed nothing: the cost was in the store reads), and the filtered relief looked twice as fast (it was scheduling noise; the real parity is ×0.99).
  - `tests/scope_budgets.rs` is the **guard-rail**: it asserts **ratios**, never absolute durations (a microsecond threshold would go false on a slower machine and end up being loosened instead of fixed), with deliberately wide budgets so it catches a change of order — a quadratic match, a filtering blow-up — not a 20% jitter.
- **The interface and the rights now speak the same language, end to end.** A Canvas describes what it shows as a **scale** and a **subject**; a graph read asked for a depth and an identity. `GET /graph/hierarchy/self?echelle=canvas` closes the loop: the read starts **from the subject the user is looking at**, **at the scale displayed**, with no translation on their side — and the caller's **IAM decision caps it** (an interface asking `noeud` where the right stops at `groupe` gets **403**, with `finest` telling it how far the right goes). The state is **re-read on every request**, so changing the scale changes the read, and nothing is ever invented: no canvas mounted → `501`, no subject declared → `404`, no identity → `401`, an ambiguous subject (two nodes sharing a name) → `409` — never "starting from nowhere". The framework does not know the interface's implementation: it declares the `CanvasState` contract (`scale_of`, `subject_of`) and the application implements it, so the interface can grow (faces, panels, a richer subject) without the graph read changing. `cargo run --features obrain --example canvas_inference` walks the five steps on the real router.
- **Granular rights are now applied PER REQUEST.** Declaring `graph.policy` arms both halves: the policy decides *who may read the graph* (403 otherwise), and a scope resolver derived from the caller's **materialised IAM decision** decides *what that reader sees* — label by label, relation type by relation type, scale by scale. The identity mapping is the **authorisation middleware's own**, never a second one (two mappings would give one person two authorisations). The default is refusal: no context, no decision, or an unreadable decision all yield an empty scope. **Masking does not leak**: a node hidden by its label is **counted**; a relation whose type is unreadable is **neither traversed nor counted** (its number would describe a structure outside the scope — the reader is told only that some exist); and a node's child count goes through the scope, so a parent never announces what it hides. Wire your own with `GraphState::with_scope_resolver`, or use `IamScopeResolver` with the application's mapping.
- **The interface's scales become a rights vocabulary.** A Canvas describes what it shows as a scale of descent — `cosmos`, `monde`, `groupe`, `noeud` — and the framework now speaks the same language instead of inventing a second one: `echelle:read:groupe` (alias `scale`, with `monde`/`world`, `groupe`/`group`, `noeud`/`node`). **A scale right is a CAP**: `echelle:read:groupe` grants cosmos, monde and groupe — not the detail of a node — so one grants *how far* a principal may descend rather than an isolated step. `Scale::descent()` is the single place that turns an intention into a depth and a per-level cap. `GET /graph/hierarchy/{id}?echelle=groupe` drives the read directly; an unknown scale is **refused with the list of valid ones**, never silently defaulted. The scales are published both in the inference manifest and in `iam.vocabulary`.
- **Granular rights over what knowledge WEAVES.** Permissions are now structured — `famille:action[:cible]` — so a principal can be granted exactly what it needs: `node:read:Page` (one label), `edge:read:CITES` (one relation type), **`edge:create:CITES`** (the right to *weave* that relation), `space:use:lab`, `node:*:Page` (every action of a family on one label), down to `*` for compatibility. `this::storage::permission` carries the grammar, a `ReadScope` built from an IAM decision's permissions, and the matching rule: **weaving is granted separately from reading** (`edge:read:CITES` lets you read relations, not create them), **a permission without a target does not cover a targeted request** (`node:read` does not grant `node:read:Page`), and **what is not recognised grants nothing** (`node:read:` is refused, never widened).
  - The **multi-scale view** and the **relief** honour a read scope: only nodes whose label is readable appear, a relation whose type is not readable is **not even traversed**, and only readable labels and edge types are published. What is masked is **counted** (`hidden`, `hidden_labels`, `hidden_edge_types`) and explained in a note — the masking is visible, never inferred, and a reader always knows something was withheld and how much.
  - The **vocabulary is published** with the IAM contract (`GET /graph → iam.vocabulary` and `iam.matching`): a consumer does not have to read the framework's source to know how to write a granular right.
  - HONEST SCOPE: the filtering is implemented and tested at the library level (`describe_scoped`, `describe_store_scoped`); **the HTTP handler does not yet resolve the caller's scope**, so scoping is not automatic per request. That is the next step, stated in `docs/droits-granulaires.md`.
- **An inference engine can read the base UNDER IAM.** The reads an inference engine actually uses — the relief, the multi-scale view, the identity contracts — used to **abstain** (no entity route, so no policy attached, only the authentication layer). Declaring `graph.policy` in `config/server.yaml` (e.g. `perm:graph:read`) makes them **judged like any other route**: a caller without the permission gets **403**, and a `perm:` policy is settled by the **materialised IAM decision** — the same indexed path as the rest of the application. Without the setting nothing changes: the relief stays descriptive and public.
- **The relief publishes the inference manifest** (`GET /graph → inference`): how to authenticate, the identity mapping, the reads in the order one discovers them — each with what it returns and **why** to call it — the bounds that apply (`depth` ≤ 32, `limit` ≤ 1000, `total`/`truncated` per level, cycles signalled), the authorization regime, and the calling rules (never enumerate the whole graph; stop when `children` is 0 or `truncated` is true). A model reads the protocol instead of guessing it.
- **The relief now publishes the SHAPE of the store**, not only the engine's capabilities: node and edge totals, every label with its node count (most populated first), every edge type with its count, the property-key catalogue, and the declared indexes — all **introspected** from the live database (`all_labels`, `node_count_by_label`, `edge_counts_by_type`, `all_property_keys`, `list_indexes`), never from a hand-kept list. What a backend cannot introspect is published **empty**, never estimated. `/graph/hierarchy/{id}` is also listed in the published surface map.
- **Multi-scale view (`GET /graph/hierarchy/{id}`)** — see the subgroups of subgroups of subgroups, one bounded level at a time. Each call takes `depth` and a per-level `limit` (clamped to 32 and 1000), publishes `children` for every node shown (so you know what is below without loading it), `total` and `truncated` per level (a cut is never silent), and a `drill_down` address to continue from any node. Cycles are **reported, never followed**: a self-referencing graph cannot make the view loop. Traversal is by **adjacency**, filtered by containment edge type — the cost follows what is *shown*, not the size of the database, and only displayed nodes are descended. `?format=outline` renders the same view as indented text — the form a model reads best — with the address to ask for more at every unfinished branch: that is how an inference engine walks the graph step by step. See `docs/multi-echelle.md`.
- **PowerDNS delegation and a domain/exposure harness.** The exposure chain now runs end to end: the declaration serves **DNS** (PowerDNS), **TLS/routing** (Caddy) and **the application** (this-rs), from ONE list — `domains.sites`.
  - `this::dns` — a **pure reconcile plan** (creates / updates / deletes / untouched) that touches **only A, AAAA and CNAME** records of the declared hosts, inside the declared zone. Never MX/TXT/NS/SRV: a framework reconciling "its" records must not be able to erase a domain's mail. Deletion requires `prune: true`; by default an unmanaged leftover is *reported*, not removed. `plan` is a pure function: it is reviewable and testable without a network.
  - `this::dns::powerdns` (feature `powerdns`, reusing the existing optional `reqwest`) — reads a zone, applies a plan (`REPLACE`/`DELETE` rrsets), rectifies, and creates a zone **only** when nameservers are given and the zone is absent (never recreating an existing one). The API key is read from the environment named by `api_key_env`, **never** from the versioned config, and never appears in a log or an error. A refused change is an **error**, never a silent success.
  - `examples/domain_harness.rs` — the micro-harness: read the declaration, print the `Caddyfile` and the desired DNS records (no network); `--plan-dns` to compute the gap against a live server; `--apply-dns` to write it. `--json` for a machine.
- **Exposure configuration: CORS, served domains, and Caddyfile delegation** (`config/server.yaml`, `ServerBuilder::with_server_config_file` / `with_cors` / `with_domains` / `delegate_to_caddy`). Nothing is enabled by default: no CORS layer, no domain guard, no proxy description — the framework does not invent a deployment's domains. See `docs/domaines-cors-caddy.md`.
  - **Domains**: a closed `allowed` list (any other `Host` is refused **421**), one-level `*.` wildcards, `trust_forwarded_host` for a trusted proxy, and a domain can designate the **tenant** (`tenants`, `subdomain_base`) — the domain now takes precedence over the token's claims, so a token from another tenant cannot change tenant by assertion.
  - **CORS**: closed origin list, methods/headers/expose/credentials/max-age; a preflight passes **without a token** because the CORS layer is the outermost one (verified: `a_preflight_passes_without_a_token_even_when_authentication_is_installed`). Impossible configurations (no origin while enabled, `*` with credentials) fail at **build time**.
  - **Caddyfile delegation**: `host.caddyfile()` and `cargo run --example caddyfile -- config/server.yaml` render a `Caddyfile` **from the same configuration** (one site block per served host, upstream, ACME email, `tls internal|off`, security headers, `extra_directives` escape hatch). This is a description the operator applies, not automation: this-rs does not drive Caddy, and the file says who sets CORS (the application, so two layers never contradict each other).
  - `ServerHost::exposure()` publishes **counts and facts, never the lists**: an operator who wants the values reads their configuration.
- **Fixed (behaviour, high impact): the policies were evaluated BEFORE authentication.** In axum the last layer applied is the outermost, and the authorization layer was applied last — so it ran first, saw `Anonymous`, and refused every non-public route to a caller holding a valid token. Fail-closed, so not a leak, but an application that worked accused its own policies. Verified by `tests/chain_order.rs` (an authenticated caller was 403) and fixed: application order is now policies → tenant → auth → domain → CORS, i.e. execution CORS → domain → auth → tenant → policies.
- **Fixed: `CorsConfig`'s programmatic `Default` differed from its YAML default** (empty method/header lists in code, usual lists in YAML) — two defaults for one type. A test now pins them equal.

### Security
- **Audit of the authorization chain — every path of every protocol.** The resolver only judged the shapes it "expected"; the router serves more, and everything else **abstained**, i.e. was not judged at all. Measured and fixed:
  - **`/notifications/{user_id}/…` and `/device-tokens/{user_id}` were an IDOR.** They are not entity routes (so no `auth:` policy attached) and took the user id **from the path**, with no comparison to the caller: any caller could read, delete and mute another user's notifications, read and rewrite their preferences, and register or delete their **device tokens** — i.e. hijack their push notifications. None of these routes had a test. A single guard layer now runs on the whole router: no context → **401**, another user → **403**, self / admin / service → allowed (documented). Twelve routes, one guard, impossible to forget on a route added later.
  - **Four link shapes were served but not judged**: `POST /{entity}/{id}/{target}` (which **creates a linked entity**), and `GET|POST|PUT|DELETE /{src}/{id}/{route}/{target}` (4 segments, where the resolver expected 5), plus every deep path served by the link router's **fallback**. A client could therefore create an entity, or create/update/delete links, without any policy being consulted. Now: any read under an entity (`>= 3 segments`) is `list_links`, `POST` at 3 segments is judged by the **target** entity's `create` (creating an invoice is an invoice creation, not an order operation), writes at `>= 4` are `create_link`/`update_link`/`delete_link`. New operation `update_link` in `EntityAuthConfig` (`#[serde(default)]`: existing configs keep working).
  - **GraphQL, gRPC and WebSocket bypassed every policy.** `GraphQLExposure::build_router` mounted a query+**mutation** executor with no authorization at all, and `ServerBuilder::build_with_grpc()` — a framework shortcut — built REST (enforced) and gRPC (not enforced) into one router, silently. These surfaces are not resolved by route, so no policy can attach: the framework now **refuses to mount them while policies are armed**, and an application that accepts the risk says so in its code (`build_router_unenforced`, `build_router_no_fallback_unenforced`, `build_with_grpc_unenforced`), logged as a warning at every mount.
  - **The frontend is a surface too, and the last one to escape the layers.** It is served by a **fallback** (`Frontend::attach` → `fallback_service`), so it appears in no `.route(…)` inventory, and whether the auth/authz layers see it depends on the **order** they are applied. Measured: `api.layer(mw)` then `attach` → the fallback answers **200 without the layer seeing the request**; `attach` then `layer(mw)` → the layer sees it. Attaching last — the natural order, "build my API, then plug the frontend" — therefore serves the SPA **and `/__dev`** outside authentication. Static assets are harmless; `/__dev` runs the rebuild command. Documented at the call site with the measurement, and locked by `tests/frontend_surface.rs`.
  - **The command bridge is now CLOSED by default.** `THIS_DEV=true` used to be enough to open `/__dev/command*`, which executes the rebuild command on HTTP request — a deployment inheriting that variable exposed it. Opening it now requires `THIS_DEV_COMMANDS=true` (or `1`/`yes`/`on`), and the opening is logged as a warning.
  - The framework adds **no CORS layer of its own** (asserted by test): that policy belongs to the application, and the framework must not let a deployment believe it is protected — or exposed — by something it did not write.
  - **The relief publishes the surface map** (`surfaces`), now including `frontend` and `dev_bridge`:: each surface, the paths it serves, whether it enforces policies, by what mechanism, and what protects it otherwise — plus the explicit opt-in required. `enforces_policies: false` is published, not hidden.
- **Tests that encoded the gaps were updated, deliberately** (`shapes_outside_the_contract_abstain`, `every_abstention_carries_an_actionable_reason`, the notification handler tests which called the routes with no identity at all): they asserted the old, unsafe contract. `tests/policy_surfaces.rs` (9 tests) replaces them with the corrected contract, including the invariant that **no read shape of a registered entity abstains any more**, at any depth.
- **Security test suites** (`tests/security_iam.rs`, 24 tests · `tests/security_durability.rs`, 11) — each test names the attack it models: privilege escalation through policy composition (`any:not:admin_only` authorises everyone *except* admins), route bypass (path traversal, method-override header, HEAD as a side door), decision capture (key collision, neighbouring tenant, neighbouring **space**, replay after invalidation), identity collisions (length-ambiguous names, Unicode normalisation) and the trust boundary itself (`documented_limit_*` tests assert what an attacker with store write access CAN do, so it is known rather than assumed).
- **Fixed — IAM lookups were not scoped to the active space.** `principal_node` and `acl_node` queried the **root** index: in a named space, a decision from another space could answer. (An earlier claim of mine that this was fixed was wrong — the replacement never matched the multi-line calls. The security suite is what caught it.)
- **Fixed — the ACL key separator was not injective.** Names containing the US separator (0x1F) collided (`("a\u{1f}b","c")` and `("a","b\u{1f}c")` produced the same key, so one decision overwrote the other). Identity parts containing control characters, or empty, are now **refused** — fail-closed, and reachable through a custom principal mapping.
- **Hardened — a duplicated ACL key makes the decision ambiguous, and an ambiguous decision is refused** rather than resolved by index order (an attacker adding a node could otherwise flip it).

### Added
- **Adoption by an existing project** — the framework now has a documented, tested migration path (see `docs/adoption-projet-existant.md`): branch the store, **observe** the traffic, make the IAM native to your own identities, then arm.
  - `ServerBuilder::authorization_report_only()` — denials are computed, logged and **published in the response** (`x-this-rs-authz: observed-deny` plus `x-this-rs-authz-policy` / `x-this-rs-authz-scope`) but nothing is blocked. This is how a running service adopts policy enforcement without being broken by it.
  - `ServerBuilder::with_principal_mapping(...)` — the application supplies `AuthContext -> (principal, scope)`, so materialised IAM decisions apply to **its** identity model (`account:…`/`org:…`, API key/project, subject/workspace) instead of this-rs's. Without a mapping a foreign identity is **refused, not guessed** (fail-closed, measured).
  - `with_obrain_store()` is now genuinely one call: if no `LinkService` is provided, the obrain link service is attached automatically — plugging a store no longer fails on `LinkService is required`, a requirement unrelated to what the caller asked for.
  - `Module::get_entity_fetcher` / `get_entity_creator` default to `None` (they exist only for the GraphQL and link-by-id surfaces). They were mandatory, which forced every module — and every project adopting this-rs — to write two empty methods just to compile.
- **Engine-agnostic entity identity** (`obrain` feature, `this::core::identity` + `this::storage::obrain_identity`) — the *same entity whatever the engine* wrote it: the `_uuid`/`_data` convention first, then derived identity (uuid v8) for stores with flat properties. Measured on real bases: **0 resolved by convention / 39 by derived identity** on aletheia's `entity.obrain`, **1,553,071** on the Linux kernel graph. IDs are recomputable by a third party (`canonical_bytes_hex`), and `tests/identity_conformance.rs` locks the non-regression: a store written by this-rs yields **exactly** the same identifiers as before.
- **Write fencing and epochs** (`this::storage::writer_lease`) — a second writer is **refused**, including *in the same process* (obrain returns an existing lock part, so two `ObrainDB::open()` of one directory both succeed). Checked at operation **entry**, not at commit: obrain writes the WAL before the mmap mutation. A stale writer can no longer write but can still read; the epoch only ever grows. Frame format frozen by test (128 bytes, `THISLSE1`, CRC32).
- **Durable ordered journal** (`this::events::log_obrain`) — `EventLog` over obrain's journal zone: re-sorted and de-duplicated on read (the engine returns *physical* order), idempotent by `seq`, durable across reopen, per-consumer positions, replicas carrying their source positions.
- **Policy enforcement, armed by default** (`this::server::middleware::authorization`) — declared `auth:` policies are now **applied**. Route → entity → operation → policy resolution, fail-closed (an unreadable policy denies), abstention on unknown routes (never an invented policy), `403` on refusal with `{scope, policy, origin, reason}`. Opt out explicitly with `ServerBuilder::without_authorization()` (logged as a warning). **Breaking for deployments that relied on open access** — which is the point: the default must be safe.
- **Materialised IAM decisions** (`this::storage::iam`) — the decision is an indexed property node per (principal, scope): **7.96 µs** against 115.25 µs by traversal (**×14**), batched writes **×20** cheaper than one commit per principal. Policies spelled `perm:<permission>` are answered by that decision, composed with the existing vocabulary.
- **Turn-key obrain attachment** — `ServerBuilder::with_obrain_store(store)`: one call wires the relief (`GET /graph`), identity, the IAM plan, policy enforcement and — on a writable store — the write lease. Fencing is **fail-closed at construction**: if the lease cannot be taken, `build_host()` fails with the shard named and the escape hatch (`without_write_fencing()`). A read-only store never fences.
- **`GraphBackend` — the interface switch** (`this::storage::GraphBackend`) — one object-safe trait implemented by **both** `obrain_engine::ObrainDB` and `ObrainStore`, so a consumer's graph code runs unchanged on either (`pages(store.db())` / `pages(&obrain_store)`) and gains identity, IAM, fencing and journal by switching implementation.
- **Self-describing relief** — `GET /graph` publishes identity, lease, journal and IAM state, and **says what is missing** ("fencing NOT armed", "no transport") instead of looking complete; `GET /graph/identity` publishes the identity chain and a bounded `?counts=true`.
- **Measured latency contract** (`examples/latency_contract`) — in-process **230 ns** (hot lease check) to **5.77 µs** (IAM decision); HTTP **63.75 µs** to **184.94 µs**; journal append **2.90 ms** (durability, not a hot path).
- **vite-rs frontend hosting** (`frontend` feature, `this::server::frontend`) — the API process can serve the project SPA itself, reusing vite-rs serving rules (statics/ETag/304/MIME/SPA fallback) for both dev and embedded production assets; dev mode adds the vite-rs hot-reload loop, browser capture and the `/__dev` AI command bridge (`DevState`), activated via `SERVE_FRONTEND`/`THIS_*` env vars (`attach_from_env`).
- **Obrain engine re-coupled to this-rs/obrain (banc-obrain) checkout** — the durable cognitive graph engine (substrate + `EveryCommit` fsync WAL); `delete_by_entity` now ends in a WAL commit, and reopen-persistence tests cover data, links, search, post-reopen mutations and batch deletes.

### Planned
- ScyllaDB storage backend
- PostgreSQL storage backend
- GraphQL subscriptions support
- gRPC API exposure
- Webhook system for entity events
- Performance optimizations for large datasets

---

## [0.0.6] - 2025-01-29

### Added - GraphQL API Support 🎉

#### Core GraphQL Features
- **Dynamic GraphQL Schema Generation** - Automatic schema from entity definitions
- **Specific Entity Types** - Each entity gets its own GraphQL type (`Order`, `Invoice`, `Payment`)
- **Full CRUD Operations** - Complete Create, Read, Update, Delete via GraphQL
- **Automatic Relations** - Relations discovered from `links.yaml` configuration
- **Nested Query Support** - Query entities with their relations recursively

#### GraphQL Endpoints
- `POST /graphql` - Main GraphQL endpoint
- `GET /graphql/playground` - Interactive GraphQL playground
- `GET /graphql/schema` - SDL schema introspection endpoint

#### GraphQL Queries
- List queries with pagination (`orders(limit: Int, offset: Int)`)
- Single entity queries (`order(id: ID!)`)
- Automatic relation resolution (`order.invoices`, `invoice.payments`)

#### GraphQL Mutations
- CRUD mutations (`createOrder`, `updateOrder`, `deleteOrder`)
- Specialized link mutations (`createInvoiceForOrder`, `linkPaymentToInvoice`)
- Generic link mutations (`createLink`, `deleteLink`)

#### Implementation Details
- **Custom GraphQL Executor** - Runtime query execution with dynamic field resolution
- **Schema Generator** - Dynamically creates SDL from entities and links
- **Modular Architecture** - Executor split into 6 sub-modules for maintainability:
  - `core.rs` - Main executor orchestration
  - `query_executor.rs` - Query resolution logic
  - `mutation_executor.rs` - Mutation resolution logic
  - `link_mutations.rs` - Link-specific mutations
  - `field_resolver.rs` - Field and relation resolution
  - `utils.rs` - Utility functions

#### Multi-Protocol Architecture
- **ServerHost** - Transport-agnostic core for multi-protocol support
- **Exposure Modules** - Separate modules for REST and GraphQL
- **RestExposure** - REST API exposure implementation
- **GraphQLExposure** - GraphQL API exposure implementation
- Feature flag `graphql` for optional GraphQL support

### Changed
- **ServerBuilder** - Added `build_host()` method for transport-agnostic host creation
- **EntityFetcher trait** - Added `get_sample_entity()` and `list_as_json()` for schema introspection
- **EntityCreator trait** - Added `update_from_json()` and `delete()` for full CRUD support

### Documentation
- New comprehensive [GraphQL Guide](docs/guides/GRAPHQL.md)
- New [GraphQL Implementation](docs/architecture/GRAPHQL_IMPLEMENTATION.md) architecture doc
- Updated [README](README.md) with GraphQL examples
- Updated [microservice example](examples/microservice/) with GraphQL support
- New [GraphQL microservice README](examples/microservice/README_GRAPHQL.md)

### Dependencies
- Added `graphql-parser = "0.4"` for custom executor
- Added `futures = "0.3"` for async recursion
- Updated `async-graphql = "7"` and `async-graphql-axum = "7"`

---

## [0.0.5] - 2024-12-15

### Added - Validation and Filtering

#### Validation System
- **`impl_data_entity_validated!` Macro** - Extended entity macro with validation support
- **`Validated<T>` Extractor** - Axum extractor for automatic validation
- **Reusable Validators**:
  - `required` - Field must be present and non-empty
  - `optional` - Field is optional
  - `positive` - Numeric field must be > 0
  - `string_length(min, max)` - String length constraints
  - `max_value(n)` - Maximum numeric value
  - `in_list([...])` - Value must be in list
  - `date_format(format)` - Date string format validation

#### Filtering System
- **Reusable Filters**:
  - `trim` - Remove leading/trailing whitespace
  - `uppercase` - Convert to uppercase
  - `lowercase` - Convert to lowercase
  - `round_decimals(n)` - Round to n decimal places

#### Operation-Specific Rules
- Different validation/filter rules for `create` vs `update` operations
- Declarative syntax in entity macros

### Documentation
- New [Validation and Filtering Guide](docs/guides/VALIDATION_AND_FILTERING.md)
- Updated [Getting Started](docs/guides/GETTING_STARTED.md) with validation examples
- Updated [Quick Start](docs/guides/QUICK_START.md) with validation examples

---

## [0.0.4] - 2024-11-20

### Added - Pagination and Query Filtering

#### Generic Pagination
- **Automatic pagination** for all list endpoints
- Query parameters: `?limit=20&offset=0`
- `PaginatedResponse<T>` with metadata:
  - `total` - Total number of items
  - `limit` - Items per page
  - `offset` - Current offset
  - `has_more` - Boolean flag for more pages

#### Query Filtering
- **QueryableStore trait** for filterable storage
- Generic query filtering system
- Support for common filter operations

### Documentation
- New [Pagination and Filtering Guide](docs/guides/PAGINATION_AND_FILTERING.md)
- Updated API documentation with pagination examples

---

## [0.0.3] - 2024-11-01

### Added - DynamoDB Support

#### Storage Backend
- **DynamoDBLinkService** - AWS DynamoDB backend for links
- **DynamoDB feature flag** - Optional dependency
- Configuration via AWS SDK

#### Features
- Efficient link queries using DynamoDB secondary indices
- Support for link metadata
- Batch operations for performance

### Changed
- Made storage backends optional via feature flags
- Default to `in-memory` storage

### Documentation
- Added DynamoDB setup guide
- Updated architecture documentation with storage layer

---

## [0.0.2] - 2024-10-25

### Added

#### Macro System
- **`impl_data_entity!` Macro** - Generate complete Data entities with zero boilerplate
- **`impl_link_entity!` Macro** - Generate custom Link entities
- **Helper macros**: `entity_fields!`, `data_fields!`, `link_fields!`
- 97% reduction in entity definition code

#### Entity Creation System
- **EntityCreator trait** - Dynamically create entities
- **Two ways to create links**:
  1. Link existing entities: `POST /{source}/{id}/{route}/{target_id}`
  2. Create entity + link: `POST /{source}/{id}/{route}` with entity data
- Auto-enriched responses with full entity data

#### Entity Features
- **Built-in soft delete** - `soft_delete()` and `restore()` methods
- **Automatic timestamps** - `created_at`, `updated_at` auto-managed
- **Status management** - `set_status()` with timestamp update
- **Type safety** - Entity type enforcement via macros

#### Auto-Routing
- **ServerBuilder** - Fluent API for building servers
- **Auto-route generation** - 88% code reduction (340 → 40 lines)
- **EntityRegistry** - Automatic CRUD route creation
- **Module system** - Group related entities with configuration

### Changed

#### Breaking Changes
- **Removed multi-tenancy** - Simplified architecture without `tenant_id`
- **New entity hierarchy** - `Entity` → `Data` / `Link` trait hierarchy
- **Updated Module trait** - Added `register_entities()` and entity fetcher/creator methods
- **Simplified service APIs** - Removed tenant_id from all operations

### Removed
- Multi-tenancy support (can be implemented at application level if needed)
- Tenant-scoped queries and operations
- `tenant_id` field from all entities

### Documentation
- Complete rewrite of all documentation
- New [Quick Start Guide](docs/guides/QUICK_START.md)
- New [Getting Started Guide](docs/guides/GETTING_STARTED.md)
- Updated [Architecture Documentation](docs/architecture/ARCHITECTURE.md)
- New [ServerBuilder Implementation Guide](docs/architecture/SERVER_BUILDER_IMPLEMENTATION.md)

### Migration Guide

#### Entities
```rust
// Old (v0.0.1)
struct Order {
    id: Uuid,
    tenant_id: Uuid,  // ❌ Removed
    name: String,
    amount: f64,
}

// New (v0.0.2)
impl_data_entity!(Order, "order", ["name"], {
    amount: f64,
});
// ✅ tenant_id removed, macro generates all boilerplate
```

#### Services
```rust
// Old (v0.0.1)
service.create(&tenant_id, order).await?;
service.get(&tenant_id, &id).await?;

// New (v0.0.2)
service.create(order).await?;
service.get(&id).await?;
// ✅ No more tenant_id parameter
```

---

## [0.0.1] - 2024-10-22

### Added - Initial Release

#### Core Framework
- Generic entity and relationship management framework
- Bidirectional link navigation
- YAML-based configuration for links
- Auto-pluralization system
- Flexible relationships between entities
- In-memory storage implementation

#### Multi-Tenancy (Removed in v0.0.2)
- Multi-tenant support with tenant isolation
- Tenant-scoped queries and operations

#### API Features
- Auto-generated CRUD routes for entities
- Link enrichment with full entity data
- Contextual link fetching (forward/reverse/direct)

#### Core Components
- `EntityDescriptor` trait for route generation
- `Module` trait for grouping entities
- `LinkService` for relationship management
- `EntityFetcher` for dynamic entity loading

#### Examples
- `simple_api` - Basic CRUD example
- `full_api` - Complete API with all features
- `microservice` - Billing microservice example

#### Documentation
- Comprehensive README with examples
- Architecture documentation
- Getting started guide
- API documentation
- Link enrichment guide
- Multi-level navigation guide

---

## Version Comparison Summary

| Version | Key Feature | Impact |
|---------|-------------|--------|
| **0.0.6** | 🆕 **GraphQL Support** | Multi-protocol APIs (REST + GraphQL) |
| 0.0.5 | Validation & Filtering | Automatic data validation |
| 0.0.4 | Pagination | Generic pagination for lists |
| 0.0.3 | DynamoDB Support | Production storage backend |
| 0.0.2 | Macro System + Auto-Routing | 97% less boilerplate |
| 0.0.1 | Initial Release | Core framework |

---

[Unreleased]: https://github.com/triviere/this-rs/compare/v0.0.6...HEAD
[0.0.6]: https://github.com/triviere/this-rs/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/triviere/this-rs/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/triviere/this-rs/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/triviere/this-rs/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/triviere/this-rs/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/triviere/this-rs/releases/tag/v0.0.1
