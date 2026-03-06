//! EventMatcher — compiles a TriggerConfig into an executable matcher
//!
//! The EventMatcher is created from a `TriggerConfig` and provides a
//! `matches(&FrameworkEvent) -> bool` method that determines whether an
//! incoming event should trigger a flow.
//!
//! # Supported event kinds
//!
//! - `link.created` — matches `FrameworkEvent::Link(LinkEvent::Created { .. })`
//! - `link.deleted` — matches `FrameworkEvent::Link(LinkEvent::Deleted { .. })`
//! - `entity.created` — matches `FrameworkEvent::Entity(EntityEvent::Created { .. })`
//! - `entity.updated` — matches `FrameworkEvent::Entity(EntityEvent::Updated { .. })`
//! - `entity.deleted` — matches `FrameworkEvent::Entity(EntityEvent::Deleted { .. })`
//!
//! # Filters
//!
//! - `link_type` — Only match link events with this link type (e.g., "follows", "likes")
//! - `entity_type` — Only match entity events with this entity type (e.g., "user", "post")
//!
//! When a filter is `None`, it acts as a wildcard (matches any value).

use crate::config::events::TriggerConfig;
use crate::core::events::{EntityEvent, FrameworkEvent, LinkEvent};

/// Compiled event kind for fast matching (avoids string comparisons at runtime)
#[derive(Debug, Clone, PartialEq)]
enum EventKind {
    LinkCreated,
    LinkDeleted,
    EntityCreated,
    EntityUpdated,
    EntityDeleted,
}

/// Compiled event matcher
///
/// Created from a `TriggerConfig`, provides zero-allocation matching
/// via enum dispatch instead of string comparisons.
#[derive(Debug, Clone)]
pub struct EventMatcher {
    /// The compiled event kind to match
    kind: EventKind,

    /// Optional link type filter (only for link events)
    link_type: Option<String>,

    /// Optional entity type filter (only for entity events)
    entity_type: Option<String>,
}

/// Error returned when a TriggerConfig has an invalid `kind` string
#[derive(Debug, thiserror::Error)]
#[error(
    "unknown event kind: '{kind}'. Expected one of: link.created, link.deleted, entity.created, entity.updated, entity.deleted"
)]
pub struct UnknownEventKind {
    pub kind: String,
}

impl EventMatcher {
    /// Compile a TriggerConfig into an EventMatcher
    ///
    /// Returns an error if the `kind` string is not recognized.
    ///
    /// # Examples
    ///
    /// ```
    /// use this::config::events::TriggerConfig;
    /// use this::events::matcher::EventMatcher;
    ///
    /// let config = TriggerConfig {
    ///     kind: "link.created".to_string(),
    ///     link_type: Some("follows".to_string()),
    ///     entity_type: None,
    /// };
    /// let matcher = EventMatcher::compile(&config).unwrap();
    /// ```
    pub fn compile(config: &TriggerConfig) -> Result<Self, UnknownEventKind> {
        let kind = match config.kind.as_str() {
            "link.created" => EventKind::LinkCreated,
            "link.deleted" => EventKind::LinkDeleted,
            "entity.created" => EventKind::EntityCreated,
            "entity.updated" => EventKind::EntityUpdated,
            "entity.deleted" => EventKind::EntityDeleted,
            _ => {
                return Err(UnknownEventKind {
                    kind: config.kind.clone(),
                });
            }
        };

        Ok(Self {
            kind,
            link_type: config.link_type.clone(),
            entity_type: config.entity_type.clone(),
        })
    }

    /// Check whether a framework event matches this matcher
    ///
    /// Returns `true` if:
    /// 1. The event kind matches (e.g., link.created)
    /// 2. Any type filters match (link_type or entity_type)
    ///
    /// When a filter is `None`, it's treated as a wildcard (always matches).
    pub fn matches(&self, event: &FrameworkEvent) -> bool {
        match event {
            FrameworkEvent::Link(link_event) => self.matches_link(link_event),
            FrameworkEvent::Entity(entity_event) => self.matches_entity(entity_event),
        }
    }

