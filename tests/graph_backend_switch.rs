#![cfg(feature = "obrain")]
//! Bascule obrain → this-rs : le même consommateur, deux backends.
//!
//! # Le périmètre, en une phrase
//!
//! Un consommateur qui parle aujourd'hui à obrain doit pouvoir **pointer ses
//! interfaces vers this-rs sans réécrire une ligne** : c'est ce que verrouille
//! `this::storage::GraphBackend`, et c'est ce que ce fichier met à l'épreuve.
//!
//! Trois choses sont distinguées, et c'est la distinction qui donne son sens
//! aux tests :
//!
//! 1. **le contrat commun** (`GraphBackend`) — ce qu'obrain brut et this-rs
//!    savent tenir tous les deux, et qui doit donc rendre *exactement* la même
//!    chose des deux côtés ;
//! 2. **les apports de this-rs** (espaces nommés, bail d'écriture, identité,
//!    IAM) — volontairement **hors** du trait, parce qu'obrain ne les a pas :
//!    un consommateur qui n'en a pas besoin ne doit jamais avoir à les
//!    connaître ;
//! 3. **la frontière opérationnelle** — c'est le bail qui ferme le trou
//!    intra-processus d'obrain (deux `open()` du même répertoire réussissent),
//!    et c'est le branchement clé en main qui l'arme par défaut.
//!
//! # Note d'accès
//!
//! `obrain_engine`, `obrain_common` et `obrain_core` sont des dépendances
//! **optionnelles activées par la feature `obrain`** : elles restent donc
//! accessibles depuis `tests/` (Cargo passe `--extern` pour toute dépendance
//! activée, dépendance normale comme de développement). Ce fichier le prouve
//! par l'usage — c'est une information décisive pour la suite du chantier.

use std::collections::BTreeMap;
use std::sync::Arc;

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::Direction;
use obrain_engine::ObrainDB;
use this::server::ServerBuilder;
use this::storage::writer_lease::FileAuthority;
use this::storage::{GraphBackend, ObrainLinkService, ObrainStore};

// ---------------------------------------------------------------------------
// La charge de travail d'un consommateur — écrite UNE fois, contre le trait
// ---------------------------------------------------------------------------

/// Rapport d'observation d'un graphe.
///
/// **Canonisé** à dessein : les collections sont triées et les valeurs rendues
/// en texte. Sans cette canonisation, comparer deux backends reviendrait à
/// comparer deux ordres d'itération — un test qui échoue pour une raison qui
/// n'intéresse personne, et qui finit ignoré. Ce qui est comparé ici est le
/// **contenu observable**, pas l'ordre de parcours interne.
#[derive(Debug, PartialEq, Eq)]
struct Report {
    is_substrate: bool,
    node_count: usize,
    labels: Vec<String>,
    pages: Vec<u64>,
    out_degrees: BTreeMap<u64, usize>,
    outgoing: BTreeMap<u64, Vec<u64>>,
    incoming: BTreeMap<u64, Vec<u64>>,
    properties: BTreeMap<u64, BTreeMap<String, String>>,
    found_by_property: Vec<u64>,
    index_present: bool,
    index_dropped: bool,
    unknown_node_is_absent: bool,
}

/// **Le consommateur générique** : il ne connaît que `GraphBackend`.
///
/// Charge non triviale — création, propriétés (à la création **et** après),
/// arêtes typées, index, commit — pour que la parité ne soit pas établie sur un
/// chemin trivial qui n'exerce rien.
fn charge<B: GraphBackend + ?Sized>(backend: &B) {
    // L'index est posé AVANT les écritures : c'est l'ordre d'un vrai
    // consommateur, et c'est aussi celui qui exerce la mise à jour de l'index.
    backend.create_property_index("qualified_name");

    let a = backend.create_node(
        &["Page"],
        vec![
            ("qualified_name", Value::from("entity:a")),
            ("titre", Value::from("A")),
        ],
    );
    let b = backend.create_node(&["Page"], vec![("qualified_name", Value::from("entity:b"))]);
    let c = backend.create_node(&["Note"], vec![("qualified_name", Value::from("note:c"))]);

    backend.create_edge(a, b, "POINTS_TO");
    backend.create_edge(a, c, "MENTIONS");
    backend.create_edge(c, b, "CITE");

    // Propriété posée APRÈS la création : chemin `set_node_property`, distinct
    // du chemin `create_node`.
    backend.set_node_property(b, "titre", Value::from("B"));

    backend.commit().expect("commit");
}

