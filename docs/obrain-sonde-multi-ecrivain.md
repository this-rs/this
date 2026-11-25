# Sonde multi-écrivain d'obrain — rapport de mesure

Mesure datée de l'écriture partagée multi-processus d'obrain, menée **au-dessus de
this-rs** : la sonde appelle `obrain_engine::ObrainDB` directement, sans passer par
le bail d'écriture de `src/storage/writer_lease.rs` (c'est l'intention — le bail
interdit deux écrivains, la sonde veut justement ouvrir deux écrivains).

- **Sonde** : `tests/multi_ecrivain_sonde.rs` (ce dépôt).
- **obrain épinglé** : `99e53f21bc443ba8eecd0dfc22bbf270fe1fa3af` (rev de `Cargo.toml`).
- **Clone de référence** : `/Users/triviere/projects/obrain/obrain-db` à `62f650e0`
  (1 commit devant le pin, même API).
- **Doc des invariants** : `obrain-db/docs/LIMITES-v0.0.1.md`, section « Écriture
  partagée » (lignes 16 à 24).
- **Outillage** : Rust `1.99.1`.

## Commande exacte

```bash
export RUSTUP_HOME=/Users/triviere/.rustup CARGO_HOME=/Users/triviere/.cargo PATH=/Users/triviere/.cargo/bin:$PATH
cargo +1.99.1 test -p this-rs --features obrain --test multi_ecrivain_sonde -- --ignored --nocapture
```

Le test principal `sonde_multi_ecrivain` est `#[ignore]` (il ne pèse pas sur la suite
ordinaire). Il se relance lui-même en sous-processus (`std::env::current_exe()` +
`--ignored --exact sonde_multi_ecrivain --nocapture`), rôle choisi par `SONDE_ROLE`,
et coordonne les écrivains par lignes sur stdout et fichiers-drapeaux. Durée bornée :
~1,6 s par passe, répertoires temporaires `tempfile` nettoyés.

## Tableau de synthèse

| Garantie / limite | Ce que la doc annonce | Ce que la sonde a mesuré | Verdict |
|---|---|---|---|
| **MP-3** — propriétés sur entités DISTINCTES (courtes, pages de propriétés v2 + tas) | garantie : 0 valeur relue sur un autre nœud (ligne 17) | `courtes_fausses = 0/400`, 0 nœud manquant | **tenue** |
| **Valeurs longues (> 256 o, arène des blobs) sur entités distinctes** | hors garantie : l'arène est écrasée entre écrivains vivants (ligne 21) | `200/400` valeurs longues relues comme celles d'un autre nœud | **limite confirmée (violée)** |
| **MP-3b** — MÊME entité, verrou inter-processus | garantie partielle : 0 perte (47/240 avant correctif → 0 après) | `0/240` clés perdues | **tenue** |
| **#386** — import en lot contre écrivain unitaire, même entité | corrigé (commit `e5936671`, présent au pin) | `0/240` clés perdues, chaîne préexistante intacte | **tenue (corrigé)** |
| **#387** — entité créée par un pair APRÈS notre ouverture | invisible + écriture no-op silencieuse (ligne 18, borne b, « non mesurée ») | nœud non vu (`VU 0`), écriture relue absente (`LU 0`), absente après réouverture | **limite confirmée (mesurée)** |
| **#387 côté ARÊTES** — arête créée par un pair APRÈS notre ouverture | même famille, jamais mesurée (ni par la doc, ni par #398 : « symétrique mais aucun test inter-processus ») | arête non vue (`VU 0`), écriture relue absente (`LU 0`), absente après réouverture | **limite confirmée (mesurée)** |
| **Arène des blobs** — deux écrivains vivants, séquentiel | limite connue : le second écrase les octets du premier (ligne 21) | B corrompu, relu comme le préfixe de A | **limite confirmée** |
| **Vecteurs** — deux écrivains + `kill -9` | rejoués par l'écrivain EXCLUSIF à la réouverture (ligne 23) | `0/400` perdus ou faux | **tenue** (réouverture exclusive) |

## Scénarios détaillés

### A — entités distinctes (MP-3), un écrivain tué -9

