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
| c | Composition : collision de noms, module installé/désinstallé, droits révoqués entre annonce et appel, grants dynamiques vs droits figés | **preuve (mécanisme), manque d'un test nommé pour la séquence annonce→révocation** | Collision : `robustesse_ports.rs::un_outil_declare_deux_fois_n_est_annonce_qu_une_fois_et_la_premiere_part_gagne` + `ports.rs:339-346`. Module installé/désinstallé : table de routage reconstruite à chaque appel (`ports.rs:288-290`), traversée testée `declarations.rs::declarations_traversent_un_toolboxes`. Droits dynamiques vs figés : `grants.rs::l_outillage_prime_sur_les_autorisations_figees`. **Aucun test ne joue explicitement « annonce → révocation → appel »** : la relecture à chaque appel est prouvée, la séquence complète non. |
| d | Reprise ou changement de moteur ne rétablit pas un consentement révoqué ; l'enfant ne dépasse jamais l'enveloppe du parent | **manque** | Pas de sous-agent dans le harnais (`session.rs:19`) : l'« enveloppe du parent » n'existe pas à qualifier ici. La reprise (`snapshot`/`restore`, `session.rs:1114, 2477`) ne porte **pas** les `grants` (champ du `Harness`, pas du `State`), donc ils survivent structurellement — mais **aucun test** ne le vérifie. |
| e | Approbation liée à un contenu immuable et à une autorité hors portée de l'agent, preuve de révocation | **preuve (autorité hors portée), manque (contenu immuable + révocation)** | Autorité hors portée : liste `trusted` de l'hôte (`approval.rs:22-25`), testée `approbation.rs::approbation_affirmee_par_le_document_seul_ne_suffit_pas` et `approbation.rs::document_modifie_apres_approbation_est_refuse`. **Contenu immuable : non testé** — `Approval.digests` et `is_approved_strict` existent (`approval.rs:64-72, 207`) mais aucun test ne les épingle (les tests d'approbation utilisent tous `digests: Default::default()`). **Révocation : non testée** — retirer un approbateur de `trusted` refuse structurellement (chemin 4/5 de `is_approved`), mais aucun test ne le joue. |
| f | Secrets masqués avant journalisation/export ; isolation utilisateur/projet testée côté hôte | **manque (dans le harnais) ; preuve hors harnais** | Le masquage est dans `this-agent` : `masque.rs` (page avant envoi, magasin avant écriture — `masque.rs:8-9`), appliqué par le magasin (`graph.rs:481, 498, 525, 543`), avec sa suite de tests (`masque.rs:376` sq). **Le harnais ne masque pas lui-même** : il pousse des `Step` au `Recorder` tels quels. L'isolation utilisateur/projet n'a **aucun** test dans le crate (c'est une affaire d'hôte). |
| g | Rôle de relecteur inter-équipe : accès et notifications révocables, aucune délégation de pouvoir ni auto-approbation | **manque** | `Approval.by` est une chaîne libre (`approval.rs:64-66`) : pas de rôle de relecteur, pas de notification, pas de quorum. L'**auto-approbation** est bien refusée (l'affirmation du document ne suffit pas, `approval.rs:22-25`), mais le « relecteur révocable avec notifications » n'existe nulle part. |

---

## 4. Ce que cet inventaire ne change pas

Aucune règle de permission n'est modifiée par ce document. Les écarts nommés en « manque »
— l'enveloppe parent/enfant des sous-agents (case d), l'épinglage du contenu immuable et la
révocation explicite (case e), le masquage au niveau du `Recorder` et l'isolation
utilisateur/projet (case f), le rôle de relecteur (case g) — sont des **manques de
qualification**, à adresser par une RFC (ou un ticket de test dédié), pas par une
modification de règle ici.
