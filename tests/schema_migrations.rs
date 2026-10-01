//! Le schéma SQL et le code Rust disent-ils la MÊME chose ? — vérifié SANS base.
//!
//! # Pourquoi ce fichier existe
//!
//! L'atlas du dépôt (2026-10-01) a relevé un fait : le SQL de référence
//! (`migrations/001_create_entities.up.sql`, `002_create_links.up.sql`) n'était
//! appliqué par **aucun** code de production — le seul `sqlx::migrate!` vivait
//! dans un test — pendant que MySQL et Scylla **réécrivaient le même schéma en
//! dur**, chacun de son côté. Trois définitions du même contrat, aucun lien
//! entre elles : la première divergence se serait découverte en production, sur
//! une colonne absente, une requête à la fois.
//!
//! Ces tests tiennent le lien sans rien déployer : ils lisent le SQL et le
//! source Rust, et exigent que **toute colonne que le code écrit existe dans le
//! schéma** — pour Postgres (sa migration) et pour MySQL (son DDL en dur).
//! Ils tournent partout, sans Docker, sans conteneur, sans base.
//!
//! # Ce que ces tests ne couvrent PAS — écrit, pas tu
//!
//! - **Scylla n'est pas couvert**, et c'est un écart CONSTATÉ, pas un oubli :
//!   son DDL ne porte ni `tenant_id` ni `deleted_at`. Le test
//!   `l_ecart_scylla_est_consigne_et_tombera_quand_il_sera_comble` le dit, et
//!   **échouera le jour où quelqu'un le comblera** — pour forcer la mise à jour
//!   plutôt que de laisser une exception silencieuse.
//! - **Les types** ne sont pas comparés (UUID contre CHAR(36), JSONB contre
//!   JSON) : le contrat tenu ici est celui des NOMS de colonnes, parce que
//!   c'est ce que le code Rust nomme. Les divergences de type sont réelles et
//!   connues ; elles demandent une base pour être mesurées.
//! - **La conformité à l'exécution** (une requête qui passe vraiment) n'est pas
//!   mesurée ici : elle l'est par `tests/postgres_tests.rs` et
//!   `tests/mysql_tests.rs`, qui lancent un vrai serveur — donc seulement là où
//!   Docker tourne.

use std::collections::BTreeSet;
use std::fs;

/// Lit un fichier du dépôt (chemin relatif à la racine du crate).
fn lire(chemin: &str) -> String {
    let complet = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), chemin);
    fs::read_to_string(&complet).unwrap_or_else(|e| panic!("lecture de {} : {}", complet, e))
}

/// Colonnes déclarées par un `CREATE TABLE IF NOT EXISTS <table> ( … )`.
///
/// Fonctionne sur le SQL des migrations comme sur le DDL écrit dans une chaîne
/// Rust : on s'arrête à la première ligne qui commence par `)`, et les
/// directives de table (`INDEX`, `PRIMARY KEY`, …) ne sont pas des colonnes.
fn colonnes_declarees(texte: &str, table: &str) -> BTreeSet<String> {
    let marqueur = format!("CREATE TABLE IF NOT EXISTS {}", table);
    let debut = texte
        .find(&marqueur)
        .unwrap_or_else(|| panic!("aucun CREATE TABLE pour `{}`", table));
    let apres = &texte[debut..];
    let ouvrant = apres
        .find('(')
        .unwrap_or_else(|| panic!("parenthèse ouvrante absente pour `{}`", table));

    let mut colonnes = BTreeSet::new();
    for ligne in apres[ouvrant + 1..].lines() {
        let ligne = ligne.trim();
        if ligne.starts_with(')') {
            break;
        }
        let ligne = ligne.trim_end_matches(',').trim_end();
        if ligne.is_empty() || ligne.starts_with("--") {
            continue;
        }
        let premier = ligne.split_whitespace().next().unwrap_or("").to_string();
        let majuscule = premier.to_uppercase();
        if [
            "INDEX",
            "PRIMARY",
            "UNIQUE",
            "KEY",
            "CONSTRAINT",
            "FOREIGN",
            "CHECK",
        ]
        .contains(&majuscule.as_str())
        {
            continue;
        }
        colonnes.insert(premier);
    }
    assert!(
        !colonnes.is_empty(),
        "aucune colonne lue pour `{}` — le format a changé, le test ne mesure plus rien",
        table
    );
    colonnes
}

/// Colonnes nommées par les `INSERT INTO <table> ( … )` du source donné.
fn colonnes_ecrites(texte: &str, table: &str) -> BTreeSet<String> {
    let marqueur = format!("INSERT INTO {} (", table);
    let mut colonnes = BTreeSet::new();
    let mut reste = texte;
    while let Some(position) = reste.find(&marqueur) {
        let apres = &reste[position + marqueur.len()..];
        let fermante = apres
            .find(')')
            .unwrap_or_else(|| panic!("INSERT INTO {} sans parenthèse fermante", table));
        for colonne in apres[..fermante].split(',') {
            let colonne = colonne.trim();
            // La liste peut être coupée par une continuation de ligne Rust (`\`).
            let colonne = colonne.trim_end_matches('\\').trim();
            if !colonne.is_empty() {
                colonnes.insert(colonne.to_string());
            }
        }
        reste = &apres[fermante..];
    }
    assert!(
        !colonnes.is_empty(),
        "aucun INSERT INTO {} trouvé — le code a changé de forme, le test ne mesure plus rien",
        table
    );
    colonnes
}

