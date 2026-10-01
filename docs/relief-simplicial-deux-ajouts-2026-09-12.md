# Relief simplicial — les deux ajouts (test de maintenance + banc), 2026-09-12

**Provenance.** Extrait VERBATIM du journal de la session
`session-7f3f9299-8808-47fb-b915-9d697abc3a1f`, message du **2026-09-12 19:35**
(store `runtime-dev.obrain`). Aucune réécriture, aucune reformulation : c'est le
texte de l'agent, avec ses preuves, ses sources et ses réserves. Le dépôt vers
Aletheia étant bloqué, ce fichier en est la copie durable, dans le dépôt qui
porte le code concerné.

---

## Les deux ajouts sont faits et vérifiés

### 1. Le test qui prouve la promesse (elle était affirmée, elle est maintenant exigée)

`test_simplex_relief_follows_this_rs_writes` suit le chemin d'écriture **réel** de this-rs :

1. trois entités + trois liens via `ObrainDataService` / `ObrainLinkService` → un triangle ;
2. `configurer_passes_simpliciales(maintenance: true)` puis **une seule** passe → `triangles == 1` ;
3. un **second** triangle écrit par un lien this-rs ordinaire, **aucune passe relancée** → `triangles == 2`.

C'est la maintenance incrémentale au commit d'obrain, prouvée depuis ce framework : le relief suit les écritures, il n'y a pas de job de rattrapage à faire tourner. Le test vérifie aussi que le document publie `configured`, `maintenance` et les compteurs cohérents.

**870 tests verts** avec `--features obrain`, **788** sans.

### 2. Le bench — et ce qu'il mesure vraiment

`this/examples/relief_bench.rs`, dans la convention du dépôt (les exemples servent de bancs, comme `weave_bench`) : `--release --example relief_bench -- [nœuds] [itérations]`, graphe en chaîne de triangles construite par session obrain brute.

| mesure | n=2 000 | n=20 000 | ×10 de données |
|---|---|---|---|
| `GET /graph` (défaut) | 33,5 µs | 34,4 µs | **constant** — c'est du O(registre) |
| `GET /graph?counts=true` | 47,8 µs | 368,5 µs | **×7,7 — linéaire** |
| `use_graph` (×2, invalidation) | 51 ns | 28 ns | constant |
| `rebuild_index_for_label` | 955 µs | 8,54 ms | **×8,9 — linéaire** |

Ce que ça tranche, chiffres en main :

- **Le document est sûr en HTTP** : ~34 µs, et ça ne bouge pas avec la taille du graphe — la partie introspectée est le registre (28 procédures), pas les données.
- **Les compteurs sont un balayage**, comme je l'avais déduit de l'absence de compteur O(1) chez obrain : ×10 de données → ×7,7 de temps. Le cache n'est donc pas un confort, c'est une nécessité — et l'option est déjà opt-in par défaut.
- **Le prix de mon choix d'espace est mesuré** : la bascule est gratuite (28 ns), mais la reprise du cache est linéaire (955 µs → 8,5 ms). C'est l'argument chiffré pour l'amélioration que j'avais nommée — un cache indexé **par espace** (via `graph_scope()` d'obrain) supprimerait ces 8,5 ms.

Une note d'honnêteté : la première version du banc mesurait `rebuild_index_for_label` **sur un espace vide** — la bascule m'avait laissé dans `bench-b`, donc le chiffre était flatteur (254 ns) et ne disait rien. Corrigé par un `reset_graph()` explicite avant la mesure, avec le commentaire qui explique pourquoi.

### Fichiers

- Nouveau : `this/examples/relief_bench.rs`, `this/examples/graph_relief.rs`
- Modifié : `this/src/storage/obrain.rs` (test de maintenance), `this/Cargo.toml` (`[[example]] relief_bench`)

État final : rustfmt clean, release recompilé, banc relancé après formatage.

Prochaine étape que je propose, dans l'ordre : le **cache indexé par espace** (les 8,5 ms ci-dessus, et la reprise manuelle disparaît), puis le même relief en **MCP** et en **gRPC** — la surface REST étant désormais stable et versionnée (`this-rs/graph-relief/1`).
