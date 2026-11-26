# Correction `AutoriteK` — allocation atomique, verrou, domaine

base auditée  = `a583633faf2a29c8a641cd4723788636dde84d49` (candidat G, `chore/lot-g-convergence-ab`)
worktree      = `/Users/triviere/projects/this-rs/_wt-k-alloc` (détaché, isolé)
worktree base = `/Users/triviere/projects/this-rs/_wt-k-base` (détaché, même SHA, témoin seul)
toolchain     = rustc 1.100.0 (0ef1bc386 2026-11-09) — `std::fs::File::lock` (stabilisé 1.89)
cible         = `CARGO_TARGET_DIR=/Users/triviere/projects/this-rs/_wt-k-alloc-target`
features      = `obrain` (+ `objects-toolbox` pour `concurrence_g1g5`)
checkout partagé `this` : NON utilisé, `git status` vide avant et après.

## 1. Le défaut, reproduit (DÉMONTRÉ)

`a583633f` lisait l'état durable **à l'ouverture** (`AutoriteK::ouvrir`) et le
persistait à l'émission, sans aucun verrou. Deux instances — ou deux processus —
lisaient donc le même compteur, calculaient la même identité, publiaient.

Témoin `tests/k_collision_concurrente.rs` (API publique commune aux DEUX arbres,
`ouvrir`/`emettre` seulement : le même code doit échouer sur l'un et passer sur
l'autre). Interleaving **déterministe** : deux processus ouvrent (lisent) avant
une barrière de fichiers, puis émettent.

| arbre | K(a) | K(b) | état durable | verdict |
|---|---|---|---|---|
| base `a583633f` | 1 | 1 | 2 | **COLLISION** (`bench/k-alloc/base-collision.log`, EXIT=101) |
| corrigé | 1 | 2 | 3 | distincts (`bench/k-alloc/fix-collision.log`, EXIT=0) |

Contention 4 processus × 20 émissions, même fichier :

| arbre | K émis | K distincts |
|---|---|---|
| base | 80 | **20** (60 doublons) |
| corrigé | 80 | **80** |

Sur la base, chaque processus ré-émet sa propre suite 1..20 : la collision est
massive et systématique, pas une course rare.

## 2. La correction (RELU, `src/autorite_k.rs`)

1. **Allocation atomique** : `emettre_observe` prend un verrou exclusif
   (`File::lock`, sémantique `flock`, bloquant) puis, **sous ce verrou** : relit
   l'état, contrôle la régression, contrôle la saturation, persiste la valeur
   suivante, met à jour la vue, publie. Le verrou couvre TOUTE la décision ; la
   lecture n'est plus faite à l'ouverture (`prochain()` est une vue cachée,
   `prochain_durable()` relit sous verrou).
2. **Le verrou ne porte pas sur l'objet renommé** : fichier DISTINCT
   `<état>.verrou`. `ecrire_durable` renomme `<état>` à chaque émission ; un
   verrou posé sur `<état>` serait un autre verrou à chaque fois. Mesuré par
   inodes (`verrou_porte_sur_un_objet_distinct_de_l_etat_renomme`).
3. **Persistance AVANT publication** : `ecrire_durable` (fsync fichier → rename →
   fsync répertoire) avant que le K ne soit rendu. Ordre exposé et mesuré :
   `VerrouPris → EtatLu → SuivantPersiste → Publie`
   (`ordre_mesure_verrou_puis_lecture_puis_persistance_puis_publication`) ; à
   l'étape `SuivantPersiste`, l'état sur disque vaut déjà k+1.
4. **Création initiale sous verrou** : `ouvrir` décide « fichier absent → 1 »
   SOUS le verrou, sinon deux ouvreurs concurrents pouvaient écraser un compteur
   déjà avancé (réinitialisation silencieuse par course).
5. **Refus nommés, sans réinitialisation** : état tronqué, nul, disparu sous
   verrou, régressé → `ErrorKind::InvalidData`, octets jamais réécrits, fichier
   jamais recréé. Saturation `u128::MAX` → refus, aucun repli (ni 0, ni 1).
6. **Le noyau libère le verrou** : un processus mort sans destructeur
   (`process::exit`) ne laisse pas de verrou orphelin (mesuré).
7. Aucune modification de `src/reconciliation.rs` : le codec OBRR, la
   quarantaine V1 et `ecrire_durable` sont INCHANGÉS — c'est la garantie de
   non-régression du format.

## 3. Ce qui est établi, et ce qui ne l'est pas (six notions distinctes)

| notion | établi ici | borne mesurée |
|---|---|---|
| atomicité d'allocation | verrou exclusif tenu de la lecture à la publication, entre instances et entre processus | verrou **consultatif** : local au système de fichiers ; ne protège pas d'un écrivain qui l'ignore ; deux COPIES de l'état = deux autorités |
| persistance | `fsync` fichier avant `rename`, `fsync` répertoire après ; survit à la mort du processus (mesuré : K brûlé, état k+1) | survie à une coupure d'alimentation NON PROUVÉE (`fsync` = condition nécessaire) |
| identité d'autorité | K = u128, distinct par opération, stable par tentative (réutilisé, jamais ré-émis) | n'atteste aucune provenance |
| domaine canonique | LE fichier d'état est le domaine | **non-unicité globale MESURÉE** : deux fichiers distincts émettent les MÊMES K (1 et 1) |
| continuité | redémarrage : le compteur reprend ; un K brûlé n'est jamais réattribué (saut accepté) ; régression refusée dans un processus VIVANT | une RESTAURATION du fichier (retour arrière de snapshot) n'est pas détectable par une instance fraîche — mesuré, et nommé comme limite |
| domaine backend | rien : `AutoriteK` ne sait pas quel backend produit la référence | `ReferenceTentative` ne prouve pas son origine (deux bases partageant `writer.id` produisent des références identiques) : ce domaine exige une autorité INDÉPENDANTE |

**Cette correction n'est PAS une résolution universelle de l'identité.** Elle rend
l'ALLOCATION atomique dans un domaine de fichiers. Un verrou local n'empêche pas
la réutilisation d'un K après restauration historique du compteur, ni la
collision entre deux domaines de stockage distincts.

## 4. Témoins (DÉMONTRÉ : chacun échoue si la propriété manque)

`tests/k_allocation_atomique.rs` (9) :
1. `collision_concurrente_interleaving_deterministe` — 2 processus, barrière
2. `contention_multiprocessus_sans_doublon` — 4 × 20, aucun doublon
3. `interruption_apres_persistance_ne_reattribue_jamais` — mort entre persistance
   et publication, K brûlé, verrou non orphelin, pas de réutilisation
4. `corruption_refusee_sans_reinitialisation` — tronqué / nul / tardif / disparu
5. `compteur_regresse_refuse_dans_un_processus_vivant` — restauration sous les pieds
6. `verrou_porte_sur_un_objet_distinct_de_l_etat_renomme` — inodes (unix)
7. `obrr_v1_string_jamais_reinterprete_en_u128` — V1 « 7 » en quarantaine, sur
   disque aussi ; `decoder` refuse `VersionInconnue(1)` ; V2 K=7 courant
   (contrôle positif) ; corruption V2 nommée
8. `domaine_canonique_deux_fichiers_deux_autorites` — non-unicité mesurée
9. `vue_perimee_vs_etat_durable_relu_sous_verrou` — la vue d'ouverture est
   périmée, l'état réel relu sous verrou

`tests/k_collision_concurrente.rs` (2) : le même code sur les deux arbres.

`src/autorite_k.rs` (5 unités) : durabilité/monotonie, corruption, deux instances
même processus, ordre mesuré, saturation.

## 5. Contrôles positifs et non-régression (MESURÉ)

Voir `final-suites.log` (une exécution, un seul arbre) :
`k_allocation_atomique` 9 · `k_collision_concurrente` 2 · `k_durable_autorite` 8 ·
`concurrence_g1g5` · `durabilite_obrr` 15 · `h3_v2_chaine_autorite` 7 ·
`h3_v2_cycle_harness` 5 · `reconciliation_composant` 2 · lib `autorite_k` 5.

## 6. Registre épistémique

- **DÉMONTRÉ** : collision à l'interleaving imposé sur la base (`K(a)=1 K(b)=1`) ;
  absence de collision après correctif ; 80/80 distincts ; ordre
  verrou→lecture→persistance→publication ; inode du verrou stable contre inode de
  l'état qui change ; V1 String non réinterprété ; K brûlé non réattribué.
- **MESURÉ** : 2 processus, 4 processus × 20, compteur final `émissions + 1`,
  `ECART` doublons base = 60/80.
- **RELU** : `src/autorite_k.rs` (les 7 points ci-dessus), `reconciliation.rs`
  inchangé (format V2, `admissibilite`, quarantaine V1).
- **INTERPRÉTÉ** : la portée du verrou sur un montage réseau sans sémantique de
  verrou (non testé ici, pas de NFS sous la main).
- **NON QUALIFIÉ** : coupure d'alimentation réelle ; restauration snapshot sur une
  instance fraîche au-delà de ce qui est mesuré ; collision entre deux domaines
  backend distincts.

## 7. Défaut connexe, NON corrigé ici (hors périmètre, nommé)

`tests/concurrence_g1g5.rs` porte
`#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]` : lancé avec la
seule feature `obrain`, il compile à **0 test**. Constat de relu, pas une
correction de cette tâche.
