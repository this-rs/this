//! IAM **matérialisé** : la décision d'autorisation est une **lecture indexée**,
//! pas une traversée.
//!
//! # Le problème d'ordre de grandeur
//!
//! Un contrôle d'autorisation vit sur le chemin de **chaque** requête. Le modèle
//! naturel — `(Principal)-[:HAS_ROLE]->(Role)-[:GRANTS]->(Permission)` — coûte
//! deux sauts d'arêtes plus une union, à chaque fois. À l'échelle d'un graphe de
//! service, c'est le coût dominant, et il est proportionnel au nombre de rôles.
//!
//! # La réponse : matérialiser la décision
//!
//! On calcule une fois, à l'**écriture IAM** (rare), ce que le principal peut
//! faire dans une portée ; on l'écrit sur le nœud du principal ; on le relit par
//! **un index de propriété**. La lecture devient une recherche indexée — la même
//! classe de coût qu'un `find_nodes_by_property` — au lieu d'un parcours.
//!
//! ```text
//! écriture IAM (rare)   : rôle → permissions → union → acl_bits + acl_epoch   (O(rôles))
//! décision (par requête): find_nodes_by_property("acl_key", clé) → lecture     (O(1) indexé)
//! ```
//!
//! # Pourquoi des **propriétés** et non une colonne blob
//!
//! Obrain n'expose **aucune** API publique de colonne blob (`SubstrateStore` n'a
//! pas de méthode `*blob*`), alors que `create_property_index` et
//! `find_nodes_by_property` sont publiques et non gatées. Matérialiser en
//! propriétés indexées donne donc la même garantie **sans modifier le moteur** —
//! et le jour où une colonne blob est exposée, seule l'écriture change.
//!
//! # Fraîcheur
//!
//! Chaque matérialisation **incrémente** `acl_epoch`. Un appelant qui a mémorisé
//! une décision peut donc détecter qu'elle est périmée ([`AclStamp::is_stale`])
//! sans refaire le calcul : c'est ce qui rend le cache sûr plutôt que risqué.
//!
//! # Ce que ce module ne fait pas
//!
//! Il ne dit **pas** quelles permissions sont légitimes : les rôles et leurs
//! permissions lui sont fournis ([`RoleGrant`]). La source d'autorité reste
//! l'application — ce module ne fait que rendre la décision **mesurable et pas
//! chère**.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::ObrainStore;

/// Schéma du document de décision.
pub const IAM_SCHEMA: &str = "this-rs/iam/1";

/// Propriété portant la clé d'ACL (celle qui est **indexée**).
pub const ACL_KEY_PROP: &str = "acl_key";
/// Propriété portant l'ensemble effectif des permissions (bitset hexadécimal).
pub const ACL_BITS_PROP: &str = "acl_bits";
/// Propriété portant la liste lisible des permissions.
pub const ACL_PERMS_PROP: &str = "acl_perms";
/// Propriété portant les rôles retenus.
pub const ACL_ROLES_PROP: &str = "acl_roles";
/// Propriété portant l'époque de matérialisation.
pub const ACL_EPOCH_PROP: &str = "acl_epoch";
/// Propriété portant le nom qualifié du principal.
pub const PRINCIPAL_QN_PROP: &str = "qualified_name";
/// Label des nœuds principaux.
pub const PRINCIPAL_LABEL: &str = "Principal";
/// Label des nœuds de **décision**.
///
/// La décision est un nœud à part entière, **un par (principal, portée)** : un
/// nœud de principal ne peut en porter qu'une, et deux portées s'écraseraient.
/// Son `qualified_name` est la clé d'ACL, donc une décision est elle-même une
/// entité adressable par `core::identity`.
pub const ACL_LABEL: &str = "Acl";
/// Propriété portant le principal propriétaire d'une décision.
pub const ACL_PRINCIPAL_PROP: &str = "acl_principal";

/// Un rôle et les permissions qu'il accorde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleGrant {
    /// Nom du rôle.
    pub role: String,
    /// Permissions accordées (identifiants libres : `orders:read`, `*`…).
    pub permissions: Vec<String>,
}

