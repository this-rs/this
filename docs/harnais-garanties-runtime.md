# Garanties runtime du harnais — inventaire des preuves (issue #285)

Ce document confronte ce que `this-harness` **annonce** en matière de garanties
d'exécution (annulation, bornes temporelles, isolation, supervision) à ce qui est
**réellement exécuté** par les tests et la CI. Il ne propose aucun changement : c'est
un inventaire. Une garantie qui n'est vérifiée que par une compilation `wasm32` n'est
pas une garantie d'exécution, et ce document le dit là où c'est le cas.

Mesuré sur `docs/harness-contrats` (parent `origin/main` = `bd387e8`). Chemins relatifs
à la racine du dépôt ; `session.rs` désigne `crates/this-harness/src/session.rs`,
`ports.rs` désigne `crates/this-harness/src/ports.rs`, `ci.yml` désigne
`.github/workflows/ci.yml`.

Le point de départ vérifié : les 26 fichiers de `crates/this-harness/tests/` portent
**280** `#[test]` (comptés par `grep -rc`), et **aucun** n'est gaté par un
`#[cfg(target_arch = "wasm32")]` ni par `wasm-bindgen-test` (grep nul sur le dossier
`tests/`). Ils tournent donc tous en natif. Le détail de ce constat est au §2.

---

## 1. Les quatre profils de capacités runtime de l'hôte

Le harnais ne fait aucune entrée/sortie lui-même (`lib.rs:1-11`) : tout ce qui touche au
monde passe par un port. Quatre capacités sont exigées de l'hôte, avec des degrés
d'exigence différents.

### 1.1 Timer (horloge monotone + `sleep`)

**Ce que le cœur exige.** Une `Clock` avec deux méthodes : `now_ms()` (« maintenant », en
millisecondes) et `sleep(ms)` (une attente), plus un `nonce()` (`session.rs:108-126`).
Sans `sleep` réel, les chiens de garde ne peuvent pas être armés : la doc du trait le
dit explicitement — « les chiens de garde sont alors éteints et les relances partent sans
délai » (`session.rs:111-113`).

**Ce qu'un hôte peut ne pas fournir.** Un `sleep` qui rend `None`. C'est le cas de la
cible wasm32 : `SystemClock::sleep` y rend `None` (`session.rs:160-163`), car le crate
n'embarque pas `wasm-bindgen-futures` (`session.rs:145-146`). Un hôte wasm qui veut la
borne temporelle injecte sa propre horloge (`setTimeout`) par `Harness::with_clock`
(`session.rs:1046`, `session.rs:146`).

**Ce qui se dégrade alors.** Sans minuteur, la borne de **temps** n'existe pas : un port
qui pend retient le tour jusqu'à l'annulation, et un `settle` qui pend (non coupé par
l'annulation, puisqu'un run annulé doit lui aussi être rendu) retient `Stopped`. C'est la
limite documentée dans `docs/harnais-mode-auto.md:401-407`, qui renvoie à la même limite
portée par `Recorder` (`RECORDER_TIMEOUT_MS`, `session.rs:426`). L'annulation, elle,
reste fonctionnelle **sans** minuteur : `Cancel::cancel` réveille les attentes
(`ports.rs:34-37`), testé par `mode_auto.rs::l_annulation_coupe_un_routeur_qui_pend`.

**Garantie annoncée mais inapplicable.** La borne temporelle (`Limits::budget_ms`,
`silence_ms`, `no_progress_ms`, `tool_timeout_ms`) est **silencieusement dégradée** en
l'absence de minuteur, pas refusée. Un hôte wasm qui oublie `with_clock` obtient un
harnais qui *semble* borné (les `Limits` sont là) alors qu'aucune borne de temps ne
sonne. La dégradation est documentée, mais rien ne la **signale** à l'exécution : c'est
un défaut de la garantie « le budget est une fin » (`lib.rs:32`) sur ce profil.

### 1.2 Annulation coopérative

**Ce que le cœur exige.** Un transport et des outils qui consultent le drapeau `Cancel`
pendant leurs attentes. `Transport::stream` reçoit `cancel` (`ports.rs:134-139`),
`Toolbox::call_with_cancel` le reçoit aussi (`ports.rs:250-260`). Un transport qui
l'ignore n'est « pas faux, seulement plus lent à s'arrêter » (`ports.rs:131-133`).