/// **Le lecteur générique** — même exigence : une seule écriture, deux backends.
fn observe<B: GraphBackend + ?Sized>(backend: &B) -> Report {
    let mut labels = backend.labels();
    labels.sort();

    let mut pages: Vec<u64> = backend
        .nodes_by_label("Page")
        .into_iter()
        .map(|id| id.0)
        .collect();
    pages.sort_unstable();

    let mut out_degrees = BTreeMap::new();
    let mut outgoing = BTreeMap::new();
    let mut incoming = BTreeMap::new();
    let mut properties = BTreeMap::new();

    for id in backend
        .nodes_by_label("Page")
        .into_iter()
        .chain(backend.nodes_by_label("Note"))
    {
        let mut sortants: Vec<u64> = backend
            .neighbors(id, Direction::Outgoing)
            .into_iter()
            .map(|n| n.0)
            .collect();
        sortants.sort_unstable();

        let mut entrants: Vec<u64> = backend
            .neighbors(id, Direction::Incoming)
            .into_iter()
            .map(|n| n.0)
            .collect();
        entrants.sort_unstable();

        out_degrees.insert(id.0, backend.out_degree(id));
        outgoing.insert(id.0, sortants);
        incoming.insert(id.0, entrants);
        properties.insert(
            id.0,
            backend
                .node_properties(id)
                .into_iter()
                .map(|(cle, valeur)| (cle, format!("{valeur:?}")))
                .collect(),
        );
    }

    let mut found_by_property: Vec<u64> = backend
        .find_nodes_by_property("qualified_name", &Value::from("entity:b"))
        .into_iter()
        .map(|id| id.0)
        .collect();
    found_by_property.sort_unstable();

    let index_present = backend.has_property_index("qualified_name");
    let index_dropped = backend.drop_property_index("qualified_name");

    Report {
        is_substrate: backend.is_substrate(),
        node_count: backend.node_count(),
        labels,
        pages,
        out_degrees,
        outgoing,
        incoming,
        properties,
        found_by_property,
        index_present,
        index_dropped,
        unknown_node_is_absent: backend.get_node(NodeId::INVALID).is_none(),
    }
}

/// Le consommateur minimal dont parle la mission — trois lignes, deux backends.
fn pages<B: GraphBackend + ?Sized>(backend: &B) -> usize {
    backend.nodes_by_label("Page").len()
}

/// Un second consommateur minimal, pour montrer que la couture se prête à
/// plusieurs fonctions indépendantes (et pas à une seule fonction témoin).
fn compte<B: GraphBackend + ?Sized>(backend: &B) -> usize {
    backend.node_count()
}

// ---------------------------------------------------------------------------
// 1. Parité stricte — LE test qui donne son sens à la couture
// ---------------------------------------------------------------------------

/// Le même consommateur générique, sur les deux backends, doit rendre
/// **exactement** le même rapport.
///
/// C'est la propriété sans laquelle « on peut basculer ses interfaces » reste
/// une intention : si un seul champ diverge, basculer change le comportement.
#[test]
fn the_same_generic_consumer_sees_identical_graphs_on_both_backends() {
    let raw = ObrainDB::new_in_memory();
    let wrapped = ObrainStore::new_in_memory();

    charge(&raw);
    charge(wrapped.as_ref());

    let sur_obrain = observe(&raw);
    let sur_this_rs = observe(wrapped.as_ref());

    // Les faits absolus d'abord : sans eux, deux backends également cassés
    // passeraient pour deux backends en parité.
    assert_eq!(sur_obrain.node_count, 3, "trois nœuds écrits");
    assert_eq!(
        sur_obrain.pages,
        vec![1, 2],
        "deux `Page`, et deux seulement"
    );
    assert_eq!(
        sur_obrain.labels,
        vec!["Note".to_string(), "Page".to_string()],
        "les labels vus doivent être exactement ceux écrits"
    );
    assert_eq!(
        sur_obrain.found_by_property,
        vec![2],
        "l'index doit rendre la recherche exacte"
    );
    assert!(sur_obrain.index_present, "l'index a été posé");
    assert!(sur_obrain.index_dropped, "l'index posé doit être retirable");
    assert!(
        sur_obrain.unknown_node_is_absent,
        "un NodeId invalide ne doit jamais résoudre un nœud"
    );
    assert_eq!(sur_obrain.out_degrees.get(&1), Some(&2), "a → b et a → c");
    assert_eq!(
        sur_obrain.incoming.get(&2),
        Some(&vec![1, 3]),
        "b reçoit de a (POINTS_TO) et de c (CITE)"
    );
    assert!(
        sur_obrain.properties[&2].contains_key("titre"),
        "la propriété posée après création doit être lue"
    );

    // Puis LA propriété de la couture : égalité stricte des deux rapports.
    assert_eq!(
        sur_obrain, sur_this_rs,
        "le même consommateur générique doit voir le même graphe sur obrain brut \
         et sur this-rs — sinon la bascule change le comportement"
    );
}

// ---------------------------------------------------------------------------
// 2. Bascule à l'exécution
// ---------------------------------------------------------------------------

/// Le choix du backend peut se faire **à l'exécution**, pas seulement à la
/// compilation : une même fonction parcourt les deux.
#[test]
fn the_backend_can_be_switched_at_runtime_through_a_trait_object() {
    let raw = ObrainDB::new_in_memory();
    let wrapped = ObrainStore::new_in_memory();

    charge(&raw);
    charge(wrapped.as_ref());

    // Le consommateur choisit son backend dans une liste — il ne sait pas
    // lequel il tient.
    let backends: Vec<&dyn GraphBackend> = vec![&raw, wrapped.as_ref()];
    let rapports: Vec<Report> = backends.iter().map(|backend| observe(*backend)).collect();

    assert_eq!(rapports.len(), 2);
    assert_eq!(
        rapports[0], rapports[1],
        "un backend choisi à l'exécution doit rendre le même rapport"
    );

    // Et les consommateurs courts, écrits séparément, s'instancient aussi bien
    // sur l'objet-trait que sur le générique.
    let par_objet: Vec<usize> = backends.iter().map(|backend| pages(*backend)).collect();
    let par_generique = vec![pages(&raw), pages(wrapped.as_ref())];
    assert_eq!(par_objet, par_generique);
    assert_eq!(par_objet, vec![2, 2]);
}

