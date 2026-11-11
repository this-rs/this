# Le mode Auto du harnais — inventaire, écart avec PO, lots

Dans Project Orchestrator (PO), une conversation peut tourner en « Auto » : au lieu
d'un modèle fixé, un **routeur cognitif** choisit à chaque tour le modèle (et le
fournisseur) parmi des candidats, selon la nature du tour. `this-harness` doit arriver
à parité. Ce document dit ce que fait PO, ce que `this-harness` avait, l'écart ligne
par ligne, et le découpage en lots. Le premier lot est livré avec ce document.

Mesuré le 11/11/2026 sur PO `087e416e` (worktree `obrain-default`) et this-rs
`a8dcaec` (`origin/main`). Les chemins `PO:` sont relatifs au dépôt backend de PO ;
les autres au dépôt this-rs.

**Ce qui a été lu, et jusqu'où.** Lus en entier (code hors tests, plus les tests
cités) : `candidates.rs`, `scorer.rs`, `signature.rs`, `mode.rs`, `auto_models.rs`,
`decider.rs` 1–530 et ses tests cités, `decision.rs` 1–60 et 280–400,
`agent_hooks.rs` 596–1660, `manager.rs` 3455–3505 et 16033–16330,
`neurons/intent.rs` 36–130. **Lus par leur plan seulement** (noms de fonctions et
commentaires d'en-tête, pas le corps) : `demotion.rs`, `feedback.rs` (sauf
`confidence_from_scores`), `replay.rs`, `report.rs`, `segment.rs`, `topic.rs`,
`conversation_graph.rs`, `store.rs` (sauf `ArmStats`), `quota.rs`, les sondes de
capacité (`decider.rs` 495–890), `routing_handlers.rs`. Ce qui en est dit plus bas est
marqué « (plan) ».

---

## 1. Ce que fait le mode Auto de PO

### 1.1 L'entrée du routeur : la signature d'un tour

`PO:src/chat/provider/cognitive/signature.rs`

| Signal | D'où il vient | Ligne |
|---|---|---|
| `role` (pilote / exécuteur) | qui ouvre la session | `signature.rs:168`, `resolver.rs:33` |
| `class` (`chat.<intention>`, `simple`, `complex`, `creative`, `retry`, `utility.*`) | mots-clés bilingues FR/EN, priorité Debug > Impact > Plan > Explore > General | `signature.rs:73`, `neurons/intent.rs:40-125` |
| `complexity` 1..=10 | base de la classe + au plus 2 points pour la longueur du message | `signature.rs:127`, `:208` |
| `context_need_tokens` | 8 000 + message/4 + schémas MCP (+ fichiers et étapes pour une tâche), borné à 200 000 | `signature.rs:402-419` |
| `needs_tools` | toujours vrai pour une conversation ; faux pour un utilitaire | `signature.rs:215`, `:386` |
| `needs_images` | le tour porte une image | `signature.rs:216`, `agent_hooks.rs:863-875` |
| `budget_remaining_usd` | budget marginal du run, s'il y en a un | `signature.rs:181` |
| `attempt`, `project_slug`, `persona` | tâche du runner | `signature.rs:183-187` |

Aucun appel réseau, aucun modèle : la signature se calcule hors ligne. Sa clé de bras
(`arm_key`, `signature.rs:192`) est la famille d'apprentissage — elle ne se renomme pas.

### 1.2 Les candidats et chaque filtre

`PO:src/chat/provider/cognitive/candidates.rs`. Un candidat est une paire
(instance, modèle) avec ses faits (`ModelFacts`, `:28`). Le filtre (`check`, `:321`)
rend la **première** contrainte manquée, dans cet ordre :

| # | Filtre | Motif | Ligne |
|---|---|---|---|
| 1 | Machine distante (`claude-code@…`) : choisie par une personne, jamais par le routeur | `remote` | `:322` |
| 2 | Le projet n'a pas consenti à l'instance | `not_allowed` | `:325` |
| 3 | Quota : compte épuisé, ou un seul modèle retenu (`window: model:<nom>`) — avant la santé, car un compte épuisé est sain | `quota_exhausted`, `quota_exhausted_model` | `:329-346`, `apply_quota` `:275` |
| 4 | Instance en panne (`Some(false)` ; `None` = non mesurée, passe) | `unhealthy` | `:347` |
| 5 | Le tour a besoin d'outils, le modèle n'en appelle pas | `no_tools` | `:350` |
| 6 | Fenêtre plus petite que le besoin, ou inconnue (jamais supposée) | `context_too_small`, `window_unknown` | `:353-367` |
| 7 | Le tour porte des images, le modèle ne les lit pas | `no_images` | `:368` |
| 8 | Un tour médian coûte plus que le budget restant (prix inconnu ou abonnement : passe) | `over_budget` | `:371-375` |

