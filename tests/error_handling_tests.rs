//! Tests for the typed error handling system
//!
//! These tests verify that:
//! - Errors return correct HTTP status codes
//! - Error responses are properly formatted
//! - Error conversions work correctly
//! - Error matching allows clients to handle specific cases

use axum::http::StatusCode;
use axum::response::IntoResponse;
use this::prelude::*;
use uuid::Uuid;

// =============================================================================
// HTTP Status Code Tests
// =============================================================================

mod status_code_tests {
    use super::*;

    #[test]
    fn test_entity_not_found_returns_404() {
        let err = ThisError::Entity(EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::new_v4(),
        });
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_entity_already_exists_returns_409() {
        let err = ThisError::Entity(EntityError::AlreadyExists {
            entity_type: "user".to_string(),
            id: Uuid::new_v4(),
        });
        assert_eq!(err.status_code(), StatusCode::CONFLICT);
    }

    #[test]
    fn test_link_not_found_returns_404() {
        let err = ThisError::Link(LinkError::NotFoundById { id: Uuid::new_v4() });
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_link_already_exists_returns_409() {
        let err = ThisError::Link(LinkError::AlreadyExists {
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            link_type: "owner".to_string(),
        });
        assert_eq!(err.status_code(), StatusCode::CONFLICT);
    }

    #[test]
    fn test_validation_error_returns_400() {
        let err = ThisError::Validation(ValidationError::FieldError {
            field: "email".to_string(),
            message: "invalid format".to_string(),
        });
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_request_unauthorized_returns_401() {
        let err = ThisError::Request(RequestError::Unauthorized {
            message: "invalid token".to_string(),
        });
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_request_forbidden_returns_403() {
        let err = ThisError::Request(RequestError::Forbidden {
            message: "insufficient permissions".to_string(),
        });
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_storage_error_returns_500() {
        let err = ThisError::Storage(StorageError::ConnectionError {
            backend: "PostgreSQL".to_string(),
            message: "connection refused".to_string(),
        });
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_config_error_returns_500() {
        let err = ThisError::Config(ConfigError::ParseError {
            file: Some("config.yaml".to_string()),
            message: "invalid syntax".to_string(),
        });
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

// =============================================================================
// Error Code Tests
// =============================================================================

mod error_code_tests {
    use super::*;

    #[test]
    fn test_entity_error_codes() {
        assert_eq!(
            EntityError::NotFound {
                entity_type: "user".to_string(),
                id: Uuid::nil()
            }
            .error_code(),
            "ENTITY_NOT_FOUND"
        );

        assert_eq!(
            EntityError::AlreadyExists {
                entity_type: "user".to_string(),
                id: Uuid::nil()
            }
            .error_code(),
            "ENTITY_ALREADY_EXISTS"
        );

        assert_eq!(
            EntityError::UnknownType {
                entity_type: "unknown".to_string()
            }
            .error_code(),
            "UNKNOWN_ENTITY_TYPE"
        );
    }

    #[test]
    fn test_link_error_codes() {
        assert_eq!(
            LinkError::NotFoundById { id: Uuid::nil() }.error_code(),
            "LINK_NOT_FOUND"
        );

        assert_eq!(
            LinkError::AlreadyExists {
                source_id: Uuid::nil(),
                target_id: Uuid::nil(),
                link_type: "owner".to_string()
            }
            .error_code(),
            "LINK_ALREADY_EXISTS"
        );

        assert_eq!(
            LinkError::RouteNotFound {
                entity_type: "user".to_string(),
                route_name: "unknown".to_string()
            }
            .error_code(),
            "ROUTE_NOT_FOUND"
        );
    }

    #[test]
    fn test_request_error_codes() {
        assert_eq!(
            RequestError::Unauthorized {
                message: "test".to_string()
            }
            .error_code(),
            "UNAUTHORIZED"
        );

        assert_eq!(
            RequestError::Forbidden {
                message: "test".to_string()
            }
            .error_code(),
            "FORBIDDEN"
        );

        assert_eq!(
            RequestError::InvalidBody {
                message: "test".to_string()
            }
            .error_code(),
            "INVALID_BODY"
        );
    }
}

// =============================================================================
// Error Response Format Tests
// =============================================================================

mod error_response_tests {
    use super::*;

    #[test]
    fn test_error_response_has_code_and_message() {
        let err = ThisError::Entity(EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        });

        let response = err.to_response();

        assert_eq!(response.code, "ENTITY_NOT_FOUND");
        assert!(response.message.contains("user"));
        assert!(response.message.contains("not found"));
    }

    #[test]
    fn test_error_response_includes_details_for_entity_not_found() {
        let id = Uuid::new_v4();
        let err = ThisError::Entity(EntityError::NotFound {
            entity_type: "order".to_string(),
            id,
        });

        let response = err.to_response();

        assert!(response.details.is_some());
        let details = response.details.unwrap();
        assert_eq!(details["entity_type"], "order");
        assert_eq!(details["id"], id.to_string());
    }

    #[test]
    fn test_error_response_includes_details_for_link_not_found() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let err = ThisError::Link(LinkError::NotFound {
            source_id,
            target_id,
            link_type: "owner".to_string(),
        });

        let response = err.to_response();

        assert!(response.details.is_some());
        let details = response.details.unwrap();
        assert_eq!(details["source_id"], source_id.to_string());
        assert_eq!(details["target_id"], target_id.to_string());
        assert_eq!(details["link_type"], "owner");
    }

    #[test]
    fn test_validation_errors_include_field_details() {
        let err = ThisError::Validation(ValidationError::FieldErrors(vec![
            FieldValidationError {
                field: "email".to_string(),
                message: "invalid format".to_string(),
            },
            FieldValidationError {
                field: "name".to_string(),
                message: "required".to_string(),
            },
        ]));

        let response = err.to_response();

        assert!(response.details.is_some());
        let details = response.details.unwrap();
        let fields = details["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
    }
}

// =============================================================================
// Error Conversion Tests
// =============================================================================

mod error_conversion_tests {
    use super::*;

    #[test]
    fn test_entity_error_converts_to_this_error() {
        let entity_err = EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        };

        let this_err: ThisError = entity_err.into();

        assert!(matches!(this_err, ThisError::Entity(EntityError::NotFound { .. })));
        assert_eq!(this_err.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_link_error_converts_to_this_error() {
        let link_err = LinkError::NotFoundById { id: Uuid::nil() };

        let this_err: ThisError = link_err.into();

        assert!(matches!(this_err, ThisError::Link(LinkError::NotFoundById { .. })));
        assert_eq!(this_err.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_serde_json_error_converts_to_this_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();

        let this_err: ThisError = json_err.into();

        assert!(matches!(
            this_err,
            ThisError::Validation(ValidationError::InvalidJson { .. })
        ));
    }

    #[test]
    fn test_uuid_error_converts_to_this_error() {
        let uuid_err = uuid::Uuid::parse_str("not-a-uuid").unwrap_err();

        let this_err: ThisError = uuid_err.into();

        assert!(matches!(
            this_err,
            ThisError::Validation(ValidationError::InvalidUuid { .. })
        ));
    }
}

// =============================================================================
// Error Pattern Matching Tests
// =============================================================================

mod error_matching_tests {
    use super::*;

    #[test]
    fn test_can_match_specific_entity_errors() {
        let err = ThisError::Entity(EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::new_v4(),
        });

        let result = match err {
            ThisError::Entity(EntityError::NotFound { entity_type, id }) => {
                format!("{} with id {} not found", entity_type, id)
            }
            _ => "other error".to_string(),
        };

        assert!(result.contains("user"));
        assert!(result.contains("not found"));
    }

    #[test]
    fn test_can_match_specific_link_errors() {
        let link_id = Uuid::new_v4();
        let err = ThisError::Link(LinkError::NotFoundById { id: link_id });

        let matched_id = match err {
            ThisError::Link(LinkError::NotFoundById { id }) => Some(id),
            _ => None,
        };

        assert_eq!(matched_id, Some(link_id));
    }

    #[test]
    fn test_can_match_validation_errors() {
        let err = ThisError::Validation(ValidationError::FieldError {
            field: "email".to_string(),
            message: "invalid".to_string(),
        });

        let field_name = match err {
            ThisError::Validation(ValidationError::FieldError { field, .. }) => Some(field),
            ThisError::Validation(ValidationError::FieldErrors(_)) => None,
            _ => None,
        };

        assert_eq!(field_name, Some("email".to_string()));
    }
}

// =============================================================================
// IntoResponse Tests
// =============================================================================

mod into_response_tests {
    use super::*;

    #[test]
    fn test_this_error_into_response_status() {
        let err = ThisError::Entity(EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::new_v4(),
        });

        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_link_error_into_response_status() {
        let err = ThisError::Link(LinkError::NotFoundById { id: Uuid::nil() });

        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_validation_error_into_response_status() {
        let err = ThisError::Validation(ValidationError::FieldError {
            field: "test".to_string(),
            message: "invalid".to_string(),
        });

        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_unauthorized_into_response_status() {
        let err = ThisError::Request(RequestError::Unauthorized {
            message: "invalid token".to_string(),
        });

        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

// =============================================================================
// LinkService Error Integration Tests
// =============================================================================

mod link_service_error_tests {
    use super::*;
    use this::storage::InMemoryLinkService;

    #[tokio::test]
    async fn test_update_nonexistent_link_returns_typed_error() {
        let service = InMemoryLinkService::new();
        let fake_id = Uuid::new_v4();
        let link = this::core::link::LinkEntity::new(
            "test".to_string(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
        );

        let result = service.update(&fake_id, link).await;

        assert!(result.is_err());
        let err = result.unwrap_err();

        // Verify it's the correct typed error
        match err {
            ThisError::Link(LinkError::NotFoundById { id }) => {
                assert_eq!(id, fake_id);
            }
            other => panic!("Expected LinkError::NotFoundById, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_get_or_error_returns_typed_error() {
        let service = InMemoryLinkService::new();
        let fake_id = Uuid::new_v4();

        let result = service.get_or_error(&fake_id).await;

        assert!(result.is_err());
        let err = result.unwrap_err();

        match err {
            ThisError::Link(LinkError::NotFoundById { id }) => {
                assert_eq!(id, fake_id);
            }
            other => panic!("Expected LinkError::NotFoundById, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_successful_operations_return_ok() {
        let service = InMemoryLinkService::new();
        let link = this::core::link::LinkEntity::new(
            "owner".to_string(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
        );

        // Create should succeed
        let created = service.create(link.clone()).await;
        assert!(created.is_ok());

        // Get should succeed
        let fetched = service.get(&link.id).await;
        assert!(fetched.is_ok());
        assert!(fetched.unwrap().is_some());

        // Update should succeed
        let updated = service.update(&link.id, link.clone()).await;
        assert!(updated.is_ok());

        // Delete should succeed
        let deleted = service.delete(&link.id).await;
        assert!(deleted.is_ok());
    }
}