/// `Box<dyn GraphBackend>` doit être possible pour les **deux** implémentations.
///
/// Friction documentée ici, parce qu'elle décidera de la forme des adaptateurs :
/// `ObrainStore` n'a **aucun constructeur par valeur** (`new_in_memory`, `open`,
/// `open_read_only`, `from_db` rendent tous `Arc<Self>`). Pour l'encapsuler dans
/// un `Box`, il faut donc détenir la seule référence de l'`Arc` et le
/// déconstruire (`Arc::try_unwrap`) — ce que le test fait, et qui est un chemin
/// public, mais qui impose au consommateur de connaître le comptage de
/// références. `ObrainDB`, lui, se possède directement.
#[test]
fn a_trait_object_can_be_owned_in_a_box_for_either_backend() {
    let owned_raw: ObrainDB = ObrainDB::new_in_memory();
    let owned_store: ObrainStore = Arc::try_unwrap(ObrainStore::new_in_memory())
        .ok()
        .expect("le store fraîchement créé n'a qu'une référence : il est déconstructible");

    let backends: Vec<Box<dyn GraphBackend>> = vec![Box::new(owned_raw), Box::new(owned_store)];

    for backend in backends.iter() {
        charge(backend.as_ref());
    }

    let rapports: Vec<Report> = backends
        .iter()
        .map(|backend| observe(backend.as_ref()))
        .collect();

    assert_eq!(
        rapports[0], rapports[1],
        "deux backends encapsulés dans un Box doivent rester interchangeables"
    );

    // Et l'objet-trait encapsulé reste utilisable après coup : le Box ne fige
    // pas le backend.
    backends[0].create_node(&["Page"], vec![("qualified_name", Value::from("tardif"))]);
    assert_eq!(compte(backends[0].as_ref()), 4);
    assert_eq!(
        compte(backends[1].as_ref()),
        3,
        "l'autre backend est intact"
    );
}

// ---------------------------------------------------------------------------
// 3. Espace actif — les opérations du trait passent par l'espace courant
// ---------------------------------------------------------------------------

/// Sur `ObrainStore`, **toutes** les opérations du trait passent par l'espace
/// actif : un consommateur qui bascule n'a rien à savoir des espaces nommés
/// pour que ses lectures restent correctes.
///
/// Vérifié : `create_space`, `use_graph`, `reset_graph` et `spaces` sont
/// **publics** sur `ObrainStore` (et absents du trait).
#[test]
fn the_common_contract_reads_only_the_active_space() {
    let store = ObrainStore::new_in_memory();

    assert!(
        store.create_space("a").expect("create_space(a)"),
        "l'espace a doit être créé"
    );
    assert!(
        store.create_space("b").expect("create_space(b)"),
        "l'espace b doit être créé"
    );
    assert_eq!(
        store.spaces(),
        vec!["a".to_string(), "b".to_string()],
        "les espaces créés doivent être listés"
    );

    // --- écriture dans l'espace a, via le TRAIT uniquement -------------------
    store.use_graph("a");
    assert_eq!(store.space().as_deref(), Some("a"));
    let backend_a: &dyn GraphBackend = store.as_ref();
    backend_a.create_node(&["Page"], vec![("qualified_name", Value::from("dans-a"))]);
    backend_a.commit().expect("commit dans a");
    assert_eq!(pages(backend_a), 1, "l'espace a contient sa page");
    assert_eq!(compte(backend_a), 1);

    // --- bascule : l'espace b est VIDE, et le même appel le dit ------------
    store.use_graph("b");
    assert_eq!(store.space().as_deref(), Some("b"));
    let backend_b: &dyn GraphBackend = store.as_ref();
    assert_eq!(
        compte(backend_b),
        0,
        "l'espace b ne doit pas voir les nœuds de l'espace a"
    );
    assert_eq!(
        pages(backend_b),
        0,
        "`nodes_by_label` du trait doit être cloisonné par espace"
    );

    backend_b.create_node(&["Page"], vec![("qualified_name", Value::from("dans-b"))]);
    backend_b.commit().expect("commit dans b");
    assert_eq!(pages(backend_b), 1);

    // --- retour dans a : les mêmes appels revoient les nœuds de a ----------
    //
    // C'est le point dur : les deux espaces allouent chacun leurs `NodeId` à
    // partir de 1. Une lecture qui n'appliquerait pas l'espace rendrait ici le
    // nœud de b sous l'identité de a — silencieusement.
    store.use_graph("a");
    assert_eq!(pages(backend_a), 1);
    let id_a = backend_a.nodes_by_label("Page")[0];
    assert_eq!(
        backend_a
            .node_property(id_a, "qualified_name")
            .and_then(|v| v.as_str().map(str::to_string)),
        Some("dans-a".to_string()),
        "l'identité d'un nœud doit rester celle de son espace"
    );

    // --- hors espace nommé : plus rien ------------------------------------
    store.reset_graph();
    assert_eq!(
        store.space(),
        None,
        "`reset_graph` revient au graphe par défaut"
    );
    let racine: &dyn GraphBackend = store.as_ref();
    assert_eq!(
        compte(racine),
        0,
        "le graphe par défaut doit être vide : rien n'y a été écrit"
    );
}

