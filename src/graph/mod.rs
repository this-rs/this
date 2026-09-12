//! Relief d'obrain, publié par this-rs.
//!
//! # Le principe
//!
//! this-rs ne tient pas de catalogue du relief d'obrain : obrain le publie,
//! this-rs le **lit** et le **republic**. [`Relief::describe`](crate::graph::relief::Relief::describe) interroge
//! le moteur — registre des procédures, état du substrat, couche simpliciale,
//! espaces — et rend un document sérialisable.
//!
//! Corollaire recherché : quand obrain ajoute ou retire un algorithme, le
//! document change **sans qu'une ligne bouge ici**. Il n'y a rien à tenir à
//! jour, donc rien qui puisse diverger.
//!
//! # Ce que le document refuse de faire
//!
//! - Il **marque** chaque affirmation [`Origin::Introspected`](crate::graph::relief::Origin::Introspected) ou
//!   [`Origin::Declared`](crate::graph::relief::Origin::Declared). Ce qui n'est pas introspectable est déclaré
//!   comme tel — jamais présenté comme un fait lu.
//! - Il **publie ses propres limites** ([`Trap`](crate::graph::relief::Trap)), chacune adossée à un
//!   `fichier:ligne` d'obrain.
//! - Il ne rend **jamais** d'erreur pour un fait indisponible : un fait manquant
//!   est un `false`/`None` **accompagné de sa raison**.
//!
//! Un relief qui mentirait serait pire que pas de relief : il ferait prendre
//! des décisions sur des colonnes fausses, des compteurs vides pris pour des
//! zéros, ou des procédures absentes crues disponibles.
//!
//! # Publication
//!
//! [`graph_routes`](crate::graph::routes::graph_routes) expose le document en REST (`GET /graph`), monté
//! automatiquement par `RestExposure` dès qu'un `ObrainStore` est attaché au
//! `ServerHost` (voir `ServerHost::with_graph_store`).

pub mod relief;
pub mod routes;
pub mod scope;

pub mod canvas;
pub mod hierarchy;

pub use relief::{
    EngineRelief, FamilySpec, Origin, ParameterSpec, ProcedureSpec, RELIEF_SCHEMA, Relief,
    ReliefOptions, SimplexCounts, SimplexRelief, SpaceRelief, Trap,
};
pub use routes::{GraphState, graph_routes};