    /// Match against a link event
    fn matches_link(&self, event: &LinkEvent) -> bool {
        let (kind_matches, event_link_type) = match event {
            LinkEvent::Created { link_type, .. } => {
                (self.kind == EventKind::LinkCreated, link_type)
            }
            LinkEvent::Deleted { link_type, .. } => {
                (self.kind == EventKind::LinkDeleted, link_type)
            }
        };

        if !kind_matches {
            return false;
        }

        // Apply link_type filter (None = wildcard)
        match &self.link_type {
            Some(expected) => expected == event_link_type,
            None => true,
        }
    }

    /// Match against an entity event
    fn matches_entity(&self, event: &EntityEvent) -> bool {
        let (kind_matches, event_entity_type) = match event {
            EntityEvent::Created { entity_type, .. } => {
                (self.kind == EventKind::EntityCreated, entity_type)
            }
            EntityEvent::Updated { entity_type, .. } => {
                (self.kind == EventKind::EntityUpdated, entity_type)
            }
            EntityEvent::Deleted { entity_type, .. } => {
                (self.kind == EventKind::EntityDeleted, entity_type)
            }
        };

        if !kind_matches {
            return false;
        }

        // Apply entity_type filter (None = wildcard)
        match &self.entity_type {
            Some(expected) => expected == event_entity_type,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    // ── Helper constructors ──────────────────────────────────────────

    fn link_created(link_type: &str) -> FrameworkEvent {
        FrameworkEvent::Link(LinkEvent::Created {
            link_type: link_type.to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        })
    }

    fn link_deleted(link_type: &str) -> FrameworkEvent {
        FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: link_type.to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
        })
    }

