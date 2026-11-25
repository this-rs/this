#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L3 (premier jet)** — persistance des expériences de routage dans Obrain.
//!
//! Principe : les ÉVÉNEMENTS (décision, observation) sont la source de vérité ; les
//! statistiques Alpha/Beta/n sont une **projection recalculée** à partir d'eux, jamais
//! un compteur opaque. Fermer, rouvrir, reconstruire : les mêmes nombres doivent revenir.
//!
//! Périmètre de concurrence supporté : **un seul écrivain, un seul processus** (le bail
//! d'écriture de `ObrainStore` le garantit). Aucun verrou inter-processus n'est supposé.
use std::sync::Arc;

use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "l3_persistance_{tag}_{}_{}",
        std::process::id(),
        Unique::suivant()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Petit compteur pour noms de répertoires uniques (pas d'uuid en dev-deps).
struct Unique;
impl Unique {
    fn suivant() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }
}

/// Écrit UNE observation, avec déduplication par identité stable.
/// Rend `true` si l'événement a été écrit, `false` s'il existait déjà (rejeu).
fn enregistrer_observation(
    store: &Arc<ObrainStore>,
    scope: &str,
    decision: &str,
    classe: &str,
    provider: &str,
    modele: &str,
    favorable: bool,
) -> bool {
    // Identité STABLE et SCOPÉE : ni la graine de décision ni l'ordre d'exécution ne
    // suffisent ; le périmètre (scope) fait partie de la clé.
    let evenement = format!("{scope}/{decision}");
    let existe = store
        .execute_query(
            "gql",
            &format!(
                "MATCH (o:observation) WHERE o.evenement = '{evenement}' RETURN o.evenement AS e"
            ),
            1,
        )
        // La réponse est un OBJET {columns, rows} : c'est `rows` qu'il faut lire,
        // pas `as_array()` sur l'objet (qui rendrait toujours « absent »).
        .map(|r| {
            r.get("rows")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if existe {
        return false;
    }
    let mut s = store.db().session();
    s.begin_transaction().expect("begin");
    let n = s.create_node(&["observation"]);
    s.set_node_property(n, "evenement", Value::String(evenement.into()));
    s.set_node_property(n, "scope", Value::String(scope.into()));
    s.set_node_property(n, "decision", Value::String(decision.into()));
    s.set_node_property(n, "classe", Value::String(classe.into()));
    s.set_node_property(n, "provider", Value::String(provider.into()));
    s.set_node_property(n, "modele", Value::String(modele.into()));
    s.set_node_property(n, "favorable", Value::Bool(favorable));
    s.commit().expect("commit");
    true
}

/// **Projection** Alpha/Beta/n recalculée depuis les événements (pas stockée).
fn projeter(store: &Arc<ObrainStore>, scope: &str) -> Vec<(String, String, u32, u32, u32)> {
    let r = store
        .execute_query(
            "gql",
            &format!("MATCH (o:observation) WHERE o.scope = '{scope}' RETURN o.provider AS p, o.modele AS m, o.favorable AS f"),
            1000,
        )
        .expect("requête GQL de projection");
    // Forme rendue : {"columns":[...],"rows":[[...],[...]]} — positionnel.
    let colonnes: Vec<String> = r
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let ip = colonnes.iter().position(|c| c == "p").unwrap_or(0);
    let im = colonnes.iter().position(|c| c == "m").unwrap_or(1);
    let ifav = colonnes.iter().position(|c| c == "f").unwrap_or(2);

    let mut table: std::collections::BTreeMap<(String, String), (u32, u32, u32)> =
        Default::default();
    for ligne in r
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let p = ligne
            .get(ip)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let m = ligne
            .get(im)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let f = ligne.get(ifav).and_then(|v| v.as_bool()).unwrap_or(false);
        let e = table.entry((p, m)).or_insert((1, 1, 0)); // a priori neutre 1/1
        e.2 += 1;
        if f {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    table
        .into_iter()
        .map(|((p, m), (a, b, n))| (p, m, a, b, n))
        .collect()
}

#[test]
fn l3_decision_observations_projection_fermeture_reouverture_rejeu() {
    let d = dir("cycle");
    let scope = "conv-42";
    let attendu;

    // 1+2+3. une décision, une observation favorable, une défavorable distincte
    {
        let store = ObrainStore::open(&d).expect("ouverture");
        assert!(enregistrer_observation(
            &store,
            scope,
            "dec-1",
            "chat.general",
            "a",
            "m1",
            true
        ));
        assert!(enregistrer_observation(
            &store,
            scope,
            "dec-2",
            "chat.general",
            "b",
            "m2",
            false
        ));
        // 4. projection
        attendu = projeter(&store, scope);
        assert_eq!(attendu.len(), 2, "deux bras observés");
        assert_eq!(
            attendu
                .iter()
                .find(|(p, _, _, _, _)| p == "a")
                .map(|(_, _, a, b, n)| (*a, *b, *n)),
            Some((2, 1, 1)),
            "a/m1 : a priori 1/1 + une réussite"
        );
        assert_eq!(
            attendu
                .iter()
                .find(|(p, _, _, _, _)| p == "b")
                .map(|(_, _, a, b, n)| (*a, *b, *n)),
            Some((1, 2, 1)),
            "b/m2 : a priori 1/1 + un échec"
        );
        // 5. rejeu DANS LA MÊME SESSION — store encore ouvert, aucun redémarrage.
        // C'est le cas que l'étape 8 ne mesure pas : là, la réouverture a eu lieu.
        // L'identité de l'événement est stable, le doublon est donc refusé sans
        // redémarrage, et la projection recalculée ne bouge pas.
        assert!(
            !enregistrer_observation(&store, scope, "dec-1", "chat.general", "a", "m1", true),
            "même session : le doublon est refusé"
        );
        assert_eq!(
            projeter(&store, scope),
            attendu,
            "un rejeu dans la même session ne change pas la projection"
        );
        store.commit_wal().expect("flush");
        // 6. fermeture
    }

    // 7+8. réouverture + reconstruction EXACTE
    let store = ObrainStore::open(&d).expect("réouverture");
    let apres = projeter(&store, scope);
    assert_eq!(
        apres, attendu,
        "les statistiques doivent être reconstruites à l'identique"
    );

    // 9. rejeu d'une observation déjà enregistrée : aucun double comptage
    assert!(
        !enregistrer_observation(&store, scope, "dec-1", "chat.general", "a", "m1", true),
        "le rejeu doit être refusé (identité stable)"
    );
    assert_eq!(
        projeter(&store, scope),
        attendu,
        "un rejeu ne doit pas modifier la projection"
    );

    let _ = std::fs::remove_dir_all(&d);
}
