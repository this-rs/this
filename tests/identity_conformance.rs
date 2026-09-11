//! Conformité du contrat d'identité — intégration, sur un vrai store obrain.
//!
//! Ce fichier porte **la garantie qui compte** : le chemin historique
//! (`_uuid`/`_data`, celui qu'écrit `ObrainDataService`) doit rendre
//! **exactement les mêmes identifiants qu'avant**. Le chemin nouveau
//! (identité dérivée, pour les bases à propriétés plates) est **additif** —
//! c'est la seule façon d'être sûr qu'aucune application existante ne bouge.
//!
//! Ce que ces tests refusent, explicitement :
//! - qu'un nœud déjà identifié par `_uuid` reçoive un identifiant **dérivé**
//!   (ce serait une seconde identité pour la même entité) ;
//! - qu'une base étrangère reste invisible sous prétexte qu'elle n'a pas `_uuid` ;
//! - que les nœuds internes de this-rs (liens stockés en nœuds `_link:*`) soient
//!   pris pour des entités ;
//! - qu'un `_uuid` illisible soit **re-cléé** au lieu d'être rapporté.

#![cfg(feature = "obrain")]

use std::any::Any;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use this::core::entity::{Data, Entity};
use this::core::field::FieldValue;
use this::core::identity::{EntityKey, IdentityRegistry, Provenance, Qualifier};
use this::core::link::LinkEntity;
use this::core::service::{DataService, LinkService};
use this::storage::{ObrainDataService, ObrainLinkService, ObrainStore};
use uuid::Uuid;

/// Entité minimale — le strict nécessaire pour exercer `ObrainDataService`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Probe {
    id: Uuid,
    name: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Probe {
    fn new(name: &str) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        }
    }
}

impl Entity for Probe {
    type Service = ();

    fn resource_name() -> &'static str {
        "probes"
    }
    fn resource_name_singular() -> &'static str {
        "probe"
    }
    fn service_from_host(_: &Arc<dyn Any + Send + Sync>) -> anyhow::Result<Arc<Self::Service>> {
        Ok(Arc::new(()))
    }
    fn id(&self) -> Uuid {
        self.id
    }
    fn entity_type(&self) -> &str {
        "probe"
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    fn deleted_at(&self) -> Option<DateTime<Utc>> {
        None
    }
    fn status(&self) -> &str {
        &self.status
    }
}

impl Data for Probe {
    fn name(&self) -> &str {
        &self.name
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["name"]
    }
    fn field_value(&self, field: &str) -> Option<FieldValue> {
        match field {
            "name" => Some(FieldValue::String(self.name.clone())),
            _ => None,
        }
    }
}

/// Écrit un nœud à propriétés plates — ce que fait un outil tiers.
///
/// On passe par l'API **publique** du moteur (`db().session()`), exactement
/// comme un outil qui n'est pas this-rs : c'est le point du test.
fn write_flat_node(store: &ObrainStore, label: &str, props: &[(&str, &str)]) {
    let session = store.db().session();
    let pairs: Vec<(&str, obrain_common::types::Value)> = props
        .iter()
        .map(|(k, v)| (*k, obrain_common::types::Value::from(*v)))
        .collect();
    session.create_node_with_props(&[label], pairs);
}

// ---------------------------------------------------------------------------
// 1. NON-RÉGRESSION : une base écrite par this-rs garde ses identifiants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_store_written_by_this_rs_keeps_its_ids() {
    let store = ObrainStore::new_in_memory();
    let service = ObrainDataService::<Probe>::new(Arc::clone(&store));

    let a = service.create(Probe::new("a")).await.expect("création a");
    let b = service.create(Probe::new("b")).await.expect("création b");

    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);

    assert_eq!(
        report.total_resolved, 2,
        "deux entités attendues : {report:?}"
    );
    let label = &report.labels[0];
    assert_eq!(
        label.by_convention, 2,
        "sur une base this-rs, TOUT doit passer par la convention"
    );
    assert_eq!(
        label.by_derivation, 0,
        "aucun identifiant dérivé ne doit apparaître sur une base this-rs"
    );

    // Et l'identifiant rendu est EXACTEMENT celui de l'entité — pas un dérivé.
    for entity in [&a, &b] {
        let resolved = store
            .resolve_identity(&registry, &entity.id)
            .expect("résolution")
            .expect("entité indexée");
        assert_eq!(resolved.id, entity.id, "l'id doit être inchangé");
        assert_eq!(resolved.origin, Provenance::Convention);
        assert_eq!(resolved.motive, "convention:_uuid/_data");
        // `_data` fait autorité : le contenu vient de lui, pas des propriétés plates.
        assert_eq!(resolved.data["name"], entity.name);
    }
}