    fn entity_created(entity_type: &str) -> FrameworkEvent {
        FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "test"}),
        })
    }

    fn entity_updated(entity_type: &str) -> FrameworkEvent {
        FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "updated"}),
        })
    }

    fn entity_deleted(entity_type: &str) -> FrameworkEvent {
        FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
        })
    }

    fn trigger(kind: &str, link_type: Option<&str>, entity_type: Option<&str>) -> TriggerConfig {
        TriggerConfig {
            kind: kind.to_string(),
            link_type: link_type.map(String::from),
            entity_type: entity_type.map(String::from),
        }
    }

    // ── link.created tests ───────────────────────────────────────────

    #[test]
    fn test_link_created_wildcard() {
        let m = EventMatcher::compile(&trigger("link.created", None, None)).unwrap();
        assert!(m.matches(&link_created("follows")));
        assert!(m.matches(&link_created("likes")));
        assert!(m.matches(&link_created("blocks")));
        // Should NOT match other kinds
        assert!(!m.matches(&link_deleted("follows")));
        assert!(!m.matches(&entity_created("user")));
    }

    #[test]
    fn test_link_created_with_type_filter() {
        let m = EventMatcher::compile(&trigger("link.created", Some("follows"), None)).unwrap();
        assert!(m.matches(&link_created("follows")));
        assert!(!m.matches(&link_created("likes")));
        assert!(!m.matches(&link_created("blocks")));
    }

    // ── link.deleted tests ───────────────────────────────────────────

    #[test]
    fn test_link_deleted_wildcard() {
        let m = EventMatcher::compile(&trigger("link.deleted", None, None)).unwrap();
        assert!(m.matches(&link_deleted("follows")));
        assert!(m.matches(&link_deleted("likes")));
        assert!(!m.matches(&link_created("follows")));
        assert!(!m.matches(&entity_deleted("user")));
    }

    #[test]
    fn test_link_deleted_with_type_filter() {
        let m = EventMatcher::compile(&trigger("link.deleted", Some("likes"), None)).unwrap();
        assert!(m.matches(&link_deleted("likes")));
        assert!(!m.matches(&link_deleted("follows")));
    }

    // ── entity.created tests ─────────────────────────────────────────

    #[test]
    fn test_entity_created_wildcard() {
        let m = EventMatcher::compile(&trigger("entity.created", None, None)).unwrap();
        assert!(m.matches(&entity_created("user")));
        assert!(m.matches(&entity_created("post")));
        assert!(!m.matches(&entity_updated("user")));
        assert!(!m.matches(&link_created("follows")));
    }

    #[test]
    fn test_entity_created_with_type_filter() {
        let m = EventMatcher::compile(&trigger("entity.created", None, Some("capture"))).unwrap();
        assert!(m.matches(&entity_created("capture")));
        assert!(!m.matches(&entity_created("user")));
        assert!(!m.matches(&entity_created("post")));
    }

    // ── entity.updated tests ─────────────────────────────────────────

    #[test]
    fn test_entity_updated_wildcard() {
        let m = EventMatcher::compile(&trigger("entity.updated", None, None)).unwrap();
        assert!(m.matches(&entity_updated("user")));
        assert!(m.matches(&entity_updated("post")));
        assert!(!m.matches(&entity_created("user")));
        assert!(!m.matches(&entity_deleted("user")));
    }

    #[test]
    fn test_entity_updated_with_type_filter() {
        let m = EventMatcher::compile(&trigger("entity.updated", None, Some("user"))).unwrap();
        assert!(m.matches(&entity_updated("user")));
        assert!(!m.matches(&entity_updated("post")));
    }

    // ── entity.deleted tests ─────────────────────────────────────────

    #[test]
    fn test_entity_deleted_wildcard() {
        let m = EventMatcher::compile(&trigger("entity.deleted", None, None)).unwrap();
        assert!(m.matches(&entity_deleted("user")));
        assert!(m.matches(&entity_deleted("post")));
        assert!(!m.matches(&entity_created("user")));
        assert!(!m.matches(&entity_updated("user")));
    }

    #[test]
    fn test_entity_deleted_with_type_filter() {
        let m = EventMatcher::compile(&trigger("entity.deleted", None, Some("post"))).unwrap();
        assert!(m.matches(&entity_deleted("post")));
        assert!(!m.matches(&entity_deleted("user")));
    }

    // ── Error cases ──────────────────────────────────────────────────

    #[test]
    fn test_unknown_kind_returns_error() {
        let result = EventMatcher::compile(&trigger("link.updated", None, None));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("link.updated"));
    }

    #[test]
    fn test_invalid_kind_returns_error() {
        let result = EventMatcher::compile(&trigger("banana", None, None));
        assert!(result.is_err());
    }

    // ── Cross-kind non-matching ──────────────────────────────────────

    #[test]
    fn test_link_matcher_never_matches_entity_events() {
        let m = EventMatcher::compile(&trigger("link.created", None, None)).unwrap();
        assert!(!m.matches(&entity_created("user")));
        assert!(!m.matches(&entity_updated("user")));
        assert!(!m.matches(&entity_deleted("user")));
    }

    #[test]
    fn test_entity_matcher_never_matches_link_events() {
        let m = EventMatcher::compile(&trigger("entity.created", None, None)).unwrap();
        assert!(!m.matches(&link_created("follows")));
        assert!(!m.matches(&link_deleted("follows")));
    }

    // ── Filter combinations ──────────────────────────────────────────

    #[test]
    fn test_link_type_filter_ignored_for_entity_matcher() {
        // Even if link_type is set, it doesn't affect entity matching
        let m = EventMatcher::compile(&trigger("entity.created", Some("follows"), Some("user")))
            .unwrap();
        // entity_type filter applies, link_type is irrelevant
        assert!(m.matches(&entity_created("user")));
        assert!(!m.matches(&entity_created("post")));
    }

    #[test]
    fn test_entity_type_filter_ignored_for_link_matcher() {
        // Even if entity_type is set, it doesn't affect link matching
        let m =
            EventMatcher::compile(&trigger("link.created", Some("follows"), Some("user"))).unwrap();
        // link_type filter applies, entity_type is irrelevant
        assert!(m.matches(&link_created("follows")));
        assert!(!m.matches(&link_created("likes")));
    }
}