**Ce qu'un hôte peut ne pas fournir.** Un transport qui n'écoute pas `cancel` du tout
(sourd). Le harnais le traite : après `INTERRUPT_GRACE_MS` (10 s, `session.rs:47`), le
tour est abandonné et le dit. Testé par
`garde.rs::un_transport_sourd_a_l_annulation_est_abandonne_apres_la_grace`.

**Ce qui se dégrade alors.** Rien de caché : l'abandon est nommé (`Stop::Interrupted`).
La seule dégradation est le délai (la grâce), pas l'absence d'arrêt.

**Garantie annoncée mais inapplicable.** Les deux ports de **préparation** — le hook de
contexte (`TurnContext::before_turn`, `session.rs:141`) et le compacteur
(`Compactor::compact`, `ports.rs:278`) — **n'avaient pas de paramètre `cancel`** au
moment de cet inventaire : un hook ou un compacteur qui ne rend jamais la main retenait
le run même après `interrupt()`. C'est exactement le ticket #291, corrigé en parallèle :
voir §4.

### 1.3 Isolation des outils

**Ce que le cœur exige.** Une part de `Toolbox` qui **panique** ne doit pas faire tomber
le routage des autres. `part_specs` et `part_declarations` enveloppent les appels dans
`catch_unwind` et rendent une liste vide sur panique (`ports.rs:317-331`). Un enregistreur
qui panique n'arrête pas le run (`guarded`, `session.rs:427-434`).

**Ce qu'un hôte peut ne pas fournir / ce que le cœur ne peut pas borner.** Deux cas sont
**explicitement documentés comme non couverts** (`ports.rs:320-322`, `session.rs:430-434`) :
1. une part qui **boucle sans fin** dans `specs()` — la méthode est synchrone, rien ne
   peut la borner ;
2. un build `panic = "abort"` (wasm32 inclus), où la panique n'est pas rattrapable.

**Ce qui se dégrade alors.** Sur `panic = "abort"`, la panique d'une part **tue le
processus** : aucune frontière ne la contient. Il n'existe **aucun test** de ce cas (et
il ne peut pas y en avoir un qui survive au crash). Le repli documentaire est honnête
mais ce n'est pas une frontière d'isolation : c'est une exclusion déclarée.

### 1.4 Supervision (chiens de garde)

