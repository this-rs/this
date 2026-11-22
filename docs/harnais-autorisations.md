# Autorisations du harnais — suite de conformité entre hôtes, outils composés, moteurs délégués (issue #287)

Ce document est un **inventaire de qualification sécurité**, pas la démonstration d'une
vulnérabilité. Chaque chemin qui n'est pas couvert par un test ou par une garde écrite
est nommé « non qualifié », et ce qui manque renvoie à une RFC, jamais à une modification
de règle (le ticket interdit de proposer des changements de permission). Il ne recrée pas
ce qui existe déjà : il le cite et dit *pourquoi* c'est une preuve.

Mesuré sur `docs/harness-contrats` (parent `origin/main` = `bd387e8`). `ports.rs` désigne
`crates/this-harness/src/ports.rs`, `session.rs` désigne `crates/this-harness/src/session.rs`,
`approval.rs` désigne `crates/this-agent/src/approval.rs`, `masque.rs` désigne
`crates/this-agent/src/masque.rs`, `substrate.rs` désigne `crates/this-agent/src/substrate.rs`.

**Rejoué le 21/11/2026** sur `origin/main` = `e8b2fd7` : des verdicts « manque » de la §3
étaient périmés, parce que #299 (conformité des refus d'autorisation) et les tests
d'approbation stricte ont atterri **après** le premier inventaire. Les cases (c), (d), (e) et
(f) portent désormais la preuve exacte, **et le manque exact** : une revue indépendante a
montré que plusieurs « preuves » de révocation ne couvraient qu'une partie du scénario (voir
les cases (c), (d) et (e)). Une « preuve » ici dit ce que le test joue, pas ce que son nom
promet.

---

## 1. Ce qui existe déjà (cité, pas recréé)

- **`Permission`** (`ports.rs:167-198`) : la porte rendue à un transport de la **famille A**
  (le moteur exécute les outils pendant `stream`). Elle porte `allows` (la même décision
  que `gate`) et `record` (la même journalisation que `run_tool`).
- **`Toolbox::grants`** (`ports.rs:219-231`) : les droits **dynamiques** tenus par
  l'outillage, relus à chaque appel et prioritaires sur les droits figés.
- **L'approbation de composition** : `Harness::from_approved_doc` (`session.rs:925-109`),
  `HarnessDoc::is_approved` (`approval.rs:138-196`), l'empreinte canonique
  (`approval.rs:111-123`). Le numéro #175 est le contrat d'origine.