Deux écrivains (`open_shared_write`) posent chacun `N=200` nœuds portant une
propriété courte `_proprietaire`/`_index`, une valeur longue `_long` (600 octets,
routée vers l'arène) et un vecteur `_vec` (16 flottants). Chaque écrivain **annonce
ses ids** ; l'un est tué `SIGKILL` pendant qu'il continue d'écrire (jamais flushé),
l'autre ferme proprement ; réouverture EXCLUSIVE et relecture par id.

```
[A] TOTAL manquants=0 courtes_fausses(MP-3)=0 longues_mauvaises(arène)=200
    longues_manquantes=0 longues_corrompues=0 vecteurs_mauvais=0 vecteurs_manquants=0
```

- **MP-3 tenu** : aucune propriété courte n'est relue sur un autre nœud, aucun nœud
  ne manque après le `kill -9`.
- **Arène violée** : 200 valeurs longues sur 400 sont relues comme celles d'un autre
  nœud (l'arène des blobs reste allouée à un `arena_end` mis en cache PAR PROCESSUS ;
  le correctif MP-3 n'a porté que sur les pages de propriétés et de tas, pas sur la
  colonne blob). La répartition A↔B varie d'une passe à l'autre (73/127, 200/0, …),
  le TOTAL 200/400 est stable.
- **Vecteurs intacts** : colonne distincte de l'arène, rejouée par la réouverture
  exclusive.

### B — même entité (MP-3b), verrou d'entité

Deux écrivains martèlent le même nœud : 120 clés distinctes chacun (240 au total) +
une clé `partagee` écrite 150 fois chacun.

```
[B] clés distinctes perdues=0/240 | clé partagée finale=Some("A-149" ou "B-149") bien_formée=true
```

- **0 clé perdue** : le verrou d'entité inter-processus tient (le « 47/240 avant
  correctif, 0 après » est vérifié au pin).
- La clé martelée converge vers une valeur bien formée (dernier écrivain gagne, pas
  de page déchirée).

### C — import en lot contre écrivain unitaire, même entité (#386)

Nœud préexistant portant `base` ; départ synchronisé par drapeau ; « lot » fait un
`bulk_import_props` de 120 clés sur le slot, « unitaire » pose 120 clés une à une.

```
[C] clés perdues=0/240 | propriété préexistante 'base' relue=Some("presente")
```

- **0 clé perdue, chaîne intacte** : le correctif #386 (lot routé sur le chemin
  unitaire pour les entités déjà chaînées) est présent au pin et tient.
- Non couvert par cette passe : la fenêtre résiduelle inter-processus entre la
  relecture et la pose (le mutex d'entité est PAR PROCESSUS, la course reste ouverte,
  cf. message du commit `e5936671`) ne s'est pas déclenchée ; il faudrait un martelage
  plus agressif pour la provoquer.

### D — entité créée par un pair APRÈS notre ouverture (#387)

L'écrivain « nous » ouvre en écriture partagée, attend l'id du nœud que le pair crée
ensuite, puis tente de le lire et d'y écrire.

```
[D] pair vu par l'écrivain antérieur + écriture relue par lui : 0 LU 0
    après réouverture exclusive, 'cle_du_pair'=None
```

- **Confirmé et mesuré** : le nœud créé par le pair est invisible pour l'écrivain
  déjà ouvert (`VU 0`), l'écriture dessus est un **no-op silencieux** (`LU 0`), et
  rien n'apparaît après réouverture (`cle_du_pair = None`). **Perte de données sans
  aucune erreur** — c'est le défaut le plus grave mesuré ici.

### E — arène des blobs, deux écrivains vivants (séquentiel)

Reproduction fidèle de `obrain-engine/tests/blob_arene_deux_processus.rs` : base avec
une valeur longue, A ouvre (cache `arena_end`), B écrit 300 octets et ferme, A écrit
400 octets à son `arena_end` périmé, relecture par Cypher.

```
[E] A intacte=true | B corrompue=true (B relue=Some("aaaaaaaaaa"))
```

- **Limite confirmée** : B est relu comme les 300 premiers octets de la valeur de A
  (corruption silencieuse, exactement le symptôme verrouillé par le test d'obrain).

## Reproduction minimale

Fichier : `tests/multi_ecrivain_sonde.rs` (rôle `d_nous` / `d_pair`, scénario D).
Commande unique rejouant tout :

```bash
cargo +1.99.1 test -p this-rs --features obrain --test multi_ecrivain_sonde -- --ignored --nocapture
```

Le défaut #387 est reproductible en isolant le scénario D : un écrivain ouvre
`ObrainDB::open_shared_write(dir)`, un pair crée un nœud puis ferme, l'écrivain
antérieur appelle `set_node_property(id_du_pair, "cle", …)` — l'appel retourne
normalement mais rien n'est écrit (vérifié : `get_node_property` rend `None` dans le
processus antérieur comme après réouverture exclusive).

### F — arête créée par un pair APRÈS notre ouverture (#387, côté ARÊTES)

Pendant exact du scénario D, côté zone des **arêtes** : « nous » ouvre d'abord, le pair
crée deux nœuds et une arête puis ferme, « nous » écrit une propriété sur l'arête.

```
[F] arête du pair vue par l'écrivain antérieur + écriture relue par lui : 0 LU 0
    après réouverture exclusive, 'cle_du_pair'=None
```