**Ce que le cœur exige.** Trois bornes de temps par tour (silence, sans-progrès, plafond
d'outil) et une borne de budget de run, plus le budget de contexte. Constantes :
`SILENCE_MS` (120 s, `session.rs:50`), `NO_PROGRESS_MS` (10 min, `session.rs:68`),
`TOOL_TIMEOUT_MS` (5 min, `session.rs:79`), `WATCH_TICK_MS` (1 s, `session.rs:98`).
En natif, un fil dédié (`this-harness-timer`) tient les échéances et réveille
(`session.rs:170-238`).

**Ce qu'un hôte peut ne pas fournir.** Le minuteur lui-même (wasm32, cf. §1.1).

**Ce qui se dégrade alors.** La supervision temporelle est éteinte ; seuls restent la
borne de tours, la borne de contexte (synchrone) et l'annulation.

---

## 2. Inventaire : ce qui est réellement EXÉCUTÉ

Trois sources confrontées : les 26 fichiers de tests, le workflow CI, et ce qui n'est
couvert nulle part.

### 2.1 Les tests d'intégration : 26 fichiers, 280 tests, tous natifs, tous en exécuteur monofil

Les 26 fichiers de `crates/this-harness/tests/` (280 `#[test]` au total) tournent dans un
**exécuteur monofil `!Send`** : `futures::executor::block_on`, présent dans 23 des 26
fichiers (grep `block_on`). Ce n'est **pas** un `LocalSet` tokio — le crate n'a aucune
dépendance tokio (`crates/this-harness/Cargo.toml:32-36` ne liste que `futures` et
`this-llm-providers` en dev). La mention `LocalSet` n'existe qu'en commentaire, pour dire
qu'un serveur *pourrait* y envelopper la boucle (`ports.rs:122-125`) : **aucun test ne
vérifie cet hébergement serveur sous `LocalSet`**.

Une tranche minimale de cette suite (5 scénarios, `tests/commun/mod.rs`) est désormais
aussi exécutée dans un navigateur par `tests/navigateur.rs` (`wasm-bindgen-test`, voir
§5) ; le reste de la suite (le gros des 280 tests) reste natif seulement.

La seule exception à « tout est natif » est un test **unitaire** natif-only dans
`session.rs` : `un_reveil_qui_panique_ne_tue_pas_le_minuteur` (`session.rs:3186`),
gaté `#[cfg(not(target_arch = "wasm32"))]`, qui vérifie que le fil du minuteur survit à
un réveil qui panique. Il tourne sous `cargo test --lib`, jamais en wasm (mais rien ne
tourne en wasm de toute façon).

### 2.2 Ce que la CI exécute réellement

| Garantie | Exécutée où | Preuve |
|---|---|---|
| Boucle, annulation, relance, file, arrêt nommé | natif (Linux, `block_on`) | `ci.yml:135` `cargo test --features ${{ matrix.features }} --test '*' -- --test-threads=1` ; tests `scenario.rs`, `garde.rs`, `gardes.rs` |
| Chiens de garde (silence, sans-progrès, plafond outil, budget temps) | natif, horloge factice + **une** exécution horloge système réelle | `garde.rs::un_flux_muet_est_interrompu_en_disant_pourquoi`, `garde.rs::un_outil_qui_depasse_son_plafond_rend_delai_depasse`, `garde.rs::l_horloge_du_systeme_arme_vraiment_la_garde` (celle-ci sans `FakeClock`, elle arme le fil natif) |
| Routage mode Auto (pool/settle/record qui pendent, annulation, horloge figée) | natif | `mode_auto.rs::un_pool_qui_pend_ne_bloque_pas_le_tour`, `mode_auto.rs::un_settle_qui_pend_ne_retient_ni_stopped_ni_busy`, `mode_auto.rs::l_annulation_coupe_un_routeur_qui_pend`, `mode_auto.rs::une_horloge_figee_ne_gele_pas_le_routeur` |
| Isolation par panique (specs/declarations/enregistreur/outil) | natif | `residus.rs::a_part_whose_specs_panic_does_not_block_the_others`, `residus.rs::a_panicking_recorder_does_not_stop_the_run`, `robustesse.rs::un_outil_qui_panique_ne_laisse_pas_un_fil_invalide` |
| Bornes dégénérées, transport muet en boucle, run lâché pendant l'enregistreur | natif | `robustesse.rs::des_bornes_degenerees_se_terminent_en_disant_leur_arret`, `robustesse.rs::un_transport_muet_en_boucle_est_borne`, `robustesse.rs::un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret` (horloge `NoTimer`) |
| Tests **unitaires** du crate (dont le minuteur natif) | natif (Linux, macOS, Windows) | `ci.yml:132` `cargo test --lib …` ; `ci.yml:553` `cargo test --lib --features all,wami` (cross-platform) |
| Compilation **wasm32** du harnais | **seulement compilée** (jamais exécutée) | `ci.yml:688` `cargo check -p this-agent -p this-harness --target wasm32-unknown-unknown` ; `ci.yml:734` `cargo check -p this-harness --target wasm32-unknown-unknown --no-default-features` ; lint de liste noire `ci.yml:92` (`scripts/lint-harness-portability.rs`, qui bannit `std::process`, `std::fs`, `std::net`, `std::thread`, `std::time::SystemTime`… `lint-harness-portability.rs:22-33`) |
| Tests d'intégration sur **macOS/Windows** | **absents** (Linux seulement) | `ci.yml:556-557` `if: runner.os == 'Linux'` avant `cargo test … --test '*'` ; sur macOS/Windows seul `--lib` tourne (`ci.yml:553`) |
| Exécution dans un **navigateur** (`wasm-bindgen-test`) | **5 scénarios partagés** (job `wasm32_browser`) | `tests/navigateur.rs` ; scénarios dans `tests/commun/mod.rs`, aussi appelés en natif par `robustesse.rs`. Voir §5 pour ce que cela prouve et ne prouve pas |

### 2.3 Ce qui n'est couvert nulle part

- L'arrêt effectif des **processus enfants** côté adaptateur natif (voir §3, case f).
- Le comportement du harnais quand la **page est fermée** en pleine exécution (voir §3,
  case b).
- `panic = "abort"` et la boucle synchrone infinie dans `specs()` (documentés non couverts,
  jamais exécutés — voir §1.3).
- L'hébergement serveur sous `LocalSet` tokio (mentionné en commentaire, jamais testé).

---

## 3. Les six cases d'acceptation du ticket

| # | Case d'acceptation | Verdict | Preuve / manque |
|---|---|---|---|
| a | Même suite comportementale en navigateur WASM et en natif `LocalSet` | **partiel** | Cinq scénarios partagés sont exécutés dans Chrome headless et en natif (voir §5), mais aucun test `LocalSet`. Le reste de la suite native tourne en `block_on` monofil (`scenario.rs:129`, `famille_a.rs:126`…), qui est l'analogue `!Send` le plus proche, sans éprouver l'hébergement serveur. |
| b | Tests transport muet, Recorder/settle suspendu, interruption, fermeture de page, nouvelle requête après arrêt | **preuve, sauf « fermeture de page »** | transport muet : `garde.rs::un_flux_muet_est_interrompu_en_disant_pourquoi` et `robustesse.rs::un_transport_muet_en_boucle_est_borne` ; Recorder suspendu : `residus.rs::a_silent_recorder_delays_the_run_by_a_bounded_time`, `robustesse.rs::un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret` ; settle suspendu : `mode_auto.rs::un_settle_qui_pend_ne_retient_ni_stopped_ni_busy` ; interruption : `scenario.rs::interrupt_arrete_le_run_et_le_dit`, `garde.rs::un_transport_sourd_a_l_annulation_est_abandonne_apres_la_grace` ; nouvelle requête après arrêt : `residus.rs::an_interrupt_between_two_runs_is_ignored_and_does_not_poison_the_next_one`, `scenario.rs::un_message_tenu_part_au_tour_suivant`. **Fermeture de page : manque** — aucun test ne simule la fermeture ; les plus proches sont les futures lâchées (`gardes.rs::une_future_abandonnee_rend_la_place_et_le_dit`, `gardes.rs::une_future_lachee_pendant_un_outil_laisse_un_fil_valide`), qui testent l'abandon de la future, pas la fermeture d'une page. |
| c | Hôte sans timer : garantie dégradée explicite **ou** refus du profil | **preuve (dégradée explicite), refus absent** | `SystemClock::sleep` rend `None` en wasm (`session.rs:160-163`) ; `Clock::sleep` documente « chiens de garde éteints » (`session.rs:111-113`) ; `docs/harnais-mode-auto.md:401-407` le dit noir sur blanc. Testé par `robustesse.rs::un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret` (horloge `NoTimer`) et `mode_auto.rs::l_annulation_coupe_un_routeur_qui_pend` (horloge sans minuteur). **Il n'y a pas de refus du profil** : la dégradation est silencieuse à l'exécution, rien ne signale qu'aucune borne de temps n'est armée. |
| d | `panic=abort` et outils synchrones bloquants : frontière d'isolation **ou** documentés non couverts | **preuve (documentés non couverts) ; frontière limitée** | Documenté : `ports.rs:320-322` (boucle synchrone infinie non bornable, `panic=abort` non rattrapable) et `session.rs:430-434` (idem pour l'enregistreur). Frontière d'isolation par `catch_unwind` **testée** : `residus.rs::a_part_whose_specs_panic_does_not_block_the_others`, `robustesse.rs::un_outil_qui_panique_ne_laisse_pas_un_fil_invalide`. Mais il n'existe **aucune** frontière réelle pour `panic=abort` ni pour la boucle synchrone : c'est de la documentation, pas une isolation. |
| e | Temps monotone distingué des dates civiles + horloge simulée en tests | **manque (monotone/civil) ; preuve (horloge simulée)** | Horloge simulée : `FakeClock` injectée par `with_clock`, testée partout (`garde.rs:35-60`, `adverse.rs:35-60`, `declarations.rs:22-45`…). Distinction monotone/civil : **non faite proprement**. `now_ms()` est une horloge **civile** (epoch) sur les deux cibles — `SystemTime::now().duration_since(UNIX_EPOCH)` en natif (`lib.rs:106-112`), `js_sys::Date::now()` en wasm (`lib.rs:101-104`) — et c'est elle qui sert à **toute** la comptabilité temporelle (`clock.now_ms()` aux `session.rs:812, 1174, 1270, 1346, 1379, 1713, 1802`). Le temps **monotone** (`std::time::Instant`) n'existe que pour les échéances de `sleep` du fil natif (`session.rs:35, 67`) ; il n'est pas exposé par `Clock`. Un saut d'horloge (NTP, DST) fausse donc budget/silence/plafonds. |
| f | Arrêt effectif des processus enfants côté adaptateur natif | **manque** | Aucun adaptateur de processus enfant dans le crate : grep `child\|process\|spawn\|kill` sur `src/` ne trouve que le fil du minuteur (`session.rs:194`) et le thread de test (`session.rs:3204`). Le harnais ne fait aucune I/O (`lib.rs:1-11`) : la future se termine, mais rien ne garantit qu'un sous-processus lancé par un outil de l'hôte est tué. |