- **Les droits actualisés** (#276) : `Harness::with_grants` (`session.rs:931-134`) et la
  relecture à chaque appel (`session.rs:1831-1839`).
- **`Toolboxes`** (`ports.rs:291-397`) : la composition des déclarations. Deux règles y
  font autorité : la **première** part qui expose un nom gagne pour l'annonce
  (`ports.rs:339-346`) et pour le routage (`ports.rs:384-396`), et une `Declaration`
  d'effet n'est gardée que si sa part est la première à exposer le nom
  (`ports.rs:348-371`).

Le point d'architecture central : la **décision d'autorisation est unique**. `gate_decision`
(`session.rs:2074-2106`) est appelée par `gate` (hors flux, `session.rs:1831-1839`) et par
`RunPorts::allows` (dans le flux, `session.rs:2107-2113`). Une seule règle, deux portes :
c'est ce qui rend le refus identique des deux côtés (case b).

---

## 2. Tableau du propriétaire de chaque autorité

| Autorité réelle | Qui la détient (fichier:ligne) | Où est-il écrit que le contenu du modèle ne peut pas la produire |
|---|---|---|
| **Accorder un outil** | l'hôte — `Harness::with_grants` (`session.rs:931-134`), valeur `Grants` passée en code | `session.rs:115-118` : « des autorisations sont une décision de l'hôte (qui a le droit, accordé par qui) ». La `Grants` n'est **jamais** lue dans le transcript : aucune ligne ne parse la sortie du modèle en `Grants`. Fail-closed testé : `grants.rs::un_outil_sans_aucune_autorisation_est_refuse_fail_closed`. |
| **Révoquer un outil** | l'hôte/outillage — `Toolbox::grants` relu à chaque appel (`session.rs:1831-1839`, `ports.rs:226` « fait foi à chaque appel ») | Même séparation : la révocation est une valeur d'hôte. `Grants::allows` est fail-closed et nomme « explicitement retiré » (`substrate.rs:329-340`). Testé : `grants.rs::l_outillage_prime_sur_les_autorisations_figees`. |
| **Approuver une composition** | l'hôte — sa liste `trusted` dans `from_approved_doc` (`session.rs:925-109`) | `approval.rs:22-25` : « le champ `approval` du document est une **affirmation**, écrite par n'importe qui (l'agent compris). Seule la liste `trusted`, tenue par l'hôte, fait foi. » Testé : `approbation.rs::approbation_affirmee_par_le_document_seul_ne_suffit_pas`. |
| **Exécuter un outil pendant le flux (famille A)** | le transport, sous la porte `Permission` — `ports.rs:167-198`, `gate_decision` (`session.rs:2074-2106`) | Le refus est **renvoyé au moteur**, pas seulement noté (`ports.rs:174-177`). La porte est la même que `gate` : le moteur ne peut pas contourner la décision en exécutant lui-même. Testé : `famille_a.rs::un_refus_de_la_porte_remonte_au_moteur_et_journalise_la_meme_etape`. |
| **Déclarer un outil / son effet** | la première part qui expose le nom — `Toolboxes::specs` (`ports.rs:339-346`), `declarations` gardées par propriétaire (`ports.rs:348-371`) | Un module ne peut pas déclarer `reads("install_module")` s'il n'est pas la première part à exposer `install_module` (`ports.rs:348-352`). Un outil inventé hors `specs` est refusé sans appel : `adverse.rs::s3_outil_invente_hors_specs_est_refuse_par_le_harnais`. Usurpation testée : `declarations.rs::declarations_usurp_reads_un_module_ne_declare_pas_install_module`. |
| **Déléguer à un sous-agent** | **absent du harnais** — `session.rs:19` liste les sous-agents parmi « ce qui n'est **pas** repris » | Aucune garde à citer : l'autorité n'existe pas dans ce crate. Le routage *connaît* la classe de tour « sous-agent » (`signature.rs:27`, `mode_auto.rs:487`) mais il n'y a aucune délégation, donc aucune enveloppe parent/enfant. **Manque** (case d). |
| **Lire un secret** | **absent du harnais** — le crate ne fait aucune I/O (`lib.rs:1-11`) | Le masquage vit hors du harnais, dans `this-agent` (`masque.rs`), appliqué par le magasin (`graph.rs:481, 498, 525, 543`). Le harnais pousse des `Step` au `Recorder` sans masquer lui-même. **Non qualifié côté harnais** (case f). |

Deux constantes traversent ce tableau : (1) les valeurs d'autorisation (`Grants`, `Approval`,
`trusted`, `Declaration`) sont toutes des **valeurs d'hôte**, jamais produites en lisant la
sortie du modèle ; (2) le texte du modèle est, partout, traité comme **donnée non fiable** —
cloisonné (`this_agent::untrusted`), masqué (`masque.rs`), et les tentatives d'en faire un
conteneur ou un identifiant sont neutralisées (`adverse.rs::s4_note_hostile_wrap_puis_neutralize_rien_ne_s_echappe`,
`adverse.rs::s5_un_id_forge_dans_une_note_hostile_compte_comme_vu`). Il n'existe **aucun**
chemin de code qui lit la sortie du modèle pour en faire une autorisation.

---

## 3. Les sept cases d'acceptation

| # | Case d'acceptation | Verdict | Preuve / manque |
|---|---|---|---|
| a | Tableau du propriétaire de chaque autorité | **preuve** | §2 ci-dessus, avec `fichier:ligne` pour chaque autorité et, pour chacune, où la garde « pas produit par le modèle » est écrite — ou son absence nommée. |
| b | Même refus côté boucle et côté moteur famille A, sans effet avant refus | **preuve** | Décision unique `gate_decision` (`session.rs:2074-2106`) partagée par `gate` et `RunPorts::allows` (`session.rs:2107-2113`). Sans effet avant refus : `approbation_guarded.rs::composition_approuvee_outil_non_autorise_refuse_avant_l_appel` (l'appel n'atteint jamais la toolbox), `grants.rs::un_outil_non_declare_est_refuse_et_jamais_appele`. Refus remonté au moteur : `famille_a.rs::un_refus_de_la_porte_remonte_au_moteur_et_journalise_la_meme_etape`. |
| c | Composition : collision de noms, module installé/désinstallé, déclaration retirée entre annonce et appel, grants dynamiques vs droits figés | **preuve (déclaration retirée) ; manque (retrait d'un grant entre annonce et appel)** | Collision : `robustesse_ports.rs::un_outil_declare_deux_fois_n_est_annonce_qu_une_fois_et_la_premiere_part_gagne` + `ports.rs:339-346`. Module installé/désinstallé : table de routage reconstruite à chaque appel (`ports.rs:288-290`), traversée testée `declarations.rs::declarations_traversent_un_toolboxes`. Droits dynamiques vs figés : `grants.rs::l_outillage_prime_sur_les_autorisations_figees`. **Séquence « annonce → retrait de la déclaration → appel » : jouée depuis #299** — `conformite_autorisations.rs::cas3_droit_revoque_entre_annonce_et_appel_est_refuse` (le moteur lit `request.tools()`, **retire la déclaration d'effet** pendant son flux, puis réclame l'outil : refus, aucun effet). **Ce que ce test ne joue PAS** : le retrait d'un *grant* (`toolbox.grants()` ou `Grants` figés) entre l'annonce et l'appel — une mutation de `gate_decision` qui ignorerait les grants dynamiques laisse `cas3` et `cas4` verts (mesurée à la revue, 21/11) avec son témoin `cas3_temoin_sans_revocation_l_outil_est_execute`. Collision et propriété des déclarations : `declarations.rs::declarations_une_part_ne_redeclare_pas_l_outil_d_une_autre`, `declarations_usurp_reads_meme_par_la_part_qui_l_expose`. |
| d | Reprise ou changement de moteur ne rétablit pas un consentement révoqué ; l'enfant ne dépasse jamais l'enveloppe du parent | **preuve partielle (un `Grants` refusé tient pour un nouveau moteur ou un nouveau tour) ; manque (propagation d'une révocation, persistance, enveloppe parent/enfant)** | Changement de moteur : `conformite_autorisations.rs::cas4_consentement_revoque_ne_repart_ni_au_nouveau_tour_ni_au_nouveau_moteur` (#299) — il construit un `Grants` déjà refusé et prouve qu'il refuse ; il **ne prouve ni une révocation propagée, ni sa persistance**. Reprise : les `grants` vivent dans le `Harness`, pas dans le `State` (`snapshot`/`restore`, `session.rs:1114, 2477`). Portée exacte, relue le 22/11 : ils survivent à un `restore` **sur la même instance déjà configurée** (`with_grants`), et c'est tout. `Snapshot` ne porte aucun grant (`event.rs`, champs `version`/`transcript`/`pending`), et un `Harness` neuf démarre avec `grants: None` (`session.rs:899`) : après un redémarrage, **l'hôte doit réinstaller les grants lui-même**, et une révocation faite avant l'arrêt **n'est pas persistée** par le harnais — si l'hôte réinstalle sa liste d'avant, le consentement révoqué repart. **Aucun test ne joue ni la survie ni la perte.** Enveloppe parent/enfant : pas de sous-agent dans le harnais (`session.rs:19`), rien à qualifier ici. |
| e | Approbation liée à un contenu immuable et à une autorité hors portée de l'agent, preuve de révocation | **preuve (autorité hors portée ; digests DÉCLARÉS cohérents) ; manque (contenu réellement chargé ≠ digest : non vérifié ; retrait d'un approbateur)** | Autorité hors portée : liste `trusted` de l'hôte (`approval.rs:22-25`), testée `approbation.rs::approbation_affirmee_par_le_document_seul_ne_suffit_pas` et `approbation.rs::document_modifie_apres_approbation_est_refuse`. **Contenu immuable : testé sur des digests DÉCLARÉS, pas sur des octets.** `is_approved` (`approval.rs:138-160`) compare `slot.digest` — une chaîne écrite dans le document — à `approval.digests` — une chaîne écrite par l'approbateur ; **aucun des tests cités ne hache les octets d'un module chargé** pour les confronter à ce digest. Et `Harness::from_approved_doc` (`session.rs:952`) appelle `is_approved`, le contrôle **tolérant** (un emplacement sans digest passe), pas `is_approved_strict` — qui n'a aucun appelant de production. Le « contenu immuable » de bout en bout (octets réellement chargés = digest approuvé) **reste un manque** tant qu'il n'est pas établi ailleurs (chargeur de modules). Tests existants : `this-agent/tests/approbation.rs::is_approved_strict_refuse_un_emplacement_sans_digest` et `is_approved_strict_accepte_un_document_entierement_digeste`, avec `doc_avec_digests` (`this-agent/tests/approbation.rs:44`) ; côté contournement : `le_strict_refuse_un_digest_vide_comme_un_digest_absent` et `le_strict_accepte_un_digest_reel` (`this-agent/tests/contournement.rs:85,110`). **Révocation d'un approbateur : NON testée.** Les tests cités prouvent seulement qu'une approbation affirmée par le document seul ne suffit pas et qu'aucun harnais n'existe sans approbation reconnue (`approbation.rs::approbation_affirmee_par_le_document_seul_ne_suffit_pas`, `composition_non_approuvee_aucun_harnais_n_existe`) ; aucun ne retire un approbateur de `trusted`. L'ancienne mention « manque » était juste pour ce point. |
| f | Secrets masqués avant journalisation/export ; isolation utilisateur/projet testée côté hôte | **manque (dans le harnais) ; preuve hors harnais, sur le chemin configuré (`harness_journal`) seulement** | Le masquage est dans `this-agent` : `masque.rs` (page avant envoi, magasin avant écriture — `masque.rs:8-9`), appliqué par le magasin (`graph.rs:481, 498, 525, 543`), avec sa suite de tests (`masque.rs:376` sq). **Le PORT ne masque pas** : la boucle pousse des `Step` au `Recorder` tels quels. L'implémentation de **ce** dépôt masque avant d'écrire **quand le masque est installé** : `this-agent/src/graph.rs::run_nodes_with` (doc `:305-315`, tests `l_identifiant_du_run_passe_par_le_masque`, `le_masque_passe_avant_la_borne_de_l_extrait`). Or il dépend du chemin de construction : `ServerHost::harness_journal` (`src/server/host.rs:597-600`) installe `with_masque(masquer)`, mais `HarnessGraph::new` démarre avec `masque: None` (`src/storage/harness_graph.rs:72-75`) et `HostRecorder::new` accepte un `HarnessGraph` quel qu'il soit (`host.rs:830`). Un hôte qui construit le journal autrement que par `harness_journal` **et n'installe pas de masque** (`with_masque` est publique : il peut le faire lui-même) écrit **sans masque, sans erreur ni avertissement**. Le masquage est donc une preuve **sur le chemin configuré**, pas une propriété du journal. La conséquence à retenir : **un hôte qui écrit son propre `Recorder` doit masquer lui-même** ; le contrat du port ne le fait pas pour lui. Les `Event` poussés à l'interface ne sont pas masqués par le cœur non plus (la page masque avant d'envoyer). L'isolation utilisateur/projet n'a **aucun** test dans le crate (affaire d'hôte). |
| g | Rôle de relecteur inter-équipe : accès et notifications révocables, aucune délégation de pouvoir ni auto-approbation | **manque** | `Approval.by` est une chaîne libre (`approval.rs:64-66`) : pas de rôle de relecteur, pas de notification, pas de quorum. L'**auto-approbation** est bien refusée (l'affirmation du document ne suffit pas, `approval.rs:22-25`), mais le « relecteur révocable avec notifications » n'existe nulle part. |

---

## 4. Ce que cet inventaire ne change pas

Aucune règle de permission n'est modifiée par ce document. Les écarts qui **restent** des
manques — l'enveloppe parent/enfant des sous-agents et la reprise jouée par un test (case d),
le masquage au niveau du **port** `Recorder` et l'isolation utilisateur/projet (case f), le
rôle de relecteur (case g) — sont des **manques de qualification**, à adresser par une RFC
(ou un ticket de test dédié), pas par une modification de règle ici.

> **Mise à jour du 21/11** : des cases étaient écrites « manque » sur la foi de l'état du
> 20/11 : le contenu immuable (e) et la séquence « déclaration retirée entre annonce et
> appel » (c) sont devenus des preuves. #299 (tests de conformité des refus) et les tests
> d'approbation stricte de `this-agent` sont arrivés **après** cet inventaire. **Restent des
> manques, nommés** : le retrait d'un *grant* entre annonce et appel, la propagation et la
> persistance d'une révocation, et le retrait d'un approbateur. Un verdict « manque » est daté :
> il doit être rejoué contre `main` avant d'être cité, sinon on qualifie un travail déjà
> fait — l'erreur symétrique de croire une issue corrigée sur la foi d'un document.
