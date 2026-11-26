# Verdict convergence A/B/D — SHA ba7ff047

## Identité
- SHA this-rs : `7601e5626f7af5b41ee14b07fe9c6f5e072dd867` (final annoncé en fin de session) (branche `chore/convergence-abd`, worktree `_wt-conv-abd`)
- Base : `a583633f` (Lot G = A/B corrigé u128)
- Candidats réunis : A=0b234822, B=73a27273, D=00b9d9ce+19208a3b
- Contrat secondaire : obrain rev `dabcf91d` = merge LOCAL de `c26824ea` (feat/334-lot-d, EffetResolu.etat_courant) + `e972a472` (feat/334-lot-e, identité de domaine)

## Autonomie vis-à-vis du pin 7a795370 (main 50b2851a)
RELU (git grep obrain-substrate @ 7a795370) : `ReferenceTentative`, `ResolutionTentative`,
`IntegriteTentative` ABSENTS. Introduits dans les 34 commits entre 7a795370 et 89d169d7.
=> NON autonomes sur le pin : « conservation des obligations » (réservation/récupération/résolution).
=> AUTONOMES (purs this-rs) : codec OBRR, allocation K u128, écriture durable (fsync),
   validation (refus corruption), quarantaine (V1).

## Commandes + codes de sortie (toolchain 1.100.0)
- `cargo check -p this-rs --features obrain` → EXIT 0
- `cargo test -p this-rs --features obrain --test reconciliation_composant --test h3_v2_cycle_harness --test h3_v2_chaine_autorite --test k_durable_autorite --test durabilite_obrr` → EXIT 0

## Suites (37 tests verts)
- durabilite_obrr : 15 passed
- h3_v2_chaine_autorite : 7 passed
- h3_v2_cycle_harness : 5 passed
- k_durable_autorite : 8 passed
- reconciliation_composant : 2 passed

## Registre épistémique
- DÉMONTRÉ : 37 tests passent sur le contrat corrigé (etat_courant + domaine) ; les
  contre-épreuves de substitution/collision sont dans h3_v2_chaine_autorite.
- MESURÉ : 37/37 tests, EXIT 0, toolchain 1.100.0, obrain dabcf91d.
- RELU : EffetResolu.etat_courant (EtatPropriete) consommé dans reconcilier() ; le
  champ domaine est encodé mais JAMAIS vérifié depuis la référence (hors-bande).
- INTERPRÉTÉ : l'écart V1→V5 vaut 51 octets (decision_id 16 + autorite 8 + K str→u128 7 + domaine 19 + tag 1).
- NON QUALIFIÉ : autonomie des parties pures démontrée PAR SÉPARATION (non extraites en module indépendant) ;
  le pin de production (7a795370) n'est pas re-mesuré ici.

## Limites
- Le pin obrain est pointé sur `file://` + rev LOCAL dabcf91d (non poussé) : c'est
  une preuve EXPÉRIMENTALE, pas un changement du pin de production.
- Les parties « autonomes » n'ont PAS été extraites en un module compilant sur
  7a795370 (le module reconciliation.rs importe les types substrate pour reconcilier).
