//! gRPC: `ContractError` as `tonic::Status`.
//!
//! - code from the kind table (`ErrorKind::grpc_code`), never a blanket `INTERNAL`;
//! - `details` = the whole JSON envelope (`code`, `kind`, `message_key`, `params`,
//!   `fields`), decodable by any client with `serde_json::from_slice::<ContractError>`;
//! - metadata `x-this-error: <code>` (same header as REST: a client tells a contract error
//!   from a transport failure without decoding `details`);
//! - message localized in the locale of the call (`x-this-locale`, `cookie`,
//!   `accept-language` metadata) by [`status_for`].
//!
//! Migration of a service method: `.map_err(|e| Status::internal(e.to_string()))` becomes
//! `.map_status()` (or `.map_status_for(request.metadata(), &catalog)` to localize).

use super::{ERROR_HEADER, from_anyhow};
use crate::core::i18n::locale_from_metadata;
use crate::descriptor::{ContractError, I18nCatalog, Locale};
use tonic::metadata::{MetadataMap, MetadataValue};

/// gRPC status (code from the kind table, JSON envelope in `details`, `x-this-error`).
pub fn to_status(err: ContractError) -> tonic::Status {
    let code = MetadataValue::try_from(err.code.as_str()).ok();
    let mut status: tonic::Status = err.into();
    if let Some(v) = code {
        status.metadata_mut().insert(ERROR_HEADER, v);
    }
    status
}

/// Same, with the message (and the envelope's messages) rendered in `locale`.
pub fn to_status_localized(
    err: ContractError,
    catalog: &I18nCatalog,
    locale: &Locale,
) -> tonic::Status {
    to_status(catalog.localize(err, locale))
}

/// Status for an error raised while serving a call with `metadata` (locale negotiated
/// from it).
pub fn status_for(
    err: ContractError,
    metadata: &MetadataMap,
    catalog: &I18nCatalog,
) -> tonic::Status {
    let locale = locale_from_metadata(metadata, catalog);
    to_status_localized(err, catalog, &locale)
}

/// Status of any `anyhow::Error` (contract kept, anything else `this.internal`).
pub fn status_from_anyhow(err: &anyhow::Error) -> tonic::Status {
    to_status(from_anyhow(err))
}

/// Read the envelope back from a `Status` (client side, tests).
pub fn contract_error_of(status: &tonic::Status) -> Option<ContractError> {
    serde_json::from_slice(status.details()).ok()
}

/// `Result` → `Result<_, Status>` with the contract table.
pub trait StatusResultExt<T> {
    /// Framework default message (French).
    fn map_status(self) -> Result<T, tonic::Status>;
    /// Message in the locale of the call.
    fn map_status_for(
        self,
        metadata: &MetadataMap,
        catalog: &I18nCatalog,
    ) -> Result<T, tonic::Status>;
}

impl<T> StatusResultExt<T> for Result<T, anyhow::Error> {
    fn map_status(self) -> Result<T, tonic::Status> {
        self.map_err(|e| status_from_anyhow(&e))
    }
    fn map_status_for(
        self,
        metadata: &MetadataMap,
        catalog: &I18nCatalog,
    ) -> Result<T, tonic::Status> {
        self.map_err(|e| status_for(from_anyhow(&e), metadata, catalog))
    }
}

impl<T> StatusResultExt<T> for Result<T, ContractError> {
    fn map_status(self) -> Result<T, tonic::Status> {
        self.map_err(to_status)
    }
    fn map_status_for(
        self,
        metadata: &MetadataMap,
        catalog: &I18nCatalog,
    ) -> Result<T, tonic::Status> {
        self.map_err(|e| status_for(e, metadata, catalog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::errors::{codes, forbidden, not_found, unauthenticated, validation};
    use crate::descriptor::FieldViolation;
    use tonic::Code;

    #[test]
    fn contract_error_to_grpc_code() {
        let s = to_status(ContractError::framework("this.validation.failed"));
        assert_eq!(s.code(), Code::InvalidArgument);
        let back: ContractError = serde_json::from_slice(s.details()).unwrap();
        assert_eq!(back.code.as_str(), "this.validation.failed");
    }

    #[test]
    fn contract_error_to_grpc_status_code_and_details() {
        let invalid = validation([FieldViolation::new("amount", codes::VALIDATION_POSITIVE)
            .with_param("field", "amount")
            .with_param("value", -1)]);
        let cases = [
            (invalid.clone(), Code::InvalidArgument),
            (not_found("invoice", 9), Code::NotFound),
            (forbidden("create"), Code::PermissionDenied),
            (unauthenticated(), Code::Unauthenticated),
            (
                ContractError::framework(codes::ENTITY_CONFLICT),
                Code::AlreadyExists,
            ),
            (
                ContractError::framework(codes::VFS_PRECONDITION_FAILED),
                Code::Aborted,
            ),
            (
                ContractError::framework(codes::NOT_IMPLEMENTED),
                Code::Unimplemented,
            ),
        ];
        for (err, code) in cases {
            let s = to_status(err.clone());
            assert_eq!(s.code(), code, "{}", err.code);
            assert_eq!(s.metadata().get(ERROR_HEADER).unwrap(), err.code.as_str());
            assert_eq!(
                contract_error_of(&s).as_ref(),
                Some(&err),
                "details = whole envelope"
            );
        }
        // Localized message and envelope, from the call's metadata.
        let cat = I18nCatalog::framework();
        let mut meta = MetadataMap::new();
        meta.insert("accept-language", "en".parse().unwrap());
        let s = status_for(invalid, &meta, &cat);
        assert_eq!(s.message(), "Validation failed");
        let back = contract_error_of(&s).unwrap();
        assert_eq!(
            back.fields[0].message,
            "Field 'amount' must be positive (value: -1)"
        );
        // No generic INTERNAL for a business error carried by anyhow; no leak otherwise.
        let r: Result<(), anyhow::Error> =
            Err(anyhow::Error::new(not_found("invoice", 1)).context("grpc get"));
        assert_eq!(r.map_status().unwrap_err().code(), Code::NotFound);
        let r: Result<(), anyhow::Error> = Err(anyhow::anyhow!("pg: password=secret"));
        let s = r.map_status().unwrap_err();
        assert_eq!(s.code(), Code::Internal);
        assert!(!s.message().contains("secret"));
        let r: Result<(), ContractError> = Err(forbidden("delete"));
        assert_eq!(
            r.map_status_for(&meta, &cat).unwrap_err().message(),
            "Operation denied: delete"
        );
    }
}
