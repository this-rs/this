# Journal et reprise durable — ce que le harnais promet, ce que l'hôte possède

Un run d'agent produit trois choses de natures différentes : ce qu'on **regarde**
(les événements), ce qu'il faut pour **reprendre** (le fil et la file), et ce qu'il a
**fait au monde** (les effets des appels d'outils). Les confondre est l'erreur
habituelle : on croit qu'enregistrer les étapes rend le run rejouable, alors qu'un
journal d'étapes décrit le passé d'un processus et non les droits du suivant.

Ce document est un inventaire de preuves. Il ne propose pas de mécanisme : il dit,
pour chacun de ces états, qui le porte **aujourd'hui**, s'il est durable ou au mieux,
et à qui appartient sa persistance. Puis il tranche les cinq cases d'acceptation du
ticket #284, une par une, en nommant le fichier et la ligne quand la preuve existe, et
en disant « manque » quand elle n'existe pas.

Mesuré le 20/11/2026 sur `origin/main` `bd387e8` (copie de travail
`docs/harness-contrats`, `crates/this-harness`).

## 0. Ce qui a été lu, et jusqu'où

Lus **en entier** : `crates/this-harness/src/ports.rs`, `event.rs`, `lib.rs`,
`context.rs`, `recall.rs`, `embed.rs`, `search.rs` ; les tests `enregistreur.rs`,
`robustesse_ports.rs`, `residus.rs`, `robustesse.rs`, `scenario.rs`, `garde.rs`,
`contexte.rs`, `indices.rs`, `borne_de_contexte.rs`, `budget_contexte.rs`,
`compaction.rs`, `famille_a.rs`, `approbation.rs`, `approbation_guarded.rs`,
`grants.rs` (noms et corps des tests cités) ; `src/storage/harness_graph.rs` (en-tête,
API publique, noms des tests) ; `src/storage/agent_substrate.rs` 85–160 et les tests ;
`src/server/host.rs` 819–860.

Lus **par leur plan seulement** (noms de fonctions, en-têtes, commentaires de
contrat) : `crates/this-harness/src/session.rs` au-delà des zones citées (le fichier
fait 3 215 lignes ; `pump` 1172–1738, `run_tool` 1752–1827, `request`/`compact`
1941–2046, le `Handle` 2317–2516 et les tests unitaires 2520–3200 ont été lus) ;
`crates/this-agent/src/graph.rs`, `approval.rs` 1–120, `masque.rs` et `flux.rs` (en-têtes).

**Non lus, donc non qualifiés ici** : `crates/this-harness/src/routing/*` (le mode Auto
n'intervient pas dans la durabilité d'un run), `src/storage/obrain.rs` (le magasin
lui-même), et **les hôtes hors de ce dépôt** (l'Atelier, Project Orchestrator) : quand
une propriété n'est tenue que par eux, elle est écrite « non qualifié », jamais
supposée.

---

## 1. La matrice

Cinq lignes : les quatre états du ticket — observation, état de reprise, effets
externes, permissions en attente — plus le journal, que le code sépare du reste et qui
mérite donc sa ligne. Trois questions par ligne. « Au mieux » veut dire : le harnais
pousse, personne ne garantit que c'est écrit ; « durable » veut dire : une
implémentation de ce dépôt écrit sur disque et le relit après réouverture.

| État du run | Qui le porte aujourd'hui (`fichier:ligne`) | Durable ou au mieux | Propriétaire de la persistance |
|---|---|---|---|
| **Observation / télémétrie** — ce que l'interface lit pendant le run | `Event` (`crates/this-harness/src/event.rs:40-184`), émis par `Out::emit` dans une file en mémoire vidée vers un puits **synchrone** (`session.rs:681-699`) ; un puits qui panique tue le run, pas le harnais (`session.rs:655-657`) | **Au mieux** : rien n'est écrit, rien n'est relu ; la file est en mémoire et non bornée (`session.rs:659-661`) | **Hôte** (le puits). Le cœur ne persiste aucun événement (`session.rs:18-19`) |
| **Journal des étapes** — la trace durable du même run | Port `Recorder` (`ports.rs:422-432`), poussé par la boucle après chaque appel (`session.rs:1817-1825`) et à l'arrêt (`session.rs:1726-1733`) ; implémentation réelle : `HostRecorder` (`src/server/host.rs:827-860`) → `HarnessGraph::append_step_live` (`src/storage/harness_graph.rs:397`) et `record_run` (`:358`), nœuds `_run`/`_step` du graphe obrain | **Durable** (sous la feature `obrain`), un commit par étape (~35 ms médian, `src/storage/harness_graph.rs:27-33`), et relisible : `load_run` (`:453`), `load_runs` (`:459`) | **Hôte** (this-rs, `src/storage/harness_graph.rs`) — mais le **cœur** décide quand pousser, et le contrat est explicitement « au mieux » (`ports.rs:414-420`) |
| **État indispensable à la reprise** — le fil et la file | `State` (`session.rs:630-641`) → `Snapshot { version, transcript, pending, revoked }` (`event.rs`), produit par `Handle::snapshot` (`session.rs:2477-2487`), remis par `Handle::restore` (`session.rs:2489-2515`) | **Au mieux, et même pas poussé** : le harnais n'écrit rien ; **aucune implémentation de stockage de `Snapshot` n'existe dans ce dépôt** (`grep -rn "Snapshot" src/` ne rend que des homonymes d'autres modules). Le seul test de reprise remplit une seconde instance en mémoire (`tests/scenario.rs:489`) | **Hôte**, entièrement. Ce que le `Snapshot` ne porte pas : `busy`, `generation`, `cancel`, `run_seq` (`session.rs:630-641` contre `2477-2487`), le `Run` lui-même (étapes, effets inconnus, arrêt), le registre des ids vus, l'état du routeur |
| **Effets externes** — ce que les outils ont fait au monde | `Outcome.before` / `after` (`event.rs:270-273`, `unchanged()` `:305-308`), `Step` (`crates/this-agent/src/lib.rs:99-128`), `Run.steps` (`:202`), `effet_inconnu` (`:302-314`) ; l'outil lui-même est appelé par le port (`ports.rs:247-260`) | **Au mieux** : aucun enregistrement transactionnel. La boucle émet `ToolUse` **avant** l'appel (`session.rs:1761-1765`) mais n'écrit l'étape qu'**après** (`session.rs:1817-1825`) : entre les deux, aucun état durable ne dit qu'un effet a été tenté. Le seul marqueur est le nœud de run resté `_stop: "running"` (`crates/this-agent/src/graph.rs:335`, `src/storage/harness_graph.rs:435`) | **Hôte** pour l'effet (l'outil est à lui), **cœur** pour la qualification de l'effet (`effet_inconnu`, `verdict_of` `session.rs:2289-2296`) |
| **Permissions humaines en attente** — « l'agent demande, la personne répond » | **Une file en mémoire (#286).** Un outil déclaré SANS aucun grant ouvre une demande (`PermissionRequest`, `session.rs` `demander`) ; l'appel est refusé pour ce tour. `Harness::answer_permission(id, oui/non)` installe le grant pour les appels SUIVANTS ; un id inconnu est rejeté. Un refus explicite ne redemande jamais. `Permission` (`ports.rs`) reste une décision synchrone ; `Approval` ne porte que des compositions. | **Non persisté, volontairement** : la file n'entre pas dans le `Snapshot` (ce serait rouvrir la matrice). Une demande restée sans réponse à l'arrêt est reposée au prochain appel. Une réponse, elle, vit dans les grants de l'hôte, qui les persiste comme le reste de ses autorisations | **Cœur fait pour la file ; hôte pour la persistance des réponses** (la révocation, elle, survit : #287) |

### Ce que la cinquième ligne dit du contrat

Le cœur **refuse** de porter une permission en attente, et c'est un choix écrit : la
boucle ne fait « aucune entrée/sortie » (`session.rs:18-19`), et une autorisation qui
attendrait dans un état portable devrait être reprenable après un redémarrage — donc
révoquée, datée, réaffichée. La file de demandes existe désormais dans le cœur (#286), mais elle est **en mémoire** :
une question en attente n'est pas reprise après un redémarrage — elle est reposée au
prochain appel. Une réponse devient un grant ; c'est à l'hôte de le persister, et la
révocation, elle, survit grâce au `Snapshot` (#287).

### Ce que la troisième ligne dit du contrat

`Snapshot` est un état de **conversation**, pas un état de **run** : il ne contient ni
identité de run, ni étapes, ni budget consommé, ni effets inconnus. Reprendre après une
fermeture, c'est donc repartir sur un objectif neuf avec le fil d'avant — ce qui est
exactement ce que le harnais promet (`resume`, `session.rs:1144-1152`, qui reprend la
**file**, pas un run), et pas davantage.

---

## 2. Les cinq cases d'acceptation

Verdict binaire. « Preuve existante » = un test nommé ou une ligne de code le prouve
**tel qu'énoncé** ; « manque » = il faut écrire quelque chose.

### (a) Matrice explicite — **manque** (produite par ce document)

Rien dans le dépôt ne met ces cinq lignes côte à côte. Ce qui existe est dispersé et
partiel : le contrat du `Recorder` (`ports.rs:399-420`), la doc de `Snapshot`
(`event.rs:236-241` : « ce qu'il faut pour reprendre après une fermeture d'app »), et
la note de reprise de `session.rs:1144`. La section 1 ci-dessus est la matrice ; il
reste à l'**épingler** par un test qui échoue si une ligne cesse d'être vraie — par
exemple un test qui vérifie que `Snapshot` reste à trois champs tant que la reprise ne
prétend pas plus, ou un test de `Recorder` défaillant qui nomme explicitement « perdu,
assumé » (`session.rs:420-426` porte cette phrase, aucun test ne l'épingle).

### (b) Crash avant l'effet / après l'effet avant acquittement / après checkpoint, reprise sur processus neuf et store réel — **preuve** (partielle)

Ce qui existe :

- une **réouverture** de magasin réel, dans le processus de test :
  `une_composition_survit_a_la_reouverture` (`src/storage/harness_graph.rs:588`),
  `les_aretes_has_step_survivent_a_la_reouverture` (`:780`),
  `les_autorisations_sont_fail_closed_et_survivent`
  (`src/storage/agent_substrate.rs:305`, qui ferme et rouvre le magasin) ;
- une reprise **en mémoire** du fil et de la file :
  `un_etat_sauvegarde_se_repare_et_se_rejoue` (`tests/scenario.rs:489`) ;
- un journal qui distingue un run vivant d'un run fini (`_stop: "running"`,
  `crates/this-agent/src/graph.rs:335`, test `:665`), et une réparation de fil après
  abandon (`repair_thread`, `session.rs:764-786`, qui écrit « interrompu : le run a été
  abandonné avant la réponse de l'outil »).

Ce qui manque, et pourquoi ce n'est pas un oubli de test : **la crate ne peut pas
tuer un processus**. Ses `dev-dependencies` sont `futures` et `this-llm-providers`
(`crates/this-harness/Cargo.toml`, section *dev-dependencies*) : ni `tempfile`, ni
`std::process`, ni `std::fs` n'y sont utilisés (`grep -rn "std::process\|tempfile\|std::fs"
crates/this-harness/tests/` → rien). Les trois cas demandent donc **soit** un test
dans un hôte qui possède un magasin (l'endroit naturel : `src/storage/harness_graph.rs`
a déjà le magasin et `src/server/host.rs:827` le `Recorder`), **soit** trois tests
`#[ignore]` dans cette crate précédés d'un `std::process::Command` qui relance le
binaire de test avec une variable d'environnement (« je suis l'enfant, je meurs au
point N ») — ce qui exige d'ajouter `tempfile` en dev-dependency et un point de coupe
injectable dans `Recorder` (une part qui `std::process::abort()`).

Trois points de coupe précis à écrire, dans cet ordre de valeur :

1. **avant l'effet** : `Event::ToolUse` est émis (`session.rs:1761`), le processus
   meurt avant `toolbox.call_with_cancel` (`session.rs:1788`) — le store relit un run
   sans étape, et rejouer est sans risque ;
2. **après l'effet, avant acquittement** : l'outil a écrit, le processus meurt avant
   `session.rs:1817` — le store ne contient **aucune** étape (`ToolUse` n'est qu'un
   événement, `session.rs:1761`), donc la reprise ne peut pas savoir que l'effet a été
   tenté. C'est le cas qui manque le plus, et le seul remède possible est un
   enregistrement **avant** l'appel (« intention ») que le cœur ne fait pas ;
3. **après checkpoint** : le store contient l'étape, `_stop` vaut encore `running` ; la
   reprise doit lire `load_run` (`src/storage/harness_graph.rs:453`) et refuser de
   rejouer.

**Preuve** : `tests/reprise_crash.rs` (crate racine, feature `obrain`). Un sous-processus (le binaire
de test ré-invoqué sur `enfant`) pilote un vrai `Harness` branché sur le vrai journal
(`HostRecorder` → `HarnessGraph` → `ObrainStore` sur disque), puis meurt par
`std::process::abort()` (le parent exige SIGABRT) ; le parent rouvre le magasin dans un
tempdir et relit. Chaque test a été vu rouge sous mutation de la garantie qu'il vérifie.

| Cas | Test | Ce qu'il fige — et rien de plus |
|---|---|---|
| avant l'effet | `crash_avant_l_effet_ni_journal_ni_monde_rejouer_est_sans_risque` | journal sans run, effet absent du monde |
| après l'effet, avant acquittement | `crash_apres_l_effet_avant_l_acquittement_le_journal_ne_sait_pas` | l'effet a eu lieu, le journal est **identique** au cas précédent : la reprise ne peut pas savoir que l'effet a été tenté. C'est la limite (pas d'intention écrite avant l'appel) ; le test l'épingle, il ne la comble pas |
| après checkpoint | `crash_apres_checkpoint_l_etape_est_relue_et_le_run_n_est_pas_fini` | l'étape commitée se relit (effet accompli, ni refus ni échec), le run n'a pas d'arrêt (`running`) |

La matrice est épinglée par `tests/matrice_snapshot.rs`
(`snapshot_reste_a_trois_champs_tant_que_la_matrice_le_dit`) : destructuring exhaustif de
`Snapshot`, un quatrième champ ne compile plus.

Cas « effet de résultat inconnu non rejoué » : case (c), pas dupliqué.

**Reste non couvert** : (1) le remède du cas « après l'effet » (journaliser une intention
avant l'appel) n'existe pas : seule la limite est prouvée. (2) Aucun test ne fait **agir**
une reprise : l'hôte n'a pas de logique de reprise de run, les tests prouvent ce que le
journal donne à lire, pas une décision de rejeu. (3) Un seul point de coupe par cas, un
seul outil, un seul processus écrivain ; pas de crash pendant le commit lui-même
(`commit_wal`), pas de coupure d'alimentation (SIGABRT ne perd pas le cache de l'OS). (4) Le
`Snapshot` (fil + file) n'a toujours pas de stockage disque : la ligne 3 de la matrice reste
« hôte, entièrement ». (5) Les tests ne tournent qu'en `cfg(unix)` pour la vérification du
signal. (6) **Ces tests ne tournent PAS en CI aujourd'hui** : le journal durable vit dans le
crate racine et le magasin est obrain, dont le dépôt est privé ; le job `obrain` ne s'active
que si le secret `OBRAIN_TOKEN` est défini, sinon il n'émet qu'un avertissement. Ils se
lancent en local : `cargo test --features obrain --test reprise_crash`. Placés d'abord dans
`this-harness` avec une dev-dépendance vers le crate racine, ils cassaient Clippy et
Coverage (les crates `obrain-*` y sont des stubs, d'où `unresolved import obrain_common::types`)
et auraient cassé la suite navigateur de #285 : la dev-dépendance a été retirée.

### (c) Un effet non idempotent au résultat inconnu n'est pas rejoué aveuglément — **preuve existante**

Trois preuves, à trois niveaux :

- **la relance ne rejoue pas l'outil** : la condition de relance exige qu'aucun outil
  n'ait tourné dans le tour (`run.borrow().steps.len() == steps_at_turn`,
  `session.rs:1433-1455`), et le test
  `une_panne_au_tour_suivant_un_outil_est_relancee_sans_rejouer_l_outil`
  (`tests/garde.rs:436`) compte **un** `ToolResult` pour trois appels au modèle ;
- **un outil abandonné a un effet inconnu, pas un échec** : `session.rs:1598-1610` pose
  `run.effet_inconnu(index)` ; `un_outil_abandonne_a_un_effet_inconnu_pas_un_echec_prouve`
  (`session.rs:3074`), `une_etape_abandonnee_n_arrete_pas_le_run_en_echec`
  (`session.rs:3101`) vérifient que le verdict ne conclut ni réussite ni échec ;
- **la relance demande de vérifier, pas de refaire** : le rappel d'effet inconnu est
  un texte distinct de « appelles maintenant » (`session.rs:3101`, assertions
  `contains("vérifie")` et `!contains("appelles maintenant")`).

Résidu, qui n'invalide pas la case mais borne sa portée : la non-répétition est
**locale au processus**. Un outil non idempotent dont l'effet est inconnu au moment
d'un crash ne laisse aucune trace (voir (b), point 2), et `Run.inconnues` n'est pas
dans le `Snapshot` : la reprise repart d'un run neuf. La règle tenue est « le harnais
ne rejoue pas », pas « personne ne peut rejouer ».

### (d) File utilisateur conservée, permission périmée non restaurée comme accordée, versions futures refusées avec motif — **manque**

- **File conservée : preuve.** `Snapshot.pending` (`event.rs:247-248`) est écrit
  (`session.rs:2480-2485`) et relu (`session.rs:2502`) ;
  `un_etat_sauvegarde_se_repare_et_se_rejoue` (`tests/scenario.rs:489`) compare les
  identifiants, les textes et l'instant d'arrivée (`at_ms`, à 1e-3 près, `:509-516`) ;
  `la_file_se_manipule_sans_perdre_l_ordre` (`tests/scenario.rs:467`) couvre
  ajout/édition/remontée/retrait ; `un_pending_se_relit_dans_le_json`
  (`tests/scenario.rs:649`) la sérialisation. Les identifiants ne reculent pas après
  une reprise (`session.rs:2503-2512`, test `restore_ne_fait_pas_reculer_les_identifiants`,
  `tests/robustesse.rs:191`).
- **Permission révoquée : restaurée comme REFUS, jamais comme octroi (#287).** Le
  `Snapshot` porte désormais `revoked` : les outils dont le consentement a été retiré au
  moment de la sauvegarde. Restaurer applique ces refus, et **ne peut rien accorder** :
  un snapshot importé ne peut donc pas élever de privilège. Une révocation restaurée sur
  un hôte qui n'a installé aucun grant n'est pas appliquée en silence : un `Hint` le dit.
  Limite : un transport de la famille A garde sa copie des grants jusqu'au run suivant.
- **Octroi : jamais persisté.** Une autorisation accordée n'est pas dans le `Snapshot` :
  c'est à l'hôte de la réinstaller, et c'est ce qui rend la révocation nécessaire. Un hôte qui persiste ses `Grants` à côté
  (`src/storage/agent_substrate.rs:157`) doit décider lui-même ce qu'une autorisation
  accordée hier vaut aujourd'hui — le cœur ne l'aide pas. « Non qualifié » pour tout
  hôte réel.
- **Versions futures : manque net.** `Snapshot.version` est **écrit** (`version: 1`,
  `session.rs:2483`, défaut `one()` `event.rs:243,251-253`) et **jamais lu** :
  `restore` (`session.rs:2489-2515`) ne touche pas au champ. Un `Snapshot` de
  version 7 est donc accepté et appliqué sans un mot, alors que la case demande un
  refus **motivé**. Ce qu'il faut écrire : dans `restore`, avant toute mutation,
  `if snapshot.version != 1 { emit(Hint « version N inconnue, reprise refusée »); return; }`
  — et le test qui l'accompagne (« un snapshot de version future est refusé, le fil en
  place n'est pas touché »). Le pendant existe déjà pour les événements : un genre
  inconnu retombe sur `Other` (`event.rs:200-214`) et un verdict inconnu sur `Unknown`
  (`event.rs:366-391`) — le harnais sait faire du « futur toléré ». `Snapshot`, lui,
  n'a choisi ni de tolérer ni de refuser : il ignore le champ.

### (e) Recorder défaillant toléré en observation, aucune promesse de durabilité implicite — **preuve existante**

- **Tolérance : preuve.** Le trait ne rend rien (`ports.rs:427`, `:431`), le contrat le
  dit à l'endroit où on le cherche (« Un échec d'enregistrement n'interrompt jamais le
  run », `ports.rs:414-420`), et la boucle isole chaque appel : panique avalée, blocage
  borné à `RECORDER_TIMEOUT_MS` = 2 s (`session.rs:420-461`). Tests :
  `a_panicking_recorder_does_not_stop_the_run` (`tests/residus.rs:166`),
  `a_silent_recorder_delays_the_run_by_a_bounded_time` (`:172`, qui borne la dépense à
  `2 × RECORDER_TIMEOUT_MS`) ; `un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret`
  (`tests/robustesse.rs:249`) vérifie qu'une future lâchée pendant l'enregistrement ne
  prive pas l'interface de son `Stopped` (`garde.finale`, `session.rs:738-741`,
  `1718-1736`). La composition `Recorders` pousse à toutes les parts sans s'arrêter à
  la première qui échoue (`ports.rs:466-479`, test
  `les_enregistreurs_sont_appeles_dans_l_ordre_d_ajout`, `tests/robustesse_ports.rs:138`).
- **Aucune promesse implicite : preuve d'intention, pas de garde.** La perte est
  écrite noir sur blanc — « L'étape ou l'arrêt concerné est alors perdu pour ce
  journal, et c'est assumé » (`session.rs:424-426`) — et l'implémentation réelle
  l'applique (`src/server/host.rs:841-849` avale l'erreur **et** la panique du masque).
  Ce qui manque pour que la case soit *tenue* et non seulement *dite* : rien dans le
  type n'empêche un hôte de lire `Recorder::step` comme une durabilité. La séparation
  demandée est un **contrat nommé** (« ce port est de la télémétrie ; la durabilité du
  run est un autre port, à écrire ») ; elle vit aujourd'hui dans deux commentaires, pas
  dans une signature.

---

## 3. Ce qui existe déjà et qu'il ne faut pas recréer

Cherché dans les 26 fichiers de `crates/this-harness/tests/` et dans les tests
unitaires de `session.rs`. La liste est exhaustive pour `Recorder`, `Snapshot`,
reprise, effets, permissions en attente.

**Note d'inventaire.** Le brief annonçait un ou des fichiers `resume*` : **il n'y en a
pas**. `ls crates/this-harness/tests/` rend 26 noms (`adverse`, `approbation`,
`approbation_guarded`, `borne_de_contexte`, `budget_contexte`, `codes`, `compaction`,
`contexte`, `declarations`, `embeddings`, `enregistreur`, `famille_a`, `garde`,
`gardes`, `grants`, `indices`, `mcp`, `mode_auto`, `rappels`, `recherche`, `residus`,
`robustesse`, `robustesse_fil`, `robustesse_ports`, `scenario`, `spoke`) ; la reprise y
est couverte par `scenario.rs` et `robustesse.rs`. `Harness::resume` (`session.rs:1144`)
n'est testé que sur file vide (`tests/garde.rs:216`).

### Recorder (branchement, composition, défaillance)

| Test | Fichier:ligne | Ce qu'il fige |
|---|---|---|
| `a_run_pushes_its_steps_and_its_stop` | `tests/enregistreur.rs:96` | une étape poussée avec le run déjà à jour, sous `run-1` ; un seul `stopped`, et `run.stop == stop` |
| `each_goal_gets_its_own_run_id` | `tests/enregistreur.rs:113` | deux objectifs → `run-1`, `run-2` |
| `les_enregistreurs_sont_appeles_dans_l_ordre_d_ajout` | `tests/robustesse_ports.rs:138` | `Recorders` pousse à toutes les parts, dans l'ordre |
| `a_panicking_recorder_does_not_stop_the_run` | `tests/residus.rs:166` | panique avalée |
| `a_silent_recorder_delays_the_run_by_a_bounded_time` | `tests/residus.rs:172` | blocage borné à 2 × 2 s |
| `un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret` | `tests/robustesse.rs:249` | `busy` rendu, un seul `Stopped` |
| `un_refus_de_la_porte_remonte_au_moteur_et_journalise_la_meme_etape` | `tests/famille_a.rs:124` | la porte hors-bande (`Permission::record`) journalise la **même** étape que `run_tool` |
| `un_outil_autorise_est_journalise_hors_bande_sans_rejeu` | `tests/famille_a.rs:168` | famille A : journalisée, pas rejouée |

### Snapshot, file, reprise

| Test | Fichier:ligne | Ce qu'il fige |
|---|---|---|
| `un_etat_sauvegarde_se_repare_et_se_rejoue` | `tests/scenario.rs:489` | aller-retour JSON complet, file et `at_ms` conservés |
| `la_file_se_manipule_sans_perdre_l_ordre` | `tests/scenario.rs:467` | ajout, édition, remontée, retrait idempotent |
| `un_pending_se_relit_dans_le_json` | `tests/scenario.rs:649` | forme sérialisée de `Pending` |
| `restore_ne_fait_pas_reculer_les_identifiants` | `tests/robustesse.rs:191` | `next_id` monotone après reprise |
| `restore_pendant_un_run_ne_corrompt_pas_le_fil` | `tests/robustesse.rs:206` | `restore` refusé et **dit** pendant un run (`session.rs:2490-2498`) |
| `dix_mille_messages_en_file` | `tests/robustesse.rs:448` | la file est bornée (`MAX_PENDING = 1000`, `session.rs:2312`) et le refus se dit (`:2360-2368`) |
| `un_pump_sans_objectif_n_annonce_pas_termine` | `tests/garde.rs:208` | `resume()` sur file vide → `None`, aucun `Stopped` inventé |

### Effets, annulation, abandon

| Test | Fichier:ligne | Ce qu'il fige |
|---|---|---|
| `une_panne_au_tour_suivant_un_outil_est_relancee_sans_rejouer_l_outil` | `tests/garde.rs:436` | la relance ne rejoue pas l'outil |
| `une_panne_durable_n_est_pas_relancee` | `tests/garde.rs:473` | une panne non passagère ne tourne pas en boucle |
| `aucune_relance_apres_du_texte_montre` | `tests/garde.rs:419` | rejouer après avoir montré doublerait un effet déjà lu |
| `un_outil_qui_depasse_son_plafond_rend_delai_depasse` | `tests/garde.rs:598` | plafond → délai dépassé, jamais un succès |
| `sans_plafond_l_annulation_libere_quand_meme_un_outil_pendu` | `tests/garde.rs:631` | l'annulation borne même sans plafond |
| `l_annulation_atteint_les_outils` | `tests/gardes.rs:371` | le drapeau arrive jusqu'à l'outil |
| `une_future_lachee_pendant_un_outil_laisse_un_fil_valide` | `tests/gardes.rs:575` | `repair_thread` (`session.rs:764-786`) : pas d'appel sans réponse |
| `une_future_abandonnee_rend_la_place_et_le_dit` | `tests/gardes.rs:144` | `BusyGuard` rend `busy` et annonce `Interrupted` |
| `un_outil_abandonne_a_un_effet_inconnu_pas_un_echec_prouve` | `src/session.rs:3074` | effet inconnu ≠ échec prouvé |
| `une_etape_abandonnee_n_arrete_pas_le_run_en_echec` | `src/session.rs:3101` | le rappel demande de vérifier, pas de refaire |
| `le_verdict_ignore_l_etape_inconnue_mais_garde_le_reste` | `src/session.rs:3154` | un refus antérieur reste un refus |
| `des_bornes_degenerees_se_terminent_en_disant_leur_arret` | `tests/robustesse.rs:274` | bornes à zéro → un seul `Stopped`, pas de boucle |
| `une_action_sans_effet_n_est_pas_un_progres` | `tests/scenario.rs:403` | `Outcome::unchanged()` n'est pas un progrès |

### Journal durable (hors de la crate, même dépôt)

| Test | Fichier:ligne | Ce qu'il fige |
|---|---|---|
| `une_composition_survit_a_la_reouverture` | `src/storage/harness_graph.rs:588` | le magasin relit après fermeture |
| `un_run_se_relit_avec_ses_etapes_dans_l_ordre` | `src/storage/harness_graph.rs:603` | `_index`, but, arrêt, empreintes survivent |
| `deux_runs_de_meme_id_et_de_meme_but_se_relisent_distincts` | `src/storage/harness_graph.rs:694` | deux runs de même id ne s'écrasent pas (`base@<empreinte>`, `:331-334`) |
| `les_aretes_has_step_survivent_a_la_reouverture` | `src/storage/harness_graph.rs:780` | les arêtes `HAS_STEP` sont durables |
| `une_section_retiree_disparait_du_graphe` | `src/storage/agent_substrate.rs:278` | un retrait de contexte est un vrai retrait |

### Autorisation et approbation (ce n'est pas une permission en attente)

`tests/grants.rs` (`un_outil_non_declare_est_refuse_et_jamais_appele:115`,
`un_outil_sans_aucune_autorisation_est_refuse_fail_closed:141`,
`un_outil_qui_apparait_en_cours_de_session_n_est_pas_refuse:309`),
`tests/approbation.rs:113-155`, `tests/approbation_guarded.rs:152-184`,
`tests/declarations.rs` : ces tests prouvent une autorisation **décidée**, fail-closed,
et une composition **approuvée** par empreinte. Aucun ne couvre une question posée à
une personne et sa réponse différée — c'est la cinquième ligne de la matrice (la seule
sans aucun porteur), et elle est vide des deux côtés, cœur et tests.

