# Contexte cognitif — provenance, cloisonnement et mémoire après compaction

Un agent qui « se souvient » mélange au moins six choses : ce qu'une personne lui a
demandé, ce que le graphe sait, ce qu'un outil a rapporté, ce qu'il a lui-même conclu,
ce qu'un résumé a gardé, et ce qu'il croit avoir lu. Une seule de ces six familles est
vérifiable (les ids), et elle est vérifiée **deux fois, par deux règles différentes** —
c'est le résultat le plus net de cet inventaire.

Ce document dit quelles familles de contenu le code manipule réellement, qui les
possède, ce qu'elles portent comme provenance, version, périmètre, budget et fraîcheur.
Puis il tranche les sept cases du ticket #290, et nomme la frontière entre le cœur
(`this-harness`, `this-agent`) et l'hôte.

Mesuré le 20/11/2026 sur `origin/main` `bd387e8` (copie de travail `docs/harness-contrats`).

**Note de cadrage, pas un bug.** `Cargo.toml:218-221` documente que la feature
`obrain-cognitive` est un **alias de compatibilité** : le relief cognitif n'est pas
branché, `core::events::CognitiveSignal` est un vocabulaire propre à this-rs, et le
moteur cognitif d'obrain n'est pas une dépendance. Ce n'est donc pas un branchement
manquant, et rien dans ce document ne le traite comme tel.

## 0. Ce qui a été lu, et jusqu'où

Lus **en entier** : `crates/this-harness/src/{context,recall,embed,event,ports,search}.rs` ;
`crates/this-agent/src/{substrate,playbook,cite,untrusted}.rs` (sauf les tests de
`untrusted.rs` au-delà de 470) ; les tests `compaction.rs`, `contexte.rs`,
`borne_de_contexte.rs`, `budget_contexte.rs`, `rappels.rs`, `embeddings.rs`,
`recherche.rs`, `declarations.rs`, `adverse.rs` (parties 4 et 5) ; `src/storage/agent_substrate.rs`
(en-tête, `save_context`/`context`/`render`/`save_grants`, tests).

Lus **par leur plan** : `crates/this-agent/src/{graph,masque,flux,surface}.rs`,
`crates/this-agent/src/approval.rs` 1–150, `crates/this-harness/src/{session,mcp}.rs`
autour des zones citées.

