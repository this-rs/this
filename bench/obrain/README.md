# Banc de validation durabilité/cognition obrain (validateur this-harness)

## Usage

```sh
bench/obrain/run.sh --repo <clone obrain> --rev <sha|ref> --artifacts <dir> [--forced]
```

`--forced` dérive les 5 suites historiques en variantes **observées**
(`open_substrate_observed` / `new_in_memory_observed`), **assertions inchangées**.

## Ce que le script garantit

| Exigence | Comment |
|---|---|
| dépôt source explicite | `--repo`, jamais deviné ; refuse un chemin non-git |
| SHA complet avant campagne | `git rev-parse --verify "$REV^{commit}"`, écrit dans `contexte.txt` |
| worktree isolé | `git worktree add --detach` ; **aucun `git checkout`** sur un dépôt partagé |
| artefacts uniques | `<artifacts>/{resume.txt,commands.txt,contexte.txt,generated/,*.log}` |
| tests réellement exécutés | `test result:` parsé ; **0 test exécuté = suite en échec** |
| code de sortie global | non nul si une suite inattendue échoue |
| nettoyage | `trap` : `git worktree remove --force`, fichiers générés retirés |
| variantes vérifiées | `cmp -s` source/variante : une transformation sans effet **échoue** ; le diff est conservé dans `generated/` |
| portable | POSIX `sh`, pas de chemin `$HOME` codé, `cargo` requis dans le `PATH` |

## Révisions et résultats

Chaque campagne est attachée au SHA qu'elle a testé — voir `RESULTS.md`.
Un passage sur une nouvelle révision est une **nouvelle** mesure.