/// **Défaut constaté** — dans le trait, `find_nodes_by_property` n'est pas
/// cloisonné par espace alors que `nodes_by_label` l'est.
///
/// `impl GraphBackend for ObrainStore` route la recherche par propriété vers
/// **Corrigé** : `find_nodes_by_property` passe désormais par l'**espace
/// actif**, comme `nodes_by_label`, `node_count`, `get_node` et `neighbors`.
/// Avant, il était routé vers `self.db()` — l'index **racine** — et le contrat
/// se contredisait lui-même : dans un espace nommé, un nœud était trouvé par son
/// label et introuvable par sa propriété indexée.
#[test]
fn the_property_lookup_follows_the_active_space_like_every_other_reader() {
    let store = ObrainStore::new_in_memory();
    store.create_space("tenant").expect("create_space(tenant)");
    store.use_graph("tenant");

    let backend: &dyn GraphBackend = store.as_ref();
    backend.create_property_index("qualified_name");
    let id = backend.create_node(
        &["Page"],
        vec![("qualified_name", Value::from("entity:tenant"))],
    );
    backend.commit().expect("commit");

    // Le même nœud, lu par les deux chemins du MÊME contrat.
    assert_eq!(
        backend.nodes_by_label("Page"),
        vec![id],
        "le nœud est bien dans l'espace actif"
    );
    assert_eq!(
        backend
            .node_property(id, "qualified_name")
            .and_then(|v| v.as_str().map(str::to_string)),
        Some("entity:tenant".to_string()),
        "sa propriété est bien lisible dans l'espace actif"
    );
    assert_eq!(
        backend.find_nodes_by_property("qualified_name", &Value::from("entity:tenant")),
        vec![id],
        "défaut constaté : `find_nodes_by_property` interroge l'index RACINE \
         (`self.db()`), pas l'espace actif — dans un espace nommé, le nœud \
         écrit par le trait reste introuvable par sa propriété indexée, alors \
         que `nodes_by_label` le rend. Les deux méthodes du contrat se \
         contredisent."
    );
}

// ---------------------------------------------------------------------------
// 4. Parité de lecture après écriture croisée, sur le MÊME store
// ---------------------------------------------------------------------------

/// Écrire par le chemin **obrain brut**, lire par le chemin **this-rs** :
/// les deux doivent voir la même chose — même nœud, même identifiant, même
/// propriété.
#[test]
fn writes_through_raw_obrain_are_visible_through_the_this_rs_facade() {
    let store = ObrainStore::new_in_memory();

    // Écriture BRUTE, sur l'ObrainDB sous-jacent.
    let raw_id = store.db().create_node_with_props(
        &["Page"],
        [("qualified_name", Value::from("ecrit-par-obrain"))],
    );
    store.db().commit_wal().expect("commit brut");

    // Lecture via this-rs (le trait), sur le même store.
    let via_facade: &dyn GraphBackend = store.as_ref();
    assert_eq!(
        via_facade.nodes_by_label("Page"),
        vec![raw_id],
        "this-rs doit voir le nœud écrit par obrain, avec le MÊME NodeId"
    );
    assert_eq!(
        via_facade.get_node(raw_id).map(|node| node.id),
        Some(raw_id),
        "`get_node` doit résoudre le même identifiant"
    );
    assert_eq!(
        via_facade
            .node_property(raw_id, "qualified_name")
            .and_then(|v| v.as_str().map(str::to_string)),
        Some("ecrit-par-obrain".to_string()),
        "la propriété écrite par obrain doit être lue par this-rs"
    );
    assert_eq!(compte(via_facade), 1, "un seul nœud, vu des deux côtés");
}

/// Le sens inverse : écrire par **this-rs**, lire par **obrain brut**.
#[test]
fn writes_through_the_facade_are_visible_through_raw_obrain() {
    let store = ObrainStore::new_in_memory();

    let via_facade: &dyn GraphBackend = store.as_ref();
    let id_this_rs = via_facade.create_node(
        &["Note"],
        vec![("qualified_name", Value::from("ecrit-par-this-rs"))],
    );
    let second = via_facade.create_node(&["Note"], vec![("qualified_name", Value::from("deux"))]);
    via_facade.create_edge(id_this_rs, second, "CITE");
    via_facade.commit().expect("commit");

    // Lecture BRUTE : `GraphStore` est en portée, donc `nodes_by_label` et
    // `all_labels` s'appliquent au magasin du moteur.
    let bruts = store.db().store().nodes_by_label("Note");
    assert_eq!(
        bruts.len(),
        2,
        "obrain brut doit voir les deux nœuds écrits par this-rs"
    );
    assert!(
        bruts.contains(&id_this_rs),
        "l'identifiant rendu par this-rs doit être celui du moteur"
    );
    assert!(
        store
            .db()
            .store()
            .all_labels()
            .contains(&"Note".to_string()),
        "le label écrit par this-rs doit être visible par obrain brut"
    );
    assert_eq!(
        store.db().store().out_degree(id_this_rs),
        1,
        "l'arête créée par this-rs doit être une arête du moteur, pas une convention locale"
    );
}