Avant ce filtre, l'ensemble des candidats est rétréci par :

- **le pool de la conversation** (modèles cochés) : `agent_hooks.rs:1209-1215`, `:1384-1386` ;
- **la liste de la personne** (« modèles qu'Auto peut utiliser », un document par
  personne, `LlmSetting {scope: person:<sub>, key: auto_models}`) : seulement pour une
  conversation Auto sans pool à elle ; si rien de la liste n'est utilisable, la
  restriction tombe pour cette décision et le motif le dit
  (`auto_models.rs:99-113`, `agent_hooks.rs:1532-1577`) ;
- **le moteur disponible** : la CLI absente retire l'instance intégrée du défaut
  (`manager.rs:16129-16133`) ; le coffre verrouillé refuse Auto (`AutoVaultLocked`,
  `manager.rs:16183-16190`).

Un choix **explicite** (nommé par la demande, la tâche, la persona) n'entre jamais dans
le filtre (`candidates.rs:385-388`).

Le raisonnement (« thinking ») n'est **pas** un filtre de PO : `ModelFacts` ne le porte pas.

### 1.3 La décision : note, départage, hystérésis

`PO:src/chat/provider/cognitive/scorer.rs` et `decider.rs`.

- **Note** (`scorer.rs:284`) : un tirage de Thompson Beta(α, β) par paire éligible —
  le bras stocké, sinon l'a priori Beta(1, 1) plus un succès quand un alias déclaré
  convient à la classe (`deep` / `fast`, `scorer.rs:72`) — moins `cost_weight ×` coût
  normalisé, `latency_weight ×` latence normalisée, `quota_weight ×` pression de quota
  (0 jusqu'à 90 % de la fenêtre, puis linéaire, `scorer.rs:120`).
- **Déterminisme** : la graine est un hachage de l'identifiant de la décision
  (`seed_of`, `scorer.rs:132`) ; chaque bras tire sur `graine XOR FNV-1a(classe,
  instance, modèle)` (`:345`) — donc l'ordre d'entrée ne compte pas.
- **Départage** : utilité décroissante, puis instance, puis modèle (`scorer.rs:370-375`).
- **Exploration** : avec probabilité ε (bornée à 0,25), une paire tirée uniformément
  passe en tête, marquée `explored` ; jamais avec un seul candidat (`scorer.rs:379-389`).
- **Hystérésis** (`decider.rs:360-377`) : la paire en vigueur reste tant qu'aucun
  challenger ne la dépasse de 0,1 ; au-delà de 10 observations de chaque côté la marge
  s'élargit quand le champ est serré (`0,1 × (2 − confiance)`, `:204-238`).
- **Tour stable** (`steady`, `decision.rs:335-340`) : juste après un changement, un tour
  de la même classe ne tire pas d'exploration et compare les **moyennes** des
  postérieurs, pas les tirages — deux tours de même classe ne font jamais A → B → A.
- **Motif** (`reason_of`, `decider.rs:92`) : une phrase, sans URL ni secret.
- **Appliquée ou notée** (`is_applied`, `decider.rs:53`) : table mode × étape × rôle.

### 1.4 Les trois modes

Deux vocabulaires (`mode.rs:40-119`) :

| Menu de la conversation | Code | Ce que fait PO | Réglages (autre enum) |
|---|---|---|---|
| un modèle coché — « strict » | `primary` | rien n'est décidé : le modèle est imposé | `primary` : note seulement |
| deux modèles ou plus — « mixed » | `mixed` | route parmi les cochés, **comme `full` restreint au pool** ; change de fournisseur si le pool en couvre deux | `executors_only` (ancien nom `mixed`) : seuls les exécuteurs |
| Auto | `full` | route tout, le fournisseur compris | `full` |

Un choix fait sur la conversation vaut l'étape `auto` pour elle, quoi que disent les
réglages (`conversation_stage`, `mode.rs:406`). La lecture unique des droits est
`routing_of` (`agent_hooks.rs:1509`).

### 1.5 Le changement de mode en cours de conversation

`TurnRouter::reroute` (`agent_hooks.rs:899-918`), appelé par
`PUT /api/chat/sessions/{id}/routing` : remplace mode, pool et épingle ; relâche un
fournisseur imposé à l'ouverture ; oublie un réglage à la main. Vaut dès le tour suivant.
Un changement de modèle à la main (`mark_manual`, `:921`) fait taire le routeur.

### 1.6 Un candidat, zéro candidat

- **Un seul éligible** : la note tourne sur lui seul, il est choisi, sans exploration
  (`scorer.rs:385`). Le cas mono est de premier ordre, pas un multi dégradé
  (`mode.rs:388-393`).
- **Zéro éligible** : la décision vaut `chosen: None`, motif `no_candidate`, rejets
  listés (`decider.rs:304-308`). Le tour garde le modèle en vigueur
  (`TurnDirective::none()`, `agent_hooks.rs:1407-1418`) ; à l'ouverture, ce sont le
  pilote du pool, la paire du parent ou les règles déclarées qui ouvrent
  (`manager.rs:16207-16266`). Jamais une erreur « aucun candidat ».
- **`auto` n'est pas un modèle** : `is_auto_model` (`manager.rs:3473`) ; transmis tel
  quel, la CLI le rejette (`[claude-code:unrecognized_model]`).

### 1.7 Le routage par tour, et le fournisseur

Une session PO est liée à **un** fournisseur. Donc deux chemins :

- `directive_for_turn` (`agent_hooks.rs:1346`) : le modèle du tour, parmi ceux du
  fournisseur de la session ; appelé une fois par message (`before_turn`,
  `nexus:providers/native/loop.rs:171`).
- `plan_provider_move` (`agent_hooks.rs:1194`) : avant le tour, faut-il **déplacer** la
  conversation (relais de l'historique vers une nouvelle session) ? Gardes : pas deux
  déplacements de suite, un tirage d'exploration ne déplace jamais, l'écart doit passer
  la marge — sauf si le tour porte une image que la paire en vigueur ne lit pas.
- `model_for_images` (`agent_hooks.rs:1589`) : le modèle d'un tour à image est choisi
  **avant** l'envoi, parce que le moteur vérifie la vision du modèle actif à l'envoi.
- `model_after_limit` (`agent_hooks.rs:1667`, début lu) : après un refus de quota sur
  un modèle, bascule sur un autre modèle de la même instance.

### 1.8 L'apprentissage et ce qui est persisté

- Bras `ArmStats` (classe × instance × modèle : α, β, n, coût, latence) et chaque
  `CognitiveDecision`, appliquée ou non, dans Neo4j (`store.rs:48`, `neo4j/routing.rs`
  (plan), `decider.rs:436-441`).
- Récompense d'un tour : succès conclu = 1, erreur du fournisseur = 0, fin ambiguë
  (interrompu, max_turns, refus du compte) = rien (`decider.rs:464-492`,
  `decision.rs:241-280` (plan)).
- Rétrogradation automatique de l'étape quand un bras appris fait pire que la
  référence (`demotion.rs` (plan)) ; journal rejouable et propensions hors-politique
  (`replay.rs`, `p_top1` — `scorer.rs:222` — (plan)) ; segmentation de la conversation
  (`segment.rs`, `topic.rs` (plan)).

### 1.9 Les événements vers l'interface

| Quoi | Où |
|---|---|
| `ChatEvent::ModelChanged { model, reason }` quand le modèle change | `chat/types.rs:987` |
| Sur la session : `routed_by: auto`, `route_rule: auto:<classe>`, `reason`, `routing_mode`, `routing_pool` | `manager.rs:16316-16324`, `chat/types.rs:1815-1821` |
| Le choix « fantôme » quand la décision n'est pas appliquée (`shadow`) | `manager.rs:16325-16330` |
| Les décisions et leurs alternatives rejetées, par API | `api/routing_handlers.rs` (plan) |
| La capacité image effective (« via qui ») | `agent_hooks.rs:1737` (plan) |

---

## 2. Ce que `this-harness` avait

- **Aucune notion de modèle.** `grep -i model crates/this-harness/src/{session,ports,wire,event}.rs`
  ne rendait rien : la `Request` portait la consigne, le fil et les outils
  (`crates/this-harness/src/wire.rs:212`), et c'est le `Transport` de l'hôte qui
  choisissait son modèle, seul.
- **Abstraction de fournisseur** : `Transport` (`src/ports.rs:60`), `?Send`, un flux par
  tour. Pas de capacités déclarées côté harnais. Côté `this-llm` : `Provider`
  (base, modèle, référence de secret, `crates/this-llm/src/lib.rs:185`) ; côté
  `this-llm-providers` : `ModelInfo { id, context_window, max_output_tokens }`
  (`src/model.rs:11`) — la fenêtre seulement, ni outils ni images.
- **Où une décision s'insère** : dans `Harness::pump`, à l'ouverture d'un objectif
  (un message tenu), avant le premier tour (`src/session.rs`, boucle `'goals`). Le
  harnais tient le transcript : contrairement à PO, **rien ne lie une conversation à un
  fournisseur**, donc changer de paire n'est pas un relais de session.
- **Garde-fous wasm** (CI `wasm32_harness`, `.github/workflows/ci.yml:624-647`) : la
  liste noire `scripts/lint-harness-portability.rs` (pas de `std::fs`, `std::net`,
  `std::thread`, `std::process`, `tokio::{fs,net,process}`, `reqwest`, `rusqlite`,
  `std::time::SystemTime` hors `cfg(not(wasm32))`) et
  `cargo check -p this-harness --target wasm32-unknown-unknown --no-default-features`.

---

## 3. Tableau d'écart PO → this-rs

Légende : **là** = existait ; **porté** = porté tel quel dans le lot 1 ; **repensé** =
porté autrement, pour wasm ou pour le harnais ; **hôte** = reste côté hôte, derrière un
port ; **plus tard** = lot suivant.

| # | PO | this-rs | État |
|---|---|---|---|
| 1 | Signature de conversation (`from_chat_request`) | `routing/signature.rs:244` | **porté** ; le projet et la persona sortent (hôte) ; le fil tenu compte dans le besoin (divergence D2) |
| 2 | Détecteur d'intention bilingue | `routing/signature.rs:113` | **porté** (mêmes mots-clés, même priorité) |
| 3 | Signatures de tâche, délégation, sous-agent | — | **plus tard** (lot 5) |
| 4 | `ModelFacts` | `routing/candidate.rs:123` (`Candidate`, `Capabilities`) | **porté** ; `allowed_for_project` → `allowed`, préfixe distant → `remote` (D3) ; `reasoning` ajouté, informatif |
| 5 | Filtres 1–8 et leur ordre | `routing/candidate.rs:290` | **porté** ; plus le filtre 0 `sentinel` (D1) |
| 6 | `QuotaRegistry` (horloge, fenêtres, retenues) | port `QuotaView` `routing/candidate.rs:380` ; `MemoryQuota` `:407` (impl `:455`) | **hôte** : c'est lui qui a l'horloge |
| 7 | `HealthCache`, sondes de capacité | — | **hôte** (E/S, horloge) : l'hôte remplit `healthy` et `capabilities` |
| 8 | Catalogue vivant (`claude_code_facts`) | — | **hôte** : il remplit `context_window` / `window_unknown` |
| 9 | Scoreur de Thompson | `routing/scorer.rs:317` | **porté** ligne à ligne ; `p_top1` reste au lot d'apprentissage (D5) |
| 10 | `decide_with` (hystérésis, tour stable, motif) | `routing/decide.rs:311` | **porté** ; l'identifiant est un `u128` injecté, sans UUID ni date (D4) |
| 11 | `is_applied` | `routing/decide.rs:35` | **porté** |
| 12 | Modes des réglages et de la conversation, étape | `routing/mode.rs` | **porté** ; `parse` lit aussi les mots de l'écran |
| 13 | Validation et stockage des réglages (`/api/chat/routing`, projet > global) | — | **hôte** |
| 14 | Liste de la personne (`auto_models`) : la règle | `routing/router.rs:491` | **porté** |
| 15 | Liste de la personne : le document, l'API | — | **hôte** |
| 16 | `TurnRouter` : droits, pool, gardes, repli | `routing/router.rs:416`, `route` `:666` | **repensé** : une seule décision par message, sans relais de session (D6) |
| 17 | `reroute`, `mark_manual` | `routing/router.rs:550`, `:604` | **porté** |
| 18 | `model_for_images` (décider avant l'envoi) | dans `route` : `needs_images` + garde « aveugle » | **repensé** : la décision précède déjà la requête, il n'y a pas de second chemin |
| 19 | `plan_provider_move` + relais | gardes dans `route` `verdict` `routing/router.rs:882` | **repensé** (D6) |
| 20 | `model_after_limit` | — | **plus tard** (lot 3) |
| 21 | Ouverture (`resolve_provider_choice_for` : rôles, alias, politique, repli du parent) | — | **hôte** : le harnais n'ouvre pas de session ; il reçoit la paire de départ |
| 22 | Sentinelle `auto` | `routing/candidate.rs:30`, `Route::of` `routing/router.rs:56` | **repensé** : garanti par construction (D1) |
| 23 | Magasin de bras et de décisions | port `Learning` `routing/router.rs:90` | **hôte** ; le cœur lit les bras |
| 24 | Récompense, rétrogradation, rejeu, rapport | — | **plus tard** (lot 2) |
| 25 | Segmentation de la conversation, sujets | — | **plus tard** (lot 4) |
| 26 | Décision de compaction | — | **plus tard** (lot 5) |
| 27 | Délai de 2 s sur le décideur (`DECIDE_TIMEOUT`, `agent_hooks.rs:598`) | `ROUTER_TIMEOUT_MS` `src/session.rs`, `cut` + `race` | **repensé** : la borne passe par `Clock::sleep` de l'hôte, jamais une horloge du crate ; repli `timed_out` ; l'annulation coupe aussi |
| 28 | `ChatEvent::ModelChanged` | `Event::Routed` `src/event.rs:143` | **repensé** : un événement par message, avec le pourquoi |
| 29 | Paire portée jusqu'au moteur | `Request::model` / `provider` `src/wire.rs` | **nouveau** : le harnais n'avait pas de modèle |
| 30 | Savoir si le moteur sert la paire | `Transport::honors_route` (faux par défaut), `Event::Routed.advisory`, `Learning::settle(served: Option)` | **nouveau** : une décision que rien n'honore est dite consultative |
| 31 | Les transports réels lisent la paire (`this-llm-providers`, page web) | — | **plus tard** (lot 3) : il leur suffira de déclarer `honors_route` |

---

## 4. Divergences assumées

- **D1 — `auto` est écarté par construction, à deux endroits.** PO traite la
  sentinelle à l'ouverture d'une session. Ici (a) un candidat nommé `auto` ou vide est
  rejeté par le filtre (`sentinel`), (b) `Route` a un modèle privé et un seul
  constructeur qui l'écarte. Pourquoi : la paire sort par `Request`, que n'importe quel
  transport lit ; une garde à l'ouverture seule ne couvrirait ni le modèle de session,
  ni un modèle coché, ni un réglage à la main.
- **D2 — le fil tenu compte dans le besoin de contexte.** PO compte 8 000 + le message +
  les schémas MCP : c'est le moteur qui tient l'historique. Le harnais tient le
  transcript, donc il l'estime (borné par `Limits::context_budget`). Sans fil ni outil,
  les deux formules donnent le même nombre.
- **D3 — `remote` et `allowed` sont des faits que l'hôte donne.** PO lit le préfixe
  `claude-code@` et le consentement du projet. Le cœur ne connaît ni convention de nom
  ni projet.
- **D4 — l'identifiant d'une décision est un `u128` injecté** (`Seeds`), pas un UUID tiré
  ici ; pas d'horodatage dans la décision. Pourquoi : ni hasard ni horloge dans le cœur.
  La graine se calcule comme chez PO sur la même valeur 128 bits.
- **D5 — `p_top1` et la propension ne sont pas calculés.** Ce sont 512 tirages par bras
  par décision, qui ne servent qu'à l'estimateur hors-politique : ils viennent avec
  l'apprentissage. La décision elle-même n'en dépend pas.
- **D6 — une décision par message, tous fournisseurs confondus.** PO sépare « changer de
  modèle » (dans la session) et « déplacer la conversation » (relais), parce qu'une
  session est liée à un fournisseur. Le harnais tient le fil : il prend **une** décision
  sur l'ensemble des candidats, et garde les protections de PO contre l'oscillation
  entre fournisseurs. Conséquences, toutes traitées dans le code et prouvées par un test :
  (a) un pool coché sur un seul autre fournisseur route vers lui, là où PO refuse la
  demande (`routing_pool_other_provider`) ; (b) pas d'attente « le modèle se change-t-il à
  chaud » (`set_model_live`) ; (c) la garde « changement de direction » de la
  segmentation n'existe pas encore (lot 4) : après un changement de fournisseur, le tour
  suivant reste toujours chez lui, sauf image illisible ;
  (d) **la garde d'exploration** (« un tirage d'exploration ne change jamais de
  fournisseur ») ne protège que si la paire en vigueur est elle-même éligible. Hors du
  pool coché ou de la liste de la personne, elle n'a rien à protéger : le tour part sur
  le meilleur candidat non exploratoire (la décision est refaite à ε = 0, même graine),
  jamais sur un modèle qu'on n'a pas le droit de choisir ; (e) **l'ouverture sur la
  sentinelle** (`model: auto`) n'a pas de paire à quitter : son premier choix n'est
  ni un changement de fournisseur ni soumis aux gardes qui le protègent (PO : la décision
  d'ouverture est hors de ces gardes, `agent_hooks.rs:777-826`) ; (f) **les images du
  fil** : PO ne regarde que le message du tour (`set_turn_input`, `agent_hooks.rs:863`),
  car le moteur de la session garde son historique et `model_for_images` ne décide que
  pour le tour. Ici le harnais renvoie tout le fil à chaque tour : le besoin d'images est
  celui du fil (transcript, compaction non déduite). Au pire trop prudent — une image
  que la compaction a retirée interdit encore les modèles sans vision —, jamais aveugle.
- **D7 — `needs_tools` vient de l'hôte** (`RouterSpec::needs_tools`, vrai par défaut
  comme PO), pas de `Toolbox::specs()` : un transport de la famille A déclare ses
  outils lui-même, la boîte peut être vide alors que le tour en appelle.
- **D8 — un fournisseur imposé a son propre code de motif** (`provider_imposed`), là où
  PO ne demande simplement aucune décision de fournisseur.
- **D9 — les faits de l'hôte sont validés à l'entrée.** PO les lit dans ses propres
  magasins ; ici ils viennent d'un hôte quelconque.
  - Prix : `NaN`, infini, négatif ou au-delà de `MAX_PRICE_PER_MTOK` (1 000 000 USD par
    million de jetons) écarte la paire (`invalid_price`). Un prix fini mais démesuré
    faisait déborder le coût estimé (`inf / inf = NaN` dans la normalisation).
  - Bras : un postérieur qui n'est pas une loi Beta (`alpha` ou `beta` non fini, nul ou
    négatif, coût cumulé non fini) écarte la paire (`invalid_arm`). **Seul `∞` faisait
    boucler le tirage** (Marsaglia-Tsang sur une forme infinie) ; `NaN`, `-3` et `0`
    passaient le tirage, bornés par `max(1e-3)` comme chez PO, mais empoisonnaient la
    MOYENNE `alpha / (alpha + beta)` — `NaN`, ou `1,5` pour `(-3, 1)` — que lisent
    l'hystérésis d'un tour stable et le motif (« NaN% success »). **Divergence assumée
    avec PO**, qui tolère `alpha ≤ 0` par ce même `max(1e-3)` : son magasin ne produit
    jamais un tel bras (a priori ≥ 1), donc la règle ne change rien pour des données
    saines, et pour des données corrompues elle préfère écarter la paire en le disant
    plutôt que de la classer sur une moyenne absurde.
  - Note : une utilité non finie malgré des faits valides (deux coûts mesurés au
    maximum des `f64` : leur médiane déborde) écarte la paire (`not_scorable`).
  - Réglages : un poids ou un taux non fini reprend son défaut ; les poids sont bornés à
    `0..=1` et le taux à `0..=0,25` (`RoutingSettings::sanitized`). **Changement
    silencieux** pour un hôte qui passerait un poids > 1 : PO refuse ces valeurs à
    l'entrée de son API (`validate_routing_settings`, bornes identiques), elles ne
    peuvent donc pas exister chez lui ; ici le cœur n'a pas d'API à refuser, il borne.
  - Identifiant de décision : **chaîne**, et seulement une chaîne (un `u128` ne survit
    pas à un `Number` JavaScript ; une forme « chaîne ou nombre » exigerait
    `deserialize_any`, illisible par un format non auto-descriptif comme bincode).
- **D10 — « même graine, même décision » vaut à plateforme égale.** `ln`, `cos`,
  `powf` n'ont pas de précision garantie d'une plateforme à l'autre : natif et wasm
  peuvent différer au dernier bit d'un tirage. Un rejeu d'une décision persistée sur
  une autre plateforme ne doit jamais corriger son choix (`chosen` / `used` font foi) ;
  il compare les notes avec une tolérance. Un rejeu bit à bit demande la même
  plateforme, ou des fonctions mathématiques déterministes fournies par l'hôte (hors de
  ce lot).