// ---------------------------------------------------------------------------
// Postgres : les migrations de référence
// ---------------------------------------------------------------------------

/// Le code écrit-il dans des colonnes que la migration ne déclare pas ?
///
/// C'est l'écart le plus coûteux de tous : une colonne écrite et jamais créée
/// ne se voit qu'à la première requête, en production, sur la base de quelqu'un
/// d'autre.
#[test]
fn les_colonnes_ecrites_par_postgres_existent_dans_ses_migrations() {
    let migration_entites = lire("migrations/001_create_entities.up.sql");
    let migration_liens = lire("migrations/002_create_links.up.sql");
    let code = lire("src/storage/postgres.rs");

    for (table, migration) in [
        ("entities", &migration_entites),
        ("links", &migration_liens),
    ] {
        let declarees = colonnes_declarees(migration, table);
        let ecrites = colonnes_ecrites(&code, table);
        let inconnues: Vec<_> = ecrites.difference(&declarees).cloned().collect();
        assert!(
            inconnues.is_empty(),
            "postgres.rs écrit dans des colonnes que migrations/{}.up.sql ne déclare pas : {:?}\n\
             déclarées : {:?}",
            table,
            inconnues,
            declarees
        );
    }
}

/// Le geste d'application du schéma existe, il est **public**, et il porte le
/// même nom pour les trois backends.
///
/// Un geste qu'on ne peut pas appeler n'existe pas : c'était exactement l'état
/// de Postgres avant ce correctif — du SQL de référence que seul un test
/// jouait. Ces références ne l'**appellent** pas (il faudrait une base) : elles
/// tiennent son existence et sa visibilité — rendu privé ou renommé, ce
/// fichier ne compilerait plus.
#[cfg(feature = "postgres")]
#[test]
fn le_geste_postgres_est_public() {
    let _geste = this::storage::postgres::ensure_schema;
}

#[cfg(feature = "mysql")]
#[test]
fn le_geste_mysql_est_public() {
    let _geste = this::storage::mysql::ensure_schema;
}

#[cfg(feature = "scylladb")]
#[test]
fn le_geste_scylla_est_public() {
    let _geste = this::storage::scylladb::ensure_schema;
}

// ---------------------------------------------------------------------------
// MySQL : le même schéma, écrit en dur
// ---------------------------------------------------------------------------

/// MySQL déclare-t-il les mêmes colonnes que la référence ?
#[test]
fn mysql_declare_les_memes_colonnes_que_la_reference_postgres() {
    let source = lire("src/storage/mysql.rs");
    let reference = format!(
        "{}{}",
        lire("migrations/001_create_entities.up.sql"),
        lire("migrations/002_create_links.up.sql")
    );

    for table in ["entities", "links"] {
        let attendues = colonnes_declarees(&reference, table);
        let declarees = colonnes_declarees(&source, table);
        let manquantes: Vec<_> = attendues.difference(&declarees).cloned().collect();
        assert!(
            manquantes.is_empty(),
            "mysql.rs ne déclare pas {:?} pour `{}` — le même contrat, écrit deux fois, a divergé\n\
             mysql   : {:?}",
            manquantes,
            table,
            declarees
        );
    }
}

/// MySQL écrit-il dans les colonnes qu'il déclare ?
#[test]
fn les_colonnes_ecrites_par_mysql_existent_dans_son_ddl() {
    let source = lire("src/storage/mysql.rs");
    for table in ["entities", "links"] {
        let declarees = colonnes_declarees(&source, table);
        let ecrites = colonnes_ecrites(&source, table);
        let inconnues: Vec<_> = ecrites.difference(&declarees).cloned().collect();
        assert!(
            inconnues.is_empty(),
            "mysql.rs écrit dans des colonnes que son DDL ne déclare pas : {:?}",
            inconnues
        );
    }
}

// ---------------------------------------------------------------------------
// L'écart consigné : Scylla
// ---------------------------------------------------------------------------

/// L'écart de Scylla est **consigné**, pas tu — et ce test tombera quand il
/// sera comblé.
///
/// Scylla range l'entité entière dans une colonne `entity_data` et son DDL ne
/// porte ni `tenant_id` ni `deleted_at` : **le confinement par locataire et
/// l'effacement doux n'y sont pas représentés**, alors que les deux autres
/// backends les portent. Le combler demande une base Scylla pour être mesuré
/// (les tests de ce backend lancent un conteneur) : tant que ce n'est pas fait,
/// l'écart est écrit ici plutôt que laissé à la découverte.
///
/// Le jour où Scylla les portera, ce test échouera — c'est voulu : il faudra
/// alors l'étendre aux trois backends au lieu de laisser une exception
/// silencieuse.
#[test]
fn l_ecart_scylla_est_consigne_et_tombera_quand_il_sera_comble() {
    let source = lire("src/storage/scylladb.rs");
    for colonne in ["tenant_id", "deleted_at"] {
        assert!(
            !source.contains(colonne),
            "scylladb.rs porte désormais `{}` : l'écart est comblé — retire cette \
             exception et couvre Scylla comme les deux autres backends",
            colonne
        );
    }
}