// ---------------------------------------------------------------------------
// 2. ADDITIVITÉ : une base étrangère est invisible pour la convention, lisible
//    par l'identité dérivée — les deux chiffres coexistent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_foreign_store_is_invisible_to_the_convention_and_readable_by_identity() {
    let store = ObrainStore::new_in_memory();
    write_flat_node(&store, "Page", &[("qualified_name", "entity:mp_hodge")]);

    // Chemin historique : la convention ne voit RIEN (c'est le « 0 » mesuré
    // sur les vraies bases d'aletheia).
    let service = ObrainDataService::<Probe>::new(Arc::clone(&store));
    assert!(
        service.list().await.expect("list").is_empty(),
        "sans `_uuid`, la convention ne voit rien"
    );

    // Chemin nouveau : l'entité est résolue, avec un identifiant DÉRIVÉ.
    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);
    assert_eq!(report.total_resolved, 1);
    assert_eq!(report.labels[0].by_derivation, 1);
    assert_eq!(report.labels[0].by_convention, 0);
}

// ---------------------------------------------------------------------------
// 3. VÉRIFIABILITÉ : l'identifiant se recalcule depuis la clé canonique
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_third_party_can_recompute_the_identity() {
    let store = ObrainStore::new_in_memory();
    write_flat_node(&store, "Page", &[("qualified_name", "entity:mp_hodge")]);
    let registry = IdentityRegistry::obrain_default();
    store.rebuild_identity_index(&registry);

    // Recalcul à partir des seules RÈGLES publiées (aucun accès au store).
    let expected = EntityKey::new(
        None,
        "Page",
        Qualifier::Natural {
            property: "qualified_name".into(),
            value: "entity:mp_hodge".into(),
        },
    )
    .expect("clé valide");

    let resolved = store
        .resolve_identity(&registry, &expected.uuid())
        .expect("résolution")
        .expect("entité retrouvée par un identifiant recalculé");
    assert_eq!(resolved.id, expected.uuid());
    assert_eq!(resolved.key.canonical_hex(), expected.canonical_hex());
}

// ---------------------------------------------------------------------------
// 4. LES NŒUDS INTERNES NE SONT PAS DES ENTITÉS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn link_nodes_are_not_entities() {
    let store = ObrainStore::new_in_memory();
    let links = ObrainLinkService::new(Arc::clone(&store));
    links
        .create(LinkEntity::new(
            "knows",
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
        ))
        .await
        .expect("création du lien");

    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);
    assert_eq!(
        report.total_resolved, 0,
        "un nœud `_link:*` est un détail d'implémentation, pas une entité"
    );
    assert!(
        report
            .labels
            .iter()
            .any(|l| l.skipped_reserved > 0 || l.label.starts_with("_link:")),
        "le nœud de lien doit être vu puis écarté : {report:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. UN `_uuid` ILLISIBLE EST RAPPORTÉ, JAMAIS RE-CLÉÉ
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_malformed_uuid_is_reported_and_never_rekeyed() {
    let store = ObrainStore::new_in_memory();
    write_flat_node(
        &store,
        "Page",
        &[
            ("_uuid", "pas-un-uuid"),
            ("qualified_name", "entity:mp_hodge"),
        ],
    );

    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);
    assert_eq!(
        report.total_resolved, 0,
        "donner un id dérivé ici créerait une SECONDE identité pour la même entité"
    );
    assert_eq!(report.labels[0].malformed.len(), 1);
    assert!(report.labels[0].malformed[0].contains("pas-un-uuid"));
}

// ---------------------------------------------------------------------------
// 6. LE RAPPORT EST HONNÊTE : il publie son schéma, ses bornes et ses raisons
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_report_publishes_its_scheme_chain_and_limits() {
    let store = ObrainStore::new_in_memory();
    write_flat_node(&store, "Page", &[("qualified_name", "entity:a")]);
    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);

    assert_eq!(report.schema, "this-rs/graph-identity/1");
    assert_eq!(report.scheme, this::core::identity::ENTITY_ID_SCHEME);
    assert_eq!(report.motives[0].motive, "convention:_uuid/_data");
    assert_eq!(
        report.motives[0].rank, 0,
        "la convention est en tête, non déplaçable"
    );
    assert_eq!(report.motives[1].motive, "derived:flat-property");
    assert_eq!(report.space, None);
    assert!(!report.truncated);

    // Le plafond est réel : il borne et le DIT.
    let capped = store.rebuild_identity_index_capped(&registry, 1, 100);
    assert_eq!(capped.total_nodes, 1);
    assert_eq!(capped.total_resolved, 1);
}
