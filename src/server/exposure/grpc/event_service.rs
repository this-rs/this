//! gRPC Event Service implementation — server-streaming real-time events
//!
//! Subscribes to the framework's `EventBus` (the same broadcast channel used by
//! WebSocket) and streams matching events to gRPC clients. Filters from the
//! `SubscribeRequest` are applied server-side (AND logic, absent = wildcard).
//!
//! ## La portée d'un flux : fail-closed (2026-10-01)
//!
//! Un flux d'événements ne se reprend pas : une fois qu'un client a lu ce qu'il
//! n'aurait pas dû, c'est lu. La portée du flux est donc décidée **avant**
//! d'ouvrir quoi que ce soit, et l'absence de portée est un **refus** —
//! `PermissionDenied`, avec la raison et le remède (`portee_du_flux`).
//!
//! Avant, l'absence de portée n'était pas un refus : aucun interceptor ne pose
//! d'`AuthContext` côté gRPC, l'en-tête `x-tenant-id` était le seul recours, et
//! sans lui la garde de filtrage ne s'appliquait **pas** — le flux recevait les
//! événements de tous les locataires. Le test d'intégration
//! `un_flux_d_evenements_sans_portee_est_refuse` (`tests/grpc_integration.rs`)
//! exerce désormais ce refus sur un serveur réel.
//!
//! ## Architecture
//!
//! ```text
//! REST/GraphQL Handler → EventBus::publish()
//!                              ↓
//!                      broadcast channel
//!                        ↓           ↓
//!              WebSocket Manager   EventServiceImpl::subscribe()
//!                                     ↓ (portée, puis filtre)
//!                                  gRPC stream → client
//! ```

use super::convert::json_to_struct;
use super::proto::{EventResponse, SubscribeRequest, event_service_server::EventService};
use crate::core::auth::AuthContext;
use crate::core::events::{EntityEvent, EventEnvelope, FrameworkEvent, LinkEvent};
use crate::server::host::ServerHost;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// gRPC Event Service implementation
///
/// Subscribes to the framework's `EventBus` and streams events to gRPC clients.
/// Each `Subscribe` call creates an independent broadcast receiver, filters
/// events according to `SubscribeRequest`, and forwards matching events as
/// `EventResponse` messages on the server-streaming response.
pub struct EventServiceImpl {
    host: Arc<ServerHost>,
}

impl EventServiceImpl {
    /// Create a new `EventServiceImpl` from a `ServerHost`
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
    }
}

// ---------------------------------------------------------------------------
// Filter logic
// ---------------------------------------------------------------------------

/// Check if an event matches the subscribe request filters.
///
/// All fields are AND conditions. An absent (empty) field means "match any".
fn matches_filter(event: &FrameworkEvent, filter: &SubscribeRequest) -> bool {
    // Filter by kind ("entity" or "link")
    if let Some(ref kind) = filter.kind
        && !kind.is_empty()
        && event.event_kind() != kind
    {
        return false;
    }

    // Filter by entity_type
    if let Some(ref entity_type) = filter.entity_type
        && !entity_type.is_empty()
    {
        match event.entity_type() {
            Some(et) if et == entity_type => {}
            Some(_) => return false,
            None => return false,
        }
    }

    // Filter by entity_id
    if let Some(ref entity_id) = filter.entity_id
        && !entity_id.is_empty()
    {
        let parsed = entity_id.parse::<Uuid>().ok();
        match (parsed, event.entity_id()) {
            (Some(filter_id), Some(event_id)) if filter_id == event_id => {}
            _ => return false,
        }
    }

    // Filter by event_type (action: "created", "updated", "deleted")
    if let Some(ref event_type) = filter.event_type
        && !event_type.is_empty()
        && event.action() != event_type
    {
        return false;
    }

    // Filter by link_type (only relevant for link events)
    if let Some(ref link_type) = filter.link_type
        && !link_type.is_empty()
    {
        match extract_link_type(event) {
            Some(lt) if lt == link_type => {}
            Some(_) => return false,
            None => return false,
        }
    }

    true
}