---

## 4. Ce que le ticket ne demande pas

À lire avant d'écrire une ligne de code, parce que chacune de ces choses est tentante
et chacune contredirait un choix déjà fait et mesuré.

- **Pas de base de données ni de `fsync` dans le cœur portable.** `this-harness`
  compile en wasm32 sans système de fichiers et « ne fait aucune entrée/sortie »
  (`session.rs:18-19`, `ports.rs:1-5`). Un `fsync` dans la boucle rendrait le crate
  impossible à compiler pour la page — c'est-à-dire là où il sert. La séparation est
  déjà outillée : le magasin vit dans `src/storage/harness_graph.rs`, derrière la
  feature `obrain`, et la crate du harnais ne l'a jamais dans ses dépendances.
- **Pas de promesse exactly-once sur des services externes.** Le harnais ne peut pas
  la tenir : il ne voit pas l'effet, il voit le retour de l'outil
  (`ports.rs:247-260`). La seule chose qu'il peut promettre est ce qu'il tient déjà :
  ne pas **rejouer** (case (c)) et ne pas **conclure** (verdict, `session.rs:2289`).
- **Pas de réécriture du `Recorder` sans analyse des hôtes.** Une implémentation réelle
  existe hors de la crate (`HostRecorder`, `src/server/host.rs:827`) et les tests en
  portent plusieurs doublures (`tests/enregistreur.rs:65` `Count`,
  `tests/robustesse_ports.rs:126` `Rec`, `tests/residus.rs:102` `Mauvais`,
  `tests/robustesse.rs:237` `SlowRecorder`, `tests/famille_a.rs:89` `Journal`). Ajouter un `Result` au trait (`ports.rs:427`, `:431`) obligerait
  chaque hôte à décider quoi faire d'une erreur que le contrat dit déjà d'avaler : ce
  serait une rupture d'API pour un gain nul côté boucle. Si la durabilité doit être un
  contrat distinct, c'est un **port de plus**, pas un `Result` de plus.
