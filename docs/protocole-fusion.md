# Protocole de fusion autonome

Ce document dit quand l'agent de développement peut fusionner une PR **sans** validation humaine, et quand il ne le peut **jamais**. Il s'applique à `this-rs/this`, branche `main`.

## 0. Faits qui fondent le protocole

Le ruleset `main` (id 9096464) exige **0 approbation humaine** et un seul check obligatoire, `CI gate`. Aucun acteur n'a le droit de le contourner. Les méthodes de fusion autorisées par le ruleset sont `merge`, `squash` et `rebase` ; le dépôt, lui, n'accepte que `squash`. GitHub laisserait donc fusionner dès que `CI gate` est vert. Le protocole ajoute ce que GitHub ne vérifie pas : une revue, une traçabilité, et les cas d'arrêt.

## 1. Conditions cumulatives de fusion autonome

L'agent fusionne **seulement si toutes** ces conditions sont vraies, sur la tête exacte de la PR :

1. **Non-brouillon**, et PR **sans** fichier sensible (§2).
2. **Tête verte** : `CI gate` en `success` sur le SHA de la tête. Un run `cancelled` d'une tête remplacée ne compte pas.
3. **Aucun `failure`** sur la tête courante (tous les checks terminés).
4. **Fusion propre** : `mergeStateStatus = CLEAN`, `mergeable = MERGEABLE`.
5. **Revue liée au SHA** : une revue est faite sur le diff `origin/main...<SHA de la tête>`, et son verdict est **publié en commentaire de la PR** avec ce SHA. Le verdict est : aucun point bloquant. Une revue faite par l'agent lui-même, ou sur un SHA antérieur, ne compte pas.
6. **Preuve** : la description dit ce qui est prouvé et comment (commandes, codes de sortie), et ce qui ne l'est pas.
7. **Squash uniquement**, avec `--match-head-commit <SHA>` : si la tête change entre la vérification et la fusion, la fusion est refusée et l'agent recommence depuis le §1.

## 2. Cas d'arrêt : ne JAMAIS fusionner en autonomie

L'agent s'arrête et le signale à la personne si la PR :

- touche aux **secrets, clés, droits, configuration du ruleset**, aux **workflows CI** (`.github/workflows/`), aux **scripts de publication**, aux **métadonnées de version** (`Cargo.toml`, tags, CHANGELOG de release) ;
- touche aux **instructions de l'agent** (`.claude/`, `CLAUDE.md`, `AGENTS.md`) ou à **ce protocole lui-même** ;
- **change une politique de fusion** ou une règle de gouvernance, quelle qu'elle soit ;
- vient d'une **autre personne** ou d'un **autre dépôt** (Atelier, par exemple), sauf demande explicite ;
- change un **comportement visible** : règle d'apprentissage du routeur, API publique, politique de repli ;
- contient un **point bloquant** non résolu dans la revue ;
- est **refusée** par le ruleset ou par GitHub, pour quelque raison que ce soit.

Une panne d'infrastructure (job sans runner ni étape, journaux `BlobNotFound`) est aussi un arrêt : relancer ne suffit pas, il faut regarder les réglages Actions.

**Jamais** : `--admin`, force-push sur `main`, suppression de règle, contournement de `CI gate`, et **retenter la fusion avec une autre méthode** (`--merge`, `--rebase`) après un refus.

## 3. Procédure

1. Vérifier les conditions du §1 sur la tête exacte.
2. Publier la revue (§1.5), ou la faire si elle manque.
3. `gh pr merge <n> --squash --match-head-commit <SHA>`. Tout refus est un arrêt (§2).
4. Vérifier que `origin/main` contient le commit attendu.
5. Commenter l'issue concernée : ce qui est fusionné, ce qui reste.
6. Consigner la fusion dans l'orchestrateur, avec le SHA de tête et le SHA fusionné.

## 4. Limites

- **La revue par un agent qui a été lancé par l'agent auteur n'est pas indépendante au sens strict.** Elle reste utile pour attraper des erreurs, mais elle ne remplace pas un regard humain. C'est pourquoi les changements de politique (§2) restent à la personne.
- Une CI verte prouve seulement ce qu'elle exécute. La couverture réelle d'une feature se vérifie séparément (`ci.yml`, job de couverture).
- Le protocole est révocable : la personne peut demander l'arrêt des fusions autonomes à tout moment, et l'agent s'y conforme immédiatement.

## 5. Historique

- 23/11/2026 : premier protocole. Première version refusée par revue indépendante (revue liée au SHA absente, fusion relançable avec une autre méthode, fichiers sensibles oubliés, protocole non protégé). Corrigée dans cette version.