/// Extract the link_type from a `FrameworkEvent`, if it's a link event.
fn extract_link_type(event: &FrameworkEvent) -> Option<&str> {
    match event {
        FrameworkEvent::Link(link) => match link {
            LinkEvent::Created { link_type, .. } | LinkEvent::Deleted { link_type, .. } => {
                Some(link_type)
            }
        },
        FrameworkEvent::Entity(_) => None,
        FrameworkEvent::Cognitive(_) => None,
        FrameworkEvent::Module(_) => None,
        FrameworkEvent::GdprErasure { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// Envelope → EventResponse conversion
// ---------------------------------------------------------------------------

/// Convert an `EventEnvelope` into a proto `EventResponse`.
fn envelope_to_response(envelope: &EventEnvelope) -> EventResponse {
    let event = &envelope.event;

    let (entity_type, entity_id, link_type, source_id, target_id, data, metadata) = match event {
        FrameworkEvent::Entity(e) => match e {
            EntityEvent::Created {
                entity_type,
                entity_id,
                data,
            } => (
                entity_type.clone(),
                entity_id.to_string(),
                String::new(),
                String::new(),
                String::new(),
                Some(json_to_struct(data)),
                None,
            ),
            EntityEvent::Updated {
                entity_type,
                entity_id,
                data,
            } => (
                entity_type.clone(),
                entity_id.to_string(),
                String::new(),
                String::new(),
                String::new(),
                Some(json_to_struct(data)),
                None,
            ),
            EntityEvent::Deleted {
                entity_type,
                entity_id,
            } => (
                entity_type.clone(),
                entity_id.to_string(),
                String::new(),
                String::new(),
                String::new(),
                None,
                None,
            ),
        },
        FrameworkEvent::Link(l) => match l {
            LinkEvent::Created {
                link_type,
                link_id,
                source_id,
                target_id,
                metadata,
            } => (
                String::new(),
                link_id.to_string(),
                link_type.clone(),
                source_id.to_string(),
                target_id.to_string(),
                None,
                metadata.as_ref().map(json_to_struct),
            ),
            LinkEvent::Deleted {
                link_type,
                link_id,
                source_id,
                target_id,
            } => (
                String::new(),
                link_id.to_string(),
                link_type.clone(),
                source_id.to_string(),
                target_id.to_string(),
                None,
                None,
            ),
        },
        FrameworkEvent::Cognitive(_) => (
            "cognitive".to_string(),
            event
                .entity_id()
                .map(|id| id.to_string())
                .unwrap_or_default(),
            String::new(),
            String::new(),
            String::new(),
            None,
            None,
        ),
        FrameworkEvent::GdprErasure { tenant_id, .. } => (
            "gdpr_erasure".to_string(),
            tenant_id.to_string(),
            String::new(),
            String::new(),
            String::new(),
            None,
            None,
        ),
        FrameworkEvent::Module(m) => (
            "module".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            Some(json_to_struct(&serde_json::to_value(m).unwrap_or_default())),
            None,
        ),
    };

    EventResponse {
        event_id: envelope.id.to_string(),
        event_kind: event.event_kind().to_string(),
        event_type: event.action().to_string(),
        entity_type,
        entity_id,
        link_type,
        source_id,
        target_id,
        data,
        metadata,
        timestamp: envelope.timestamp.to_rfc3339(),
        seq_no: envelope.seq_no.unwrap_or(0),
    }
}

// ---------------------------------------------------------------------------
// gRPC trait implementation
// ---------------------------------------------------------------------------

type SubscribeStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<EventResponse, Status>> + Send>>;

/// La portée d'un flux d'événements : **ce que ce flux a le droit de voir**.
///
/// Il n'y a pas de troisième cas « je ne sais pas » : l'inconnu est un refus
/// ([`portee_du_flux`] rend une erreur). C'est la doctrine du dépôt — depuis le
/// 2026-09-13, l'abstention d'autorisation est un refus — appliquée ici, où
/// elle manquait le plus : un flux d'événements ne se reprend pas, une fois
/// qu'un client a lu ce qu'il n'aurait pas dû.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PorteeFlux {
    /// Un locataire : le flux ne porte que ses événements.
    Locataire(Uuid),
    /// **Global, et déclaré comme tel** : un administrateur authentifié voit
    /// tout. Ce n'est pas un défaut, c'est un principal nommé — la différence
    /// entre « tout le monde voit tout » et « l'administrateur voit tout ».
    Global,
}

/// La portée d'un flux, ou **le refus d'ouvrir**.
///
/// # Ce qui a changé, et pourquoi (2026-10-01)
///
/// Avant, cette résolution rendait `Option<Uuid>` :
/// 1. `AuthContext` dans les extensions (posé par un interceptor) ;
/// 2. sinon l'en-tête `x-tenant-id`.
///
/// Et `None` — aucune identité, aucun en-tête — n'était pas un refus : la garde
/// de filtrage du flux ne s'appliquait simplement **pas**, et le flux non scopé
/// recevait les événements de **tous** les locataires. Le commentaire d'origine
/// disait « anonyme / appels système » : c'était, en fait, « tout », pour qui
/// savait ouvrir un canal HTTP/2. Le fail-closed était écrit dans l'atlas comme
/// « lu dans le code, exercé par aucun test » ; il n'était, en réalité, pas
/// appliqué du tout ici.
///
/// Désormais :
/// - une identité avec locataire → `Locataire` (l'identité **prime** sur
///   l'en-tête : un client ne choisit pas son locataire quand un jeton en
///   désigne un) ;
/// - un `Admin` authentifié (sans locataire) → `Global`, explicitement ;
/// - sinon, l'en-tête `x-tenant-id` **bien formé** → `Locataire` (le geste
///   explicite reste possible, il vient de la configuration d'exposition) ;
/// - **rien du tout → `PermissionDenied`**, avec la raison et le remède.
///
/// ⚠️ Ce qui **reste** ouvert, et qu'il faut savoir : l'en-tête `x-tenant-id`
/// est un **choix du client**, pas une preuve — le middleware HTTP
/// (`server/middleware/tenant.rs`) lui accorde la même confiance, et rien ne le
/// confronte à l'identité. Le fermer demande de trancher une politique
/// d'ensemble (en-tête honoré seulement pour un administrateur, ou seulement
/// derrière une option déclarée) : c'est écrit ici pour que la décision se
/// prenne, plutôt que d'être oubliée.
fn portee_du_flux<T>(request: &Request<T>) -> Result<PorteeFlux, Status> {
    // 1. L'identité d'abord — elle seule peut désigner un locataire *prouvé*.
    if let Some(auth) = request.extensions().get::<AuthContext>() {
        if let Some(tid) = auth.tenant_id() {
            return Ok(PorteeFlux::Locataire(tid));
        }
        if matches!(auth, AuthContext::Admin { .. }) {
            return Ok(PorteeFlux::Global);
        }
        // Un principal authentifié SANS locataire et sans rôle d'administration
        // (un `User` sans locataire, p. ex.) n'a pas de portée : on ne lui
        // invente pas « tout ».
    }

    // 2. L'en-tête explicite — le geste déclaré, pas la preuve.
    if let Some(tid) = en_tete_locataire(request) {
        return Ok(PorteeFlux::Locataire(tid));
    }

    Err(Status::permission_denied(
        "flux d'événements : aucune portée — l'abstention est un refus. \
         Présentez une identité (un interceptor gRPC qui pose `AuthContext` : \
         un jeton porteur de locataire scope le flux, un administrateur le voit \
         entier), ou déclarez le locataire par l'en-tête `x-tenant-id`. \
         Auparavant, une requête sans portée recevait les événements de TOUS \
         les locataires.",
    ))
}

/// Le locataire déclaré par l'en-tête `x-tenant-id` — s'il est bien formé.
fn en_tete_locataire<T>(request: &Request<T>) -> Option<Uuid> {
    request
        .metadata()
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<Uuid>().ok())
}

#[tonic::async_trait]
impl EventService for EventServiceImpl {
    type SubscribeStream = SubscribeStream;

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        // **La portée AVANT tout le reste, et avant l'`EventBus`** : une requête
        // sans portée n'obtient pas un « service indisponible » (qui se lirait
        // comme un incident), elle obtient un REFUS motivé. L'ordre compte aussi
        // pour une raison de fond : on ne veut pas qu'un serveur mal configuré
        // (sans bus) fasse passer le refus pour une panne.
        let portee = portee_du_flux(&request)?;
        let locataire_du_flux = match portee {
            PorteeFlux::Locataire(tid) => Some(tid),
            PorteeFlux::Global => None,
        };
        let filter = request.into_inner();

        // Get the EventBus — if not configured, streaming is unavailable
        let event_bus = self
            .host
            .event_bus()
            .ok_or_else(|| {
                Status::unavailable(
                    "EventBus not configured — real-time streaming is not available",
                )
            })?
            .clone();

        // Subscribe to the broadcast channel
        let mut rx = event_bus.subscribe();

        // Channel to stream events to the gRPC response
        // Buffer of 64 — enough headroom for bursts without excessive memory
        let (tx, client_rx) = mpsc::channel::<Result<EventResponse, Status>>(64);

        // Spawn background task: receive from broadcast → filter → send to gRPC stream
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        // Tenant isolation: skip events from other tenants.
                        //
                        // `locataire_du_flux` vaut `None` **seulement** pour un
                        // administrateur authentifié (`PorteeFlux::Global`) : il
                        // n'y a plus de flux « sans portée » qui verrait tout.
                        if let Some(conn_tenant) = locataire_du_flux
                            && let Some(event_tenant) = envelope.tenant_id()
                            && conn_tenant != event_tenant
                        {
                            continue;
                        }

                        if matches_filter(&envelope.event, &filter) {
                            let response = envelope_to_response(&envelope);
                            // If the client disconnected, tx.send() returns Err → break
                            if tx.send(Ok(response)).await.is_err() {
                                tracing::debug!("gRPC event stream: client disconnected, closing");
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        tracing::warn!("gRPC event stream: lagged by {} events, skipping", count);
                        // Continue — the client misses some events but the stream stays alive
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("gRPC event stream: EventBus closed, ending stream");
                        break;
                    }
                }
            }
        });

        let stream = ReceiverStream::new(client_rx);
        Ok(Response::new(Box::pin(stream) as Self::SubscribeStream))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use serde_json::json;

    // === Portée du flux d'événements (fail-closed) ===
    //
    // Ces tests exercent le refus : avant le 2026-10-01, une requête sans
    // identité NI en-tête ouvrait un flux qui recevait les événements de TOUS
    // les locataires. Le fail-closed était écrit dans l'atlas comme « lu dans le
    // code, exercé par aucun test » : il n'était pas appliqué du tout ici.

    fn requete_vide() -> Request<SubscribeRequest> {
        Request::new(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        })
    }

    fn avec_identite(auth: AuthContext) -> Request<SubscribeRequest> {
        let mut request = requete_vide();
        request.extensions_mut().insert(auth);
        request
    }

    fn avec_en_tete(tid: Uuid) -> Request<SubscribeRequest> {
        let mut request = requete_vide();
        request
            .metadata_mut()
            .insert("x-tenant-id", tid.to_string().parse().unwrap());
        request
    }

    #[test]
    fn un_flux_sans_portee_est_refuse() {
        let err = portee_du_flux(&requete_vide())
            .expect_err("sans identité ni en-tête, le flux ne doit pas s'ouvrir");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
        // Le refus doit DIRE la raison et le remède : un refus muet apprend à
        // désarmer la garde pour faire taire le symptôme.
        assert!(
            err.message().contains("x-tenant-id") && err.message().contains("AuthContext"),
            "le refus doit nommer le remède, obtenu : {}",
            err.message()
        );
    }

    #[test]
    fn une_identite_avec_locataire_scope_le_flux() {
        let tenant = Uuid::new_v4();
        let request = avec_identite(AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: tenant,
            roles: vec![],
        });
        assert_eq!(
            portee_du_flux(&request).unwrap(),
            PorteeFlux::Locataire(tenant)
        );
    }

    #[test]
    fn un_anonyme_sans_en_tete_est_refuse() {
        let err = portee_du_flux(&avec_identite(AuthContext::Anonymous))
            .expect_err("un anonyme n'a pas de portée");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn un_administrateur_authentifie_voit_tout_explicitement() {
        let request = avec_identite(AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        });
        assert_eq!(portee_du_flux(&request).unwrap(), PorteeFlux::Global);
    }

    #[test]
    fn l_identite_prime_sur_l_en_tete() {
        let tenant_du_jeton = Uuid::new_v4();
        let tenant_reclame = Uuid::new_v4();
        let mut request = avec_identite(AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: tenant_du_jeton,
            roles: vec![],
        });
        request
            .metadata_mut()
            .insert("x-tenant-id", tenant_reclame.to_string().parse().unwrap());

        assert_eq!(
            portee_du_flux(&request).unwrap(),
            PorteeFlux::Locataire(tenant_du_jeton),
            "un client ne choisit pas son locataire quand un jeton en désigne un"
        );
    }

    #[test]
    fn l_en_tete_seul_declare_une_portee_mais_ne_l_invente_pas() {
        let tenant = Uuid::new_v4();
        assert_eq!(
            portee_du_flux(&avec_en_tete(tenant)).unwrap(),
            PorteeFlux::Locataire(tenant)
        );

        // Un en-tête MAL FORMÉ ne vaut rien : il ne devient pas « tout », il
        // devient un refus.
        let mut mal_forme = requete_vide();
        mal_forme
            .metadata_mut()
            .insert("x-tenant-id", "pas-un-uuid".parse().unwrap());
        assert_eq!(
            portee_du_flux(&mal_forme).unwrap_err().code(),
            tonic::Code::PermissionDenied
        );
    }

    /// Le refus est **câblé** : ce n'est pas seulement la fonction de résolution
    /// qui refuse, c'est le service — et il refuse **avant** de regarder si un
    /// bus est configuré. L'ordre est délibéré : un serveur mal configuré (sans
    /// `EventBus`) ne doit pas faire passer un refus pour une panne, sinon on
    /// apprend à relire les refus comme des incidents.
    #[tokio::test]
    async fn un_abonnement_sans_portee_est_refuse_par_le_service() {
        use crate::server::host::ServerHost;

        let event_bus = EventBus::new(64);
        let host = Arc::new(ServerHost::minimal_for_test().with_event_bus(event_bus));
        let svc = EventServiceImpl::new(host);

        match svc.subscribe(requete_vide()).await {
            Err(status) => assert_eq!(status.code(), tonic::Code::PermissionDenied),
            Ok(_) => panic!("sans portée, le flux ne doit pas s'ouvrir"),
        }
    }

    // === Filter tests ===
    #[test]
    fn test_filter_empty_matches_everything() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        };

        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(matches_filter(&entity, &filter));
        assert!(matches_filter(&link, &filter));
    }

    #[test]
    fn test_filter_by_entity_type() {
        let filter = SubscribeRequest {
            entity_type: Some("user".to_string()),
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        };

        let user = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let capture = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "capture".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(matches_filter(&user, &filter));
        assert!(!matches_filter(&capture, &filter));
    }

    #[test]
    fn test_filter_by_kind_entity() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: Some("entity".to_string()),
            link_type: None,
        };

        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(matches_filter(&entity, &filter));
        assert!(!matches_filter(&link, &filter));
    }

    #[test]
    fn test_filter_by_kind_link() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: Some("link".to_string()),
            link_type: None,
        };

        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(!matches_filter(&entity, &filter));
        assert!(matches_filter(&link, &filter));
    }

    #[test]
    fn test_filter_by_event_type() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: Some("deleted".to_string()),
            kind: None,
            link_type: None,
        };

        let created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
        });

        assert!(!matches_filter(&created, &filter));
        assert!(matches_filter(&deleted, &filter));
    }

    #[test]
    fn test_filter_by_link_type() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: Some("follow".to_string()),
        };

        let follow = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        let like = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "like".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        // Entity events should NOT match a link_type filter
        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(matches_filter(&follow, &filter));
        assert!(!matches_filter(&like, &filter));
        assert!(!matches_filter(&entity, &filter));
    }

    #[test]
    fn test_filter_combined() {
        let filter = SubscribeRequest {
            entity_type: Some("user".to_string()),
            entity_id: None,
            event_type: Some("created".to_string()),
            kind: Some("entity".to_string()),
            link_type: None,
        };

        let user_created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let user_deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
        });

        let capture_created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "capture".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(matches_filter(&user_created, &filter));
        assert!(!matches_filter(&user_deleted, &filter));
        assert!(!matches_filter(&capture_created, &filter));
    }

    #[test]
    fn test_filter_by_entity_id() {
        let target = Uuid::new_v4();
        let other = Uuid::new_v4();

        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: Some(target.to_string()),
            event_type: None,
            kind: None,
            link_type: None,
        };

        let matching = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "user".to_string(),
            entity_id: target,
            data: json!({}),
        });

        let not_matching = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "user".to_string(),
            entity_id: other,
            data: json!({}),
        });

        assert!(matches_filter(&matching, &filter));
        assert!(!matches_filter(&not_matching, &filter));
    }

    // === Conversion tests ===

    #[test]
    fn test_envelope_to_response_entity_created() {
        let entity_id = Uuid::new_v4();
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id,
            data: json!({"name": "Alice"}),
        }));

        let resp = envelope_to_response(&envelope);

        assert_eq!(resp.event_id, envelope.id.to_string());
        assert_eq!(resp.event_kind, "entity");
        assert_eq!(resp.event_type, "created");
        assert_eq!(resp.entity_type, "user");
        assert_eq!(resp.entity_id, entity_id.to_string());
        assert!(resp.link_type.is_empty());
        assert!(resp.source_id.is_empty());
        assert!(resp.target_id.is_empty());
        assert!(resp.data.is_some());
        assert!(resp.metadata.is_none());
    }

    #[test]
    fn test_envelope_to_response_link_created() {
        let link_id = Uuid::new_v4();
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();

        let envelope = EventEnvelope::new(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id,
            source_id: source,
            target_id: target,
            metadata: Some(json!({"via": "mobile"})),
        }));

        let resp = envelope_to_response(&envelope);

        assert_eq!(resp.event_kind, "link");
        assert_eq!(resp.event_type, "created");
        assert!(resp.entity_type.is_empty());
        assert_eq!(resp.entity_id, link_id.to_string());
        assert_eq!(resp.link_type, "follow");
        assert_eq!(resp.source_id, source.to_string());
        assert_eq!(resp.target_id, target.to_string());
        assert!(resp.data.is_none());
        assert!(resp.metadata.is_some());
    }

    #[test]
    fn test_envelope_to_response_entity_deleted() {
        let entity_id = Uuid::new_v4();
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "capture".to_string(),
            entity_id,
        }));

        let resp = envelope_to_response(&envelope);

        assert_eq!(resp.event_kind, "entity");
        assert_eq!(resp.event_type, "deleted");
        assert_eq!(resp.entity_type, "capture");
        assert!(resp.data.is_none());
        assert!(resp.metadata.is_none());
    }

    // === Integration test — full subscribe flow ===

    #[tokio::test]
    async fn test_event_service_subscribe_receives_matching_events() {
        use crate::server::host::ServerHost;
        use tokio_stream::StreamExt;

        let event_bus = EventBus::new(64);

        // Create a minimal ServerHost with an EventBus
        let host = ServerHost::minimal_for_test().with_event_bus(event_bus.clone());
        let host = Arc::new(host);

        let svc = EventServiceImpl::new(host);

        // Subscribe to "user" entity events only
        //
        // **Une portée est désormais exigée** (fail-closed, 2026-10-01) : le flux
        // présente l'identité d'un utilisateur de son locataire, comme un vrai
        // client le ferait par son interceptor.
        let mut request = Request::new(SubscribeRequest {
            entity_type: Some("user".to_string()),
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        });
        request.extensions_mut().insert(AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        });

        let response = svc.subscribe(request).await.unwrap();
        let mut stream = response.into_inner();

        // Publish a user event (should match)
        let user_id = Uuid::new_v4();
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: user_id,
            data: json!({"name": "Alice"}),
        }));

        // Publish a capture event (should NOT match)
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "capture".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        // Publish a link event (should NOT match)
        event_bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));

        // Should receive exactly 1 event (the user one)
        let msg = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out waiting for event")
            .expect("stream ended unexpectedly")
            .expect("received error");

        assert_eq!(msg.event_kind, "entity");
        assert_eq!(msg.event_type, "created");
        assert_eq!(msg.entity_type, "user");
        assert_eq!(msg.entity_id, user_id.to_string());

        // No more matching events should arrive
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_millis(50), stream.next()).await;
        assert!(
            timeout_result.is_err(),
            "should time out — no more matching events"
        );
    }

    #[tokio::test]
    async fn test_event_service_wildcard_receives_all() {
        use crate::server::host::ServerHost;
        use tokio_stream::StreamExt;

        let event_bus = EventBus::new(64);
        let host = Arc::new(ServerHost::minimal_for_test().with_event_bus(event_bus.clone()));

        let svc = EventServiceImpl::new(host);

        // Subscribe with no filters (wildcard) — **en administrateur**, puisque
        // « recevoir tout » est désormais un droit nommé et non un défaut : c'est
        // toute la différence entre « tout le monde voit tout » et
        // « l'administrateur voit tout ».
        let mut request = Request::new(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        });
        request.extensions_mut().insert(AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        });

        let response = svc.subscribe(request).await.unwrap();
        let mut stream = response.into_inner();

        // Publish 2 events of different types
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        event_bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));

        // Should receive both
        let msg1 = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("error");
        assert_eq!(msg1.event_kind, "entity");

        let msg2 = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("error");
        assert_eq!(msg2.event_kind, "link");
    }

    #[tokio::test]
    async fn test_event_service_client_disconnect_ends_task() {
        use crate::server::host::ServerHost;

        let event_bus = EventBus::new(64);
        let host = Arc::new(ServerHost::minimal_for_test().with_event_bus(event_bus.clone()));

        let svc = EventServiceImpl::new(host);

        // Le client déclare son locataire par l'en-tête : le geste explicite
        // reste possible (une identité, posée par un interceptor, primerait).
        let mut request = Request::new(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        });
        request
            .metadata_mut()
            .insert("x-tenant-id", Uuid::new_v4().to_string().parse().unwrap());

        let response = svc.subscribe(request).await.unwrap();

        // Drop the stream to simulate client disconnect
        drop(response);

        // The spawned task should detect the closed mpsc and exit.
        // Publish an event to trigger the detection
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        // Give the task time to notice and exit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // If we get here without hanging, the task properly exited
    }
}