// ---------------------------------------------------------------------------
// 5. `commit()` — ce qui est vérifiable, et ce qui ne l'est pas en processus
// ---------------------------------------------------------------------------
//
// ⚠️ Constat de méthode, qui vaut pour tout ce fichier : dans CE processus,
// une « réouverture » (`ObrainStore::open` du même répertoire) n'en est pas une.
// obrain partage le substrat par répertoire **à l'intérieur du processus** (le
// verrou de fichier existant est rendu, cf. `storage/writer_lease.rs`), donc le
// second store voit l'état vivant du premier — y compris ses écritures NON
// commitées (mesuré, cf. `a_second_in_process_open_sees_uncommitted_writes`).
// Conséquence directe : la durabilité de `commit()` n'est PAS observable in
// processus. Les deux tests qui suivent disent chacun une moitié honnête, et le
// troisième porte le défaut.

/// Avec `commit()`, l'écriture est présente au store suivant — la moitié
/// vérifiable de la promesse.
#[test]
fn a_committed_write_is_present_when_the_store_is_opened_again() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");

    {
        let store = ObrainStore::open(dir.path()).expect("ouverture en écriture");
        let backend: &dyn GraphBackend = store.as_ref();
        backend.create_node(&["Page"], vec![("qualified_name", Value::from("committe"))]);
        backend.commit().expect("commit");
    }

    let rouvert = ObrainStore::open(dir.path()).expect("réouverture");
    let backend: &dyn GraphBackend = rouvert.as_ref();
    assert_eq!(compte(backend), 1, "l'écriture commitée doit être relue");
    assert_eq!(
        backend
            .nodes_by_label("Page")
            .into_iter()
            .filter_map(|id| backend.node_property(id, "qualified_name"))
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect::<Vec<String>>(),
        vec!["committe".to_string()],
        "c'est bien l'écriture commitée qu'on relit"
    );
    let id = backend.nodes_by_label("Page")[0];
    let relu = backend.get_node(id).expect("le nœud relu doit exister");
    assert_eq!(relu.id, id, "l'identité relue est celle demandée");
    assert_eq!(
        relu.labels
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<String>>(),
        vec!["Page".to_string()],
        "le label est relu tel qu'écrit"
    );
}

/// **Défaut constaté** — la durabilité de `commit()` n'est pas observable en
/// processus : une écriture **non commitée** survit à la réouverture du store.
///
/// Mesuré : `create_node` sans `commit()`, puis `drop` du store, puis
/// `ObrainStore::open(dir)` ⇒ le nœud est là, avec ses propriétés. Le second
/// store partage donc le substrat du premier (identifié par répertoire, dans le
/// processus), et non l'état persisté sur disque.
///
/// **Propriété observée, documentée telle quelle** : dans un même processus,
/// une écriture NON commitée survit à la fermeture et à la réouverture du store
/// — le substrat obrain est partagé par répertoire **intra-processus**. Ce test
/// ne valide donc pas « sans commit, l'écriture est perdue » : il **fige la
/// limite** de ce qu'un test in-process peut observer, pour que la promesse de
/// `GraphBackend::commit` (durabilité après un **redémarrage**) ne soit pas
/// confondue avec elle. Ce qui protège réellement l'écrivain dans le processus,
/// c'est le **bail** (section 8), pas le commit.
#[test]
fn an_uncommitted_write_survives_a_reopen_in_process_so_commit_is_not_observable_here() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");

    {
        let store = ObrainStore::open(dir.path()).expect("ouverture en écriture");
        let backend: &dyn GraphBackend = store.as_ref();
        backend.create_node(
            &["Page"],
            vec![("qualified_name", Value::from("non-committe"))],
        );
        // Aucun `commit()` — c'est tout l'objet du test.
        drop(store);
    }

    let rouvert = ObrainStore::open(dir.path()).expect("réouverture");
    let backend: &dyn GraphBackend = rouvert.as_ref();
    assert_eq!(
        backend.nodes_by_label("Page").len(),
        1,
        "observé : l'écriture non commitée survit à une réouverture DANS LE MÊME \
         PROCESSUS — `commit()` n'est donc pas observable ici, et la durabilité \
         promise ne se vérifie qu'après un vrai redémarrage"
    );
    assert_eq!(
        compte(backend),
        1,
        "observé, et c'est la limite : dans le même processus, un store rouvert \
         voit l'écriture non commitée — aucune observation in-process ne peut \
         distinguer « commitée » de « non commitée »"
    );
}

