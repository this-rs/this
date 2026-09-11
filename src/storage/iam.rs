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
///
/// # Pourquoi v2
///
/// v1 encodait les listes par `join(",")` et les relisait par `split(',')` : une
/// permission contenant une virgule était **découpée en deux** à la relecture —
/// la décision accordait des noms jamais donnés, silencieusement. v2 encode les
/// listes en **JSON**, stocke la **portée** dans sa propre propriété (la clé
/// n'est plus re-découpée), rend la clé **injective**, et remplace le bitset —
/// qui dépendait du *cardinal* et saturait à 128 — par un **digest de contenu**.
pub const IAM_SCHEMA: &str = "this-rs/iam/2";

/// Propriété portant la clé d'ACL (celle qui est **indexée**).
pub const ACL_KEY_PROP: &str = "acl_key";
/// Propriété portant la **portée** (pour ne plus la re-découper depuis la clé).
pub const ACL_SCOPE_PROP: &str = "acl_scope";
/// Propriété portant le **digest de contenu** de la décision.
///
/// Remplace l'ancien bitset `acl_bits`, qui ne distinguait pas deux décisions de
/// même longueur aux noms différents et saturait au-delà de 128 permissions.
pub const ACL_DIGEST_PROP: &str = "acl_digest";
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
    /// **Digest de contenu** de la décision (vide = invalidée).
    #[serde(default)]
    pub digest: String,
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
        // Une permission **vide** n'est jamais accordée, pas même par un joker :
        // un `perm:` sans nom est une politique fautive, et une faute refuse.
        if permission.is_empty() {
            return false;
        }
        self.permissions.iter().any(|p| p == "*" || p == permission)
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
        // Séparateur US (0x1F) : avec un `/`, `("a/b", "c")` et `("a", "b/c")`
        // produisaient la MÊME clé — la seconde décision écrasait la première,
        // soit une fuite entre portées.
        format!("{scope}\u{1f}{principal}")
    }

    /// Les deux moitiés d'une clé d'ACL (diagnostic).
    pub fn split_acl_key(key: &str) -> Option<(&str, &str)> {
        key.split_once('\u{1f}')
    }

    /// Encode une liste de façon **non ambiguë** (JSON).
    fn encode_list(values: &[String]) -> String {
        serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
    }

    /// Décode une liste JSON. **Strict** : illisible ⇒ erreur, jamais une liste
    /// vide — une décision corrompue doit refuser, pas accorder.
    fn decode_list(raw: &str) -> Result<Vec<String>, IamError> {
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str::<Vec<String>>(raw).map_err(|e| IamError::Malformed {
            node: "décision".to_string(),
            reason: format!("liste illisible ({e}) — refus par prudence"),
        })
    }

    /// Le **digest de contenu** d'une décision.
    fn digest_of(scope: &str, principal: &str, roles: &[String], permissions: &[String]) -> String {
        use sha2::{Digest, Sha256};
        let canonical = serde_json::json!({
            "scope": scope,
            "principal": principal,
            "roles": roles,
            "permissions": permissions,
        });
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string().as_bytes());
        let out = hasher.finalize();
        out.iter().take(16).map(|b| format!("{b:02x}")).collect()
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
            ACL_SCOPE_PROP,
            obrain_common::types::Value::from(scope),
        );
        store.set_node_property(
            node,
            ACL_PERMS_PROP,
            obrain_common::types::Value::from(Self::encode_list(&permissions).as_str()),
        );
        store.set_node_property(
            node,
            ACL_ROLES_PROP,
            obrain_common::types::Value::from(Self::encode_list(&role_names).as_str()),
        );
        store.set_node_property(
            node,
            ACL_EPOCH_PROP,
            obrain_common::types::Value::Int64(epoch as i64),
        );

        let digest = Self::digest_of(scope, principal, &role_names, &permissions);
        let stamp = AclStamp {
            schema: IAM_SCHEMA.to_string(),
            key: key.clone(),
            principal: principal.to_string(),
            scope: scope.to_string(),
            roles: role_names,
            permissions,
            epoch,
            digest: digest.clone(),
        };
        store.set_node_property(
            node,
            ACL_DIGEST_PROP,
            obrain_common::types::Value::from(digest.as_str()),
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
            let digest =
                Self::digest_of(pending.scope, pending.principal, &role_names, &permissions);
            let stamp = AclStamp {
                schema: IAM_SCHEMA.to_string(),
                key: key.clone(),
                principal: pending.principal.to_string(),
                scope: pending.scope.to_string(),
                roles: role_names,
                permissions,
                epoch,
                digest,
            };

            let set = |prop: &str, value: obrain_common::types::Value| {
                store.set_node_property(node, prop, value);
            };
            set(
                ACL_KEY_PROP,
                obrain_common::types::Value::from(key.as_str()),
            );
            set(
                ACL_SCOPE_PROP,
                obrain_common::types::Value::from(pending.scope),
            );
            set(
                ACL_PERMS_PROP,
                obrain_common::types::Value::from(Self::encode_list(&stamp.permissions).as_str()),
            );
            set(
                ACL_ROLES_PROP,
                obrain_common::types::Value::from(Self::encode_list(&stamp.roles).as_str()),
            );
            set(
                ACL_DIGEST_PROP,
                obrain_common::types::Value::from(stamp.digest.as_str()),
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
        // **Le label fait partie du contrat** : un nœud qui porte une propriété
        // `acl_key` sans être une décision n'en est pas une. Sans ce contrôle,
        // n'importe quel nœud porteur de la propriété était lu comme une
        // décision valide (époque 0, permissions vides).
        if !obrain_node.labels.iter().any(|l| l.as_str() == ACL_LABEL) {
            return Ok(None);
        }
        let props = obrain_node.properties_as_btree();
        let get = |prop: &str| {
            props
                .get(&obrain_common::types::PropertyKey::new(prop))
                .cloned()
        };
        let Some(key) = get(ACL_KEY_PROP).and_then(|v| v.as_str().map(str::to_string)) else {
            return Ok(None);
        };
        // Liste JSON, **strictement** décodée : une décision illisible refuse —
        // jamais une liste vide silencieuse, qui ferait passer une décision
        // perdue pour un refus (ou l'inverse, selon le lecteur).
        let decode = |v: Option<obrain_common::types::Value>| -> Result<Vec<String>, IamError> {
            Self::decode_list(
                &v.and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
            )
        };
        // Portée et principal viennent de **leurs propriétés** : plus de
        // découpage de clé, donc plus de portée relue de travers.
        let principal = get(ACL_PRINCIPAL_PROP)
            .and_then(|v| v.as_str().map(str::to_string))
            .or_else(|| Self::split_acl_key(&key).map(|(_, p)| p.to_string()))
            .unwrap_or_else(|| key.clone());
        let scope = get(ACL_SCOPE_PROP)
            .and_then(|v| v.as_str().map(str::to_string))
            .or_else(|| Self::split_acl_key(&key).map(|(s, _)| s.to_string()))
            .unwrap_or_default();

        Ok(Some(AclStamp {
            schema: IAM_SCHEMA.to_string(),
            key,
            principal,
            scope,
            roles: decode(get(ACL_ROLES_PROP))?,
            permissions: decode(get(ACL_PERMS_PROP))?,
            epoch: get(ACL_EPOCH_PROP).and_then(|v| v.as_int64()).unwrap_or(0) as u64,
            digest: get(ACL_DIGEST_PROP)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
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
        // Le **digest** aussi : sinon le nœud portait encore l'empreinte de
        // l'ancienne décision, et qui lit ce champ la croyait en vigueur.
        store.set_node_property(node, ACL_DIGEST_PROP, obrain_common::types::Value::from(""));
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
                    && let Some(name) = v.as_str()
                {
                    out.push(name.to_string());
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
        assert_eq!(stamp.key, Iam::acl_key("tenant-a", "user:42"));
        assert_eq!(stamp.epoch, 1);
        assert_eq!(
            stamp.permissions,
            vec!["catalog:read", "orders:read", "orders:write"],
            "union des rôles, dédupliquée et triée"
        );
        assert_eq!(stamp.roles, vec!["ops", "reader"]);
        assert!(stamp.allows("orders:write"));
        assert!(!stamp.allows("orders:delete"));
        assert!(!stamp.digest.clone().is_empty());

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
            .find_nodes_by_property(ACL_KEY_PROP, &ObrainValue::from(Iam::acl_key("t", "u")));
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

// ---------------------------------------------------------------------------
// Périmètre : le contrat de l'IAM matérialisé
// ---------------------------------------------------------------------------
//
// Ces tests couvrent le **contrat** du module, pas son implémentation : contenu
// exact d'une décision, idempotence, lots, invalidation, portées, permissions,
// `AclStamp`, `stamp_of`, durabilité, index et fidélité à la traversée. Chaque
// test assère une propriété — quand il tombe, son nom dit ce qui est cassé.
//
// Les tests marqués `#[ignore]` portent un **défaut constaté** : ils assèrent la
// propriété souhaitée, et échouent aujourd'hui. Ils sont laissés visibles
// exprès (`cargo test -- --ignored`) plutôt que commentés.

#[cfg(test)]
mod perimeter {
    use super::*;
    use obrain_common::types::PropertyKey;

    // -----------------------------------------------------------------------
    // Fabriques
    // -----------------------------------------------------------------------

    /// Une valeur de propriété depuis une chaîne.
    fn v(s: &str) -> obrain_common::types::Value {
        obrain_common::types::Value::from(s)
    }

    /// Combien de nœuds portent cette clé d'ACL — le comptage qui prouve
    /// qu'une écriture n'a **pas** créé de doublon.
    fn acl_nodes(store: &ObrainStore, key: &str) -> usize {
        store
            .db()
            .find_nodes_by_property(ACL_KEY_PROP, &v(key))
            .len()
    }

    /// Combien de nœuds `Principal` portent ce nom qualifié.
    fn principal_nodes(store: &ObrainStore, principal: &str) -> usize {
        store
            .db()
            .find_nodes_by_property(PRINCIPAL_QN_PROP, &v(principal))
            .len()
    }

    /// Le nœud de décision d'une clé, lu par index.
    fn decision_node(store: &ObrainStore, key: &str) -> obrain_common::types::NodeId {
        store
            .db()
            .find_nodes_by_property(ACL_KEY_PROP, &v(key))
            .into_iter()
            .next()
            .expect("nœud de décision attendu")
    }

    /// La propriété brute d'un nœud (pour regarder ce qui est *réellement*
    /// persisté, là où `AclStamp` recalcule).
    fn raw_prop(store: &ObrainStore, node: obrain_common::types::NodeId, prop: &str) -> String {
        store
            .active_store()
            .get_node_property(node, &PropertyKey::new(prop))
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn grant(role: &str, permissions: &[&str]) -> RoleGrant {
        RoleGrant {
            role: role.to_string(),
            permissions: permissions.iter().map(|p| p.to_string()).collect(),
        }
    }

    /// Les deux rôles de référence : `reader` et `ops` se recouvrent sur
    /// `orders:read`.
    fn standard_grants() -> Vec<RoleGrant> {
        vec![
            grant("reader", &["orders:read", "catalog:read"]),
            grant("ops", &["orders:read", "orders:write"]),
        ]
    }

    // -----------------------------------------------------------------------
    // 1-2. Matérialisation et idempotence
    // -----------------------------------------------------------------------

    #[test]
    fn materialize_writes_the_exact_union_the_sorted_roles_and_the_indexed_key() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();

        // Entrées volontairement désordonnées et redondantes : rôles en
        // désordre, permissions dupliquées entre rôles et dans un même rôle.
        let stamp = iam
            .materialize(
                "user:42",
                "tenant-a",
                &[
                    grant("ops", &["orders:write", "orders:read"]),
                    grant("reader", &["orders:read", "catalog:read", "orders:read"]),
                    grant("ops", &["orders:write"]),
                ],
            )
            .unwrap();

        assert_eq!(stamp.schema, IAM_SCHEMA, "le document porte son schéma");
        assert_eq!(
            stamp.key,
            Iam::acl_key("tenant-a", "user:42"),
            "acl_key = <portée>/<principal>"
        );
        assert_eq!(stamp.key, Iam::acl_key("tenant-a", "user:42"));
        assert_eq!(stamp.principal, "user:42");
        assert_eq!(stamp.scope, "tenant-a");
        assert_eq!(
            stamp.permissions,
            vec!["catalog:read", "orders:read", "orders:write"],
            "union des permissions, DÉDUPLIQUÉE et TRIÉE"
        );
        assert_eq!(
            stamp.roles,
            vec!["ops", "reader"],
            "rôles triés et dédupliqués"
        );
        assert_eq!(stamp.epoch, 1, "première matérialisation : époque 1");
        assert_eq!(
            iam.decision("user:42", "tenant-a").unwrap().as_ref(),
            Some(&stamp),
            "la relecture indexée rend EXACTEMENT le stamp écrit"
        );
    }

    #[test]
    fn rematerialising_identical_grants_keeps_the_key_and_content_but_advances_the_epoch() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let mut epochs = vec![];
        let mut stamps = vec![];
        for _ in 0..4 {
            let stamp = iam.materialize("user:1", "t", &standard_grants()).unwrap();
            epochs.push(stamp.epoch);
            stamps.push(stamp);
        }

        assert_eq!(
            epochs,
            vec![1, 2, 3, 4],
            "l'époque est strictement monotone"
        );
        for stamp in &stamps[1..] {
            assert_eq!(
                stamp.key, stamps[0].key,
                "re-matérialiser ne change pas la clé"
            );
            assert_eq!(
                stamp.permissions, stamps[0].permissions,
                "re-matérialiser ne change pas le contenu"
            );
            assert_eq!(stamp.roles, stamps[0].roles);
        }
        assert_eq!(
            acl_nodes(&store, &Iam::acl_key("t", "user:1")),
            1,
            "toujours UN seul nœud de décision"
        );
        assert_eq!(
            principal_nodes(&store, "user:1"),
            1,
            "toujours UN seul nœud principal"
        );
        assert_eq!(
            iam.decision("user:1", "t").unwrap().unwrap().epoch,
            4,
            "la décision relue porte la dernière époque"
        );
    }

    #[test]
    fn two_identical_successive_batches_produce_strictly_increasing_epochs() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let g = standard_grants();
        let names: Vec<String> = (0..3).map(|i| format!("batch:{i}")).collect();
        let batch: Vec<PendingDecision<'_>> = names
            .iter()
            .map(|p| PendingDecision {
                principal: p,
                scope: "t",
                roles: &g,
            })
            .collect();

        let first = iam.materialize_batch(&batch).unwrap();
        let second = iam.materialize_batch(&batch).unwrap();

        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), 3);
        for (before, after) in first.iter().zip(second.iter()) {
            assert_eq!(after.key, before.key);
            assert_eq!(
                after.permissions, before.permissions,
                "lot identique ⇒ contenu identique"
            );
            assert_eq!(after.epoch, before.epoch + 1, "lot identique ⇒ époque + 1");
        }
        for name in &names {
            assert_eq!(
                acl_nodes(&store, &Iam::acl_key("t", name)),
                1,
                "un nœud par clé, jamais deux"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 3. Lots
    // -----------------------------------------------------------------------

    #[test]
    fn a_batch_matches_individual_calls_exactly_at_every_batch_size() {
        let g = standard_grants();
        for size in [1usize, 2, 10, 100] {
            let names: Vec<String> = (0..size).map(|i| format!("user:{size}:{i}")).collect();

            // Chemin individuel : un appel — donc un commit — par décision.
            let store_i = ObrainStore::new_in_memory();
            let iam_i = store_i.iam();
            let individual: Vec<AclStamp> = names
                .iter()
                .map(|p| iam_i.materialize(p, "tenant-a", &g).unwrap())
                .collect();

            // Chemin groupé : N calculs, un seul commit.
            let store_b = ObrainStore::new_in_memory();
            let iam_b = store_b.iam();
            let batch: Vec<PendingDecision<'_>> = names
                .iter()
                .map(|p| PendingDecision {
                    principal: p,
                    scope: "tenant-a",
                    roles: &g,
                })
                .collect();
            let batched = iam_b.materialize_batch(&batch).unwrap();

            assert_eq!(batched.len(), size, "lot de {size} : un stamp par entrée");
            assert_eq!(
                batched, individual,
                "lot de {size} : équivalence STRICTE (Vec<AclStamp> entier) avec {size} appels individuels"
            );
            for (i, name) in names.iter().enumerate() {
                assert_eq!(
                    iam_b.decision(name, "tenant-a").unwrap().as_ref(),
                    individual.get(i),
                    "lot de {size} : relecture indexée identique à l'appel individuel"
                );
                assert_eq!(
                    acl_nodes(&store_b, &Iam::acl_key("tenant-a", name)),
                    1,
                    "lot de {size} : un seul nœud par clé"
                );
            }
        }
    }

    #[test]
    fn an_empty_batch_creates_no_node_and_returns_no_stamp() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let before = store.db().node_count();

        let stamps = iam.materialize_batch(&[]).unwrap();

        assert!(stamps.is_empty(), "un lot vide rend un vec vide");
        assert_eq!(
            store.db().node_count(),
            before,
            "un lot vide ne doit créer AUCUN nœud"
        );
        assert_eq!(
            principal_nodes(&store, "user:0"),
            0,
            "un lot vide ne crée même pas de nœud principal"
        );
        assert_eq!(iam.decision("user:0", "tenant-a").unwrap(), None);
        assert!(iam.permissions_by_traversal("user:0").unwrap().is_empty());
    }

    #[test]
    fn many_duplicate_keys_in_one_batch_produce_one_node_and_the_last_write_wins() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let a = vec![grant("reader", &["orders:write"])];
        let b = vec![grant("reader", &["catalog:read"])];
        let c = vec![grant("reader", &["orders:read"])];
        let batch = vec![
            PendingDecision {
                principal: "u",
                scope: "t",
                roles: &a,
            },
            PendingDecision {
                principal: "u",
                scope: "t",
                roles: &b,
            },
            PendingDecision {
                principal: "u",
                scope: "t",
                roles: &c,
            },
        ];

        let stamps = iam.materialize_batch(&batch).unwrap();

        assert_eq!(stamps.len(), 3, "trois entrées ⇒ trois stamps rendus");
        assert_eq!(
            acl_nodes(&store, &Iam::acl_key("t", "u")),
            1,
            "clé dupliquée ⇒ UN seul nœud"
        );
        assert_eq!(principal_nodes(&store, "u"), 1, "un seul nœud principal");
        let epochs: Vec<u64> = stamps.iter().map(|s| s.epoch).collect();
        assert!(
            epochs.windows(2).all(|w| w[0] < w[1]),
            "l'époque croît STRICTEMENT à l'intérieur du lot : {epochs:?}"
        );
        let read = iam.decision("u", "t").unwrap().unwrap();
        assert_eq!(
            read.permissions,
            vec!["orders:read"],
            "la dernière écriture du lot gagne"
        );
        assert_eq!(
            read.epoch, stamps[2].epoch,
            "et c'est bien la dernière époque"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Invalidation
    // -----------------------------------------------------------------------

    #[test]
    fn invalidate_voids_the_decision_keeps_the_same_node_and_stays_monotone() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let before = store.db().node_count();
        let stamp = iam
            .materialize("user:9", "tenant-a", &standard_grants())
            .unwrap();
        let materialised = store.db().node_count();
        assert!(
            materialised > before,
            "matérialiser crée le principal et la décision"
        );
        let node = decision_node(&store, &Iam::acl_key("tenant-a", "user:9"));

        let epoch = iam.invalidate("user:9", "tenant-a").unwrap();

        assert_eq!(epoch, 2, "invalider incrémente l'époque");
        assert!(
            stamp.is_stale(epoch),
            "un stamp mémorisé devient DÉTECTABLEMENT périmé"
        );
        assert_eq!(
            store.db().node_count(),
            materialised,
            "invalider ne doit créer AUCUN nœud"
        );
        assert_eq!(
            acl_nodes(&store, &Iam::acl_key("tenant-a", "user:9")),
            1,
            "l'identité du nœud est CONSERVÉE"
        );
        assert_eq!(
            decision_node(&store, &Iam::acl_key("tenant-a", "user:9")),
            node,
            "c'est bien le MÊME nœud, pas un homonyme"
        );

        let voided = iam
            .decision("user:9", "tenant-a")
            .unwrap()
            .expect("le nœud demeure");
        assert!(
            voided.permissions.is_empty(),
            "invalider VIDE les permissions"
        );
        assert!(voided.roles.is_empty(), "invalider VIDE les rôles");
        assert!(!voided.allows("orders:read"), "invalider REFUSE");
        assert!(!voided.allows("*"), "et refuse même le joker");
        assert_eq!(voided.epoch, 2);
        assert_eq!(voided.key, stamp.key, "la clé d'index survit");

        // La re-matérialisation retrouve le MÊME nœud : l'époque continue.
        let again = iam
            .materialize("user:9", "tenant-a", &standard_grants())
            .unwrap();
        assert_eq!(
            again.epoch, 3,
            "l'époque reprend au-dessus de l'invalidation"
        );
        assert_eq!(
            again.permissions, stamp.permissions,
            "le contenu est restauré"
        );
        assert_eq!(store.db().node_count(), materialised, "aucun second nœud");
        assert_eq!(acl_nodes(&store, &Iam::acl_key("tenant-a", "user:9")), 1);
        assert_eq!(
            decision_node(&store, &Iam::acl_key("tenant-a", "user:9")),
            node
        );
    }

    #[test]
    fn invalidating_an_unmaterialised_decision_is_a_no_op() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let before = store.db().node_count();

        assert_eq!(
            iam.invalidate("inconnu", "t").unwrap(),
            0,
            "invalider l'absent rend 0 — pas d'erreur inventée, pas d'époque inventée"
        );

        assert_eq!(store.db().node_count(), before, "et ne crée aucun nœud");
        assert_eq!(iam.decision("inconnu", "t").unwrap(), None);
    }

    /// **Défaut constaté** : `invalidate` vide `acl_perms` et `acl_roles` mais
    /// laisse `acl_bits` inchangé. Or `acl_bits` est une propriété publique du
    /// document de décision (`ACL_DIGEST_PROP`) : tout consommateur qui lit le
    /// nœud directement — ce que ce module présente comme le chemin de lecture
    /// — y lit encore un ensemble de permissions **accordées** après une
    /// invalidation censée être *fail-closed*. `Iam::decision` ne le voit pas
    /// (`stamp_of` recalcule les bits depuis `acl_perms`), donc l'incohérence
    /// est silencieuse.
    #[test]
    fn invalidate_clears_the_persisted_digest_too() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        iam.materialize("user:9", "tenant-a", &standard_grants())
            .unwrap();
        let node = decision_node(&store, &Iam::acl_key("tenant-a", "user:9"));
        let granted_bits = raw_prop(&store, node, ACL_DIGEST_PROP);

        iam.invalidate("user:9", "tenant-a").unwrap();

        let bits = raw_prop(&store, node, ACL_DIGEST_PROP);
        assert_eq!(
            bits, "",
            "après invalidation le bitset persisté doit être vide, or il vaut encore {bits:?} (accordé avant : {granted_bits:?})"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Portées
    // -----------------------------------------------------------------------

    #[test]
    fn two_principals_in_the_same_scope_are_independent() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        iam.materialize("user:a", "t", &standard_grants()).unwrap();
        iam.materialize("user:b", "t", &[grant("guest", &["catalog:read"])])
            .unwrap();

        assert_ne!(Iam::acl_key("t", "user:a"), Iam::acl_key("t", "user:b"));
        let a = iam.decision("user:a", "t").unwrap().unwrap();
        let b = iam.decision("user:b", "t").unwrap().unwrap();
        assert!(a.allows("orders:write"));
        assert!(
            !b.allows("orders:write"),
            "pas de fuite entre principaux : {b:?}"
        );
        assert_eq!(b.permissions, vec!["catalog:read"]);
        assert_eq!(b.principal, "user:b");
        assert_eq!(a.principal, "user:a");
        assert_eq!(acl_nodes(&store, &Iam::acl_key("t", "user:a")), 1);
        assert_eq!(acl_nodes(&store, &Iam::acl_key("t", "user:b")), 1);
        assert_eq!(principal_nodes(&store, "user:a"), 1);
        assert_eq!(principal_nodes(&store, "user:b"), 1);
    }

    #[test]
    fn two_scopes_of_one_principal_are_independent_and_have_distinct_keys() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        iam.materialize("user:1", "tenant-a", &standard_grants())
            .unwrap();
        iam.materialize("user:1", "tenant-b", &[grant("guest", &["catalog:read"])])
            .unwrap();

        assert_ne!(
            Iam::acl_key("tenant-a", "user:1"),
            Iam::acl_key("tenant-b", "user:1")
        );
        let a = iam.decision("user:1", "tenant-a").unwrap().unwrap();
        let b = iam.decision("user:1", "tenant-b").unwrap().unwrap();
        assert!(a.allows("orders:write"));
        assert!(
            !b.allows("orders:write"),
            "une portée ne contamine pas l'autre : {b:?}"
        );
        assert_eq!(a.scope, "tenant-a");
        assert_eq!(b.scope, "tenant-b");
        assert_eq!(a.epoch, 1, "chaque portée a sa propre époque");
        assert_eq!(b.epoch, 1);
        assert_eq!(acl_nodes(&store, &Iam::acl_key("tenant-a", "user:1")), 1);
        assert_eq!(acl_nodes(&store, &Iam::acl_key("tenant-b", "user:1")), 1);
        assert_eq!(
            principal_nodes(&store, "user:1"),
            1,
            "le nœud principal, lui, est partagé entre les portées"
        );
        // Invalider une portée ne touche pas l'autre.
        iam.invalidate("user:1", "tenant-a").unwrap();
        assert!(
            iam.decision("user:1", "tenant-a")
                .unwrap()
                .unwrap()
                .permissions
                .is_empty()
        );
        assert!(
            iam.decision("user:1", "tenant-b")
                .unwrap()
                .unwrap()
                .allows("catalog:read")
        );
    }

    /// Comportement **documenté** : une portée contenant `/` reste une clé
    /// stable et donc utilisable pour la lecture indexée, mais la relecture
    /// redécoupe la clé au **premier** `/` pour la portée et au **dernier**
    /// **Corrigé** : la clé sépare la portée du principal par un caractère US
    /// (0x1F), donc `("a/b", "c")` et `("a", "b/c")` ne collisionnent plus, et
    /// `stamp_of` lit la portée et le principal dans **leurs propres
    /// propriétés** — plus de découpage, plus de portée relue de travers.
    #[test]
    fn a_scope_with_a_slash_keeps_an_unambiguous_key_and_reads_back_exactly() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        assert_eq!(Iam::acl_key("/x", "p"), "/x\u{1f}p");

        let written = iam
            .materialize("p", "/x", &[grant("r", &["orders:read"])])
            .unwrap();
        let read = iam.decision("p", "/x").unwrap().expect("la clé est stable");

        assert_eq!(read.key, written.key, "la clé reste identique");
        assert_eq!(read.key, Iam::acl_key("/x", "p"));
        assert_eq!(
            read.permissions, written.permissions,
            "la relecture indexée reste CORRECTE : c'est la clé qui porte la décision"
        );
        assert_eq!(
            read.scope, "/x",
            "corrigé : la portée se relit EXACTEMENT (elle vient de sa propriété, \
             plus d'un découpage de clé)"
        );
    }

    /// **Corrigé** : `acl_key` est injective. La portée et le
    /// principal sont concaténés par `/` sans échappement, donc
    /// `(portée \"a/b\", principal \"c\")` et `(portée \"a\", principal \"b/c\")`
    /// produisent **la même clé** et donc **le même nœud de décision**. Le
    /// second écrase le premier : les permissions d'un principal deviennent
    /// celles d'un autre. C'est une fuite entre portées — ou entre principaux —
    /// si l'un des deux noms contient un `/`.
    #[test]
    fn acl_key_is_injective_for_every_scope_and_principal_pair() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        assert_ne!(
            Iam::acl_key("a/b", "c"),
            Iam::acl_key("a", "b/c"),
            "corrigé : les deux couples produisent des clés DISTINCTES (séparateur US 0x1F)"
        );

        iam.materialize("c", "a/b", &[grant("r1", &["p1"])])
            .unwrap();
        iam.materialize("b/c", "a", &[grant("r2", &["p2"])])
            .unwrap();

        let first = iam
            .decision("c", "a/b")
            .unwrap()
            .expect("décision du premier couple");
        assert!(
            first.allows("p1"),
            "la décision du premier couple doit rester intacte, or elle vaut {first:?} : le second couple l'a écrasée"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Permissions
    // -----------------------------------------------------------------------

    #[test]
    fn a_role_without_any_permission_materialises_an_empty_effective_set() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam
            .materialize(
                "u",
                "t",
                &[grant("auditor", &[]), grant("reader", &["orders:read"])],
            )
            .unwrap();

        assert_eq!(
            stamp.permissions,
            vec!["orders:read"],
            "un rôle vide ne contribue rien"
        );
        assert_eq!(
            stamp.roles,
            vec!["auditor", "reader"],
            "mais il est retenu comme rôle"
        );
        assert!(stamp.allows("orders:read"));

        // Un principal dont AUCUN rôle n'accorde quoi que ce soit.
        let empty = iam.materialize("v", "t", &[grant("auditor", &[])]).unwrap();
        assert!(empty.permissions.is_empty());
        assert_eq!(empty.roles, vec!["auditor"]);
        assert!(!empty.allows("orders:read"));
        assert_eq!(
            empty.digest.len(),
            32,
            "un digest de contenu, même pour un ensemble vide (ce n'est plus un masque de bits)"
        );
        assert_ne!(
            empty.digest,
            iam.materialize("w", "t", &[grant("r", &["orders:read"])])
                .unwrap()
                .digest,
            "un ensemble vide et un ensemble non vide ne partagent pas de digest"
        );
        assert_eq!(
            iam.decision("v", "t").unwrap().as_ref(),
            Some(&empty),
            "et la relecture rend bien un ensemble vide, pas None"
        );
    }

    #[test]
    fn duplicate_permissions_across_roles_are_merged_once() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam
            .materialize(
                "u",
                "t",
                &[
                    grant("r1", &["orders:read", "orders:read"]),
                    grant("r2", &["orders:read"]),
                    grant("r3", &["orders:read", "orders:write"]),
                ],
            )
            .unwrap();

        assert_eq!(
            stamp.permissions,
            vec!["orders:read", "orders:write"],
            "quatre occurrences de `orders:read` ⇒ une seule entrée"
        );
        assert_eq!(stamp.roles, vec!["r1", "r2", "r3"]);
        assert_eq!(
            iam.decision("u", "t").unwrap().unwrap().permissions,
            stamp.permissions,
            "la déduplication survit à l'aller-retour"
        );
    }

    #[test]
    fn the_wildcard_is_stored_verbatim_and_allows_every_other_name() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam
            .materialize("admin:x", "t", &[grant("root", &["*", "orders:read"])])
            .unwrap();

        assert_eq!(
            stamp.permissions,
            vec!["*", "orders:read"],
            "le joker est une permission"
        );
        assert!(stamp.allows("*"));
        assert!(stamp.allows("orders:read"));
        assert!(stamp.allows("n'importe quoi"));
        assert!(
            !stamp.allows(""),
            "corrigé : une permission VIDE n'est jamais accordée, pas même par le joker — \
             un `perm:` sans nom est une politique fautive, et une faute refuse"
        );
        assert_eq!(
            iam.decision("admin:x", "t").unwrap().unwrap().permissions,
            stamp.permissions,
            "le joker survit à l'aller-retour"
        );
    }

    #[test]
    fn a_permission_longer_than_a_thousand_characters_survives_the_round_trip() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let long = format!("perm:{}", "x".repeat(1500));
        assert!(long.len() > 1000);
        let stamp = iam
            .materialize("u", "t", &[grant("r", &[long.as_str()])])
            .unwrap();

        assert_eq!(stamp.permissions, vec![long.clone()]);
        let read = iam.decision("u", "t").unwrap().unwrap();
        assert_eq!(
            read.permissions,
            vec![long.clone()],
            "un nom long n'est ni tronqué ni altéré"
        );
        assert_eq!(read.digest.clone(), stamp.digest.clone());
    }

    #[test]
    fn unicode_and_accented_permissions_survive_the_round_trip() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let names = ["café:lire", "命令:读", "🔐:read", "naïve:écrire"];
        let stamp = iam.materialize("u", "t", &[grant("r", &names)]).unwrap();

        let mut expected: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(
            stamp.permissions, expected,
            "le tri est un tri d'octets UTF-8"
        );
        let read = iam.decision("u", "t").unwrap().unwrap();
        assert_eq!(
            read.permissions, stamp.permissions,
            "l'UTF-8 survit à l'aller-retour"
        );
        for name in names {
            assert!(read.allows(name), "{name} doit être accordée");
        }
    }

    /// **Défaut constaté, perte de données silencieuse** : les permissions sont
    /// sérialisées par `join(",")` et relues par `split(',')`. Une permission
    /// qui contient une virgule est donc **découpée en deux** à la relecture.
    /// Le `AclStamp` rendu par `materialize` et celui rendu par `decision` ne
    /// sont alors pas égaux — et le second **accorde des permissions qui n'ont
    /// jamais été données** (`orders:read` et `orders:write` séparément, au
    /// lieu du nom unique `orders:read,orders:write`). Aucune erreur n'est
    /// levée : la corruption est silencieuse. Même mécanisme pour les rôles.
    #[test]
    fn a_permission_containing_a_comma_survives_the_round_trip() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let written = iam
            .materialize("u", "t", &[grant("r", &["orders:read,orders:write"])])
            .unwrap();
        let read = iam.decision("u", "t").unwrap().unwrap();

        assert_eq!(
            read.permissions, written.permissions,
            "la décision relue doit être identique à la décision écrite, or elle vaut {read:?} \
             contre {written:?} : la virgule a été prise pour un séparateur"
        );
    }

    /// Le même défaut, décrit dans le sens de l'observation : ce test
    /// **documente** la corruption effective plutôt que de l'exiger.
    #[test]
    fn a_comma_inside_a_permission_is_read_back_as_two_permissions() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let written = iam
            .materialize("u", "t", &[grant("r", &["orders:read,orders:write"])])
            .unwrap();

        assert_eq!(
            written.permissions,
            vec!["orders:read,orders:write"],
            "écrit tel quel"
        );
        let read = iam.decision("u", "t").unwrap().unwrap();
        assert_eq!(
            read.permissions,
            vec!["orders:read,orders:write"],
            "corrigé : la liste est encodée en JSON, donc une virgule ne découpe plus \
             une permission en deux (v1 accordait des noms jamais donnés)"
        );
        assert!(
            !read.allows("orders:read"),
            "corrigé : la permission entière n'accorde PAS ses morceaux — v1 les accordait \
             (des noms jamais donnés)"
        );
        assert!(
            read.allows("orders:read,orders:write"),
            "elle n'est accordée que telle qu'elle a été écrite"
        );
        assert_eq!(read, written, "corrigé : le stamp relu EST le stamp écrit");
    }

    // -----------------------------------------------------------------------
    // 7. AclStamp
    // -----------------------------------------------------------------------

    fn stamp_with(permissions: &[&str]) -> AclStamp {
        AclStamp {
            schema: IAM_SCHEMA.to_string(),
            key: Iam::acl_key("t", "u"),
            principal: "u".to_string(),
            scope: "t".to_string(),
            roles: vec!["r".to_string()],
            permissions: permissions.iter().map(|p| p.to_string()).collect(),
            epoch: 1,
            digest: Iam::digest_of(
                "t",
                "u",
                &["r".to_string()],
                &permissions
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>(),
            ),
        }
    }

    #[test]
    fn allows_matches_exactly_and_never_by_prefix() {
        let stamp = stamp_with(&["orders:read"]);
        assert!(stamp.allows("orders:read"), "égalité exacte");
        assert!(!stamp.allows("orders"), "pas de correspondance de préfixe");
        assert!(
            !stamp.allows("orders:read:all"),
            "pas de correspondance par extension"
        );
        assert!(
            !stamp.allows("Orders:Read"),
            "la comparaison est sensible à la casse"
        );
        assert!(!stamp.allows("orders:read "), "et aux espaces");
        assert!(
            !stamp.allows("*"),
            "`*` n'est pas implicite : c'est une permission à écrire"
        );
    }

    #[test]
    fn allows_is_false_for_everything_without_permission_and_the_wildcard_never_covers_the_empty_name()
     {
        let empty = stamp_with(&[]);
        assert!(
            !empty.allows(""),
            "sans permission, même le nom vide est refusé"
        );
        assert!(!empty.allows("orders:read"));
        assert!(!empty.allows("*"));

        let wildcard = stamp_with(&["*"]);
        assert!(
            !wildcard.allows(""),
            "corrigé : le joker n'accorde pas le nom vide"
        );
        assert!(wildcard.allows("orders:read"));
    }

    #[test]
    fn is_stale_is_an_exact_epoch_comparison() {
        let stamp = AclStamp {
            epoch: 5,
            ..stamp_with(&["orders:read"])
        };
        assert!(!stamp.is_stale(5), "la même époque n'est pas périmée");
        assert!(stamp.is_stale(4), "une époque antérieure est périmée");
        assert!(
            stamp.is_stale(6),
            "une époque postérieure aussi : toute différence compte"
        );
        assert!(stamp.is_stale(0), "y compris une époque nulle");
        assert!(stamp.is_stale(u64::MAX));
    }

    #[test]
    fn the_digest_is_a_stable_thirty_two_character_lowercase_hex() {
        let empty = stamp_with(&[]);
        let one = stamp_with(&["orders:read"]);
        let two = stamp_with(&["orders:read", "orders:write"]);

        for stamp in [&empty, &one, &two] {
            let bits = stamp.digest.clone();
            assert_eq!(
                bits.len(),
                32,
                "largeur fixe (SHA-256 tronqué à 16 octets) : {bits:?}"
            );
            assert!(
                bits.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "hexadécimal minuscule : {bits:?}"
            );
            assert_eq!(
                bits,
                stamp.clone().digest.clone(),
                "stable pour un même stamp"
            );
        }
        // Un digest de contenu n'a pas de « zéro » : l'ensemble vide a son
        // propre digest, et il diffère de tout autre.
        assert_eq!(empty.digest.len(), 32);
        assert_ne!(empty.digest, one.digest);
        assert_ne!(one.digest, two.digest);
        assert_eq!(
            two.digest,
            Iam::digest_of(
                "t",
                "u",
                &["r".to_string()],
                &["orders:read".to_string(), "orders:write".to_string()]
            ),
            "le digest est déterministe pour un même contenu"
        );
        assert_ne!(one.digest.clone(), two.digest.clone());
    }

    #[test]
    fn the_digest_distinguishes_sets_of_the_same_size_with_different_names() {
        let mut seen = std::collections::HashSet::new();
        for n in 0..8 {
            let perms: Vec<String> = (0..n).map(|i| format!("p:{i}")).collect();
            let stamp = AclStamp {
                digest: Iam::digest_of("t", "u", &["r".to_string()], &perms),
                permissions: perms.clone(),
                ..stamp_with(&[])
            };
            assert!(
                seen.insert(stamp.digest.clone()),
                "un digest de contenu doit distinguer deux ensembles différents (n={n})"
            );
        }
    }

    /// **Corrigé** : le digest est un hachage du **contenu** (portée, principal,
    /// rôles, permissions) — il distingue deux décisions de même taille dont les
    /// noms diffèrent, et ne sature jamais (l'ancien masque, lui, ne distinguait
    /// que le cardinal et plafonnait à 128 permissions).
    /// Deux décisions différentes mais de même longueur produisent donc le
    /// **même** bitset. Or le commentaire du code présente ces bits comme
    /// « suffisant pour comparer deux décisions » : tout appelant qui s'en sert
    /// comme empreinte tiendra deux décisions distinctes pour identiques.
    #[test]
    fn bits_hex_distinguishes_two_decisions_of_the_same_length_with_different_names() {
        let a = stamp_with(&["orders:read", "orders:write"]);
        let b = stamp_with(&["catalog:read", "catalog:write"]);
        assert_ne!(
            a.permissions, b.permissions,
            "les deux décisions sont bien différentes"
        );
        assert_ne!(
            a.digest.clone(),
            b.digest.clone(),
            "deux décisions différentes doivent avoir des bits différents, or les deux valent {}",
            a.digest.clone()
        );
    }

    /// **Défaut constaté** : `bits_hex` sature à 128 bits (`.take(128)`). Au-delà,
    /// toutes les permissions supplémentaires sont invisibles : une décision à
    /// 130 permissions et une décision à 200 permissions ont le même bitset.
    #[test]
    fn the_digest_does_not_collapse_beyond_one_hundred_and_twenty_eight_permissions() {
        let many = |n: usize| {
            let permissions: Vec<String> = (0..n).map(|i| format!("p:{i}")).collect();
            AclStamp {
                digest: Iam::digest_of("t", "u", &["r".to_string()], &permissions),
                permissions,
                ..stamp_with(&[])
            }
        };
        let a = many(130);
        let b = many(200);
        assert_ne!(
            a.digest.clone(),
            b.digest.clone(),
            "130 et 200 permissions ne doivent pas avoir le même bitset ({} caractères de décision ignorés)",
            200 - 128
        );
    }

    #[test]
    fn a_stamp_round_trips_through_json_without_loss() {
        let stamp = AclStamp {
            permissions: vec!["*".into(), "café:lire".into()],
            roles: vec!["r1".into(), "r2".into()],
            epoch: 17,
            ..stamp_with(&[])
        };

        let json = serde_json::to_string(&stamp).expect("sérialisation");
        assert!(
            json.contains(IAM_SCHEMA),
            "le schéma est sérialisé : {json}"
        );
        assert!(json.contains("café:lire"), "l'UTF-8 est sérialisé tel quel");

        let back: AclStamp = serde_json::from_str(&json).expect("désérialisation");
        assert_eq!(back, stamp, "aller-retour JSON sans perte");
        assert!(back.allows("café:lire"));
        assert!(back.allows("*"));
        assert!(!back.is_stale(17));
    }

    // -----------------------------------------------------------------------
    // 8. stamp_of
    // -----------------------------------------------------------------------

    #[test]
    fn stamp_of_returns_none_for_a_node_that_is_not_a_decision() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let session = store.session();
        let page = session.create_node_with_props(
            &["Page"],
            [("title", v("une page quelconque")), ("name", v("page"))],
        );
        store.commit_wal().unwrap();

        assert_eq!(
            iam.stamp_of(page).unwrap(),
            None,
            "un nœud `Page` n'est pas une décision : None, sans panique ni erreur inventée"
        );
    }

    #[test]
    fn stamp_of_returns_none_for_an_absent_node_without_panicking() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let absent = obrain_common::types::NodeId::new(u64::MAX - 1);
        assert_eq!(
            iam.stamp_of(absent).unwrap(),
            None,
            "nœud inexistant ⇒ None"
        );
        assert_eq!(
            iam.stamp_of(obrain_common::types::NodeId::new(1)).unwrap(),
            None,
            "identifiant jamais alloué ⇒ None"
        );
    }

    #[test]
    fn stamp_of_reads_back_the_expected_stamp_from_a_decision_node() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let written = iam
            .materialize("user:42", "tenant-a", &standard_grants())
            .unwrap();
        let node = decision_node(&store, &Iam::acl_key("tenant-a", "user:42"));

        let read = iam
            .stamp_of(node)
            .unwrap()
            .expect("nœud de décision valide");

        assert_eq!(
            read, written,
            "stamp_of doit reconstruire EXACTEMENT le stamp écrit"
        );
        assert_eq!(read.schema, IAM_SCHEMA);
        assert_eq!(read.key, Iam::acl_key("tenant-a", "user:42"));
        assert_eq!(read.principal, "user:42");
        assert_eq!(read.scope, "tenant-a");
        assert_eq!(read.epoch, 1);
        assert_eq!(read, iam.decision("user:42", "tenant-a").unwrap().unwrap());
    }

    /// **Fragilité constatée** : `stamp_of` ne vérifie **pas** le label du nœud.
    /// Tout nœud portant une propriété `acl_key` est lu comme une décision — ici
    /// une page arbitraire devient une décision d'époque 0, sans rôle ni
    /// permission. `ACL_LABEL` n'est écrit qu'à la création et n'est jamais
    /// relu. Une simple collision de nom de propriété suffit donc à fabriquer
    /// une décision fantôme.
    #[test]
    fn stamp_of_rejects_a_non_decision_node_that_carries_an_acl_key() {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let session = store.session();
        let impostor = session.create_node_with_props(
            &["Page"],
            [
                (ACL_KEY_PROP, v(&Iam::acl_key("t", "u"))),
                ("title", v("pas une décision")),
            ],
        );
        store.commit_wal().unwrap();

        assert_eq!(
            iam.stamp_of(impostor).unwrap(),
            None,
            "un nœud non-`Acl` ne doit pas être lu comme une décision"
        );
    }

    // -----------------------------------------------------------------------
    // 9. Durabilité — la preuve que `commit_wal` est bien appelé
    // -----------------------------------------------------------------------

    #[test]
    fn a_decision_and_its_invalidation_survive_a_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();

        // Première vie : matérialiser, puis invalider.
        let written: AclStamp = {
            let store = ObrainStore::open(dir.path()).unwrap();
            let iam = store.iam();
            let stamp = iam
                .materialize("user:9", "tenant-a", &standard_grants())
                .unwrap();
            assert_eq!(iam.invalidate("user:9", "tenant-a").unwrap(), 2);
            stamp
        };

        // Seconde vie : la décision invalidée est toujours là, avec son époque.
        {
            let store = ObrainStore::open(dir.path()).unwrap();
            let iam = store.iam();
            let voided = iam
                .decision("user:9", "tenant-a")
                .unwrap()
                .expect("la décision doit survivre à la fermeture : commit_wal est bien appelé");
            assert_eq!(voided.key, written.key);
            assert_eq!(voided.principal, "user:9");
            assert_eq!(voided.scope, "tenant-a");
            assert_eq!(voided.epoch, 2, "l'époque survit");
            assert!(voided.permissions.is_empty(), "l'invalidation survit aussi");
            assert!(!voided.allows("orders:read"), "fail-closed durablement");
            assert!(written.is_stale(voided.epoch));
            assert_eq!(
                acl_nodes(&store, &Iam::acl_key("tenant-a", "user:9")),
                1,
                "l'index de clé d'ACL est reconstruit à l'ouverture"
            );
        }

        // Troisième vie : l'époque CONTINUE de croître, sans second nœud.
        {
            let store = ObrainStore::open(dir.path()).unwrap();
            let iam = store.iam();
            let again = iam
                .materialize("user:9", "tenant-a", &standard_grants())
                .unwrap();
            assert_eq!(
                again.epoch,
                written.epoch + 2,
                "l'époque reprend au-dessus de la précédente : elle ne repart pas à 1"
            );
            assert_eq!(
                again.permissions, written.permissions,
                "le contenu est restauré"
            );
            assert_eq!(
                acl_nodes(&store, &Iam::acl_key("tenant-a", "user:9")),
                1,
                "un seul nœud de décision, réutilisé après réouverture"
            );
            assert_eq!(principal_nodes(&store, "user:9"), 1);
            assert_eq!(
                iam.decision("user:9", "tenant-a").unwrap().as_ref(),
                Some(&again),
                "la relecture indexée rend la nouvelle décision"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 10. Index
    // -----------------------------------------------------------------------

    #[test]
    fn the_plan_creates_both_indexes_and_building_it_twice_has_no_side_effect() {
        let store = ObrainStore::new_in_memory();
        let first = Iam::new(Arc::clone(&store));

        // Premier plan : les deux index existent.
        assert!(
            store.db().has_property_index(ACL_KEY_PROP),
            "l'index de la clé d'ACL est le chemin de lecture : il doit exister"
        );
        assert!(
            store.db().has_property_index(PRINCIPAL_QN_PROP),
            "l'index du nom qualifié évite un balayage à la recherche du principal"
        );

        // Construire le plan deux fois ne change rien, et ne casse rien.
        let stamp = first.materialize("u", "t", &standard_grants()).unwrap();
        let nodes = store.db().node_count();
        let second = Iam::new(Arc::clone(&store));

        assert!(store.db().has_property_index(ACL_KEY_PROP));
        assert!(store.db().has_property_index(PRINCIPAL_QN_PROP));
        assert_eq!(
            store.db().node_count(),
            nodes,
            "reconstruire le plan ne crée aucun nœud"
        );
        assert_eq!(
            second.decision("u", "t").unwrap().as_ref(),
            Some(&stamp),
            "et le second plan sert la même décision, sans nouvelle époque"
        );
        assert_eq!(
            second
                .materialize("u", "t", &standard_grants())
                .unwrap()
                .epoch,
            stamp.epoch + 1,
            "l'époque est portée par le store, pas par le plan"
        );
        assert_eq!(acl_nodes(&store, &Iam::acl_key("t", "u")), 1);
    }

    // -----------------------------------------------------------------------
    // 11. Fidélité — la décision matérialisée vaut la traversée réelle
    // -----------------------------------------------------------------------

    /// Matérialise un principal, puis construit à la main le VRAI graphe
    /// `Principal→Role→Permission` et rend (matérialisé, traversé).
    fn materialised_versus_traversed(grants: &[RoleGrant]) -> (Vec<String>, Vec<String>) {
        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let stamp = iam.materialize("user:f", "t", grants).unwrap();

        let session = store.session();
        let principal = store
            .db()
            .find_nodes_by_property(PRINCIPAL_QN_PROP, &v("user:f"))
            .into_iter()
            .next()
            .expect("la matérialisation crée le nœud principal");
        for g in grants {
            let role = session.create_node_with_props(&["Role"], [("name", v(&g.role))]);
            session.create_edge(principal, role, "HAS_ROLE");
            for permission in &g.permissions {
                let perm =
                    session.create_node_with_props(&["Permission"], [("name", v(permission))]);
                session.create_edge(role, perm, "GRANTS");
            }
        }
        store.commit_wal().unwrap();

        (
            stamp.permissions,
            iam.permissions_by_traversal("user:f").unwrap(),
        )
    }

    #[test]
    fn the_materialised_set_equals_a_real_traversal_for_a_single_role() {
        let (materialised, traversed) =
            materialised_versus_traversed(&[grant("reader", &["orders:read", "catalog:read"])]);
        assert_eq!(
            materialised, traversed,
            "un rôle : la décision indexée doit valoir la traversée"
        );
        assert_eq!(traversed, vec!["catalog:read", "orders:read"]);
    }

    #[test]
    fn the_materialised_set_equals_a_real_traversal_for_three_roles() {
        let (materialised, traversed) = materialised_versus_traversed(&[
            grant("a", &["p:1"]),
            grant("b", &["p:2"]),
            grant("c", &["p:3"]),
        ]);
        assert_eq!(materialised, traversed, "trois rôles disjoints");
        assert_eq!(traversed, vec!["p:1", "p:2", "p:3"]);
    }

    #[test]
    fn the_materialised_set_equals_a_real_traversal_for_a_role_without_permission() {
        let (materialised, traversed) = materialised_versus_traversed(&[
            grant("empty", &[]),
            grant("reader", &["orders:read"]),
        ]);
        assert_eq!(
            materialised, traversed,
            "un rôle sans arête sortante ne doit rien ajouter — ni côté décision, ni côté traversée"
        );
        assert_eq!(traversed, vec!["orders:read"]);
    }

    #[test]
    fn the_materialised_set_equals_a_real_traversal_for_two_overlapping_roles() {
        let (materialised, traversed) = materialised_versus_traversed(&[
            grant("reader", &["orders:read", "catalog:read"]),
            grant("ops", &["orders:read", "orders:write"]),
        ]);
        assert_eq!(
            materialised, traversed,
            "deux rôles qui se recouvrent : la déduplication doit tomber sur le même ensemble"
        );
        assert_eq!(
            traversed,
            vec!["catalog:read", "orders:read", "orders:write"]
        );
    }
}