---

## 5. Ce que la suite navigateur prouve / ne prouve pas

Les scénarios de `crates/this-harness/tests/commun/mod.rs` sont écrits une fois ; ils
sont exécutés en natif par `tests/robustesse.rs` (`#[test]`) et dans Chrome headless par
`tests/navigateur.rs` (`#[wasm_bindgen_test]`, `run_in_browser`). Lancement local avec
`wasm-bindgen-test-runner` de la version exacte de `wasm-bindgen` du `Cargo.lock`,
Chrome et un chromedriver compatible :
`CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p this-harness --target wasm32-unknown-unknown --test navigateur`.
Configurer `CHROMEDRIVER` si nécessaire. En CI : job `wasm32_browser`.

Les cinq scénarios : transport muet en boucle borné par les tours ; mille appels d'outils
dans un tour, fil valide ; `interrupt` avant un run sans empoisonner le suivant ;
interruption pendant un outil puis nouveau `send` (nouvelle requête après arrêt) ; run
lâché pendant un `Recorder::stopped` suspendu (horloge sans minuteur).

**Prouvé** : la logique de la boucle (arrêt nommé, un seul `Stopped`, place rendue,
drapeau d'annulation remis à zéro, fil valide) s'exécute et donne le même résultat une
fois compilée en wasm32 et pilotée dans un navigateur, avec un exécuteur à sondage borné.

**Non prouvé** (ne pas le déduire de la suite navigateur) :

- **Aucun minuteur réel** : les scénarios utilisent une horloge factice ou `NoTimer`
  (`sleep` rend `None`). Les chiens de garde temporels (silence, sans-progrès, plafond
  d'outil, budget) et `RECORDER_TIMEOUT_MS` ne sont pas exercés dans le navigateur ; un
  hôte qui branche `setTimeout` via `with_clock` n'est pas testé.
- **Pas de `panic=abort`** ni de rattrapage de panique : `catch_unwind` n'existe pas sur
  wasm32 (une panique y est fatale), donc les scénarios d'isolation par panique
  (`un_outil_qui_panique…`, `un_puits_qui_panique…`) restent natifs seulement.
- **Pas d'arrêt de processus enfants** : le harnais n'en lance aucun ; la case (f) de
  l'issue reste non couverte, natif seulement par nature.
- **Pas de fermeture de page** en cours de run, ni d'hébergement sous `LocalSet`.
- Seulement 5 scénarios sur les 280 tests : le reste de la suite (mode Auto, gardes,
  `settle` suspendu de `mode_auto.rs`, etc.) n'est pas encore partagé.
- Un seul navigateur (Chrome) ; ni Firefox ni Safari.

---

## 4. Renvoi : l'annulation des ports de préparation (#291)

Le cas « hook de contexte / compacteur qui ne rend jamais la main » du §1.2 est corrigé
en parallèle sur la branche `fix/harness-budget-interrupt`, commit
`c3ba56e` (« fix(harness): borner et rendre annulables turn_context et compact (#291) »).
La correction ne change pas la signature des traits : elle enveloppe l'attente dans une
course `race(port, cut(clock, ROUTER_TIMEOUT_MS, cancel))` pour `turn_context` et
`compact`, et relit l'annulation juste après, arrêtant le run en `Interrupted` avant le
transport. Ce document la cite comme **preuve en cours** de l'annulation coopérative des
ports de préparation, sans la dupliquer : le détail des tests vit sur cette branche
(`robustesse_ports.rs`).