/// **La propriété observée, verrouillée telle quelle** — et c'est elle qui
/// justifie le bail d'écriture.
///
/// Un second `ObrainStore::open(dir)` du même répertoire, dans le même
/// processus, **voit les écritures non commitées du premier**. Deux « stores »
/// distincts sur le même répertoire ne sont donc pas isolés : ils partagent un
/// état vivant. Un consommateur qui croit ouvrir deux fois la même base pour la
/// lire en parallèle lit en réalité la même instance mutable.
///
/// C'est exactement le trou que `enable_write_fencing_with` ferme (section 8) :
/// à défaut d'un verrou d'écriture, obrain laisse faire.
#[test]
fn a_second_in_process_open_sees_uncommitted_writes_of_the_first() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");

    let premier = ObrainStore::open(dir.path()).expect("premier open");
    let b1: &dyn GraphBackend = premier.as_ref();
    b1.create_node(
        &["Page"],
        vec![("qualified_name", Value::from("vu-par-le-voisin"))],
    );
    // Toujours AUCUN commit.

    let second = ObrainStore::open(dir.path()).expect("second open");
    let b2: &dyn GraphBackend = second.as_ref();
    assert_eq!(
        compte(b2),
        1,
        "le second store du même répertoire voit l'écriture non commitée du premier \
         (substrat partagé intra-processus, comportement d'obrain)"
    );
    assert_eq!(
        b2.nodes_by_label("Page")
            .into_iter()
            .filter_map(|id| b2.node_property(id, "qualified_name"))
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect::<Vec<String>>(),
        vec!["vu-par-le-voisin".to_string()],
        "et il voit jusqu'à la propriété non commitée"
    );

    // Et la réciproque, qui précise la nature du partage : le second store est
    // une PHOTOGRAPHIE prise à son ouverture. Il ne suit PAS les écritures
    // ultérieures du premier, même commitées.
    b1.create_node(&["Page"], vec![("qualified_name", Value::from("apres"))]);
    b1.commit().expect("commit");
    assert_eq!(
        compte(b2),
        1,
        "un store déjà ouvert ne suit pas les écritures postérieures de son voisin, \
         même commitées : `commit()` ne se propage pas à un store déjà ouvert"
    );

    // Un TROISIÈME store, ouvert après l'écriture commitée, la voit — ce qui
    // montre que l'écriture est bien là, et que c'est la vue du second qui est
    // figée, pas l'écriture qui serait perdue.
    let troisieme = ObrainStore::open(dir.path()).expect("troisième open");
    let b3: &dyn GraphBackend = troisieme.as_ref();
    assert_eq!(
        compte(b3),
        2,
        "un store ouvert APRÈS l'écriture commitée la voit : l'écriture existe, \
         c'est la vue du store déjà ouvert qui ne se rafraîchit pas"
    );
}

// ---------------------------------------------------------------------------
// 6. Frontière assumée — ce que le trait ne couvre PAS, et pourquoi
// ---------------------------------------------------------------------------

/// Le contrat commun ne couvre **ni** les espaces nommés **ni** le bail.
///
/// Ce n'est pas une omission : obrain ne sait pas les tenir, et un défaut
/// silencieux laisserait croire qu'`ObrainDB` sait les faire. La propriété est
/// vérifiée **sur la source du trait** — un test qui ne ferait qu'appeler les
/// méthodes de `ObrainStore` ne prouverait rien sur ce que le trait expose.
#[test]
fn the_common_contract_covers_neither_named_spaces_nor_the_lease() {
    let source = include_str!("../src/storage/obrain_facade.rs");
    let debut = source
        .find("pub trait GraphBackend")
        .expect("le trait `GraphBackend` doit exister dans obrain_facade.rs");
    let fin = source[debut..]
        .find("/// obrain **brut**")
        .expect("la fin du trait doit être identifiable (le bloc qui suit)");
    let contrat = &source[debut..debut + fin];

    assert!(
        contrat.contains("fn nodes_by_label"),
        "garde-fou du test : la découpe doit bien contenir le corps du trait"
    );

    for hors_contrat in [
        "fn spaces",
        "fn space",
        "fn use_graph",
        "fn reset_graph",
        "fn create_space",
        "fn drop_space",
        "fn lease_info",
        "fn shard_id",
        "fn enable_write_fencing_with",
        "fn release_write_fencing",
    ] {
        assert!(
            !contrat.contains(hors_contrat),
            "le contrat commun ne doit pas exposer `{hors_contrat}` : obrain ne \
             sait pas le tenir, donc l'inclure serait un défaut silencieux côté \
             backend brut"
        );
    }
}

