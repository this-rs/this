//! Le BLOCAGE du premier trajet durable — et pourquoi un nœud « de même numéro »
//! ne le lève pas.
//!
//! `revise_with_id` publie la révision sur `NodeId(engram_id.0)` : il envoie
//! `engram_id.0.to_be_bytes()` au registre durable, qui le décode en `NodeId`. Or
//! `EngramId` (compteur de l'`EngramStore`) et `NodeId` (alloué par le substrat)
//! sont deux compteurs SÉPARÉS : il n'existe AUCUNE liaison canonique entre un
//! engramme et un nœud-ressource du magasin.
//!
//! Conséquence, mesurée ici :
//! - un engramme dont l'`EngramId` n'a pas de nœud au même numéro ne peut PAS être
//!   révisé durablement → refus NOMMÉ (`ressource ... inexistante`) malgré des
//!   membres réels et persistés ;
//! - créer un nœud dont le numéro égale l'`EngramId` ferait « réussir » le test en
//!   publiant l'effet sur un nœud ÉTRANGER au souvenir : un faux-mappage, jamais une
//!   preuve. Ce témoin ne s'y prête pas.
//!
//! Témoin du blocage, sur la branche unique, sans faux mappage ni registre bis.
#![cfg(feature = "obrain-cognitive")]

use std::sync::Arc;

use obrain_cognitive::engram::traits::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    Engram, EngramId, EngramManager, EngramManagerConfig, ObservationId, RevisionError,
    SourceRevision, SubstrateAtomicEventLedger,
};
use obrain_common::types::NodeId;
use obrain_engine::{ObrainDB, Session};
use obrain_substrate::SubstrateStore;

fn manager_sur(store: Arc<SubstrateStore>) -> EngramManager {
    let ledger = Arc::new(SubstrateAtomicEventLedger::new(store.clone()));
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
    .with_observation_ledger(ledger);
    manager.attach_substrate(store.writer_arc());
    manager
}

/// L'identifiant d'un nœud `Item` par son nom, via l'API publique du graphe.
fn id_de(session: &Session, nom: &str) -> NodeId {
    let res = session
        .execute(&format!("MATCH (n:Item {{name:'{nom}'}}) RETURN id(n)"))
        .expect("match id");
    NodeId(
        res.rows[0][0]
            .to_string()
            .parse::<u64>()
            .expect("id entier"),
    )
}

/// Insère un engramme à `id` explicite : c'est ce qui permet de distinguer l'EngramId
/// (compteur de l'EngramStore) du NodeId des membres.
fn engramme_avec(manager: &EngramManager, id: EngramId, membres: &[NodeId]) -> EngramId {
    let ensemble: Vec<(NodeId, f64)> = membres.iter().map(|n| (*n, 1.0)).collect();
    manager.store().insert(Engram::new(id, ensemble));
    id
}

#[test]
fn sans_liaison_canonique_l_engramme_ne_se_revise_pas_durablement() {
    let repertoire = std::env::temp_dir().join(format!(
        "this-engram-blocage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&repertoire);
    std::fs::create_dir_all(&repertoire).expect("répertoire de test");

    let db = ObrainDB::open(&repertoire).expect("ouverture de la base");
    let store = db.substrate_handle().expect("substrate handle");
    let manager = manager_sur(store.clone());

    // Deux nœuds RÉELS dans le vrai magasin — les membres du souvenir.
    let session = db.session();
    session
        .execute("CREATE (:Item {name:'a'}), (:Item {name:'b'})")
        .expect("create");
    let a = id_de(&session, "a");
    let b = id_de(&session, "b");
    eprintln!("membres réels : a={a:?} b={b:?}");

    // L'EngramId explicite est délibérément HORS de la plage des NodeId réels : il n'y a
    // aucun nœud substrat à ce numéro, donc aucun « hasard » ne peut faire passer le test.
    let id_engram = EngramId(9_999);
    engramme_avec(&manager, id_engram, &[a, b]);
    assert_eq!(
        manager.store().get(id_engram).map(|e| e.ensemble.len()),
        Some(2),
        "l'engramme a bien deux membres réels"
    );

    // CONSTAT — les membres existent, mais l'EngramId n'est la ressource d'AUCUN nœud.
    // La révision échoue NOMMÉMENT : c'est le blocage (pas un faux succès).
    match manager.revise_with_id(
        ObservationId::from_u128(1),
        id_engram,
        &[a],
        0.5,
        SourceRevision(0),
    ) {
        Err(RevisionError::PersistenceFailed(message)) => {
            eprintln!("blocage reproduit : {message}");
            assert!(
                message.contains("inexistante"),
                "le refus doit être NOMMÉ (ressource inexistante), mesuré : {message}"
            );
        }
        Err(autre) => panic!("erreur inattendue (ni faille ni succès) : {autre:?}"),
        Ok((resultat, _)) => panic!(
            "aucun nœud substrat à l'EngramId {id_engram:?} : la révision ne doit pas \
             « réussir » (faux-mappage attendu), mesuré {resultat:?}"
        ),
    }

    // L'engramme n'a PAS été muté par l'échec : rien n'est publié sur un nœud étranger.
    let membres: Vec<u64> = manager
        .store()
        .get(id_engram)
        .expect("engramme toujours présent")
        .ensemble
        .iter()
        .map(|(n, _)| n.0)
        .collect();
    assert_eq!(membres, vec![a.0, b.0], "l'échec n'a rien muté");

    db.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&repertoire);
}