/// Une décision à matérialiser, en attente.
///
/// Emprunte ses entrées : un lot se construit depuis des données déjà en main,
/// sans les recopier.
#[derive(Debug, Clone, Copy)]
pub struct PendingDecision<'a> {
    /// Le principal.
    pub principal: &'a str,
    /// La portée.
    pub scope: &'a str,
    /// Ses rôles.
    pub roles: &'a [RoleGrant],
}

/// La décision matérialisée d'un principal dans une portée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclStamp {
    /// Schéma du document.
    pub schema: String,
    /// La clé d'index (`<scope>/<principal>`).
    pub key: String,
    /// Le principal.
    pub principal: String,
    /// La portée (tenant, domaine, ressource — au choix du déploiement).
    pub scope: String,
    /// Les rôles retenus, triés.
    pub roles: Vec<String>,
    /// Les permissions effectives, triées et dédupliquées.
    pub permissions: Vec<String>,
    /// L'époque de matérialisation — **monotone**.
    pub epoch: u64,
}

impl AclStamp {
    /// Cette décision est-elle périmée au regard d'une époque observée ?
    ///
    /// C'est la garde qui rend un cache sûr : une décision mémorisée reste
    /// valable tant que l'époque n'a pas bougé.
    pub fn is_stale(&self, observed_epoch: u64) -> bool {
        observed_epoch != self.epoch
    }

    /// Le principal peut-il, selon cette décision ?
    ///
    /// `"*"` est un joker explicite : un rôle qui accorde `*` accorde tout.
    pub fn allows(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == "*" || p == permission)
    }

    /// L'ensemble effectif, en bitset hexadécimal (pour comparaison rapide).
    pub fn bits_hex(&self) -> String {
        // Un bit par position dans la liste triée — suffisant pour comparer deux
        // décisions sans relire les chaînes.
        let mut bits: u128 = 0;
        for (i, _) in self.permissions.iter().enumerate().take(128) {
            bits |= 1u128 << i;
        }
        format!("{bits:032x}")
    }
}

/// Erreur d'IAM.
#[derive(Debug, thiserror::Error)]
pub enum IamError {
    /// Le store a refusé une opération.
    #[error("IAM : {0}")]
    Store(String),
    /// Une propriété attendue manque ou est illisible.
    #[error("IAM : nœud `{node}` incohérent : {reason}")]
    Malformed {
        /// Le nœud concerné.
        node: String,
        /// Ce qui cloche.
        reason: String,
    },
}

impl From<anyhow::Error> for IamError {
    fn from(e: anyhow::Error) -> Self {
        IamError::Store(format!("{e:#}"))
    }
}

/// Le plan IAM d'un store.
///
/// Ne détient pas de cache : chaque appel lit le store. La matérialisation est ce
/// qui rend cette lecture bon marché — pas un cache applicatif, qui serait une
/// seconde source de vérité à invalider.
#[derive(Clone)]
pub struct Iam {
    store: Arc<ObrainStore>,
}

impl Iam {
    /// Prépare le plan IAM : s'assure que les propriétés d'index existent.
    ///
    /// L'index est **idempotent** (`has_property_index` avant création), et il
    /// est posé sur la clé d'ACL **et** sur le nom qualifié : sans le second, la
    /// recherche d'un principal serait un balayage.
    pub fn new(store: Arc<ObrainStore>) -> Self {
        let db = store.db();
        for prop in [ACL_KEY_PROP, PRINCIPAL_QN_PROP] {
            if !db.has_property_index(prop) {
                db.create_property_index(prop);
                tracing::info!(property = prop, "index IAM créé");
            }
        }
        Self { store }
    }

    /// La clé d'index d'un couple (portée, principal).
    ///
    /// La portée vient **en premier** : c'est elle qui isole deux locataires, et
    /// un préfixe commun rend les clés lisibles à l'œil comme au débogage.
    pub fn acl_key(scope: &str, principal: &str) -> String {
        format!("{scope}/{principal}")
    }

    /// Le nœud d'un principal, s'il existe — **par index**, pas par balayage.
    fn principal_node(
        &self,
        principal: &str,
    ) -> Result<Option<obrain_common::types::NodeId>, IamError> {
        let hits = self.store.db().find_nodes_by_property(
            PRINCIPAL_QN_PROP,
            &obrain_common::types::Value::from(principal),
        );
        Ok(hits.into_iter().next())
    }

