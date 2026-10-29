# Le schéma SQL : qui l'applique, et pourquoi jamais tout seul

> **État : ce document décrit le contrat réel, vérifié dans le code le 2026-10-01.**
> Il existe parce que l'atlas du dépôt a relevé un écart : les deux migrations
> Postgres **n'étaient appliquées par aucun code de production** — le seul
> `sqlx::migrate!` du dépôt vivait dans un test — pendant que MySQL et ScyllaDB
> réécrivaient le même schéma en dur, chacun de son côté.

## 1. La règle : le framework fournit le geste, il ne le prend pas

Ouvrir une base est une **décision d'application**. La modifier en est une
seconde. Un framework qui migre la base de son hôte au démarrage lui retire
cette décision — et sur une base vivante, une migration surprise est une panne,
pas une commodité.

Donc : **rien n'est créé au démarrage**, et le geste est **offert,
explicite, idempotent**.

| backend | le geste | où vit le schéma |
|---|---|---|
| PostgreSQL | `this::storage::postgres::ensure_schema(&pool).await?` | `migrations/001_create_entities.up.sql`, `migrations/002_create_links.up.sql` (appliquées par sqlx, table `_sqlx_migrations`) |
| MySQL | `this::storage::mysql::ensure_schema(&pool).await?` | DDL `CREATE TABLE IF NOT EXISTS` dans `src/storage/mysql.rs` |
| ScyllaDB | `this::storage::scylladb::ensure_schema(&session, keyspace).await?` | CQL dans `src/storage/scylladb.rs` |

```rust
// Le parcours d'une application qui attache une base Postgres :
let pool = PgPool::connect(&url).await?;
this::storage::postgres::ensure_schema(&pool).await?;   // idempotent
let invoices = PostgresDataService::<Invoice>::new(pool);
```

Les trois gestes portent **le même nom et le même contrat** : idempotents,
rejouables à chaque démarrage, et **l'échec est rendu**, pas journalisé — une
base au schéma incomplet doit arrêter le démarrage, pas produire des requêtes
qui échouent une par une.

## 2. Ce qui est verrouillé par des tests, et sans base

`tests/schema_migrations.rs` tient le lien entre le SQL et le code **sans
Docker, sans conteneur, sans base** — donc dans n'importe quelle CI :

| ce qui est vérifié | pourquoi c'est le plus coûteux à découvrir en production |
|---|---|
| toute colonne écrite par `postgres.rs` existe dans les migrations | une colonne écrite et jamais créée ne se voit qu'à la première requête, sur la base de quelqu'un d'autre |
| MySQL déclare les mêmes colonnes que la référence Postgres | deux copies du même contrat divergent toujours ; celle-ci divergeait déjà en silence |
| toute colonne écrite par `mysql.rs` existe dans son DDL | idem, côté MySQL |

La conformité **à l'exécution** (une requête qui passe vraiment) est mesurée
ailleurs, par `tests/postgres_tests.rs` et `tests/mysql_tests.rs`, qui lancent
un vrai serveur — donc seulement là où Docker tourne. Ces tests-là passent
désormais par **le geste du paquet** (`this::storage::postgres::ensure_schema`)
et non par un `sqlx::migrate!` recopié : recopier mesurait le SQL, pas le code
qui l'applique.

## 3. Ce que ce document ne prétend PAS

- **Les types ne sont pas unifiés.** PostgreSQL déclare `UUID`, `JSONB`,
  `TIMESTAMPTZ` ; MySQL déclare `CHAR(36)`, `JSON`, `DATETIME(6)`. Le contrat
  verrouillé par les tests est celui des **noms** de colonnes, parce que c'est
  ce que le code Rust nomme. Les divergences de type sont réelles.
- **ScyllaDB n'est pas aligné.** Son DDL ne porte ni `tenant_id` ni
  `deleted_at` : le confinement par locataire et l'effacement doux n'y sont pas
  représentés, alors que les deux autres backends les portent. L'écart est
  **consigné** par le test `l_ecart_scylla_est_consigne_et_tombera_quand_il_sera_comble`
  — qui échouera le jour où quelqu'un le comblera, pour forcer la mise à jour
  au lieu de laisser une exception silencieuse. Le combler demande une base
  Scylla pour être mesuré.
- **Aucune `FOREIGN KEY` n'est déclarée** dans les migrations : `source_id` et
  `target_id` ne sont pas contraints par la base. C'est un choix de
  l'implémentation d'origine, pas une propriété vérifiée ici.
