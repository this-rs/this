# Contrat versionné d'événements, reprise par curseur et attentes humaines

Ce document qualifie le flux d'événements du harnais (`crates/this-harness/src/event.rs`)
tel qu'il est servi aujourd'hui : ce que chaque événement porte, ce qu'il ne porte **pas**,
et ce qu'un hôte doit poser lui-même pour tenir un contrat versionné, une reprise par
curseur et des attentes humaines correctes. C'est l'inventaire des preuves du ticket #286.

Méthode : lecture de `event.rs` en entier, des points d'émission dans
`session.rs`, et des tests de `crates/this-harness/tests/`. Chaque affirmation porte
`fichier:ligne` ; ce qui n'est pas vérifié est dit « non qualifié », jamais supposé.

## 1. Le constat vérifié

- `Event` est un enum serde **taggé** `type`, `rename_all = "snake_case"`, et marqué
  `#[non_exhaustive]` (`crates/this-harness/src/event.rs:36,39`).
- `#[non_exhaustive]` protège le `match` Rust (il force un bras `_` chez le consommateur) ;
  il ne change **rien** à la désérialisation. `Event` a une variante de repli
  `#[serde(other)] Unknown` (`event.rs`, ajoutée par #301) : un `type` inconnu se lit donc
  **sans erreur**, comme `HintKind::Other` et `Verdict::Unknown`. Ce repli est **lossy** :
  c'est une variante unitaire, le nom du `type` et le payload sont perdus. Une lecture fidèle
  passe par `UnknownEvent` / `EventJson` (§3), pas par `Event`.
  Constat corrigé le 23/11/2026 : la version précédente affirmait l'absence de repli, ce qui
  était faux ; un test (`event.rs`, tests du module) le verrouille désormais.
- Les événements bruts ne portent **ni `session_id`, ni `run_id`, ni `seq`** : un
  `grep` sur `crates/this-harness/src/` ne trouve aucun de ces champs dans les variantes
  d'`Event`. L'identité et l'ordre existent ailleurs (voir §3) : `Handle::generation()`
  (`session.rs:2459`), le `run_id` interne `run-N` (`session.rs:1242-1244`), l'ordinal
  de tour `Turn.index` (`session.rs:1308`) et le numéro de file `Pending.id`
  (`session.rs:2339-2361`).
- `Snapshot { version, transcript, pending }` (`event.rs:242-247`) existe : il sauvegarde
  le **fil** et la **file d'attente**, pas la position dans le flux d'événements, pas le
  run en cours. Ce qu'il permet et ne permet pas est précisé en §3.

## 2. Tableau des événements (un par ligne)

Colonnes : nom — variante serde (`type`) — porte l'identité du run ? — porte un ordre ? —
terminal ? — ce que fait un lecteur qui ne le connaît pas.

| Événement | `type` | Identité de run ? | Ordre ? | Terminal ? | Lecteur qui ne le connaît pas |
|---|---|---|---|---|---|
| `User { text }` | `user` | non | non (ordre d'arrivée) | non | l'ignorer ferait perdre la question : à ranger, ou à relire via `transcript()` |
| `Pending { messages }` | `pending` | non | oui, **la liste est l'ordre** (toujours entière, jamais un delta) | non | additif : peut l'ignorer, la file se relit par `Handle::pending()` |
| `Hint { text, kind }` | `hint` | non | non | non | additif ; `kind` a un repli `other`, le texte se relit |
| `Turn { index }` | `turn` | non | partiel : ordinal de tour **dans le run** (réinitialisé à chaque `send`) | non | l'ignorer désordonne l'affichage ; `index` seul ne suffit pas hors du run |
| `Delta { text }` | `delta` | non | non (flux, ordre d'arrivée) | non | l'ignorer perd du texte en cours de frappe |
| `ToolUse { id, tool, input }` | `tool_use` | non (`id` corrèle l'appel, pas le run) | non | non | l'ignorer rend la bulle d'outil incompréhensible |
| `ToolResult { id, tool, text, refused, failed, failure, unchanged, before, after, ms }` | `tool_result` | non (`id` corrèle `ToolUse`) | non | non | l'ignorer laisse un appel sans réponse visible |
| `Spoke { text }` | `spoke` | non | non | non | additif : c'est le texte définitif qui remplace les deltas |
| `Trimmed { messages, tokens, over_budget, excess }` | `trimmed` | non | non | non | additif |
| `Compacted { messages, tokens, summary_tokens }` | `compacted` | non | non | non | **additif, ignorable** (doc `event.rs:125-130`) |
| `Routed { provider, model, changed, why, reason, mode, advisory }` | `routed` | non | non | non | **additif, ignorable** (doc `event.rs:143-149`) ; `advisory` dit si la paire est réellement servie |
| `Stopped { stop, text, turns, ms, code, verdict }` | `stopped` | non (`turns`/`ms` sont des agrégats du run, pas un id) | non | **oui** | ne pas l'ignorer : c'est l'arrêt ; `code`/`verdict` ont leur propre repli |

Deux corrélations existent déjà et suffisent au fil : `ToolUse.id ↔ ToolResult.id`
(appel/résultat, test `un_outil_puis_une_conclusion` qui vérifie `id == "a1"`,
`tests/scenario.rs:135`), et `Pending.id` (file, retrait/édition/priorité,
`session.rs:2377-2428`). Elles n'identifient pas le run.

## 3. La décision d'enveloppe

Il **n'existe pas d'enveloppe d'hôte aujourd'hui** dans le crate : ni `seq`, ni version
d'événement, ni curseur. Les briques à réutiliser existent en revanche, et la décision
est de **ne pas inventer une seconde numérotation** :

| Besoin | Ce qui existe déjà (à réutiliser) | Preuve |
|---|---|---|
| Identité de run | `Handle::generation()` (monotone par `claim`, i.e. par run) ; `run_id` `run-N` est livré au `Recorder` par objectif | `session.rs:1159,1242-1244,2459` ; `ports.rs:427-431` |
| Ordre | l'ordre d'arrivée est garanti FIFO par `Out::emit` (file + drapeau de réentrance) ; `Turn.index` ordonne les tours **dans un run** | `session.rs:643-712` (doc `Out`) ; `session.rs:1308` |
| Curseur | **aucun** : `Snapshot` ne porte que `transcript` + `pending` | `event.rs:242-247` |
| Déduplication | `Pending.id` monotone et jamais réutilisé après `restore` ; `ToolUse.id` appairé une fois | `tests/robustesse.rs:191` (`restore_ne_fait_pas_reculer_les_identifiants`) |
| Version de schéma | `Snapshot.version` (vaut 1), propre au snapshot | `event.rs:244,251` |

Décisions, tranchées explicitement :

- **Version.** L'enveloppe porte une version **à elle** (champ distinct de
  `Snapshot.version`, qui versionne l'état sauvegardé). Aujourd'hui le flux n'en émet
  aucune : un hôte qui veut un contrat versionné l'ajoute au bord.
- **Identité.** Réutiliser `generation()` comme identité de run dans l'enveloppe — c'est
  le champ déjà documenté « pour ignorer les événements d'un run qu'elle a déjà remplacé »
  (`session.rs:2457`). Ne pas réutiliser `run_id` (`run-N`) pour l'interface : il est
  **par objectif**, alors qu'un `send` (un `claim`) peut enchaîner plusieurs objectifs ;
  `generation` couvre le run entier. Le `Recorder` reçoit déjà `run_id` par objectif pour
  le journal (`enregistreur.rs:113`), c'est l'autre échelle.
- **Ordre.** L'enveloppe n'a pas besoin d'un `seq` pour ordonner : `Out::emit` est FIFO et
  sans perte (`session.rs:648-660`). Elle en a besoin si elle veut un **curseur** (voir
  ci-dessous). `Turn.index` seul ne suffit pas : il repart à 0 à chaque `send`
  (`session.rs:1187`, déclaré dans `pump`), il faut le préfixer de `generation`.
- **Curseur.** Non fourni par le cœur. `Snapshot` + un curseur `(generation, seq)` tenu
  par l'hôte est la seule reprise possible ; le cœur **refuse** `restore` pendant un run
  (`session.rs:2489-2493`, test `restore_pendant_un_run_ne_corrompt_pas_le_fil`,
  `robustesse.rs:206`) : on ne reprend pas un run à mi-course, on relit l'état sauvé.
- **Déduplication.** Dans un même message assistant, les ids d'appel sont uniques et
  chaque réponse est appairée à un appel en attente. Le même id peut cependant être
  réutilisé dans un message assistant ultérieur : le fil ne garantit pas la déduplication
  entre tours. Au niveau du flux, un `seq` d'enveloppe (posé par l'hôte) est nécessaire pour
  un transport qui livre au moins une fois ; le cœur n'a pas de quoi dédoublonner un
  événement rejoué — et ne prétend pas en avoir.
- **Comportement face à un événement futur.** La convention du code est « additif,
  ignorable » (doc `event.rs:125-130` pour `Compacted`, `event.rs:143-149` pour `Routed`).
  Mais elle ne tient qu'avec un lecteur **indulgent** : `serde_json::from_str::<Event>`
  ne garde pas le `type` inconnu (repli `Event::Unknown`, sans payload). La décision est donc : **conserver en `Unknown`** (garder
  le payload brut et le `type`, avancer le curseur), jamais « ignorer en silence » (on
  perdrait l'information) et jamais « refuser nommément » (un hôte ancien face à un cœur
  plus récent casserait). Concrètement : lire dans `serde_json::Value`, dispatcher sur
  `type`, et ranger l'inconnu. `HintKind` et `Verdict` montrent déjà cette philosophie
  (`event.rs:204`, `un_verdict_inconnu_se_lit_unknown`).
  **Écrit depuis** : `EventWire` / `UnknownEvent` (`crates/this-harness/src/event.rs`),
  l'enveloppe de **lecture** du fil. Un `type` connu y est décodé **strictement** — un
  payload incomplet est une **erreur**, pas un inconnu, parce que ranger un événement cassé
  en inconnu inventerait un succès et effacerait le défaut. Un `type` inconnu est conservé
  avec son `type_name` et sa vue `Value` (`UnknownEvent::raw`) : le relayer rend le même `type`
  et les mêmes champs, jamais `{"type":"Unknown"}` — mais **pas les mêmes octets ni toujours les
  mêmes valeurs** : `Value` accepte et **arrondit** un entier au-delà de `u64`/`i64` (mesuré :
  `18446744073709551617` → `1.8446744073709552e+19`), réécrit les nombres et peut trier les clés
  selon les features `serde_json` unifiées. `EventWire` est donc une vue d'**inspection**, pas un
  relais exact. `UnknownEvent` et `EventWire` sont **inchangés** (champs publics, littéraux
  externes constructibles).
  **Relais exact : `EventJson`** (`event.rs`), enveloppe **distincte** de serde, à internes privés
  (rien n'y est mutable, le texte relayé ne peut pas diverger de ce qui a été validé). `parse`
  lit d'abord le texte **en entier** par `Value` : ce que `Value` refuse (surrogate UTF-16
  isolé, profondeur excessive, zéro initial, caractères après l'objet, `1e400` hors `f64` sauf
  `arbitrary_precision` unifiée) est **refusé nommément**, pour ne jamais relayer un texte qu'un
  autre lecteur serde refuserait. L'événement doit être un objet (une séquence est refusée par
  `deserialize_map`) portant **un seul** `type` **textuel** ; un type connu est décodé
  strictement en `Event` ; un type inconnu garde ses **octets reçus, espaces compris**, et
  `to_json` les rend tels quels — ordre des clés, clé dupliquée hors `type`, `1E2`, `0.10`,
  `18446744073709551617`. Mesuré sous quatre jeux de features (`aucune`, `arbitrary_precision`,
  `preserve_order`+`raw_value`, les trois). `to_wire` donne la vue `EventWire`, **avec perte**,
  pour l'inspection seulement. **L'hôte doit adopter cette API** pour obtenir la garantie : le
  chemin serde ordinaire ne la donne pas.
  **Limites** : la taille n'est pas bornée (l'hôte borne ce qu'il lit) ; un type **connu** auquel
  une version future ajoute un champ perd ce champ au relais (`Event` ne le connaît pas) ;
  `Event::Unknown` (variante unitaire de #301) reste pour la lecture directe en `Event`.
  **Voie écartée** : la feature `serde_json/raw_value` (branche `wip/relais-brut-exact`) — Cargo
  l'unifie dans tout le binaire et elle change la lecture de `Value` partout (mesuré : un
  `ToolCall.arguments` dont la première clé est `$serde_json::private::RawValue` est relu comme
  le JSON qu'elle porte, ou devient une erreur). Chantier **ouvert jusqu'à revue indépendante**.
  Preuves : `chaque_type_connu_se_relit_comme_connu_et_identique` (aller-retour sur une
  instance de chaque variante, et garde de la liste des types connus),
  `un_evenement_futur_garde_son_type_et_son_payload_intact`,
  `un_type_connu_au_payload_incomplet_est_une_erreur_pas_un_inconnu`,
  `une_forme_relayee_par_une_version_precedente_reste_lisible`.

## 4. Scénarios d'acceptation

| # | Scénario | Verdict | Preuve (test exact, fichier:ligne) ou manque |
|---|---|---|---|
| S1 | Reconnexion entre `ToolUse` et `ToolResult` sans bulle dupliquée ni `busy` permanent | **partiel** | Appairage `id` : `un_outil_puis_une_conclusion` ; `busy` retombe après abandon : `une_future_abandonnee_rend_la_place_et_le_dit` ; fil valide après outil abandonné : `une_future_lachee_pendant_un_outil_laisse_un_fil_valide`. La PR #315 refuse avant effet deux appels de même id **dans une même réponse** et conserve le texte déjà affiché. Elle ne prouve pas une reconnexion réelle ni la déduplication entre réponses ou tours : `check_thread` réinitialise ses ids vus pour chaque message assistant. |
| S2 | Rechargement pendant une question/permission : carte reconstruite, réponse corrélée une seule fois | **manque** | Le harnais n'a **pas** de concept de question/permission (`answer_permission` est cité comme *non repris* de PO, `session.rs:6,19`). C'est un contrat d'hôte (Atelier), hors de ce crate. Ce qui existe et s'en rapproche : la file tenue survit au snapshot (`un_etat_sauvegarde_se_repare_et_se_rejoue`, `scenario.rs:489`) et un message livré quitte la file une fois (`un_message_tenu_part_au_tour_suivant`, `scenario.rs:166`). |
| S3 | `Stopped` terminal, un événement tardif ne réouvre pas le run | **preuve existante** | un seul `Stopped` : `un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret` (`robustesse.rs:249`) et `des_bornes_degenerees_se_terminent_en_disant_leur_arret` (`robustesse.rs:274`) ; `busy` rendu **avant** l'annonce (`session.rs:1714-1716`) ; un `delta` après annulation est ignoré (`session.rs:1382-1384`) ; interruption au repos ignorée : `an_interrupt_between_two_runs_is_ignored_and_does_not_poison_the_next_one` (`residus.rs:185`), `interrupt_avant_le_run_n_empoisonne_pas_le_suivant` (`robustesse.rs:365`). |
| S4 | Snapshot + flux concurrent sans trou ni double application | **preuve existante** (cœur) | `restore` pendant un run est refusé et ne corrompt pas : `restore_pendant_un_run_ne_corrompt_pas_le_fil` (`robustesse.rs:206`) ; ids jamais réutilisés : `restore_ne_fait_pas_reculer_les_identifiants` (`robustesse.rs:191`) ; round-trip de l'état : `un_etat_sauvegarde_se_repare_et_se_rejoue` (`scenario.rs:489`). *Manque* : « sans trou ni double application » au niveau du **flux** suppose un curseur d'enveloppe, absent du cœur (voir §3). |
| S5 | Client lent et rétention du journal | **preuve existante** | file bornée à `MAX_PENDING` (1000) et `Pending` cumulé borné : `dix_mille_messages_en_file` (`robustesse.rs:448`) ; sortie d'outil de 10 Mo bornée : `une_sortie_d_outil_de_10_mo` (`robustesse.rs:473`) ; 1000 appels dans un tour restent rangés : `mille_appels_dans_un_tour_restent_ranges` (`robustesse.rs:343`) ; enregistreur lent borné : `a_silent_recorder_delays_the_run_by_a_bounded_time` (`residus.rs:172`). *Note* : la **rétention** du journal est le choix de l'hôte (`Recorder` pousse, ne stocke pas — `ports.rs:406-431`). |
| S6 | Distinction entre acquittement d'outil et réponse humaine | **preuve existante** | `ToolResult` porte `refused`/`failed`/`unchanged`/`before`/`after` (`event.rs:76-100`), un refus ≠ une erreur (`event.rs:81-86`) ; un résultat en échec porte sa cause : `un_tool_result_en_echec_porte_sa_cause` (`scenario.rs:1092`) ; les relances du harnais sont marquées `synthetic` (`wire.rs`), distinctes d'une question humaine : `synthetic_is_additive_on_the_wire` (`wire.rs` module tests) ; la réponse humaine est `User`/`Pending`, jamais un `ToolResult`. |

Bilan des scénarios : **4 « preuve existante »** (S3, S4, S5, S6), **1 « partiel »** (S1 :
appairage et refus d'un id dupliqué dans une même réponse, mais aucune reconnexion réelle ni
déduplication entre messages), **1 « manque »** (S2, hors du crate). Parmi les manques d'enveloppe de §3, la **lecture indulgente d'un `type`
inconnu** est désormais écrite dans le cœur (`EventWire`, §3). La **version d'événement** et
le **curseur** restent à l'hôte, par décision (§3) : le cœur n'ouvre pas une seconde
numérotation à côté de celle de l'hôte.