- **Même défaut que D, mesuré** : l'arête du pair est invisible (`VU 0`), l'écriture
  dessus est un no-op silencieux (`LU 0`), et rien ne subsiste après réouverture.
- C'était le trou **explicitement non couvert** par le correctif #398 (« les arêtes :
  le code est symétrique mais aucun test inter-processus de bout en bout ne couvre la
  reprojection de la zone d'arêtes »). Cette variante le couvre.

## Suite donnée (25/11/2026)

L'issue #387 **existait déjà**, et une PR ouverte la corrige — donc **aucune issue
n'a été créée** : la mesure a été versée aux deux fils existants.

| Rev | D (nœud) | F (arête) |
|---|---|---|
| `99e53f21` (pin de this-rs) | `LU 0`, `cle_du_pair = None` | `LU 0`, `cle_du_pair = None` |
| `83c647c8` (tête de la PR #398) | `LU 1`, `cle_du_pair = Some("ecrite_par_nous")` | `LU 1`, `cle_du_pair = Some("ecrite_par_nous")` |

`VU` reste `0` dans les quatre cas : la VISIBILITÉ du pair n'est pas corrigée (c'est le
non-objectif assumé de #398, les scans restant bornés par le miroir périmé). Ce qui
change, c'est la fin du no-op silencieux — sur les nœuds **et** sur les arêtes.

- obrain#387 : [`issuecomment-7148983447`](https://github.com/obrain-db/obrain/issues/387#issuecomment-7148983447)
- obrain#398 : [`issuecomment-7148984945`](https://github.com/obrain-db/obrain/pull/398#issuecomment-7148984945)

## Corps d'issue (rédigé avant de savoir que #387 existait — conservé tel quel)

> **Titre** : `open_shared_write` : une écriture sur une entité créée par un pair
> après notre ouverture est un no-op SILENCIEUX (#387 confirmé mesuré)
>
> **Contexte.** Deux écrivains `open_shared_write` sur le même store. Le processus A
> ouvre, puis le processus B crée un nœud et ferme. A tente ensuite d'écrire sur ce
> nœud :
>
> ```rust
> let db = ObrainDB::open_shared_write(dir)?;      // A, ouvre le premier
> // … B crée un nœud `id` puis ferme …
> db.set_node_property(NodeId(id), "cle", Value::from("valeur")); // retourne sans erreur
> assert_eq!(db.get_node_property(NodeId(id), &PropertyKey::new("cle")), None); // silencieux
> ```
>
> **Mesure** (sonde `this-rs/tests/multi_ecrivain_sonde.rs`, scénario D, obrain
> `99e53f21`) : `get_node(id)` rend `None` (entité invisible), l'écriture ne lève
> aucune erreur, et après réouverture EXCLUSIVE la propriété est **absente**
> (`cle_du_pair = None`). La borne (b) de `docs/LIMITES-v0.0.1.md` (ligne 18) est donc
> **confirmée et mesurée** — elle n'était jusqu'ici que « constatée par relecture ».
>
> **Impact.** Perte de données silencieuse : aucun signal, ni au retour de
> `set_node_property` ni au journal. Un appelant qui croit avoir persisté une valeur
> n'a rien écrit. C'est le pire mode de défaillance pour une base : il ne se voit pas.
>
> **Piste.** Rendre la cible visible à l'écrivain antérieur (rafraîchissement de la
> projection sur les slots créés par un pair), ou au minimum retourner une erreur /
> un compteur quand `set_node_property` vise un slot inconnu de ce processus.
>
> **Secondaire (même sonde).** Scénario A : sur des entités DISTINCTES, les valeurs
> courtes (MP-3) sont sûres, mais les valeurs longues (> 256 o, arène des blobs)
> restent écrasées entre deux écrivains vivants — 200 valeurs sur 400 relues comme
> celles d'un autre nœud. La garantie MP-3 (ligne 17) ne couvre **pas** la colonne
> blob (ligne 21) : à signaler pour éviter qu'un lecteur de la ligne 17 croie que
> « entités distinctes » protège toutes les propriétés.

## Limites de la sonde

- Scénario C : la course résiduelle inter-processus de #386 (fenêtre de quelques
  microsecondes) n'a pas été déclenchée sur les passes exécutées ; le verdict « tenue »
  porte sur le chemin déterministe du correctif, pas sur cette fenêtre.
- Scénario A : la répartition des 200 valeurs longues fausses entre A et B est
  non déterministe (scheduling) ; le total 200/400 est stable.
- La sonde n'a **rien** modifié hors de ses deux fichiers, n'a rien commité, rien
  poussé, et n'a ouvert aucune issue.