/// Un consommateur qui n'a besoin que du contrat commun **n'a pas** à connaître
/// les espaces ni le bail : son code est identique sur les deux backends, et il
/// reste correct pendant que l'espace change et que le bail est armé.
#[test]
fn a_consumer_needing_only_the_common_contract_ignores_spaces_and_lease() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let store = ObrainStore::open(dir.path()).expect("ouverture en écriture");

    // Le store porte un bail d'écriture ET un espace nommé…
    let epoch = store
        .enable_write_fencing_with(&FileAuthority::new(dir.path()))
        .expect("le bail doit s'armer");
    assert!(epoch >= 1);
    assert!(store.lease_info().is_some(), "le bail doit être armé");
    assert!(store.create_space("tenant").expect("create_space"));
    assert_eq!(store.spaces(), vec!["tenant".to_string()]);

    // (1) BAIL armé, graphe par défaut : parité PLEINE avec obrain brut.
    //     Le bail ne doit transparaître NULLE PART dans le contrat commun.
    let backend: &dyn GraphBackend = store.as_ref();
    charge(backend);
    let sous_bail = observe(backend);

    let raw = ObrainDB::new_in_memory();
    charge(&raw);
    let brut = observe(&raw);

    assert_eq!(
        sous_bail, brut,
        "un store sous bail doit rendre EXACTEMENT le même rapport qu'obrain brut : \
         le bail est un arbitrage d'écriture, il ne doit rien changer à ce qu'un \
         consommateur du contrat commun observe"
    );

    // (2) ESPACE nommé actif : le MÊME consommateur, inchangé, suit l'espace
    //     sans jamais le nommer — c'est la transparence promise à la bascule.
    store.use_graph("tenant");
    assert_eq!(
        compte(backend),
        0,
        "l'espace vient d'être créé : le MÊME consommateur, inchangé, lit maintenant \
         l'espace actif — il suit l'espace sans jamais le nommer"
    );
    assert_eq!(pages(backend), 0);

    // Le même consommateur, rejoué tel quel dans l'espace.
    charge(backend);
    let rapport_espace = observe(backend);
    assert_eq!(
        rapport_espace.node_count, 3,
        "la charge aboutit dans l'espace, par le même code de consommateur"
    );
    assert_eq!(rapport_espace.pages, vec![1, 2], "et elle y est complète");
    assert_eq!(
        rapport_espace.properties, sous_bail.properties,
        "les mêmes écritures produisent les mêmes propriétés, dans l'espace comme en racine"
    );
    assert_eq!(
        rapport_espace.outgoing, sous_bail.outgoing,
        "et le même graphe d'arêtes, avec les mêmes identifiants"
    );

    // Ce que le consommateur n'a PAS eu à connaître, lui, est bien là et vivant :
    assert!(store.lease_info().is_some());
    assert_eq!(store.space().as_deref(), Some("tenant"));
}

// ---------------------------------------------------------------------------
// 7. Branchement clé en main + bail par défaut
// ---------------------------------------------------------------------------

/// Le branchement clé en main, sur un store inscriptible, doit **réussir** et
/// **armer le bail**.
///
/// ⚠️ Un répertoire temporaire neuf par test : le bail est un verrou de fichier
/// exclusif tenu pour la vie du processus.
#[test]
fn the_one_call_wiring_arms_the_write_lease_by_default() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let store = ObrainStore::open(dir.path()).expect("ouverture en écriture");
    assert!(!store.is_read_only(), "le store doit être inscriptible");
    assert!(store.lease_info().is_none(), "aucun bail avant branchement");

    let host = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(store.clone()))
        .with_obrain_store(store.clone())
        .build_host()
        .expect("le branchement clé en main doit réussir sur un store inscriptible");

    assert_eq!(
        host.config.entities.len(),
        0,
        "le branchement ne doit rien inventer du catalogue"
    );
    let info = store
        .lease_info()
        .expect("un store inscriptible branché par défaut doit porter un bail");
    assert!(info.epoch >= 1, "l'époque doit être celle du premier bail");

    // Le bail couvre un SHARD — (store, espace) — nommé par une clé stable de
    // 16 hexadécimaux (FNV-1a 64), pas par le chemin : c'est cette clé qui
    // nomme le fichier de bail sur le disque.
    let attendu = store.shard_id().expect("shard du store branché").key();
    assert_eq!(
        info.shard, attendu,
        "le bail publié doit couvrir le shard du store effectivement branché"
    );
    assert_eq!(
        info.shard.len(),
        16,
        "une clé de shard fait 16 hexadécimaux"
    );
    assert!(
        info.shard.chars().all(|c| c.is_ascii_hexdigit()),
        "la clé de shard doit être hexadécimale, vu : {}",
        info.shard
    );
    assert_eq!(
        info.authority,
        Some(this::storage::writer_lease::AuthorityKind::File),
        "un bail armé par défaut doit publier QUI l'a délivré (autorité de fichier)"
    );
}

/// Un **second écrivain** sur le même répertoire doit être refusé, et le
/// message doit dire pourquoi (fencing / bail) — un refus muet serait
/// indiagnosticable en production.
#[test]
fn a_second_writer_on_the_same_directory_is_refused_with_a_lease_message() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");

    let premier = ObrainStore::open(dir.path()).expect("premier open");
    let _host = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(premier.clone()))
        .with_obrain_store(premier.clone())
        .build_host()
        .expect("le premier écrivain doit être accepté");
    assert!(premier.lease_info().is_some());

    // Le SECOND `open` réussit : c'est le comportement d'obrain (verrou
    // partagé intra-processus). C'est précisément le trou que le bail ferme.
    let second = ObrainStore::open(dir.path())
        .expect("obrain permet le second open — c'est le trou constaté");

    let resultat = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(second.clone()))
        .with_obrain_store(second.clone())
        .build_host();

    let erreur = match resultat {
        Ok(_) => panic!(
            "défaut : un second écrivain sur le même répertoire a été accepté — \
             le bail n'a pas arbitré"
        ),
        Err(e) => e,
    };
    let message = format!("{erreur}");
    assert!(
        message.contains("fencing") || message.contains("bail"),
        "le refus doit nommer sa cause (fencing/bail), vu : {message}"
    );
    let shard_attendu = premier.shard_id().expect("shard du premier").key();
    assert!(
        message.contains(&shard_attendu),
        "le refus doit nommer le shard en cause ({shard_attendu}), vu : {message}"
    );
    assert!(
        message.contains("without_write_fencing"),
        "le refus doit porter son remède (le désarmement explicite), vu : {message}"
    );
    assert!(
        second.lease_info().is_none(),
        "un build refusé ne doit laisser aucun bail armé sur le store refusé"
    );
}