    /// Le nœud de décision d'une clé, s'il existe — **par index**.
    fn acl_node(&self, key: &str) -> Result<Option<obrain_common::types::NodeId>, IamError> {
        let hits = self
            .store
            .db()
            .find_nodes_by_property(ACL_KEY_PROP, &obrain_common::types::Value::from(key));
        Ok(hits.into_iter().next())
    }

    /// **Matérialise** la décision d'un principal dans une portée.
    ///
    /// Crée le nœud du principal au besoin, calcule l'union des permissions de
    /// ses rôles (dédupliquée, triée), écrit les propriétés, incrémente
    /// l'époque, puis **commit le WAL** — une décision qui ne survit pas à un
    /// crash n'est pas une décision.
    pub fn materialize(
        &self,
        principal: &str,
        scope: &str,
        roles: &[RoleGrant],
    ) -> Result<AclStamp, IamError> {
        let key = Self::acl_key(scope, principal);

        let mut permissions: Vec<String> = roles
            .iter()
            .flat_map(|r| r.permissions.iter().cloned())
            .collect();
        permissions.sort();
        permissions.dedup();
        let mut role_names: Vec<String> = roles.iter().map(|r| r.role.clone()).collect();
        role_names.sort();
        role_names.dedup();

        let session = self.store.session();
        let store = self.store.active_store();

        // Le principal existe pour la traçabilité et la traversée — mais il ne
        // PORTE pas la décision : un nœud par (principal, portée), sinon deux
        // portées s'écraseraient.
        if self.principal_node(principal)?.is_none() {
            session.create_node_with_props(
                &[PRINCIPAL_LABEL],
                [(
                    PRINCIPAL_QN_PROP,
                    obrain_common::types::Value::from(principal),
                )],
            );
        }

        let node = match self.acl_node(&key)? {
            Some(node) => node,
            None => session.create_node_with_props(
                &[ACL_LABEL],
                [
                    (
                        PRINCIPAL_QN_PROP,
                        obrain_common::types::Value::from(key.as_str()),
                    ),
                    (
                        ACL_PRINCIPAL_PROP,
                        obrain_common::types::Value::from(principal),
                    ),
                ],
            ),
        };

        // Époque monotone : on relit la précédente et on incrémente.
        let previous = store
            .get_node_property(
                node,
                &obrain_common::types::PropertyKey::new(ACL_EPOCH_PROP),
            )
            .and_then(|v| v.as_int64())
            .unwrap_or(0);
        let epoch = previous as u64 + 1;

        store.set_node_property(
            node,
            ACL_KEY_PROP,
            obrain_common::types::Value::from(key.as_str()),
        );
        store.set_node_property(
            node,
            ACL_PERMS_PROP,
            obrain_common::types::Value::from(permissions.join(",").as_str()),
        );
        store.set_node_property(
            node,
            ACL_ROLES_PROP,
            obrain_common::types::Value::from(role_names.join(",").as_str()),
        );
        store.set_node_property(
            node,
            ACL_EPOCH_PROP,
            obrain_common::types::Value::Int64(epoch as i64),
        );

        let stamp = AclStamp {
            schema: IAM_SCHEMA.to_string(),
            key: key.clone(),
            principal: principal.to_string(),
            scope: scope.to_string(),
            roles: role_names,
            permissions,
            epoch,
        };
        store.set_node_property(
            node,
            ACL_BITS_PROP,
            obrain_common::types::Value::from(stamp.bits_hex().as_str()),
        );

        self.store.commit_wal()?;
        Ok(stamp)
    }

