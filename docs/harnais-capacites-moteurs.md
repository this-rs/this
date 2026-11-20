# Matrice de conformité des capacités et du routage réellement servi

Ce document qualifie, capacité par capacité, ce que le harnais garantit, ce que le
transport doit fournir, et ce qui est réellement **servi** — par opposition à ce qui est
**routé**. C'est l'inventaire des preuves du ticket #288. Il qualifie l'existant ; il ne
propose ni nouveau routeur ni réimplémentation.

Méthode : lecture de `ports.rs`, `wire.rs`, `session.rs`, du module `routing/`, des tests
de `crates/this-harness/tests/`, et recoupement avec `docs/harnais-mode-auto.md`
(actualisé là où le code dit autre chose). Chaque affirmation porte `fichier:ligne`.

## 1. Matrice par capacité

| Capacité | Ce que le cœur garantit | Ce que le transport doit fournir | Où c'est vérifié (test exact) | Statut |
|---|---|---|---|---|
| **Streaming** | `Transport::stream` reçoit un rappel `Delta` et `Event::Delta` est émis par morceau, jamais un bloc | appeler `delta` pour chaque morceau, consulter `cancel` | `le_flux_arrive_morceau_par_morceau` (`tests/scenario.rs:525`) ; `un_run_de_plusieurs_tours_emet_deltas_puis_un_spoke_par_tour_dans_l_ordre` (`tests/spoke.rs:139`) | **qualifié** |
| **Outils internes** | `Toolbox::specs`/`call(_with_cancel)` ; routage « premier qui déclare gagne », table reconstruite à chaque appel | rendre les `specs()` et exécuter `call` | `deux_sources_d_outils_se_composent_sans_se_voir` (`scenario.rs:800`) ; `un_outil_declare_deux_fois_n_est_annonce_qu_une_fois_et_la_premiere_part_gagne` (`robustesse_ports.rs:63`) ; `un_outil_installe_pendant_le_run_devient_routable` (`scenario.rs:839`) | **qualifié** |
| **Outils externes (MCP)** | `McpToolbox` : `serveur/outil`, liste blanche de l'hôte, réponses enfermées `untrusted` | implémenter `McpLink` (un `fetch`/`reqwest`) | `only_declared_tool_is_announced_and_undeclared_call_never_reaches_the_link` (`tests/mcp.rs:77`) ; `a_dead_server_does_not_starve_the_other` (`mcp.rs:181`) | **qualifié** |
| **Permissions** | `Harness::with_grants` (`session.rs:961`) + `gate` relu à **chaque** appel ; refus **avant** l'appel | porter les autorisations via `Toolbox::grants` ou `with_grants` | `un_outil_non_declare_est_refuse_et_jamais_appele` / `un_outil_declare_mais_non_autorise_est_refuse_pour_cette_raison` / `un_outil_sans_aucune_autorisation_est_refuse_fail_closed` (`grants.rs:115,127,141`) ; `composition_approuvee_outil_non_autorise_refuse_avant_l_appel` (`approbation_guarded.rs:166`) | **qualifié** |
| **Interruption** | `Handle::interrupt` pose `Cancel` ; le transport le lit, la boucle entre deux outils ; l'abandon se dit `Stopped` | consulter `cancel` pendant le flux et dans les outils (`call_with_cancel`) | `interrupt_arrete_le_run_et_le_dit` (`scenario.rs:222`) ; `l_annulation_atteint_les_outils` (`gardes.rs:371`) ; `s6_annulation_sourde_est_interrupted_avec_la_raison` (`adverse.rs:659`) | **qualifié** |
| **Reprise** | `Snapshot`/`restore`/`seed` ; `restore` refusé pendant un run ; ids jamais réutilisés | persister/relire le `Snapshot` | `un_etat_sauvegarde_se_repare_et_se_rejoue` (`scenario.rs:489`) ; `restore_ne_fait_pas_reculer_les_identifiants` / `restore_pendant_un_run_ne_corrompt_pas_le_fil` (`robustesse.rs:191,206`) | **qualifié (borné)** — pas de curseur d'événement (cf. §3 du document #286) |
| **Images / documents** | images : `Message.images` (`ImagePart`), `Capabilities.images`, filtre `NoImages`, garde « aveugle » du routeur | résoudre la source de l'image et la traduire dans le dialecte du moteur (`wire.rs` : la source dit OÙ, pas comment envoyer) | images : `les_images_d_un_message_arrivent_au_transport_a_chaque_tour` (`scenario.rs:945`) ; `a_user_message_with_images_round_trips_and_names_its_sources` (`wire.rs:474`) ; `les_images_demandent_un_modele_qui_les_lit` (`candidate.rs:623`). documents : **non qualifié** — aucun concept de document dans le cœur | **images : qualifié / documents : non qualifié** |
| **Fenêtre de contexte** | `Limits::context_budget` (défaut `CONTEXT_BUDGET = 120_000`, `session.rs:88`), `fit`/`estimate_tokens`, événements `Trimmed`/`Compacted` | rien (l'estimation est interne) ; un hôte qui connaît la vraie fenêtre la fixe via `Limits` | `cjk_text_is_not_estimated_at_a_quarter_token_per_char` / `fit_extreme_budgets` (`robustesse_fil.rs:34,74`) ; `check_thread_refuses_what_the_engine_would_refuse` (`robustesse_fil.rs:173`) | **qualifié** (estimation, jamais mesure — `context.rs:1-20`) |
| **Usage réel (tokens)** | rien : `Reply { text, calls, finish }` ne porte **aucun** champ d'usage (`wire.rs:318`) ; `estimate_tokens` est une estimation, pas une mesure | rendre l'usage réel (prompt/completion) s'il doit être compté | **aucun test** — la mesure n'existe pas dans le cœur | **non qualifié** |
| **Changement de modèle à chaud** | la paire choisie sort par `Request::model`/`provider` (`wire.rs`) et ne vaut **qu'à partir du tour suivant** ; `Route` écarte la sentinelle `auto` par construction | lire `Request::provider()`/`model()` **et déclarer `honors_route = true`** | `un_modele_ou_un_fournisseur_impose_ne_bouge_pas_jusqu_a_ce_qu_auto_soit_rechoisi` (`mode_auto.rs:379`) ; `auto_ne_sort_jamais_vers_un_fournisseur` (`mode_auto.rs:996`) | **non qualifié** (pas de `set_model_live` ; `docs/harnais-mode-auto.md` D6(b)) |

## 2. « Routé ≠ servi »

Le routeur décide une paire ; le moteur ne la sert que si le transport le déclare. Les
points où le code vérifie (ou non) que la paire atteinte est bien celle annoncée :

- **La garantie d'honneur est déclarative et fermée par défaut.** `Transport::honors_route`
  vaut `false` (`ports.rs:162`) ; un transport qui ne le surcharge pas « sert son modèle à
  lui » (`ports.rs:154-158`).
- **La décision est consultative tant que ce n'est pas honoré.** `Event::Routed.advisory`
  est posé quand le transport n'honore pas (`session.rs:1895-1910`) ; le champ est absent
  quand il est faux (`event.rs:151-167`).
- **« Servi » exige honneur ET réponse du moteur.** `honored = transport.honors_route()
  && answered.get()` (`session.rs:1684`) — un morceau reçu, une réponse rendue ou un refus
  du moteur (`Http`/`Provider`) comptent ; `Unreachable`, `Timeout` d'ouverture ou une
  annulation avant toute réponse ne comptent **pas** (`session.rs:1417-1427`). Le test
  `un_moteur_jamais_joint_n_a_servi_aucune_paire` (`mode_auto.rs:2198`) et
  `une_paire_jamais_atteinte_est_oubliee` (`mode_auto.rs:2297`) le prouvent.
- **L'apprentissage n'est nourri que d'une paire servie.** `served = honored.then_some(&pick)`
  (`router.rs`, `settle`) ; une décision consultative n'arrive jamais à `settle`
  (`router.rs` doc de `Learning`, `:120-130`). `un_transport_qui_n_honore_pas_la_paire_rend_la_decision_consultative`
  (`mode_auto.rs:1443`).
- **Où ce n'est PAS vérifié :** aucun transport de production n'implémente `honors_route`
  ni même `Transport`. `grep -rn "fn honors_route" crates/` ne trouve que `ports.rs` (le
  défaut) et les faux transports de `tests/mode_auto.rs` (7 occurrences). `this-llm-providers`
  expose `ErrorKind` (relié par `tests/codes.rs:12`) mais **n'implémente pas `Transport`**.
  En conséquence, aujourd'hui **toute** décision du mode Auto est consultative dans un vrai
  montage : ne pas prétendre qu'un apprentissage est fondé sur une paire servie tant que le
  transport de l'hôte ne le déclare pas — c'est exactement ce que dit `docs/harnais-mode-auto.md`
  lot 3, et c'est confirmé ici, pas démenti.

## 3. Cases d'acceptation du ticket

| # | Case | Verdict | Preuve ou manque |
|---|---|---|---|
| C1 | Capacité inconnue ≠ « supportée », et erreur nommée pour fonction indisponible | **preuve existante** | la fenêtre inconnue est un rejet typé `WindowUnknown` (jamais supposée) et les capacités manquantes sont `NoTools`/`NoImages` (`candidate.rs:245-251,306-368`) ; un outil inventé est refusé nommément sans appeler la toolbox : `unknown_tool` (`session.rs:2132`) et `un_outil_non_declare_est_refuse_et_jamais_appele` (`grants.rs:115`) ; un outil MCP non déclaré est refusé sans toucher le lien (`mcp.rs`, `call_with_cancel`) ; `des_faits_invalides_sont_ecartes_avec_un_motif_nomme` (`mode_auto.rs:1781`) |
| C2 | Épuisement modèle puis provider : repli borné au même tour, pas de replay aveugle, pas de boucle de quotas | **manque** | le **filtre** distingue `QuotaExhausted` (compte) de `QuotaExhaustedModel` (modèle) et les applique avant la santé (`candidate.rs:325-336`, `apply_quota` `:411`) ; le **repli borné** d'une panne passagère existe (`MAX_RETRIES = 3`, `RETRY_BACKOFF_MS`, `transient`, `session.rs:91,94,543,1428-1460`). Mais le basculement « après un refus de quota, sur un autre modèle de la même instance » (`model_after_limit`) est **`plus tard (lot 3)`** (`docs/harnais-mode-auto.md`, ligne 20) : pas implémenté, pas testé. |
| C3 | Réglage manuel pendant une décision asynchrone qui gagne | **preuve existante** | `RouteWhy::Superseded` (`router.rs`) : si `generation` a bougé pendant l'attente, la décision est jetée et le réglage gagne (`router.rs`, bloc `state.generation != generation`) ; test `un_reglage_a_la_main_pendant_la_decision_gagne` (`mode_auto.rs:1210`) ; `un_menu_vide_ne_jette_pas_la_decision_en_cours` (`mode_auto.rs:1957`) |
| C4 | Auth expirée / coffre verrouillé distincts d'une panne réseau | **preuve existante (avec réserve)** | `ProviderError` distingue `SessionMissing`/`SessionExpired`/`SessionUnavailable` et `Secret`/`MissingSecret` (`this-llm-providers/src/error.rs:26-44`), et les range délibérément **hors** de l'infrastructure : `ErrorKind::Provider`, pas `Unreachable` (`error.rs:182-196`) ; chaque famille est un `Code` du harnais (`tests/codes.rs:12`). **Réserve** : à la frontière `Code`, `SessionExpired` et `Secret` s'effondrent tous deux en `Code::Provider` (`error.rs:184-196`) — le cœur ne les distingue **pas l'un de l'autre**, seulement d'une panne réseau (`Unreachable`/`Timeout`). |
| C5 | Faux transports versionnés + tests de raccordement réels opt-in | **manque** | les faux transports de test ne portent **pas** de version (`tests/mode_auto.rs`, `scenario.rs`, `gardes.rs`…) ; il n'existe **aucun** test de raccordement réel (opt-in ou non) — aucun `impl Transport` hors tests, aucun `honors_route` en production. À écrire : un contrat « un transport qui déclare `honors_route` doit lire `Request::model`/`provider` », et un test opt-in qui branche `this-llm-providers` sur une instance réelle. |

Bilan des cases : **3 « preuve existante »** (C1, C3, C4 avec réserve), **2 « manque »** (C2, C5).

## 4. Ce que l'inventaire a actualisé par rapport à `docs/harnais-mode-auto.md`

Le document de mode Auto reste exact sur le fond. Trois points sont précisés ici, pas
démentis : (1) le filtre de quota est **déjà qualifié** (`QuotaExhausted`/`QuotaExhaustedModel`),
mais le **repli** `model_after_limit` est toujours « plus tard », donc C2 est un manque ;
(2) `honors_route` n'a toujours **aucune** implémentation de production, ce qui rend toute
décision Auto consultative en montage réel — le tableau d'écart le dit déjà (ligne 31) ;
(3) « usage réel (tokens) » n'est une capacité **d'aucun** des deux camps : le cœur estime,
le transport de production ne renvoie pas l'usage — c'est un manque que le document de mode
Auto ne listait pas explicitement.