- **Pas de reprise d'un `Run` en cours.** Rien dans ce document ne demande de faire
  survivre les étapes, le budget consommé ou les effets inconnus d'un run à la
  fermeture de l'app : c'est un état d'exécution, et le journal le raconte déjà pour
  qui veut le lire (`load_run`, `src/storage/harness_graph.rs:453`).

---

## 5. Ce que l'inventaire a révélé d'inattendu

1. **Le journal durable existe déjà, de bout en bout, et personne ne le dit.** Le
   contrat du `Recorder` est écrit « au mieux » (`ports.rs:414-420`) et, dans le même
   dépôt, `HostRecorder` (`src/server/host.rs:827-860`) écrit chaque étape dans obrain
   et la relit après réouverture (`src/storage/harness_graph.rs:397`, tests `:588`,
   `:603`, `:780`). Le ticket #284 n'est donc pas « construire un journal » : c'est
   **nommer** la moitié durable et couvrir la moitié « reprise », qui n'a, elle, aucune
   implémentation (`Snapshot` n'est stocké nulle part dans ce dépôt).
2. **Le trou de la fenêtre `ToolUse` → `step`.** La boucle émet `Event::ToolUse` avant
   l'appel (`session.rs:1761-1765`) et n'écrit l'étape qu'après (`session.rs:1817-1825`).
   Un crash entre les deux ne laisse **aucune** trace durable d'un effet pourtant
   peut-être produit : c'est exactement le cas « après l'effet avant acquittement » de
   la case (b), et il ne se répare pas par un test — il demande une écriture d'intention
   avant l'appel, que le cœur ne fait pas.
3. **`Snapshot.version` est un champ mort.** Écrit (`session.rs:2483`, `event.rs:243`)
   et jamais lu (`session.rs:2489-2515`). La case (d) « versions futures refusées avec
   motif » est donc un manque d'une dizaine de lignes, pas un chantier — et c'est le
   seul endroit de la reprise où le harnais accepte un état qu'il ne comprend pas.
4. **La collision d'identifiants de run est déjà traitée, mais pas là où on la
   chercherait.** `run_seq` repart à 1 dans un processus neuf (`session.rs:640`,
   `1242-1245`) : deux processus produisent donc tous deux `run-1`. Le magasin le
   rattrape (`base@<empreinte>` quand l'empreinte d'origine diffère,
   `src/storage/harness_graph.rs:285-310`, `331-334`, test `:694`) — mais le
   `Snapshot`, lui, ne porte pas `run_seq`, donc la reprise ne peut pas espérer éviter
   la collision : elle dépend entièrement de l'hôte.
5. **Aucune question en attente, nulle part.** Pas de type, pas de champ, pas de test :
   `Permission` est synchrone, `Grants` est une liste, `Approval` est une empreinte. La
   cinquième ligne de la matrice est vide non par oubli de documentation mais parce que
   le cœur a explicitement refusé ce sujet (`session.rs:18-19`). Le document doit le
   dire pour qu'un hôte ne croie pas la fonctionnalité fournie.