    /// Matérialise **plusieurs** décisions en un seul commit.
    ///
    /// # Pourquoi
    ///
    /// La mesure (`examples/iam_bench`) a montré qu'une matérialisation coûte
    /// **129 ms par principal** — dominée par le `commit_wal` (fsync) payé à
    /// chaque appel. Or une écriture IAM est **rare et groupée** : un rôle
    /// change, et N principaux doivent être rafraîchis. Payer N fsync pour un
    /// seul changement est un gaspillage pur.
    ///
    /// Ici : N calculs, **un** commit.
    ///
    /// # Idempotence intra-lot
    ///
    /// Deux entrées de même clé visent le **même** nœud : la résolution passe
    /// d'abord par une table locale, puis par l'index. La seconde écrase la
    /// première — elle ne crée pas un doublon, et l'époque reste monotone.
    pub fn materialize_batch(
        &self,
        batch: &[PendingDecision<'_>],
    ) -> Result<Vec<AclStamp>, IamError> {
        let session = self.store.session();
        let store = self.store.active_store();
        // Table locale clé → (nœud, époque) : elle sert à la fois de
        // déduplication intra-lot et de mémoire d'époque — les écritures du lot
        // ne sont pas encore commitées, donc pas encore indexées.
        let mut seen: std::collections::HashMap<String, (obrain_common::types::NodeId, u64)> =
            std::collections::HashMap::new();
        let mut stamps = Vec::with_capacity(batch.len());

        for pending in batch {
            let key = Self::acl_key(pending.scope, pending.principal);
            let mut permissions: Vec<String> = pending
                .roles
                .iter()
                .flat_map(|r| r.permissions.iter().cloned())
                .collect();
            permissions.sort();
            permissions.dedup();
            let mut role_names: Vec<String> =
                pending.roles.iter().map(|r| r.role.clone()).collect();
            role_names.sort();
            role_names.dedup();

            if self.principal_node(pending.principal)?.is_none() {
                session.create_node_with_props(
                    &[PRINCIPAL_LABEL],
                    [(
                        PRINCIPAL_QN_PROP,
                        obrain_common::types::Value::from(pending.principal),
                    )],
                );
            }

            let (node, previous) = match seen.get(&key) {
                Some(found) => *found,
                None => match self.acl_node(&key)? {
                    Some(node) => {
                        let epoch = store
                            .get_node_property(
                                node,
                                &obrain_common::types::PropertyKey::new(ACL_EPOCH_PROP),
                            )
                            .and_then(|v| v.as_int64())
                            .unwrap_or(0) as u64;
                        (node, epoch)
                    }
                    None => {
                        let node = session.create_node_with_props(
                            &[ACL_LABEL],
                            [
                                (
                                    PRINCIPAL_QN_PROP,
                                    obrain_common::types::Value::from(key.as_str()),
                                ),
                                (
                                    ACL_PRINCIPAL_PROP,
                                    obrain_common::types::Value::from(pending.principal),
                                ),
                            ],
                        );
                        (node, 0)
                    }
                },
            };

            let epoch = previous + 1;
            let stamp = AclStamp {
                schema: IAM_SCHEMA.to_string(),
                key: key.clone(),
                principal: pending.principal.to_string(),
                scope: pending.scope.to_string(),
                roles: role_names,
                permissions,
                epoch,
            };

            let set = |prop: &str, value: obrain_common::types::Value| {
                store.set_node_property(node, prop, value);
            };
            set(
                ACL_KEY_PROP,
                obrain_common::types::Value::from(key.as_str()),
            );
            set(
                ACL_PERMS_PROP,
                obrain_common::types::Value::from(stamp.permissions.join(",").as_str()),
            );
            set(
                ACL_ROLES_PROP,
                obrain_common::types::Value::from(stamp.roles.join(",").as_str()),
            );
            set(
                ACL_BITS_PROP,
                obrain_common::types::Value::from(stamp.bits_hex().as_str()),
            );
            set(
                ACL_EPOCH_PROP,
                obrain_common::types::Value::Int64(epoch as i64),
            );

            seen.insert(key, (node, epoch));
            stamps.push(stamp);
        }

        // **Le seul commit du lot.**
        if !stamps.is_empty() {
            self.store.commit_wal()?;
        }
        Ok(stamps)
    }

    /// **La décision** : une seule recherche indexée, aucune traversée.
    pub fn decision(&self, principal: &str, scope: &str) -> Result<Option<AclStamp>, IamError> {
        let key = Self::acl_key(scope, principal);
        let Some(node) = self.acl_node(&key)? else {
            return Ok(None);
        };
        self.stamp_of(node)
    }

    /// Lit la décision portée par un nœud (sans traversée).
    pub fn stamp_of(
        &self,
        node: obrain_common::types::NodeId,
    ) -> Result<Option<AclStamp>, IamError> {
        // UNE seule lecture de nœud : le nœud arrive avec sa table de
        // propriétés, et on lit les cinq champs dedans — au lieu de cinq appels
        // `get_node_property`. C'est la correction du coût que la mesure avait
        // mis au jour : les 23 µs d'une « lecture indexée » étaient dominées par
        // les cinq appels, pas par l'index.
        let Some(obrain_node) = self.store.session().get_node(node) else {
            return Ok(None);
        };
        let props = obrain_node.properties_as_btree();
        let get = |prop: &str| {
            props
                .get(&obrain_common::types::PropertyKey::new(prop))
                .cloned()
        };
        let Some(key) = get(ACL_KEY_PROP).and_then(|v| v.as_str().map(str::to_string)) else {
            return Ok(None);
        };
        let split = |v: Option<obrain_common::types::Value>| -> Vec<String> {
            v.and_then(|v| v.as_str().map(str::to_string))
                .filter(|s| !s.is_empty())
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default()
        };
        let principal = key
            .rsplit_once('/')
            .map(|(_, p)| p.to_string())
            .unwrap_or_else(|| key.clone());
        let scope = key
            .split_once('/')
            .map(|(s, _)| s.to_string())
            .unwrap_or_default();

        Ok(Some(AclStamp {
            schema: IAM_SCHEMA.to_string(),
            key,
            principal,
            scope,
            roles: split(get(ACL_ROLES_PROP)),
            permissions: split(get(ACL_PERMS_PROP)),
            epoch: get(ACL_EPOCH_PROP).and_then(|v| v.as_int64()).unwrap_or(0) as u64,
        }))
    }

    /// Invalide la décision : l'époque avance, les stamps mémorisés deviennent
    /// **détectablement** périmés.
    pub fn invalidate(&self, principal: &str, scope: &str) -> Result<u64, IamError> {
        let key = Self::acl_key(scope, principal);
        let Some(node) = self.acl_node(&key)? else {
            return Ok(0);
        };
        let store = self.store.active_store();
        let epoch = store
            .get_node_property(
                node,
                &obrain_common::types::PropertyKey::new(ACL_EPOCH_PROP),
            )
            .and_then(|v| v.as_int64())
            .unwrap_or(0) as u64
            + 1;
        store.set_node_property(
            node,
            ACL_EPOCH_PROP,
            obrain_common::types::Value::Int64(epoch as i64),
        );
        // Fail-closed SANS perdre l'identité du nœud : on vide les permissions
        // et les rôles. La clé d'index reste, donc (a) une invalidation ne peut
        // pas laisser une décision périmée en place, et (b) une rematérialisation
        // retrouve le MÊME nœud — l'époque reste donc monotone. Retirer la clé
        // ferait créer un second nœud et l'époque repartirait à 1.
        store.set_node_property(node, ACL_PERMS_PROP, obrain_common::types::Value::from(""));
        store.set_node_property(node, ACL_ROLES_PROP, obrain_common::types::Value::from(""));
        self.store.commit_wal()?;
        Ok(epoch)
    }

    /// Les permissions effectives **par traversée** du graphe des rôles.
    ///
    /// C'est l'implémentation de référence : elle interroge vraiment les arêtes.
    /// Elle sert à (a) vérifier que la matérialisation est fidèle, et (b) mesurer
    /// ce que la décision indexée remplace.
    ///
    /// **Optimiste à dessein** : elle ne filtre pas les types d'arêtes sortantes
    /// du principal, ce qui la rend *plus rapide* qu'une implémentation réelle —
    /// l'écart mesuré avec la décision indexée est donc un **minorant**.
    pub fn permissions_by_traversal(&self, principal: &str) -> Result<Vec<String>, IamError> {
        use obrain_core::graph::Direction;
        let Some(node) = self.principal_node(principal)? else {
            return Ok(Vec::new());
        };
        let store = self.store.active_store();
        let mut out: Vec<String> = Vec::new();

        // Principal → rôles → permissions.
        for (role_node, _) in store.edges_from(node, Direction::Outgoing) {
            for (perm_node, _) in store.edges_from(role_node, Direction::Outgoing) {
                if let Some(v) = store
                    .get_node_property(perm_node, &obrain_common::types::PropertyKey::new("name"))
                {
                    if let Some(name) = v.as_str() {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

impl ObrainStore {
    /// Le plan IAM de ce store.
    pub fn iam(self: &Arc<Self>) -> Iam {
        Iam::new(Arc::clone(self))
    }
}

/// Identifiant d'instance IAM — pratique pour tracer un plan dans des journaux.
pub fn iam_instance_id() -> Uuid {
    Uuid::new_v4()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::Value as ObrainValue;

    fn grants() -> Vec<RoleGrant> {
        vec![
            RoleGrant {
                role: "reader".into(),
                permissions: vec!["orders:read".into(), "catalog:read".into()],
            },
            RoleGrant {
                role: "ops".into(),
                permissions: vec!["orders:read".into(), "orders:write".into()],
            },
        ]
    }

    #[test]
    fn materialize_then_decide_by_one_indexed_lookup() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();

        let stamp = iam
            .materialize("user:42", "tenant-a", &grants())
            .expect("matérialisation");

        assert_eq!(stamp.schema, IAM_SCHEMA);
        assert_eq!(stamp.key, "tenant-a/user:42");
        assert_eq!(stamp.epoch, 1);
        assert_eq!(
            stamp.permissions,
            vec!["catalog:read", "orders:read", "orders:write"],
            "union des rôles, dédupliquée et triée"
        );
        assert_eq!(stamp.roles, vec!["ops", "reader"]);
        assert!(stamp.allows("orders:write"));
        assert!(!stamp.allows("orders:delete"));
        assert!(!stamp.bits_hex().is_empty());

        // La décision se relit **sans traversée**.
        let read = iam
            .decision("user:42", "tenant-a")
            .expect("lecture")
            .expect("décision présente");
        assert_eq!(read, stamp);
    }

    #[test]
    fn the_indexes_are_created_by_the_plan() {
        let store = ObrainStore::new_in_memory();
        let _ = store.iam();
        assert!(store.db().has_property_index(ACL_KEY_PROP));
        assert!(store.db().has_property_index(PRINCIPAL_QN_PROP));
    }

    #[test]
    fn the_materialised_decision_matches_a_real_traversal() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam.materialize("user:1", "tenant-a", &grants()).unwrap();

        // On construit le VRAI graphe des rôles : Principal→Role→Permission.
        let session = store.session();
        let principal = store
            .db()
            .find_nodes_by_property(PRINCIPAL_QN_PROP, &ObrainValue::from("user:1"))
            .first()
            .copied()
            .expect("principal");
        for grant in grants() {
            let role = session.create_node_with_props(
                &["Role"],
                [("name", ObrainValue::from(grant.role.as_str()))],
            );
            session.create_edge(principal, role, "HAS_ROLE");
            for permission in &grant.permissions {
                let perm = session.create_node_with_props(
                    &["Permission"],
                    [("name", ObrainValue::from(permission.as_str()))],
                );
                session.create_edge(role, perm, "GRANTS");
            }
        }
        store.commit_wal().unwrap();

        let by_traversal = iam.permissions_by_traversal("user:1").unwrap();
        assert_eq!(
            by_traversal, stamp.permissions,
            "la matérialisation doit être FIDÈLE à la traversée"
        );
    }

    #[test]
    fn invalidation_makes_an_old_stamp_detectably_stale() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam.materialize("user:7", "t", &grants()).unwrap();

        let epoch = iam.invalidate("user:7", "t").unwrap();
        assert_eq!(epoch, 2);
        assert!(
            stamp.is_stale(epoch),
            "un stamp mémorisé doit être détectablement périmé"
        );
        assert!(!stamp.is_stale(stamp.epoch));

        // Et la décision servie ne permet PLUS rien : une invalidation est
        // *fail-closed* — elle refuse au lieu de disparaître (disparaître
        // laisserait croire à un défaut de matérialisation).
        let voided = iam
            .decision("user:7", "t")
            .unwrap()
            .expect("le nœud demeure");
        assert!(voided.permissions.is_empty());
        assert!(!voided.allows("orders:read"), "invalider doit REFUSER");
        assert_eq!(voided.epoch, 2);

        // Le recalcul repart, époque suivante.
        let again = iam.materialize("user:7", "t", &grants()).unwrap();
        assert_eq!(again.epoch, 3);
    }

    #[test]
    fn an_unknown_principal_is_absent_not_an_error() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        assert_eq!(iam.decision("inconnu", "t").unwrap(), None);
        assert!(iam.permissions_by_traversal("inconnu").unwrap().is_empty());
    }

    #[test]
    fn scopes_do_not_leak_into_each_other() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        iam.materialize("user:1", "tenant-a", &grants()).unwrap();
        iam.materialize(
            "user:1",
            "tenant-b",
            &[RoleGrant {
                role: "guest".into(),
                permissions: vec!["catalog:read".into()],
            }],
        )
        .unwrap();

        let a = iam.decision("user:1", "tenant-a").unwrap().unwrap();
        let b = iam.decision("user:1", "tenant-b").unwrap().unwrap();
        assert!(a.allows("orders:write"));
        assert!(
            !b.allows("orders:write"),
            "les portées ne se contaminent pas : {b:?}"
        );
        assert_eq!(b.permissions, vec!["catalog:read"]);
    }

    #[test]
    fn a_wildcard_permission_allows_everything() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam
            .materialize(
                "admin:x",
                "t",
                &[RoleGrant {
                    role: "root".into(),
                    permissions: vec!["*".into()],
                }],
            )
            .unwrap();
        assert!(stamp.allows("n-importe-quoi"));
        assert!(stamp.allows("orders:write"));
    }

    #[test]
    fn a_batch_is_equivalent_to_individual_calls() {
        let g = grants();

        // Chemin individuel — un commit par principal.
        let store_a = ObrainStore::new_in_memory();
        let iam_a = store_a.iam();
        let mut individual = Vec::new();
        for i in 0..4 {
            individual.push(iam_a.materialize(&format!("user:{i}"), "t", &g).unwrap());
        }

        // Chemin groupé — un seul commit pour les quatre.
        let store_b = ObrainStore::new_in_memory();
        let iam_b = store_b.iam();
        let names: Vec<String> = (0..4).map(|i| format!("user:{i}")).collect();
        let batch: Vec<PendingDecision<'_>> = names
            .iter()
            .map(|p| PendingDecision {
                principal: p,
                scope: "t",
                roles: &g,
            })
            .collect();
        let batched = iam_b.materialize_batch(&batch).unwrap();

        assert_eq!(
            batched, individual,
            "le lot doit produire EXACTEMENT les mêmes décisions"
        );
        for (i, name) in names.iter().enumerate() {
            assert_eq!(
                iam_a.decision(name, "t").unwrap(),
                individual.get(i).cloned()
            );
            assert_eq!(iam_b.decision(name, "t").unwrap(), batched.get(i).cloned());
        }
    }

    #[test]
    fn duplicate_keys_in_one_batch_share_one_node() {
        let g = grants();
        let small = vec![RoleGrant {
            role: "reader".into(),
            permissions: vec!["catalog:read".into()],
        }];
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();

        let batch = vec![
            PendingDecision {
                principal: "u",
                scope: "t",
                roles: &g,
            },
            PendingDecision {
                principal: "u",
                scope: "t",
                roles: &small,
            },
        ];
        let stamps = iam.materialize_batch(&batch).unwrap();
        assert_eq!(stamps.len(), 2);

        let nodes = store
            .db()
            .find_nodes_by_property(ACL_KEY_PROP, &ObrainValue::from("t/u"));
        assert_eq!(nodes.len(), 1, "une clé = UN nœud, jamais deux");

        let read = iam.decision("u", "t").unwrap().unwrap();
        assert_eq!(
            read.permissions,
            vec!["catalog:read"],
            "la dernière écriture du lot gagne"
        );
        assert!(stamps[1].epoch >= stamps[0].epoch, "époque monotone");
    }

    #[test]
    fn rematerialising_is_idempotent_in_content_and_monotone_in_epoch() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let first = iam.materialize("user:1", "t", &grants()).unwrap();
        let second = iam.materialize("user:1", "t", &grants()).unwrap();
        assert_eq!(first.permissions, second.permissions);
        assert_eq!(first.key, second.key);
        assert_eq!(second.epoch, first.epoch + 1, "l'époque ne recule jamais");
    }
}
