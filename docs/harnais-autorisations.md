# Autorisations du harnais — suite de conformité entre hôtes, outils composés, moteurs délégués (issue #287)

Ce document est un **inventaire de qualification sécurité**, pas la démonstration d'une
vulnérabilité. Chaque chemin qu'aucun test ni aucune garde écrite ne couvre est nommé
« non qualifié », et ce qui manque renvoie à une RFC, jamais à une modification de règle
(le ticket interdit de proposer des changements de permission). Il ne recrée pas ce qui
existe déjà : il le cite et dit *pourquoi* c'est une preuve.

**Rejoué le 24/11/2026** sur la base `787a988` (branche `fix/287-autorisations-conformite`).
Les verdicts du 21/11 et du 22/11 étaient **périmés** — #299, #345, #360 et #363 ont fusionné
entre-temps. Chaque ligne « preuve » cite un **nom de test** ; une ligne « manque » nomme **ce
qui n'est pas joué**, pas ce qui serait faux.

Raccourcis : `ports.rs` = `crates/this-harness/src/ports.rs`, `session.rs` =
`crates/this-harness/src/session.rs`, `approval.rs` = `crates/this-agent/src/approval.rs`,
`masque.rs` = `crates/this-agent/src/masque.rs`, `substrate.rs` =
`crates/this-agent/src/substrate.rs`, `harness_graph.rs` = `src/storage/harness_graph.rs`,
`host.rs` = `src/server/host.rs`.

> **Une « preuve » dit ce que le test JOUE, pas ce que son nom promet.** L'erreur a déjà été
> commise deux fois sur ce document : un test nommé « révocation » qui ne révoquait qu'une
> des trois sources d'autorité, et une « map de digests » crue probante alors qu'elle vivait
> dans le document qu'elle prétendait contrôler. Les deux sont corrigées ci-dessous.

---

## 1. Ce qui existe déjà (cité, pas recréé)

- **`Permission`** (`ports.rs:327-337`) : la porte rendue à un transport de la **famille A**
  (le moteur exécute les outils pendant `stream`). Elle porte `allows` (la même décision que
  `gate`) et `record` (la même journalisation que `run_tool`).
- **`Toolbox::grants`** (`ports.rs:358-370`) : les droits **dynamiques** tenus par
  l'outillage, relus à chaque appel, prioritaires sur les droits figés.
- **L'approbation de composition** (#175) : `Harness::from_approved_doc` (`session.rs:1031`),
  `HarnessDoc::is_approved` (`approval.rs:147`), l'empreinte canonique (`approval.rs:118`).