- **D11 — la décision se prend en deux temps, et le réglage de la personne gagne.**
  PO tient son routeur sous verrou. Ici les ports (candidats, bras) sont asynchrones :
  l'état n'est copié qu'avant l'attente et réécrit d'un bloc après, sous un compteur
  de génération. Un réglage à la main ou un menu qui arrive pendant l'attente gagne
  toujours (`superseded`) : la décision est jetée, la paire de la personne reste.
  L'**enregistrement** (`Learning::record`) vient après : l'état est réécrit et
  `Event::Routed` émis d'abord, puis le harnais appelle `TurnRouter::record`, borné et
  annulable. Un `record` qui pend ne peut donc plus rien changer au tour (relecture de
  #251, C1).
- **D12 — la paire en vigueur se reconnaît sous ses variantes.** Le pool peut nommer
  `m[1m]` quand la session tourne sur `m` (fenêtre étendue). Le routeur ramène la paire
  en vigueur au nom que le pool lui donne (`same_model` : suffixe `[1m]` et casse
  ignorés) avant de décider : sans cela, la garde d'exploration et l'hystérésis ne la
  trouvaient pas parmi les candidats et sautaient. PO compare ces noms de la même façon
  (`same_model`, `agent_hooks.rs:1463`) pour sa garde « aveugle ».

---

## 5. Les lots

Chacun est livrable seul.

1. **Le cœur de décision, pur et portable — livré.** Signature, candidats et
   capacités, filtres, trois modes, décision à départage déterministe, repli un /
   zéro candidat, routeur de conversation, changement de mode en cours de route ;
   branché sur le chemin d'un tour (`Harness::with_router`) ; ports hôte et leurs
   doubles en mémoire. *Côté hôte : rien d'obligatoire — sans routeur, rien ne change.*
2. **L'apprentissage.** Implémenter `Learning` : récompense d'une fin de tour
   (`Stop` → 1 / 0 / rien, comme `turn_reward`), mise à jour des bras, journal des
   décisions avec `p_top1`, rétrogradation de l'étape. *Côté hôte : un magasin de bras
   et de décisions (IndexedDB dans la page, base côté serveur) et une horloge pour les
   horodater.*
3. **Les transports qui honorent la paire.** `this-llm-providers` et le transport de la
   page lisent `Request::provider()` / `model()` **et déclarent `Transport::honors_route`**
   (c'est la seule chose qui sépare une décision consultative d'une décision servie) ;
   bascule après un refus de quota sur un modèle (`model_after_limit`). *Côté
   hôte : un `PoolSource` réel (catalogue des modèles, capacités, santé) et un
   `QuotaView` nourri par les refus des moteurs.*
4. **La segmentation de la conversation.** Frontières de sujet et de classe, qui
   relâchent la garde « pas deux fournisseurs de suite ». *Côté hôte : persistance des
   segments.*
5. **Les exécuteurs.** Signatures de tâche, de délégation et de sous-agent ; décision de
   compaction ; mode `executors_only`. *Côté hôte : le lanceur de sous-agents passe par
   le routeur.*
6. **Les réglages et l'interface.** Documents de réglages (global, projet, personne),
   leur validation, le menu auto / mixed / strict de la page, l'affichage de
   `Event::Routed` et des alternatives rejetées. *Côté hôte : stockage par personne et
   droits (seule la personne écrit sa liste).*

---

## 6. Contrat des ports de l'hôte

Le cœur est pur ; tout ce qui sort de lui passe par un port, et chaque port a un contrat.

| Port | Ce que l'hôte garantit | Ce que le harnais garantit |
|---|---|---|
| `Transport::honors_route` | `true` seulement s'il lit `Request::provider()` / `model()` et parle au moteur nommé | tant que c'est faux : `Event::Routed.advisory = true`, décision enregistrée `applied: false`, `used: None`, et `Learning::settle(served = None)` |
| `PoolSource::pool` | rend les candidats vivants (santé, consentement, quotas déjà appliqués) | borné à 2 s (`ROUTER_TIMEOUT_MS`) sur le `Clock` de l'hôte ; au-delà, le tour part sur la paire en vigueur (`timed_out`) ; **coupé par l'annulation, avec ou sans minuteur** (`Cancel` réveille son attente) |
| `Learning::arms`, `hints` | rend des postérieurs valides | un bras invalide écarte sa paire (`invalid_arm`), jamais une boucle ni un `NaN` |
| `Learning::record` | au mieux : peut être perdu | appelé une fois par décision, APRÈS que l'état a été réécrit et `Event::Routed` émis ; borné et annulable ; `used` = la paire du tour (`None` si inconnue : consultative, ou sentinelle — jamais `auto`), `applied` vrai seulement si le choix est cette paire |
| `Learning::settle` | traite un identifiant inconnu comme un no-op ; garde la **première** fin reçue pour un identifiant | borné à 2 s, ne retient ni `Stopped` ni `busy` (avec minuteur) ; `served = Some` seulement si le transport honore la paire ET qu'une requête est partie ; la décision reste ouverte tant que `settle` n'a pas rendu : un `settle` coupé, ou un tour abandonné, est redit au tour suivant en `Stop::Interrupted`, `served = None` |
| `Seeds::next_id` | un `u128` par décision | l'identifiant est la graine ; sérialisé en chaîne |
| `QuotaView` | une échéance passée n'est plus rendue | — |

**Annulation.** `Cancel` réveille les attentes enregistrées sur `Cancel::cancelled` :
`interrupt()` coupe `route` et `record` tout de suite, **avec ou sans minuteur** — c'est
le cas d'une page wasm, où `SystemClock::sleep` rend `None` (test
`l_annulation_coupe_un_routeur_qui_pend`, horloge sans minuteur, interruption venue
d'une autre tâche après la suspension).

**Limites connues.**
- Sans minuteur, la borne de TEMPS n'existe pas : un port qui pend retient le tour
  jusqu'à l'annulation, et un `settle` qui pend (non coupé par l'annulation, puisqu'un
  run annulé doit lui aussi être rendu) retient `Stopped`. C'est la limite que porte
  déjà `Recorder` (`RECORDER_TIMEOUT_MS`) ; un hôte wasm qui veut la borne injecte une
  horloge avec `Harness::with_clock`.
- Avec minuteur, la borne a une granularité d'au plus `WATCH_TICK_MS` (1 s). Une
  horloge dont `sleep` rend tout de suite SANS que l'heure avance n'atteint jamais
  l'échéance : la veille cède la main à chaque pas (le travail et l'annulation restent
  sondés), mais elle tourne sans fin.

**Coût.** Le besoin d'images parcourt le fil à chaque objectif (O(fil)) : c'est la
même passe que l'estimation de jetons faite juste avant (`estimate_tokens` sur le même
fil), et chaque tour clone déjà ce fil pour construire sa requête — l'ordre de grandeur
ne change pas, un compteur tenu à part ne serait qu'un état de plus à garder juste
(réfutation de M6, partie fil). Les bras sont indexés par (instance, modèle) avant la
note (`arms_of_class`) : une recherche en temps constant au lieu d'un parcours par
candidat (M6, partie bras).
