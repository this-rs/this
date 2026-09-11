//! Adaptateur obrain de l'identité canonique.
//!
//! Ce module est la moitié **I/O** de `crate::core::identity` : il transforme un
//! nœud du moteur en [`NodeView`], balaie un store pour bâtir l'index
//! d'identité, et projette les entités. La moitié **pure** (encodage, motifs,
//! chaîne de résolution) vit dans `core::identity` et se teste sans moteur.
//!
//! # Ce que ça change concrètement
//!
//! Une base écrite par this-rs (`_uuid`/`_data`) donne **exactement** les mêmes
//! identifiants qu'avant — la convention est en tête de chaîne. Une base écrite
//! par un autre outil (aletheia : `Page`/`Lemma`/`SeatVoice` à propriétés plates ;
//! un graphe de code : `Function`/`CALLS`) passe de **0** entité résolue à **N**,
//! avec des identifiants **dérivés** (uuid v8). Les deux chiffres coexistent :
//! c'est ce qui prouve que le chemin neuf est additif, pas substitutif.
//!
//! # Coût
//!
//! `rebuild_identity_index` est un **balayage complet** : il ne doit jamais être
//! appelé depuis le chemin d'une requête HTTP. La variante
//! `rebuild_identity_index_capped` existe pour ça, et l'absence de comptage se
//! **publie** (`nodes: None` + `nodes_error`) au lieu de se lire comme un zéro.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::identity::{
    IdentityError, IdentityRegistry, MotiveSpec, NodeView, PropValue, ResolvedEntity,
};
use crate::storage::ObrainStore;

// ---------------------------------------------------------------------------
// Conversion moteur → identité
// ---------------------------------------------------------------------------

/// Convertit une valeur du moteur en valeur d'identité.
///
/// Les variants scalaires sont convertis **exactement** (l'identité en dépend) ;
/// le reste passe par JSON, ce qui est documenté comme *lossy* pour les octets
/// bruts (voir [`PropValue::to_json`]).
fn prop_value(v: &obrain_common::types::Value) -> PropValue {
    use obrain_common::types::Value as V;
    match v {
        V::Null => PropValue::Null,
        V::Bool(b) => PropValue::Bool(*b),
        V::Int64(i) => PropValue::Int(*i),
        V::Float64(f) => PropValue::Float(*f),
        V::String(s) => PropValue::Str(s.to_string()),
        V::Bytes(b) => PropValue::Bytes(b.to_vec()),
        other => PropValue::Json(serde_json::to_value(other).unwrap_or(serde_json::Value::Null)),
    }
}

/// Vue d'identité d'un nœud du moteur.
///
/// Les propriétés sont **triées** (`properties_as_btree` les rend déjà dans
/// l'ordre lexicographique) : l'ordre d'insertion ne peut donc pas influencer un
/// identifiant dérivé.
pub fn node_view(node: &obrain_core::Node, space: Option<&str>) -> NodeView {
    let labels: Vec<String> = node.labels.iter().map(|l| l.to_string()).collect();
    let mut properties: BTreeMap<String, PropValue> = BTreeMap::new();
    for (key, value) in node.properties_as_btree() {
        properties.insert(key.as_str().to_string(), prop_value(&value));
    }
    NodeView {
        space: space.map(str::to_string),
        labels,
        properties,
    }
}

/// Les labels présents dans le store (introspection par le moteur).
///
/// Une liste vide n'est **pas** « aucun label » : les backends non-substrat
/// rendent la valeur par défaut du trait. C'est à l'appelant de publier la
/// raison (voir [`IdentityReport::labels_error`]).
pub fn discover_labels(store: &ObrainStore) -> Vec<String> {
    store.active_store().all_labels()
}

// ---------------------------------------------------------------------------
// Rapports
// ---------------------------------------------------------------------------

/// Ce qu'un label a donné, en compteurs **mesurés** (jamais supposés).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelReport {
    /// Label examiné.
    pub label: String,
    /// Nœuds comptés — `None` si le comptage n'a pas été fait ou a échoué.
    pub nodes: Option<usize>,
    /// Pourquoi le comptage manque, le cas échéant.
    pub nodes_error: Option<String>,
    /// Résolus par la convention `_uuid`/`_data` (identifiant inchangé).
    pub by_convention: usize,
    /// Résolus par identité **dérivée** (uuid v8).
    pub by_derivation: usize,
    /// Nœuds ignorés faute de motif applicable (repli `Skip`).
    pub skipped: usize,
    /// Labels réservés écartés (nœuds internes `_link:*` de this-rs).
    pub skipped_reserved: usize,
    /// Nœuds non résolubles (label réservé, clé absente…).
    pub unresolvable: usize,
    /// Nœuds **incohérents** : rapportés, jamais re-cléés.
    pub malformed: Vec<String>,
}