/// Avec `.without_write_fencing()`, le second build **réussit** : l'arbitrage
/// est rendu à l'exploitant, sciemment.
#[test]
fn disarming_write_fencing_lets_a_second_writer_build() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");

    let premier = ObrainStore::open(dir.path()).expect("premier open");
    let _host = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(premier.clone()))
        .with_obrain_store(premier.clone())
        .build_host()
        .expect("le premier écrivain doit être accepté");

    let second = ObrainStore::open(dir.path()).expect("second open");

    // Sans désarmement : refusé (le cas du test précédent, revérifié ici pour
    // que le test porte sa propre prémisse).
    let refuse = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(second.clone()))
        .with_obrain_store(second.clone())
        .build_host();
    assert!(
        refuse.is_err(),
        "prémisse : sans désarmement, le second écrivain est refusé"
    );

    // Avec désarmement : accepté.
    let _host2 = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(second.clone()))
        .with_obrain_store(second.clone())
        .without_write_fencing()
        .build_host()
        .expect("`.without_write_fencing()` doit rendre le second build possible");
    assert!(
        second.lease_info().is_none(),
        "un store dont le fencing est désarmé ne doit porter aucun bail"
    );
}

/// Un store ouvert en **lecture seule** se branche **sans bail** : il n'écrit
/// pas, donc il n'y a rien à arbitrer.
#[test]
fn a_read_only_store_is_wired_without_any_lease() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");

    // Le store doit exister sur le disque avant d'être ouvert en lecture seule.
    {
        let initialisation = ObrainStore::open(dir.path()).expect("initialisation");
        initialisation
            .commit_wal()
            .expect("commit d'initialisation");
    }

    let store = ObrainStore::open_read_only(dir.path()).expect("ouverture en lecture seule");
    assert!(
        store.is_read_only(),
        "le mode lecture seule doit être publié"
    );

    let _host = ServerBuilder::new()
        .with_link_service(ObrainLinkService::new(store.clone()))
        .with_obrain_store(store.clone())
        .build_host()
        .expect("un store en lecture seule doit se brancher sans bail");

    assert!(
        store.lease_info().is_none(),
        "aucun bail ne doit être armé sur un store en lecture seule"
    );
}

// ---------------------------------------------------------------------------
// 8. Verrou intra-processus — la propriété qui a motivé le bail
// ---------------------------------------------------------------------------

/// **Le trou que this-rs ferme.**
///
/// Deux `ObrainStore::open()` du **même** répertoire réussissent tous les deux
/// (comportement d'obrain : la part de verrou existante est rendue au lieu
/// d'être refusée). Mais un seul des deux peut **armer un bail** : le second
/// reçoit une erreur de verrou refusé. C'est exactement la différence entre
/// « obrain laisse faire » et « this-rs refuse ».
#[test]
fn two_in_process_opens_succeed_but_only_one_can_arm_the_lease() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let authority = FileAuthority::new(dir.path());

    let premier = ObrainStore::open(dir.path()).expect("premier open");
    let second = ObrainStore::open(dir.path())
        .expect("second open : obrain le permet — c'est le trou intra-processus");

    // Deux stores distincts, sur le même répertoire.
    assert_ne!(
        premier.shard_id().expect("shard du premier").key(),
        String::new(),
        "un store sur disque doit avoir un shard nommable"
    );
    assert_eq!(
        premier.shard_id().expect("shard 1").key(),
        second.shard_id().expect("shard 2").key(),
        "les deux stores visent le MÊME shard : c'est ce qui rend le bail décisif"
    );

    // Le premier arme son bail.
    let epoch_premier = premier
        .enable_write_fencing_with(&authority)
        .expect("le premier écrivain doit pouvoir armer son bail");
    assert_eq!(epoch_premier, 1, "premier bail : époque 1");
    assert!(premier.lease_info().is_some());

    // Le second est REFUSÉ, dans le même processus.
    let refus = second.enable_write_fencing_with(&authority).expect_err(
        "défaut : le second store du même répertoire ne doit PAS pouvoir armer un bail",
    );
    let message = format!("{refus}");
    assert!(
        message.contains("verrou") || message.contains("lock") || message.contains("bail"),
        "l'erreur doit nommer le verrou de bail, vu : {message}"
    );
    assert!(
        second.lease_info().is_none(),
        "un store dont l'armement a échoué ne doit porter aucun bail"
    );

    // Après libération, le bail est reprenable — et l'époque progresse : c'est
    // le fencing monotone, celui qui périme l'écrivain précédent.
    premier.release_write_fencing();
    assert!(
        premier.lease_info().is_none(),
        "`release_write_fencing` doit rendre le bail"
    );

    let epoch_second = second
        .enable_write_fencing_with(&authority)
        .expect("après libération, le bail doit être reprenable");
    assert!(
        epoch_second > epoch_premier,
        "l'époque doit être STRICTEMENT croissante (vue : {epoch_premier} puis \
         {epoch_second}) — sinon un writer périmé repasserait pour le titulaire"
    );
}
