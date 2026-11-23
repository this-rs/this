# Protocole de fusion autonome

Ce document dit quand une PR peut être fusionnée **sans** attendre une validation humaine, et quand elle ne le peut **jamais**. Il s'applique à `this-rs/this` (branche `main`), et à toute PR ouverte par l'agent de développement sur ce dépôt.

Le ruleset `main` exige aujourd'hui **0 approbation humaine** et le seul check obligatoire `CI gate`. GitHub autoriserait donc la fusion dès que la CI est verte. Ce protocole ajoute ce que GitHub ne vérifie pas : la revue indépendante, la traçabilité, et les cas où l'on s'arrête.

## 1. Conditions de fusion autonome

Une PR est fusionnée par l'agent **seulement si toutes** ces conditions sont vraies, au moment de la fusion :

1. **PR non-brouillon**, ouverte par l'agent sur **sa propre branche** (jamais une branche d'un autre auteur sans demande explicite, jamais `main` en direct).
2. **Tête verte** : le check `CI gate` est en `success` sur le SHA exact de la tête de la PR. Les jobs `cancelled` d'un run remplacé par un push plus récent ne comptent pas ; seul le run de la tête courante compte.
3. **Aucun échec** sur la tête courante (tous les checks terminés, aucun `failure`).
4. **Fusion propre** : `mergeStateStatus = CLEAN`, `mergeable = MERGEABLE`. Un conflit se règle **sur la branche de la PR**, puis la CI doit être re-verte sur la nouvelle tête avant la fusion.
5. **Revue indépendante** : un agent frais, sans le contexte de l'auteur, lit le diff réel et rend un verdict. Aucun point **bloquant** ne reste ouvert. Les points non bloquants sont listés dans la description de la PR.
6. **Preuve** : la description de la PR dit ce qui est prouvé, comment (commandes, codes de sortie), et ce qui ne l'est pas. Un comportement affirmé est accompagné d'une mutation ou d'un témoin quand c'est possible.
7. **Méthode de fusion : squash uniquement** (seule méthode autorisée par le dépôt).

## 2. Cas d'arrêt : ne JAMAIS fusionner

L'agent s'arrête, ne fusionne pas, et le dit à la personne, si :

- la CI est rouge sur la tête courante, ou si un job échoue **sans runner ni étape** (panne d'infrastructure : relancer ne suffit pas, il faut regarder les réglages Actions) ;
- une règle du ruleset **refuse** la fusion (par exemple la règle `require_extra_approval_for_unattributed_changes`) : on ne contourne jamais une règle, on signale ;
- la PR touche à un fichier **sensible** : secrets, clés, permissions, configuration du ruleset, workflows CI (`.github/workflows/`), scripts de publication, ou tout ce qui change ce qu'un utilisateur peut faire avec la clé d'un fournisseur ;
- la PR vient d'une **autre personne** ou d'un autre dépôt (Atelier, par exemple), sauf demande explicite ;
- la revue indépendante remonte un point **bloquant** non résolu ;
- une décision de produit est en jeu (choix d'API publique ou de comportement d'apprentissage du routeur, décisions consignées dans l'orchestrateur) : la décision est la personne, l'agent l'expose.

Jamais : `--admin`, force-push sur `main`, suppression de règle, contournement de `CI gate`.

## 3. Procédure

1. Vérifier les conditions du §1 sur la tête exacte (SHA), pas sur une tête antérieure.
2. Lancer la revue indépendante sur le diff `origin/main...<branche>` ; consigner le verdict dans la PR.
3. Si tout est vert : `gh pr merge <n> --squash` (sans `--admin`). Le ruleset peut encore refuser ; dans ce cas on s'arrête (§2).
4. Vérifier que `main` a bien reçu le commit attendu (`git fetch` puis `git log origin/main`).
5. Mettre à jour l'issue concernée par un commentaire d'état : ce qui est fusionné, ce qui reste.
6. Consigner la fusion dans l'orchestrateur (décision ou note), avec le SHA de tête et le SHA du commit fusionné.

## 4. Ce que le protocole ne couvre pas

- Il ne décide pas à la place de la personne des choix de conception. Une PR qui modifie un comportement visible (règle d'apprentissage, API publique, politique de repli) reste soumise à une décision explicite.
- Il ne remplace pas la CI. Une CI verte ne prouve que ce qu'elle exécute : la couverture réelle d'une feature se vérifie séparément (voir `ci.yml`, job `Code Coverage`).
- Il est révocable : la personne peut à tout moment demander l'arrêt des fusions autonomes, et l'agent s'y conforme immédiatement.

## 5. Historique

- 23/11/2026 : premier protocole. Déclenché par la demande de fusion autonome sur les PR de `this`, après le constat qu'aucune approbation humaine n'est requise par le ruleset `main`.