/// Le rapport d'un balayage d'identité — publié tel quel par `GET /graph/identity`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityReport {
    /// Identifiant de schéma du rapport.
    pub schema: String,
    /// Schéma d'identité en vigueur ([`crate::core::identity::ENTITY_ID_SCHEME`]).
    pub scheme: String,
    /// Espace balayé — `None` = graphe par défaut.
    pub space: Option<String>,
    /// Motifs de la chaîne, dans l'ordre, tels qu'appliqués.
    pub motives: Vec<MotiveSpec>,
    /// Repli en vigueur.
    pub fallback: crate::core::identity::Fallback,
    /// Par label.
    pub labels: Vec<LabelReport>,
    /// Pourquoi l'introspection des labels a échoué (liste vide ≠ aucun label).
    pub labels_error: Option<String>,
    /// Total de nœuds examinés.
    pub total_nodes: usize,
    /// Total d'entités résolues (convention + dérivation).
    pub total_resolved: usize,
    /// Le balayage a-t-il été tronqué par un plafond ?
    pub truncated: bool,
}

impl IdentityReport {
    /// Le rapport vide d'un store dont l'espace vient d'être fixé.
    fn empty(registry: &IdentityRegistry, space: Option<String>) -> Self {
        Self {
            schema: "this-rs/graph-identity/1".to_string(),
            scheme: crate::core::identity::ENTITY_ID_SCHEME.to_string(),
            space,
            motives: registry.motives(),
            fallback: registry.fallback(),
            labels: Vec::new(),
            labels_error: None,
            total_nodes: 0,
            total_resolved: 0,
            truncated: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Balayage
// ---------------------------------------------------------------------------

impl ObrainStore {
    /// Résout l'identité d'un nœud déjà lu.
    pub fn project_node(
        &self,
        registry: &IdentityRegistry,
        node: &obrain_core::Node,
    ) -> Result<ResolvedEntity, IdentityError> {
        registry.project(&node_view(node, self.space().as_deref()))
    }

    /// Résout une identité **depuis son identifiant**, si elle est indexée.
    ///
    /// Le nœud est relu puis re-projeté : la réponse porte donc la clé canonique
    /// et les octets hachés, qu'un tiers peut recalculer.
    pub fn resolve_identity(
        &self,
        registry: &IdentityRegistry,
        id: &Uuid,
    ) -> Result<Option<ResolvedEntity>, IdentityError> {
        let Some(handle) = self.identity_get(id) else {
            return Ok(None);
        };
        let session = self.session();
        let Some(node) = session.get_node(obrain_common::types::NodeId(handle)) else {
            return Ok(None);
        };
        self.project_node(registry, &node).map(Some)
    }

    /// Identifiant indexé → handle du moteur.
    pub fn identity_get(&self, id: &Uuid) -> Option<u64> {
        self.identity_index()
            .read()
            .ok()
            .and_then(|idx| idx.get(id))
    }

    /// Nombre d'identités indexées.
    pub fn identity_len(&self) -> usize {
        self.identity_index()
            .read()
            .map(|idx| idx.len())
            .unwrap_or(0)
    }

    /// Espace de l'index d'identité.
    pub fn identity_space(&self) -> Option<String> {
        self.identity_index()
            .read()
            .ok()
            .and_then(|idx| idx.space().map(str::to_string))
    }

    /// Balaie **tout** le store et reconstruit l'index d'identité.
    ///
    /// **Coût** : O(nœuds) — plusieurs secondes sur des millions d'éléments. À
    /// n'appeler que hors requête (démarrage, outil, tâche de fond).
    pub fn rebuild_identity_index(&self, registry: &IdentityRegistry) -> IdentityReport {
        self.rebuild_identity_index_capped(registry, usize::MAX, usize::MAX)
    }

    /// Variante **bornée** — la seule utilisable depuis HTTP.
    pub fn rebuild_identity_index_capped(
        &self,
        registry: &IdentityRegistry,
        max_per_label: usize,
        max_total: usize,
    ) -> IdentityReport {
        let space = self.space();
        let mut report = IdentityReport::empty(registry, space.clone());

        // L'index repart de l'espace courant : deux espaces ne se mélangent pas.
        if let Ok(mut index) = self.identity_index().write() {
            index.clear(space.clone());
        }

        let labels = discover_labels(self);
        if labels.is_empty() {
            // Une liste vide se lirait « aucune entité ». Or l'API ne permet PAS
            // de distinguer « store vide » de « backend non introspectable » :
            // le trait rend `Vec::new()` par défaut et le substrat lit ses
            // registres vivants — les deux cas produisent le même `[]`. On
            // publie donc l'ambiguïté au lieu de trancher à tort.
            report.labels_error = Some(
                "aucun label rendu par le moteur : soit le store est vide, soit \
                 l'introspection n'est pas disponible sur ce backend — les deux cas sont \
                 INDISCERNABLES par l'API actuelle (trait GraphStore::all_labels). En aucun cas \
                 cela ne signifie « aucune entité » sans autre vérification."
                    .to_string(),
            );
            return report;
        }

        let session = self.session();
        let mut index = match self.identity_index().write() {
            Ok(index) => index,
            Err(_) => {
                report.labels_error = Some("index d'identité verrouillé (poisoned)".to_string());
                return report;
            }
        };

        for label in labels {
            let mut lr = LabelReport {
                label: label.clone(),
                nodes: None,
                nodes_error: None,
                by_convention: 0,
                by_derivation: 0,
                skipped: 0,
                skipped_reserved: 0,
                unresolvable: 0,
                malformed: Vec::new(),
            };

            let node_ids = self.active_store().nodes_by_label(&label);
            let total = node_ids.len();
            lr.nodes = Some(total);

            let mut examined = 0usize;
            for nid in node_ids {
                if examined >= max_per_label || report.total_nodes >= max_total {
                    report.truncated = true;
                    break;
                }
                examined += 1;
                report.total_nodes += 1;

                let Some(node) = session.get_node(nid) else {
                    lr.unresolvable += 1;
                    continue;
                };
                let view = node_view(&node, space.as_deref());

                // Labels réservés : les nœuds internes de this-rs (`_link:*`)
                // ne sont pas des entités.
                if registry.canonical_label(&view.labels).is_none() {
                    lr.skipped_reserved += 1;
                    continue;
                }

                match registry.project(&view) {
                    Ok(entity) => {
                        let handle = u64::from(node.id.0);
                        if index.insert(space.as_deref(), entity.id, handle) {
                            match entity.origin {
                                crate::core::identity::Provenance::Convention => {
                                    lr.by_convention += 1
                                }
                                _ => lr.by_derivation += 1,
                            }
                            report.total_resolved += 1;
                        } else {
                            lr.unresolvable += 1;
                        }
                    }
                    Err(IdentityError::NotApplicable { .. }) => lr.skipped += 1,
                    Err(IdentityError::Unresolvable { .. }) => lr.unresolvable += 1,
                    Err(IdentityError::Malformed { node, reason, .. }) => {
                        lr.malformed.push(format!("{node}: {reason}"));
                    }
                }
            }

            report.labels.push(lr);
        }

        report
    }
}

// ---------------------------------------------------------------------------
// Tests (feature `obrain`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::{ConventionProjection, Fallback, Provenance};

    /// Crée un nœud à propriétés plates dans le store (comme le ferait un autre outil).
    fn flat_node(store: &ObrainStore, label: &str, props: &[(&str, &str)]) {
        let session = store.session();
        let pairs: Vec<(&str, obrain_common::types::Value)> = props
            .iter()
            .map(|(k, v)| (*k, obrain_common::types::Value::from(*v)))
            .collect();
        session.create_node_with_props(&[label], pairs);
    }

    fn store() -> std::sync::Arc<ObrainStore> {
        ObrainStore::new_in_memory()
    }

    #[test]
    fn flat_properties_are_adopted_where_the_convention_sees_nothing() {
        let s = store();
        flat_node(&s, "Page", &[("qualified_name", "entity:a")]);
        flat_node(&s, "Page", &[("qualified_name", "entity:b")]);

        // Convention seule : 0 (c'est le chiffre d'aujourd'hui sur aletheia).
        let conv = IdentityRegistry::new();
        let r1 = s.rebuild_identity_index(&conv);
        assert_eq!(
            r1.total_resolved, 0,
            "aucune convention `_uuid` dans ce store"
        );

        // Convention PUIS propriétés plates : les deux entités sont résolues.
        let reg = IdentityRegistry::obrain_default();
        let r2 = s.rebuild_identity_index(&reg);
        assert_eq!(r2.total_resolved, 2, "identité dérivée : {r2:?}");
        assert_eq!(r2.labels[0].by_derivation, 2);
        assert_eq!(r2.labels[0].by_convention, 0);
        assert_eq!(s.identity_len(), 2);
    }

    #[test]
    fn convention_still_wins_when_both_marks_are_present() {
        let s = store();
        let uuid = Uuid::new_v4();
        flat_node(
            &s,
            "Page",
            &[("_uuid", &uuid.to_string()), ("qualified_name", "entity:x")],
        );
        let reg = IdentityRegistry::obrain_default();
        let report = s.rebuild_identity_index(&reg);
        assert_eq!(report.total_resolved, 1);
        assert_eq!(report.labels[0].by_convention, 1);
        assert_eq!(report.labels[0].by_derivation, 0);
        // L'identifiant rendu est celui de la convention, inchangé.
        let e = s
            .resolve_identity(&reg, &uuid)
            .expect("résolution")
            .expect("entité");
        assert_eq!(e.origin, Provenance::Convention);
        assert_eq!(e.id, uuid);
    }

    #[test]
    fn malformed_uuid_is_reported_and_never_rekeyed() {
        let s = store();
        flat_node(
            &s,
            "Page",
            &[("_uuid", "pas-un-uuid"), ("qualified_name", "entity:x")],
        );
        let reg = IdentityRegistry::obrain_default();
        let report = s.rebuild_identity_index(&reg);
        assert_eq!(report.total_resolved, 0, "on ne re-clée pas : {report:?}");
        assert_eq!(report.labels[0].malformed.len(), 1);
        assert!(report.labels[0].malformed[0].contains("pas-un-uuid"));
    }

    #[test]
    fn reserved_labels_are_not_entities() {
        let s = store();
        flat_node(&s, "_link:owner", &[("qualified_name", "link:1")]);
        let reg = IdentityRegistry::obrain_default();
        let report = s.rebuild_identity_index(&reg);
        assert_eq!(report.total_resolved, 0);
        assert_eq!(report.labels[0].skipped_reserved, 1);
    }

    #[test]
    fn empty_label_listing_publishes_the_ambiguity_instead_of_a_zero() {
        // Store vide : `all_labels()` rend `[]`, et l'API ne permet PAS de dire
        // si c'est « vide » ou « backend non introspectable ». Le rapport doit
        // donc publier l'ambiguïté — jamais un zéro présenté comme un fait.
        let s = store();
        let reg = IdentityRegistry::obrain_default();
        let report = s.rebuild_identity_index_capped(&reg, 0, 0);
        assert_eq!(report.total_resolved, 0);
        let err = report.labels_error.expect("l'ambiguïté doit être publiée");
        assert!(
            err.contains("INDISCERNABLES"),
            "message honnête attendu : {err}"
        );

        // Et dès qu'un nœud existe, l'introspection rend le label : plus d'ambiguïté.
        flat_node(&s, "Page", &[("qualified_name", "entity:a")]);
        let r2 = s.rebuild_identity_index(&reg);
        assert!(r2.labels_error.is_none());
        assert_eq!(r2.total_resolved, 1);
    }

    #[test]
    fn content_digest_fallback_is_opt_in_on_a_real_store() {
        let s = store();
        flat_node(
            &s,
            "Doc",
            &[("body", "corps"), ("updated_at", "2026-01-01")],
        );

        let skip = IdentityRegistry::obrain_default();
        assert_eq!(s.rebuild_identity_index(&skip).total_resolved, 0);

        let reg = IdentityRegistry::obrain_default().with_fallback(Fallback::ContentDigest);
        assert_eq!(s.rebuild_identity_index(&reg).total_resolved, 1);
    }

    #[test]
    fn identity_index_is_scoped_to_the_space_and_reset_by_a_switch() {
        let s = store();
        flat_node(&s, "Page", &[("qualified_name", "entity:a")]);
        let reg = IdentityRegistry::obrain_default();
        assert_eq!(s.rebuild_identity_index(&reg).total_resolved, 1);
        assert_eq!(s.identity_space(), None);

        // Un espace nommé exige un substrat : on le crée puis on bascule.
        if s.create_space("tenant-a").is_ok() {
            s.use_graph("tenant-a");
            // La bascule purge l'index (les NodeId des deux espaces repartent de 1).
            assert_eq!(s.identity_len(), 0, "l'index ne survit pas à une bascule");
            assert_eq!(s.identity_space().as_deref(), Some("tenant-a"));
            // Dans l'espace vide, aucune entité.
            assert_eq!(s.rebuild_identity_index(&reg).total_resolved, 0);
            s.reset_graph();
        }
    }

    #[test]
    fn report_publishes_its_scheme_and_chain() {
        let s = store();
        flat_node(&s, "Page", &[("qualified_name", "entity:a")]);
        let reg = IdentityRegistry::obrain_default();
        let r = s.rebuild_identity_index(&reg);
        assert_eq!(r.schema, "this-rs/graph-identity/1");
        assert_eq!(r.scheme, crate::core::identity::ENTITY_ID_SCHEME);
        assert_eq!(r.motives[0].motive, "convention:_uuid/_data");
        assert_eq!(r.motives[1].motive, "derived:flat-property");
        assert!(!r.truncated);
        // Le motif convention est bien celui qu'on croit.
        assert_eq!(
            std::any::type_name::<ConventionProjection>(),
            "this::core::identity::ConventionProjection"
        );
    }
}