**Non lus, donc non qualifiés** : `crates/this-agent/src/masque.rs` au-delà de son
en-tête, `crates/this-agent/src/evidence.rs` (le verdict et les déclarations d'effet),
`crates/this-harness/src/routing/*`, et **tout hôte hors de ce dépôt** (l'Atelier,
Project Orchestrator). Chaque fois qu'une propriété n'est tenue que par eux, la cellule
dit « hôte (non qualifié) », jamais une supposition.

---

## 1. Les familles réellement manipulées

Une ligne par famille de contenu que le code **traite**, pas une par concept de
l'écosystème. Les familles qui n'existent pas dans ce code sont listées après, avec la
même précision.

| Famille | Propriétaire (qui la produit / écrit) | Source (provenance nommée) | Version | Périmètre | Budget de contexte | Fraîcheur |
|---|---|---|---|---|---|---|
| **Section de contexte** (`_context`) | `this-agent` définit la forme (`substrate.rs:83-97`, label `:46`) ; l'hôte écrit et relit (`src/storage/agent_substrate.rs:94`, `:126`) | `Section.source` (`substrate.rs:95-96` : `descriptor`, `registry`, `tool:install_module`…) | **Aucune** par section | `agent` : l'identité du nœud est `<agent>#<id>` (`substrate.rs:102-115`), deux agents ne se marchent pas dessus | **Caractères**, pas jetons (`Context::render(budget_chars)`, `substrate.rs:158-212`) ; la coupe **nomme** les sections omises (`:196-205`) | `Section.at_ms` (`:93-94`), départage à priorité égale, le plus récent d'abord (`:168-173`) |
| **Leçon du socle** (sous-famille versionnée) | `this-agent` (`playbook.rs`) | `socle v{VERSION} · {source}` (`playbook.rs:43`) — chaque leçon nomme l'échec mesuré qui l'a produite | **Oui, et épinglée** : `VERSION = 2` (`playbook.rs:31`), test `les_lecons_sont_stables_et_versionnees` (`:214-221`), et chaque leçon doit annoncer la version courante (`:199-211`) | agent `atelier` en dur (`playbook.rs:137`) | Priorité 40 (`:41`), plus basse que l'état présent ; test « le socle tient entier à 4000 » (`:181-189`) et « une leçon perdue se dit » (`:192-196`) | Aucune (contenu figé dans le binaire ; la version joue ce rôle) |
| **Fait de rappel** (`Fact`) | **Hôte** : port `Recall` (`recall.rs:97-100`) ; `this-harness` assemble (`rappel`, `:238`) | `Fact.source` (qui l'affirme, `:89-90`) et `as_of` (quand, `:87-88`) ; l'extrait de conversation est rendu **à côté** du fait (`conversation (message N) : « … »`, `:262-270`) et le harnais n'arbitre pas (`:250-254`) | Aucune | **Aucun filtre de projet dans le port** : `recall(&self, ids: &[String])` (`:99`) ne dit ni projet ni personne | `MAX_CHARS = 8000` (`:43`), `MAX_ENTITIES = 40` (`:39`), `MAX_LISTED = 20` (`:41`), `FACT_MAX = 240` (`:44`) ; chaque coupe est nommée (`:276-291`) | `as_of` par fait ; aucun âge maximal, aucun refus d'un fait trop vieux |
| **Ancre extraite** (`Anchors`/`Citation`) | `this-harness`, **dérivée** du fil à chaque appel (`anchors`, `recall.rs:103-121`) | `Citation.excerpt` (la ligne qui citait l'id, `:47-57`) ; rien n'est déduit (pas de sens, pas de succès) | Aucune | La suite de messages analysée | `EXCERPT_MAX = 120` (`:45`) | Recalculée, donc toujours à jour par construction |
| **Registre des ids vus** (`SeenLedger`) | **Hôte** : `Toolbox::vus()` (`ports.rs:233-245`) — le registre de l'outillage, partagé (doc `:234-242`) | `Provenance::{Direct, Quoted, Never}` (`cite.rs:385-395`, `:521-535`) : un id paru **seulement** dans un conteneur `<untrusted_data>` est `Quoted`, jamais `Direct` | Aucune | Session (un registre par outillage) | **Aucun** : deux `BTreeSet` non bornés (`cite.rs:375-383`) | Append-only ; aucune invalidation, aucun oubli |
| **Autorisation d'outil** (`_grant`) | `this-agent` définit (`substrate.rs:251-300`) ; l'hôte écrit (`src/storage/agent_substrate.rs:157`) | `Grant.source` (`substrate.rs:260`) : « accordé par l'utilisateur » et « par un module » n'ont pas le même poids (doc `:253-255`) | Aucune | `agent` (`_uuid = <agent>#<outil>`, `:293`) | Hors budget de contexte (ce n'est pas du texte envoyé au modèle) | **Révocation par réécriture** : ce qui n'est plus accordé est retiré du graphe (`save_grants`, doc `:155-157`) ; fail-closed à la lecture (`Grants::allows`) |
| **Approbation de composition** (`Approval`) | **Hôte** : la liste `trusted` est un paramètre de la vérification (`approval.rs:138`, `:207`) | `by` (qui), `at_ms` — « informatif (audit) » | `fingerprint` : SHA-256 de la forme canonique, `approval` exclue (`approval.rs:106-117`) ; `digests` épingle le **contenu** de chaque module (`:63-72`) | Une composition (`HarnessDoc`) | Hors budget | **Une approbation n'expire pas toute seule** (`:52-53`) : modifier la composition l'invalide mécaniquement |
| **Espace vectoriel** (`EmbeddingModel`) | **Hôte** : port `Embed` (`embed.rs:65-73`) | `EmbedError` nomme la cause (`Unsupported`/`Dimension`/`Failed`, `:31-40`) | **`nom` + `dimension`** = l'estampille (`:23-27`) ; deux estampilles ≠ deux espaces incomparables (`:20-22`) | Aucun | Aucun (le harnais n'en fait rien, `:1-6`) | Aucune : pas de péremption, pas de réindexation — ce n'est pas dans ce crate |

### Familles qui n'existent pas dans ce code

**Notes, décisions, skills, personas, plans, tâches** : aucun type, aucun champ, aucun
test dans `this-harness` ni `this-agent`. Elles n'arrivent au modèle que de deux
façons, et les deux sont des **hôtes** : soit comme texte non fiable dans un
`<untrusted_data>` (une note lue), soit comme fait étiqueté par le port `Recall`
(`recall.rs:79-91`), qui ne transporte qu'un `id`, un `label`, une `source` et une
date — **pas** le corps de la note, **pas** son statut, **pas** son projet. Un hôte qui
veut distinguer une note « confirmée » d'une note « invalidée » doit donc le mettre dans
le `label` ou refuser de la rendre : le contrat du port ne le prévoit pas. C'est écrit
ici plutôt que supposé ailleurs.

---

## 2. La frontière : ce que le cœur porte, ce que l'hôte porte

### Ce que `this-harness` porte réellement

- **Le fil** (`State.transcript`, `session.rs:630-641`) et **la file** (`State.pending`) ;
  c'est le seul état de connaissance que le harnais possède, et il le sauvegarde sur
  demande (`Snapshot`, `event.rs:242-249`, `session.rs:2477-2515`).
- **La borne de contexte** : `estimate_tokens` (estimation, jamais une mesure,
  `context.rs:14-23`, `:67-74`), `fit` (`:171-189`), `removable` (`:200-210`), et
  l'événement qui dit l'oubli (`session.rs:1957-1966`).
- **La compaction** : le harnais appelle le `Compactor` de l'hôte sur **exactement** ce
  que `fit` jetterait (mêmes frontières de groupe, `context.rs:191-199`), vérifie le
  résumé (rôle, non-vacuité, coût strictement inférieur, survie au refit,
  `session.rs:1986-2027`), l'écrit dans le fil et l'annonce (`:2029-2045`).
- **Le contexte éphémère** : `TurnContext::before_turn` (`session.rs:128-142`), recalculé
  à chaque tour, **inséré dans la requête seulement** (`wire.rs:241-252`) — jamais dans
  le fil, donc jamais sauvegardé (`tests/contexte.rs:110-136`).
- **Le cloisonnement des textes qu'il recopie lui-même** : le motif d'un refus ou la
  cause d'un échec passent dans `untrusted::wrap_with` avec le nonce de l'horloge
  (`session.rs:2186-2233`, `Clock::nonce` `:115-125`), bornés à 300 points de code.
- **Le registre déclaré mais non branché** : `Toolbox::vus` (`ports.rs:233-245`).

### Ce que `this-agent` porte réellement

- **Le modèle du contexte** : `Section`/`Context`/`Rendered` et son budget honnête
  (`substrate.rs:83-212`), `_grant`/`Grants` (fail-closed, `:236-300`).
- **Le cloisonnement** : `untrusted::wrap` / `sanitize` / `PREAMBLE` (`untrusted.rs:320-436`),
  et la règle qui reconnaît un conteneur (`container_slices`, `cite.rs:397-426` — seule
  la balise fermante portant **le même** nonce ferme le conteneur).
- **La preuve de citation** : `SeenLedger` + `classify` (`cite.rs:374-383`, `:577-596`).
- **Le socle** : `playbook::seed()` versionné (`playbook.rs:31-142`).
- **La projection vers le graphe** : `_harness`/`_run`/`_step` (`graph.rs:23-31`),
  `run_nodes`/`step_node` (`:300`, `:385`) — dont `_ok`/`_out_excerpt` **calculés
  depuis l'étape observée**, jamais recopiés d'une affirmation du modèle (`graph.rs:29-31`).
- **Le masquage des secrets** avant écriture (`masque.rs:1-20`, branché par l'hôte :
  `graph.rs:309`, `src/storage/harness_graph.rs:84`).

### Ce que l'hôte porte, et que le cœur ne peut pas porter

- **Les six familles de contenu** (notes, décisions, skills, personas, plans, tâches) et
  leur cycle de vie (revue, invalidation, partage).
- **Le `Compactor`** (`ports.rs:275-279`) : résumer demande un modèle, donc une
  entrée/sortie, et le crate n'en fait aucune (`session.rs:18-19`).
- **Le `Recall`** (`recall.rs:97-100`) et **le `Embed`** (`embed.rs:65-73`) : même raison.
- **La persistance du contexte** : `AgentSubstrate` (`src/storage/agent_substrate.rs:94`),
  qui implémente la révocation par différence (`:105-120`) et le périmètre par agent
  (`:109`).
- **L'injection de `untrusted::PREAMBLE`** : la constante existe (`untrusted.rs:55-59`)
  et **aucun code du dépôt ne l'appelle** (`grep -rn "PREAMBLE" crates/` ne rend que sa
  déclaration et deux tests). La règle « ceci est une donnée, pas une instruction »
  n'atteint donc le modèle que si l'hôte l'écrit dans sa consigne système — « non
  qualifié » côté hôte réel.

---

## 3. Les sept cases d'acceptation

Verdict binaire : « preuve existante » = un test nommé ou une ligne prouve la case
**telle qu'énoncée** ; « manque » = il faut écrire quelque chose. Chaque case commence
par ce qui est **déjà prouvé et cité** : quatre d'entre elles ont une moitié solide, et
la nommer évite de la reconstruire.

### (a) Le tableau de familles — **manque** (produit par ce document)

Rien dans le dépôt ne rassemble ces huit familles avec leur provenance et leur budget :
la connaissance est dispersée dans cinq modules (`substrate`, `playbook`, `recall`,
`cite`, `embed`) et deux magasins (`agent_substrate`, `harness_graph`). Le §1 est le
tableau. Ce qui manque pour qu'il reste vrai : un test qui épingle les invariants
coûteux — par exemple que `Fact` ne porte toujours pas de projet (une famille qui
gagne un champ sans que la colonne « périmètre » bouge est une fuite), ou que
`playbook::VERSION` reste citée dans `source` (`playbook.rs:199-211` le fait déjà pour
le socle : c'est le modèle à imiter).

### (b) Outil et contexte partagent le registre des ids vus ; une citation jamais vue n'est pas présentée comme preuve — **manque**

Deux moitiés, dont une seule est tenue.

**Tenue** : `classify` (`cite.rs:577-596`) ne rend `Status::Ok` que si
`ledger.has_seen(r)` — donc `Provenance::Direct` seulement (`:537-541`). Un id réel
jamais lu est `Unseen` (`adverse.rs:590-613`, `s_5_citation_d_une_note_reelle_jamais_vue_est_unseen`),
un id inventé est `NotFound` (`:607-612`).

**Non tenue** : le partage lui-même. `Toolbox::vus()` est **déclaré** (`ports.rs:233-245`),
**testé au niveau du port** (`le_registre_se_partage_quand_l_outillage_en_tient_un`,
`ports.rs:517-549`, qui vérifie `Rc::ptr_eq`), et **aucun code ne l'appelle** :
`grep -rn "vus()" crates/ src/ examples/` ne rend que `ports.rs`. Le harnais ne lit
jamais ce registre, et le contexte non plus — `Context`/`render` (`substrate.rs`) ne
connaît pas `SeenLedger`. Le partage est donc, dans ce dépôt, une **couture** offerte à
l'hôte ; « non qualifié » pour un hôte réel.

**Et il y a pire, dans le même dépôt** : `anchors()` (`recall.rs:103-121`) extrait les
ids de **tout** texte du fil, conteneur compris, **sans consulter le registre**. Un id
paru seulement dans une donnée hostile est donc « cité par la conversation » et reçoit,
si le graphe le connaît, un fait **étiqueté** (`rappel`, `:259-275`). C'est exactement
ce que `classify` refuse de faire (`cite.rs:585-589`) : deux règles pour la même
question, dans deux modules, et la plus permissive est celle qui écrit dans le prompt.
Ce qu'il faut écrire : `rappel` prend le registre (ou seulement les citations
`Direct`), et un test qui rejoue `s5_un_id_forge_dans_une_note_hostile_compte_comme_vu`
(`adverse.rs:616-627`) **en bout de chaîne**, de la note hostile au message de rappel,
en exigeant soit l'absence du fait, soit une mention « provenance : cité, non lu ».

### (c) Texte non fiable après compaction et reprise — **manque**

**Prouvé, pour les textes que le harnais recopie lui-même** :
`indices_motif_hostile_reste_dans_le_conteneur` (`tests/indices.rs:326-383`) place un
`</untrusted_data id="…">` hostile et un `## SYSTEM` dans le motif d'un refus, et
vérifie qu'il y a exactement **une** fermeture, que le texte fixe du harnais est
**dehors** et que l'interface reçoit le même texte que le modèle. Côté `this-agent`,
les tests `s4_*` (`adverse.rs:475-581`) couvrent la neutralisation (balises, titres,
fences, bidi, casse).

**Non prouvé, pour la compaction** : `compact` ne vérifie **pas** que le résumé
préserve les conteneurs. Il vérifie le rôle, la non-vacuité, le coût et la survie au
refit (`session.rs:1990-2027`) ; le résumé accepté est rangé au rôle utilisateur
synthétique (`:1992-2000`). Un `Compactor` qui rend une paraphrase sans les balises
fait donc entrer du texte non fiable **hors conteneur**, et rien ne le signale. Deux
aggravants mesurables :

1. **Un résumé peut être épinglé comme la question.** `planifier` prend le dernier
   message utilisateur **non synthétique** ; faute de question, il retombe sur le
   dernier message utilisateur, quel qu'il soit (`context.rs:224-231`) — et un résumé
   de compaction **est** un message utilisateur (`session.rs:1995-1999`). Un fil dont
   la vraie question a été compactée garde donc le résumé comme « question » à ne
   jamais retirer.
2. **Le résumé n'est pas dit « avec perte » au modèle.** `Event::Compacted` le dit à
   l'**interface** (`event.rs:118-132`, émis `session.rs:2040-2044`) ; dans le fil, le
   résumé est un message utilisateur ordinaire, sans en-tête. Le modèle lit une
   paraphrase comme un fait de la conversation.

**Pour la reprise (#290)** : un `Snapshot` ne porte toujours pas de `SeenLedger`, mais
le harnais **rebâtit** le registre depuis le fil à chaque `restore` / `seed` (remis à
zéro, puis rejeu des résultats d'outils). Par défaut la reprise est **conservatrice** :
un id imprimé par un résultat d'outil restauré redevient « **cité** », jamais « lu » —
un snapshot n'est pas authentifié par construction. `restore_trusted` / `seed_trusted`
le relisent comme une lecture directe, et ne conviennent qu'à un stockage protégé en
intégrité. Conséquence assumée : après une reprise par défaut, les vraies lectures du
fil précédent portent la marque « cité seulement » dans le rappel tant qu'elles ne sont
pas relues.

### (d) Suppression/révocation et changement de projet invalident ; pas de mémoire croisée entre personnes — **manque**

**Prouvé** : la révocation d'une section de contexte. `save_context`
(`src/storage/agent_substrate.rs:89-122`) écrit le contexte voulu puis **supprime** du
graphe les nœuds `_context` du même agent qui ne sont plus voulus (`:105-120`), en un
seul commit — et le périmètre est filtré sur `_agent` (`:109`), sinon deux agents se
supprimeraient mutuellement. Test sur magasin réel :
`une_section_retiree_disparait_du_graphe` (`:278-302`). Idem pour les autorisations
(`save_grants`, doc `:155-157`). Les identités sont namespacées `<agent>#<id>`
(`substrate.rs:102-115`, test `deux_agents_peuvent_avoir_la_meme_section_sans_se_marcher_dessus`,
`:461`).

**Manque** :

- **Aucune notion de personne.** Le périmètre est `agent` (`substrate.rs:121`), pas
  « qui est devant l'écran ». Rien n'empêche deux personnes du même poste de partager
  un contexte. « Pas de mémoire croisée entre personnes » n'est donc pas tenu par le
  cœur, et rien dans ce dépôt ne prouve qu'un hôte le tienne.
- **Aucune notion de projet** dans les ports de connaissance : `Recall::recall`
  (`recall.rs:99`) prend des ids nus, `Embed` (`embed.rs:72`) des textes nus. Un
  changement de projet ne peut donc pas invalider quoi que ce soit au niveau du
  harnais : c'est à l'hôte de changer de registre, de `Recall` et d'`Embed`. Ce qu'il
  faudrait écrire : un champ de périmètre dans les deux ports (ou un port par projet),
  et un test qui prouve qu'un fait d'un projet n'est pas rendu dans un autre.
- **Rien n'invalide un fait de rappel.** `Fact.as_of` est un texte libre (`recall.rs:87-88`) :
  aucune borne d'âge, aucun refus. Un hôte peut rendre un fait de 2023 comme actuel ;
  le contrat ne l'en empêche pas et ne le lui reproche pas.

### (e) Migration d'embeddings : modèle et dimension préservés, espaces incompatibles refusés — **manque**

**Prouvé** : la **règle**, pure et testée. `EmbeddingModel { nom, dimension }`
(`embed.rs:23-27`) est l'estampille ; `verifier` refuse **le lot entier** au premier
vecteur de mauvaise dimension et nomme les deux (`:104-114`) ;
`vectoriser` applique la règle sans la réécrire (`:123-130`) ; `vecteur_dense` retombe
sur le lexique seul et rend la raison pour le journal (`:137-143`) au lieu d'échouer.
Tests : `une_mauvaise_dimension_est_refusee_nomment` (`tests/embeddings.rs:52`),
`le_refus_de_dimension_dit_les_deux_dimensions` (`:71`),
`verifier_refuse_un_seul_vecteur_fautif_meme_pas_le_premier` (`:158`),
`vectoriser_applique_la_regle_du_lot_entier` (`:185`).

**Manque** : il n'y a **pas de migration**, parce qu'il n'y a **pas de stockage** : le
crate ne persiste aucun vecteur (`embed.rs:1-16`). « Préserve modèle/dimension »,
« réindexe ou refuse », « jamais mélangés » ne peuvent donc pas être prouvés ici. Ce
qu'il faudrait écrire, dans l'hôte qui stocke : une table `(id, modele, dimension,
vecteur)` avec refus à l'insertion d'un vecteur d'estampille différente, une
réindexation explicite par lot, et un test qui insère deux modèles et vérifie qu'aucun
cosinus n'est calculé entre eux. La règle du crate (`verifier`) est ce que cette
migration doit appeler — pas réécrire.

### (f) Budget mesuré avec schémas d'outils et pièces jointes, résumés signalés comme avec perte, aucune double injection — **manque**

Deux sous-cases sur quatre sont prouvées, une l'est à moitié, et la quatrième — les schémas d'outils — est celle qui compte.

- **Pièces jointes : prouvé.** `COUT_IMAGE = 1000` par image (`context.rs:34`), compté
  par `cout_message` (`:46-65`), donc par `estimate_tokens` (`:69-74`) et par `fit`.
- **Aucune double injection : prouvé.** Le contexte de tour est inséré dans une
  **copie** (`wire.rs:241-252`) et jamais dans le fil ; `tests/contexte.rs:110-136`
  vérifie qu'il est dans la requête (position 3 sur 5) et **absent** du transcript,
  `contexte_recalcule_a_chaque_tour` (`:156`) qu'il est recalculé. Le résumé de
  compaction, lui, **remplace** les messages retirés au lieu de s'y ajouter
  (`session.rs:2011-2019`) et le fil écrit est celui qui a servi (`:2029-2039`).
- **Résumé signalé : prouvé côté interface, manque côté modèle.** `Event::Compacted`
  porte `messages`, `tokens`, `summary_tokens` (`event.rs:118-132`, test
  `avec_un_compacteur_le_fil_envoye_est_resume_plus_queue`, `tests/compaction.rs:189`) ;
  mais le message qui remplace les anciens n'a aucune marque de perte
  (`session.rs:1992-2000`).
- **Schémas d'outils : manque.** Le budget porte sur le **fil**, pas sur la requête :
  `fit(messages, budget)` (`context.rs:171`, `session.rs:1956`) ne voit ni la consigne
  système (`Request::new`, `wire.rs:222`, ajoutée en tête par `thread()`, `:256-263`),
  ni le contexte éphémère (`wire.rs:241-252`), ni les schémas d'outils
  (`toolbox.specs()`, `wire.rs:269-271`). Les seuls endroits où les schémas sont
  comptés sont l'estimation **du routeur** (`session.rs:1861-1867`) et le besoin
  d'images — jamais la borne. Le test `la_borne_de_contexte_mord_et_le_dit`
  (`tests/borne_de_contexte.rs:57-123`) mesure bien la requête entière
  (`estimate_tokens(request.thread())`, `:98`), mais avec une consigne de huit
  caractères : il passe parce que la consigne est négligeable, pas parce que la borne
  l'a comptée. Allonger la consigne de quelques centaines de caractères ferait échouer
  cette assertion — c'est le test qui manque, et il est à une ligne. C'est le sujet de
  #292, traité au §4.

### (g) Propriétés cognitives persistées vs dérivées, inventoriées séparément — **manque**

**Persistées** (écrites par un magasin de ce dépôt) : `_context` et `_grant`
(`src/storage/agent_substrate.rs:94`, `:157`), `_run`/`_step` (`src/storage/harness_graph.rs:358`,
`:397`), `Approval` dans `HarnessDoc` (`approval.rs:63-72`), et le fil + la file dans
`Snapshot` — **par l'hôte seulement** (aucun stockage de `Snapshot` dans ce dépôt,
voir `docs/harnais-reprise-durable.md` §1).

**Dérivées à chaque tour, jamais écrites** : `estimate_tokens` (`context.rs:69`),
`anchors` (`recall.rs:103`), `Verdict` (`verdict_of`, `session.rs:2289-2296`),
`_ok`/`_out_excerpt`/`_out_hash` (`graph.rs:29-31`, `session.rs:2255-2270`),
`SeenLedger` (état de session, `cite.rs:374`).

**Le cas ambigu, et c'est celui qui compte** : le **résumé de compaction** est une
propriété **dérivée** qui devient **persistée** — écrite dans le fil
(`session.rs:2029-2036`), donc dans le `Snapshot`, et relue plus tard comme un message
ordinaire. Rien ne la distingue d'un vrai tour, sinon son rôle synthétique
(`wire.rs:113-122`) — le même marqueur que les relances du harnais (`Message::hint`,
`wire.rs:72-80`). Un seul drapeau pour deux sens : « le harnais a écrit ceci comme
consigne » et « un modèle a résumé cela ». Ce qu'il faudrait écrire : un inventaire
explicite (ce tableau) **et** un champ qui distingue les deux sens — par exemple
`Message::User { synthetic: Synthetic::Hint | Synthetic::Summary }`, avec la règle
d'épinglage écrite pour chacun. Tant que les deux partagent un booléen, « signalé comme
avec perte » ne peut pas être tenu sans casser les relances.

---

## 4. Articulation avec les deux correctifs en cours

Les deux défauts sont visibles dans le code d'aujourd'hui ; ils sont corrigés en
parallèle sur la branche `fix/harness-budget-interrupt`, et ce document ne les
corrige pas — il dit seulement ce que chacun change pour la **provenance**.

**#292 — le budget ignore la consigne système et le contexte éphémère.** La preuve est
en trois lignes : `fit` ne reçoit que `messages` (`session.rs:1956`, signature
`context.rs:171`), la consigne est ajoutée **après** par `Request::new` puis `thread()`
(`wire.rs:222`, `:256-263`), et le contexte éphémère **encore après** par
`with_context` (`wire.rs:241-252`). Conséquence pour la provenance : le seul texte du
tour qui dit « ceci est une donnée non fiable » (le `PREAMBLE` de l'hôte et les
conteneurs des résultats récents) est précisément celui que la borne ne compte pas — un
fil « sous budget » peut donc dépasser la fenêtre du moteur en portant ses consignes,
et l'échec se lit comme une panne du modèle. Compter la requête entière rend le chiffre
de `Trimmed` commensurable au texte réellement envoyé, ce qui est la condition pour que
« ce qui est resté » veuille dire quelque chose.

**#293 — un dépassement sans retrait n'émet aucun `Trimmed`.** `Trimmed` n'est émis que
si `fitted.dropped` n'est pas vide (`session.rs:1957`), et `Dropped::is_empty()` teste
`messages == 0` (`context.rs:93-97`). Or `planifier` rend exactement ce cas : toutes
les unités épinglées (les `System` `:146`, la question `:224-238`) peuvent dépasser le
budget sans qu'une seule soit retirée — `over_budget` et `excess` sont alors calculés
(`:183-188`) et **jamais annoncés**. Conséquence pour la provenance : un tour qui part
avec 40 000 jetons pour une fenêtre de 32 000 ne dit rien, et la compaction est
pourtant le moment où le fil perd sa mémoire. Émettre `Trimmed` dès que `over_budget`
est vrai (même à `messages: 0`) rend le dépassement visible sans inventer une coupe.

---

## 5. Ce que l'inventaire a révélé d'inattendu

1. **Deux règles de provenance pour la même question, et la permissive écrit dans le
   prompt.** `classify` (`cite.rs:577-596`) refuse un id seulement *cité* ; `anchors`
   (`recall.rs:103-121`) l'accepte et `rappel` lui donne un fait **étiqueté**
   (`:259-275`). Un id paru dans un conteneur hostile peut donc devenir une ligne
   « graphe : … [source : obrain] » — alors que le test `s5_un_id_forge_dans_une_note_hostile_compte_comme_vu`
   (`adverse.rs:616-627`) documente déjà le risque côté `classify`.
2. **`Toolbox::vus()` est une couture sans consommateur.** Déclarée avec soin
   (`ports.rs:233-245`), testée (`ports.rs:517-549`), et appelée **nulle part** dans le
   dépôt. La case (b) du ticket est donc à moitié un branchement, pas seulement une
   règle. `PREAMBLE` (`untrusted.rs:55`) est dans le même état : la constante existe,
   aucun code ne l'injecte.
3. **Le seul message vraiment « avec perte » du système est indistinguable d'une
   consigne.** Résumé de compaction (`session.rs:1995`) et relance du harnais
   (`wire.rs:74`) partagent `synthetic: true` ; pire, le résumé peut être **épinglé
   comme la question** (`context.rs:228-231`). Le drapeau porte deux sens et
   l'épinglage traite les deux pareil.
4. **Le seul budget du système porte sur le fil, pas sur la requête** : consigne
   système, contexte éphémère et schémas d'outils sont hors du compte
   (`context.rs:171`, `wire.rs:222`, `:241-252`, `:269`). Le test qui mesure la
   requête (`tests/borne_de_contexte.rs:98`) passe avec une consigne de huit
   caractères : la borne n'a jamais eu à la compter, donc rien ne prouve qu'elle le
   ferait.
5. **La règle « espaces vectoriels incompatibles » est écrite et testée ; la migration
   qu'elle protège n'existe pas** (`embed.rs:104-114` contre l'absence de tout stockage
   dans le crate) — ce qui est le bon ordre, mais il faut le dire pour qu'un hôte ne
   croie pas la réindexation fournie.
