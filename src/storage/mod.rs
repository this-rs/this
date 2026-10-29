//! Storage implementations for different backends

/// Annuaire d'identités persisté (chantier STS-PERSISTANT) : les utilisateurs
/// de la passerelle dans un store obrain dédié, mots de passe argon2id.
#[cfg(all(feature = "obrain", feature = "wami"))]
pub mod annuaire_identites;
#[cfg(feature = "dynamodb")]
pub mod dynamodb;
/// IAM **matérialisé** : la décision d'autorisation est une lecture indexée.
#[cfg(feature = "obrain")]
pub mod iam;
pub mod in_memory;
#[cfg(feature = "lmdb")]
pub mod lmdb;
#[cfg(feature = "mongodb_backend")]
pub mod mongodb;
#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "neo4j")]
pub mod neo4j;
#[cfg(feature = "obrain")]
pub mod obrain;
/// `GraphBackend` — la couture de bascule entre obrain et this-rs.
#[cfg(feature = "obrain")]
pub mod obrain_facade;
/// Adaptateur obrain de l'identité canonique — la moitié I/O de `core::identity`.
#[cfg(feature = "obrain")]
pub mod obrain_identity;
pub mod permission;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "scylladb")]
pub mod scylladb;
pub mod tenant_scoped;
/// Catalogue tenant Uuid <-> u32 d'un store de donnees — substrate.tenantcat
/// (NATIF-J4, RFC principal-natif-v2 §5.1).
#[cfg(feature = "obrain")]
pub mod tenantcat;
/// Virtual file system and module store (interfaces; module descriptor RFC §3.8).
pub mod vfs;
/// Bail d'écriture et jeton de fencing — l'unicité de l'écrivain, **imposée**.
///
/// Non gaté : std + serde seulement. Le format de trame (128 octets, CRC32) est
/// gelé par test.
pub mod writer_lease;

#[cfg(all(feature = "obrain", feature = "wami"))]
pub use self::annuaire_identites::{AnnuaireIdentites, Utilisateur};
#[cfg(feature = "obrain")]
pub use self::iam::{AclStamp, IAM_SCHEMA, Iam, IamError, RoleGrant};
#[cfg(feature = "lmdb")]
pub use self::lmdb::{LmdbDataService, LmdbLinkService};
#[cfg(feature = "mongodb_backend")]
pub use self::mongodb::{MongoDataService, MongoLinkService};
#[cfg(feature = "mysql")]
pub use self::mysql::{MysqlDataService, MysqlLinkService};
#[cfg(feature = "neo4j")]
pub use self::neo4j::{Neo4jDataService, Neo4jLinkService};
#[cfg(feature = "obrain")]
pub use self::obrain::{ObrainDataService, ObrainLinkService, ObrainStore};
#[cfg(feature = "obrain")]
pub use self::obrain_facade::GraphBackend;
#[cfg(feature = "obrain")]
pub use self::obrain_identity::{IdentityReport, LabelReport};
#[cfg(feature = "dynamodb")]
pub use dynamodb::{DynamoDBDataService, DynamoDBLinkService};
pub use in_memory::{InMemoryDataService, InMemoryLinkService};
#[cfg(feature = "postgres")]
pub use postgres::{PostgresDataService, PostgresLinkService};
#[cfg(feature = "scylladb")]
pub use scylladb::{ScyllaDataService, ScyllaLinkService};
pub use tenant_scoped::{TenantAware, TenantScopedLinkService};