- **Les droits actualisés** (#276) : `Harness::with_grants` (`session.rs:1067`) et
  `Harness::set_grants` (`session.rs:1240`) — ce dernier prend `&self`, donc une révocation
  peut tomber **pendant** un run —, relus à chaque appel (`session.rs:2370`, `session.rs:2919`).
- **`Toolboxes`** (`ports.rs:441`, `impl Toolbox for Toolboxes` `ports.rs:483`) : la
  composition des outils. Trois règles y font autorité pour l'annonce, les déclarations et le
  routage : la **première** part qui expose un nom gagne ; une `Declaration` n'est gardée que
  si sa part est la première à exposer le nom ; le registre `vus()` de la première part qui en
  tient un est **transmis** (`ports.rs:528`).
- **Le port `Recorder`** (`ports.rs:602`) : la boucle pousse chaque `Step` à l'implémentation.
  Il ne masque rien (voir §3(f) et la doc du trait).

Le point d'architecture central : **la décision d'autorisation est unique**. `gate_decision`
(`session.rs:2876`) est appelée par `gate` (hors flux, `session.rs:2370`) et par
`RunPorts::allows` (dans le flux, `session.rs:2919`). Une seule règle, deux portes : c'est ce
qui rend le refus identique des deux côtés (case b).

---

## 2. Tableau du propriétaire de chaque autorité (rejoué le 24/11/2026)

| Autorité réelle | Qui la détient (fichier:ligne) | Où il est écrit que le contenu du modèle ne peut pas la produire |
|---|---|---|
| **Accorder un outil** | l'hôte — `Harness::with_grants` (`session.rs:1067`), valeur `Grants` passée en code | `session.rs` (doc de `with_grants`) : « autorisations sont une décision de l'hôte » (`session.rs:1052`). Aucune ligne ne parse une sortie du modèle en `Grants`. Fail-closed testé : `grants.rs::un_outil_sans_aucune_autorisation_est_refuse_fail_closed`. Absence d'autorisation **installée** = rien n'est vérifié : `grants.rs::sans_autorisation_installee_tout_passe_comme_avant` — c'est une décision d'hôte explicite, pas un défaut. |
| **Révoquer un outil** | l'hôte — `set_grants` (`session.rs:1240`), lu à **chaque** appel (`session.rs:2370`, `session.rs:2919`) | Même séparation : une révocation est une valeur d'hôte. `Grants::allows` est fail-closed (`substrate.rs`). Testé : `grants.rs::l_outillage_prime_sur_les_autorisations_figees`, `demande_permission.rs::repondre_non_refuse_sans_redemander`, `conformite_autorisations.rs::cas3_ter_*` et `cas3_quater_*` (24/11). |
| **Accorder au nom d'une personne** | l'hôte, par `answer_permission` (`session.rs`, #286) | Une demande **n'accorde rien** : elle refuse et met en attente (`demande_permission.rs::un_outil_sans_grant_demande_la_personne_et_ne_s_execute_pas`). Le oui vient d'un appel d'API de l'hôte, jamais du texte du modèle. |
| **Approuver une composition** | l'hôte — sa liste `trusted` dans `from_approved_doc` (`session.rs:1031`) | `approval.rs:25-28` : le champ `approval` du document est une **affirmation**, écrite par n'importe qui (l'agent compris) ; seule `trusted` fait foi. Testé : `approbation.rs::approbation_affirmee_par_le_document_seul_ne_suffit_pas`, `document_modifie_apres_approbation_est_refuse`, et la révocation d'un approbateur : `this-agent/tests/approbation.rs::retirer_un_approbateur_de_la_liste_de_l_hote_revoque_son_approbation` (#363). |
| **Épingler le CONTENU d'un module** | l'hôte — `trusted[].digests`, confrontés aux `slot.digest` de l'empreinte approuvée (`approval.rs:147`, corrigé le 24/11) | La map `approval.digests` du document n'est **plus lue** par une décision : elle vivait hors empreinte et n'était confrontée à rien. Voir §4, décision (e). Tests : `un_contenu_qui_ne_correspond_pas_a_l_enregistrement_de_l_hote_est_refuse_et_nomme`, `vider_la_map_du_document_ne_retire_pas_l_epingle_de_contenu`. |
| **Exécuter un outil pendant le flux (famille A)** | le transport, sous la porte `Permission` — `ports.rs:327`, `gate_decision` (`session.rs:2876`) | Le refus est **renvoyé au moteur**, pas seulement noté. La porte est la même que `gate`, alimentée par le même état : le moteur ne peut pas contourner la décision. Testé : `famille_a.rs::un_refus_de_la_porte_remonte_au_moteur_et_journalise_la_meme_etape`, `conformite_autorisations.rs::cas2_*`. |
| **Déclarer un outil / son effet** | la première part qui expose le nom — `Toolboxes::specs` (`ports.rs:483`), `declarations` gardées par propriétaire | Un module ne peut pas déclarer `reads("install_module")` s'il n'est pas la première part à exposer `install_module`. Testé : `declarations.rs::declarations_usurp_reads_un_module_ne_declare_pas_install_module`, `declarations_une_part_ne_redeclare_pas_l_outil_d_une_autre` ; outil inventé : `adverse.rs::s3_outil_invente_hors_specs_est_refuse_par_le_harnais`, `s3_outil_invente_via_toolboxes_est_refuse_et_jamais_done` ; collision : `robustesse_ports.rs::un_outil_declare_deux_fois_n_est_annonce_qu_une_fois_et_la_premiere_part_gagne`. |
| **Porter les droits d'un outillage COMPOSÉ** | **non transmis** — `impl Toolbox for Toolboxes` (`ports.rs:483-560`) définit `specs`, `declarations`, `vus`, `call`, `call_with_cancel`, **pas `grants()`** | **Manque nommé** (chemin non qualifié, mesuré le 24/11) : la composition rend `None`, donc les droits dynamiques d'une part n'atteignent jamais la porte. `vus()` est transmis (`ports.rs:528`) avec exactement l'argument qui vaut ici — « sans ça, la composition qui sert les vrais hôtes rendrait `None` … resterait inerte en production ». Mesuré par deux tests jumeaux : `conformite_autorisations.rs::cas5_un_outillage_seul_porte_ses_grants_dynamiques` (seul : refus tenu) et `cas5_compose_le_meme_outillage_perd_ses_grants_manque_nomme` (composé : l'outil s'exécute). Correctif = transmettre, comme `vus()` — mais il change **qui décide** dans un hôte composé (les droits d'une part remplaceraient ceux de `with_grants`) : **RFC d'abord** (§5). |
| **Déléguer à un sous-agent** | **absent du harnais** — `session.rs:18-19` liste les sous-agents parmi « ce qui n'est pas repris » | Aucune garde à citer : l'autorité n'existe pas dans ce crate. **Manque** (case d). |
| **Lire un secret** | **absent du harnais** — le crate ne fait aucune I/O | Le masquage vit dans l'application (`masque.rs`), appliqué **au point d'écriture** par l'hôte (`harness_graph.rs`). Le port `Recorder` ne masque pas, et le contrat est écrit (`ports.rs:602`, §3(f)). |
| **Journaliser un run** | l'implémentation du `Recorder` | `harness_graph.rs:101` (`masque_installe`) et `host.rs:844` rendent le choix **constatable** : `ServerHost::harness_journal` (`host.rs:597`) installe le masque, `HarnessGraph::new` (`harness_graph.rs:76`) n'en installe aucun. Tests : `tests/journal_sans_masque.rs::un_recorder_sans_masque_ecrit_tel_quel_et_la_garde_le_dit`, `le_chemin_de_l_hote_masque_et_le_dit`. |

Deux constantes traversent ce tableau : (1) les valeurs d'autorisation (`Grants`, `Approval`,
`trusted`, `Declaration`) sont toutes des **valeurs d'hôte**, jamais produites en lisant la
sortie du modèle ; (2) le texte du modèle est, partout, traité comme **donnée non fiable** —
cloisonné (`this_agent::untrusted`), masqué (`masque.rs`), et les tentatives d'en faire un
conteneur ou un identifiant sont neutralisées (`adverse.rs::s4_*`, `adverse.rs::s5_*`). Il
n'existe **aucun** chemin de code qui lit la sortie du modèle pour en faire une autorisation.

---

## 3. Les sept cases d'acceptation (rejouées le 24/11/2026)

| # | Case d'acceptation | Verdict au 24/11 | Preuve / manque exact |
|---|---|---|---|
| a | Tableau du propriétaire de chaque autorité | **preuve** | §2, avec `fichier:ligne` par autorité, la garde « pas produit par le modèle » — ou son absence nommée. Une ligne y est un **manque** : la transmission des droits par `Toolboxes`. |
| b | Même refus côté boucle et côté moteur famille A, sans effet avant refus | **preuve** (renforcée le 24/11) | Décision unique `gate_decision` (`session.rs:2876`) partagée par `gate` (`session.rs:2370`) et `RunPorts::allows` (`session.rs:2919`). Sans effet avant refus : `approbation_guarded.rs::composition_approuvee_outil_non_autorise_refuse_avant_l_appel`, `grants.rs::un_outil_non_declare_est_refuse_et_jamais_appele`. Refus remonté au moteur : `famille_a.rs::un_refus_de_la_porte_remonte_au_moteur_et_journalise_la_meme_etape`, `conformite_autorisations.rs::cas2_moteur_interne_refuse_n_execute_jamais_l_outil`. **Égalité des deux portes dans le temps** : `conformite_autorisations.rs::cas3_quater_famille_a_voit_une_revocation_faite_pendant_son_flux` — avant le 24/11, la porte du flux gardait une **copie** des grants prise au démarrage du run et rendait `Allow` là où la boucle refusait (§4, décision b/d). |
| c | Composition : collision de noms, module installé/désinstallé, droits révoqués entre annonce et appel, grants dynamiques vs droits figés | **preuve pour les trois sources de révocation ; manque pour la transmission des droits par `Toolboxes`** | Collision et première-part-gagnante : `robustesse_ports.rs::un_outil_declare_deux_fois_n_est_annonce_qu_une_fois_et_la_premiere_part_gagne` ; module installé/désinstallé : table de routage reconstruite à chaque appel, traversée testée `declarations.rs::declarations_traversent_un_toolboxes`, `grants.rs::un_outil_qui_apparait_en_cours_de_session_n_est_pas_refuse`. **Révocation entre annonce et appel — les trois sources d'autorité sont maintenant jouées séparément** : déclaration (`cas3_droit_revoque_entre_annonce_et_appel_est_refuse`, #299), grant dynamique `Toolbox::grants` (`cas3_bis_grant_dynamique_retire_entre_annonce_et_appel_est_refuse`, #345), grant installé `set_grants` (`cas3_ter_grant_installe_retire_entre_annonce_et_appel_est_refuse`, 24/11) — chacune avec son témoin. Droits dynamiques vs figés : `grants.rs::l_outillage_prime_sur_les_autorisations_figees`, `l_outillage_autorise_sans_qu_on_appelle_with_grants`. **Manque** : la composition perd les droits dynamiques (`cas5_*` ci-dessus, §2). |
| d | Reprise ou changement de moteur ne rétablit pas un consentement révoqué ; l'enfant ne dépasse jamais l'enveloppe du parent | **preuve (changement de moteur, redémarrage, restauration) ; manque (enveloppe parent/enfant)** | Changement de moteur et de tour : `conformite_autorisations.rs::cas4_consentement_revoque_ne_repart_ni_au_nouveau_tour_ni_au_nouveau_moteur`. **Persistance** : `Snapshot.revoked` porte les révocations (`session.rs:1228`), réappliquées par `restore` (`session.rs:1289`), testées par `revocation_reprise.rs::une_revocation_ne_revient_pas_apres_redemarrage_et_restauration`, `une_restauration_n_octroie_jamais`, `temoin_sans_revocation_l_outil_s_execute_apres_redemarrage`. Une révocation faite pendant le flux d'un moteur famille A l'atteint désormais au prochain appel du **même** run (`cas3_quater_*`, 24/11). **Manque** : aucun sous-agent dans le crate (`session.rs:19`), donc aucune enveloppe parent/enfant à qualifier — à écrire avec la délégation, pas avant. |
| e | Approbation liée à un contenu immuable et à une autorité hors portée, preuve de révocation | **preuve (autorité hors portée, digests déclarés épinglés par l'hôte, révocation d'un approbateur) ; manque (octets réellement chargés)** | Autorité hors portée : `trusted` (`approval.rs`), testée `approbation.rs::approbation_affirmee_par_le_document_seul_ne_suffit_pas`, `document_modifie_apres_approbation_est_refuse`, `composition_non_approuvee_aucun_harnais_n_existe`. **Révocation d'un approbateur** : `this-agent/tests/approbation.rs::retirer_un_approbateur_de_la_liste_de_l_hote_revoque_son_approbation` (#363) — la composition inchangée est refusée dès que `trusted` le retire. **Contenu déclaré** : `slot.digest` entre dans l'empreinte (`c_est_le_digest_de_l_emplacement_dans_l_empreinte_qui_epingle_le_contenu_declare`) et l'hôte l'épingle par `trusted[].digests` (corrigé le 24/11 : `un_contenu_qui_ne_correspond_pas_a_l_enregistrement_de_l_hote_est_refuse_et_nomme`). **Manque** : aucun test ne hache les **octets d'un module réellement chargé** pour les confronter à `slot.digest` — la chaîne s'arrête au contenu *déclaré*. Et `from_approved_doc` (`session.rs:1031`) appelle `is_approved`, le contrôle **tolérant** (un emplacement sans digest passe), pas `is_approved_strict` : le « contenu immuable » de bout en bout reste à établir dans le chargeur de modules. |
| f | Secrets masqués avant journalisation/export ; isolation utilisateur/projet testée côté hôte | **preuve du CONTRAT au point d'écriture, constatable ; manque (masquage dans le port, isolation utilisateur/projet)** | Le harnais **ne masque pas** : c'est écrit dans la doc du port `Recorder` (`ports.rs:602`, section « Le MASQUE n'est pas dans ce port »), et c'est un contrat, pas un oubli. Le choix est **constatable** : `harness_graph.rs:101` / `host.rs:844` (`masque_installe`) ; `ServerHost::harness_journal` (`host.rs:597`) installe `this_agent::masque::masquer`, `HarnessGraph::new` (`harness_graph.rs:76`) n'installe rien. Tests : `tests/journal_sans_masque.rs::un_recorder_sans_masque_ecrit_tel_quel_et_la_garde_le_dit` (sans masque, la clé est écrite **et** la garde le dit) et `le_chemin_de_l_hote_masque_et_le_dit` (chemin de l'hôte, masqué au point d'écriture). Côté application, le masque a sa propre suite (`masque.rs`) et le magasin l'applique avant d'écrire (`this-agent/src/graph.rs::run_nodes_with`). **Manque** : aucune isolation utilisateur/projet n'est testée dans ce crate (affaire d'hôte), et le masquage n'est pas une propriété du port — un `Recorder` tiers qui n'installe pas de masque écrit en clair (choix visible, pas silencieux, mais non couvert). |
| g | Rôle de relecteur inter-équipe : accès et notifications révocables, aucune délégation de pouvoir ni auto-approbation | **manque** | `Approval.by` est une chaîne libre (`approval.rs`), sans rôle, sans notification, sans quorum. L'**auto-approbation** est refusée (l'affirmation du document ne suffit pas), mais le « relecteur révocable avec notifications » n'existe nulle part. À spécifier par RFC. |

---

## 4. Décisions prises dans ce ticket (24/11/2026)

Aucune de ces décisions ne change **qui** détient une autorité ; deux d'entre elles changent
**quand** une révocation prend effet, dans le sens d'un refus plus tôt.

### (e) La map `approval.digests` n'est plus une source de preuve — c'est `trusted.digests`

**Constat.** `is_approved` comparait `slot.digest` à `self.approval.digests`, c'est-à-dire à
une map portée par le **document**, hors empreinte (`fingerprint()` retire `approval`), jamais
confrontée à l'hôte. La vider désarmait le contrôle ; la faire diverger ne coûtait qu'un refus.
Ce n'était donc pas une preuve, et la case (e) reposait dessus.

**Décision.** Les digests de contenu comparés viennent de la liste `trusted` de l'hôte
(`Approval::digests` **côté hôte**), confrontés aux `slot.digest` du document. La map portée par
le document n'est lue par aucune décision ; elle reste sérialisée (documents d'hier) et
documentée comme une affirmation.

**Rationale.** L'empreinte approuvée couvre déjà `slot.digest`, donc le contenu déclaré était
déjà épinglé — mais l'hôte n'avait aucun moyen de dire « voici le contenu que MOI j'ai
approuvé » et de le voir respecté. C'est exactement ce que `trusted.digests` porte. Le contrôle
ne peut que **refuser davantage** : il n'accorde jamais un contenu que l'hôte n'a pas enregistré.

**Alternatives écartées.** (1) *Retirer le champ* : migration de format pour un gain nul, et
les documents d'hier perdraient un diagnostic utile. (2) *Garder la map du document comme
preuve* : c'est l'état d'avant, où le document décidait de ce qui l'épingle. (3) *Comparer les
deux maps* : deux affirmations ne font pas une preuve.

**Conséquence assumée.** Un hôte qui garderait **plusieurs** approbations du même approbateur
avec des digests différents pour le même module verrait un refus (fail-closed), et le motif
nomme l'emplacement. Aucun hôte de ce dépôt ne construit une telle liste (le paramètre `trusted`
n'est fourni que par l'appelant) ; le cas est écrit ici pour qu'il ne soit pas découvert en
production.

### (b/d) La porte du flux (famille A) lit l'état VIVANT des grants

**Constat.** `RunPorts` recevait `grants: Option<Grants>` par **copie** au démarrage du run
(`session.rs`, construction de `RunPorts`). Une révocation faite pendant le flux
(`set_grants`, ou une réponse de permission) n'atteignait donc la porte du moteur qu'au run
suivant : la boucle refusait, le moteur exécutait — au même instant, dans le même run. La
limite était écrite dans la doc de `set_grants`, mais la case (b) demande le **même** refus des
deux côtés.

**Décision.** `RunPorts` partage le `Shared<RefCell<Option<Grants>>>` du harnais et le relit à
chaque `allows` (copie hors du `RefCell` avant toute émission, pour qu'un hôte qui répond à une
permission depuis le rappel d'événement ne provoque pas de double emprunt).

**Rationale.** La règle n'est pas modifiée : c'est la même `gate_decision`, sur le même état,
par les deux portes. Seule la **fraîcheur** de la lecture change, et dans le sens du refus. Un
outil révoqué qui s'exécute encore une fois est précisément ce qu'une révocation promet
d'empêcher.

**Alternatives écartées.** (1) *Garder la copie et documenter la limite* : la limite devient un
contournement documenté — le moteur famille A devient le chemin le plus faible, et rien ne le
signale. (2) *Interdire `set_grants` pendant un run* : impossible à vérifier sans verrou, et
cela retirerait la révocation d'urgence, qui est le cas d'usage.

**Preuve.** `cas3_quater_famille_a_voit_une_revocation_faite_pendant_son_flux` (rouge avant,
vert après) et son témoin `cas3_quater_temoin_famille_a_sans_revocation_execute`. Mutation
mesurée : rétablir la copie (`Rc::new(RefCell::new(self.grants.borrow().clone()))`) laisse
**tous** les autres tests du crate verts et ne tue que ce test — c'est la définition du trou
silencieux que ce ticket devait fermer.

### (f) Le masquage est un contrat du point d'écriture, et il se constate

**Décision.** Le port `Recorder` **ne masque pas** (c'est désormais écrit dans sa doc), et
l'implémentation rend son état interrogeable (`masque_installe`). Rien n'est changé au
comportement : `HarnessGraph::new` continue de ne pas masquer, `ServerHost::harness_journal`
continue de masquer. Ce qui change, c'est qu'un journal sans masque **se constate** au lieu de
se deviner, et qu'un test le montre.

**Rationale.** Le masquage ne peut pas vivre dans le harnais (aucune I/O, aucune table de
motifs) ; il ne peut pas non plus être imposé à un `Recorder` tiers. Le seul contrat tenable
est : « le port pousse tel quel, l'implémentation masque, et l'hôte peut vérifier laquelle il a
branchée ». C'est ce que la case (f) demande — un choix visible, pas un oubli silencieux.

**Alternatives écartées.** (1) *Masquer dans la boucle* : ferait entrer une politique de texte
dans un crate qui n'en a aucune, et masquerait aussi ce que l'outil doit voir. (2) *Masquer par
défaut dans `HarnessGraph::new`* : changerait le comportement d'un constructeur public existant
sans decision d'hôte, et un masque imposé n'est pas un masque vérifié.

---

## 5. Ce qui reste ouvert (et pourquoi)

1. **`Toolboxes` ne transmet pas `grants()`** (§2, case c) — mesuré par `cas5_*`. Correctif
   d'une ligne, mais il **change qui décide** dans un hôte composé : à trancher par RFC. Deux
   options à examiner : transmettre la première part qui en tient un (règle de `vus()`), ou
   n'accepter les droits dynamiques que d'une part désignée par l'hôte à la construction.
2. **Enveloppe parent/enfant** (case d) : pas de sous-agent dans le crate. À écrire avec la
   délégation.
3. **Octets réellement chargés vs `slot.digest`** (case e) : la chaîne s'arrête au contenu
   *déclaré*. À établir dans le chargeur de modules, et à décider si `from_approved_doc` doit
   passer au contrôle strict (`is_approved_strict`).
4. **Isolation utilisateur/projet** (case f) : aucun test dans ce crate ; c'est une affaire
   d'hôte, et le masque ne dit rien de la séparation des journaux entre utilisateurs.
5. **Rôle de relecteur inter-équipe** (case g) : n'existe pas. RFC.
6. **Le port `Recorder` n'impose pas le masque** (case f) : le contrat est écrit et constatable,
   mais un `Recorder` tiers peut écrire en clair. C'est un choix visible ; il n'est pas couvert
   par une garantie du crate.

---

## 6. Historique des rejeux

| Date | Base | Ce qui a changé |
|---|---|---|
| 20/11 | `docs/harness-contrats` | Premier inventaire : 3 preuves / 4 manques. |
| 21/11 | `e8b2fd7` | #299 arrive **après** l'inventaire : le retrait d'une déclaration entre annonce et appel devient une preuve. Les verdicts « manque » sont rejoués. |
| 22/11 | `main` | Relevé du manque exact : aucun test ne retire un **grant** (seulement une déclaration) ; la map `approval.digests` n'est pas authentifiée. |
| 24/11 | `787a988` | #345 (grant dynamique), #363 (révocation d'un approbateur) et `revocation_reprise.rs` sont arrivés : les cases (c) et (d) changent de verdict. **Ce rejeu** ajoute `cas3_ter`, `cas3_quater` (+ témoins), `cas5` (mesure du manque `Toolboxes`), tranche la politique des digests (case e), rend le contrat de masquage explicite (case f) et nomme la limite restante du port. |

> Un verdict « manque » est daté : il doit être rejoué contre `main` avant d'être cité, sinon on
> qualifie un travail déjà fait — l'erreur symétrique de croire une issue corrigée sur la foi
> d'un document.
