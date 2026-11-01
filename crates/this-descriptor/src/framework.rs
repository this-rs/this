//! The framework's own declarations: every `this.*` error code, its kind, and its French
//! and English messages.
//!
//! It lives in this crate (wasm32) and not in this-rs, so the Leptos front can render
//! `this.validation.*` with the same texts as the server. French is the default locale: it
//! keeps the historical messages of the legacy `errors` array.

use crate::error::{ErrorCode, ErrorDesc, ErrorKind};
use crate::i18n::{I18nDecl, Locale, MessageBundle, MessageDecl, MessageKey, MessageSyntax};
use crate::module::{DESCRIPTOR_FORMAT, ModuleDescriptor};
use std::collections::BTreeMap;

/// Name of the framework namespace (`this.`). No module may use it.
pub const FRAMEWORK_MODULE: &str = "this";

/// Constants for the codes emitted by framework code (avoid string typos at call sites).
pub mod codes {
    pub const VALIDATION_FAILED: &str = "this.validation.failed";
    pub const VALIDATION_REQUIRED: &str = "this.validation.required";
    pub const VALIDATION_TYPE_MISMATCH: &str = "this.validation.type_mismatch";
    pub const VALIDATION_POSITIVE: &str = "this.validation.positive";
    pub const VALIDATION_STRING_LENGTH: &str = "this.validation.string_length";
    pub const VALIDATION_MIN_VALUE: &str = "this.validation.min_value";
    pub const VALIDATION_MAX_VALUE: &str = "this.validation.max_value";
    pub const VALIDATION_IN_LIST: &str = "this.validation.in_list";
    pub const VALIDATION_DATE_FORMAT: &str = "this.validation.date_format";
    pub const VALIDATION_PATTERN: &str = "this.validation.pattern";
    pub const VALIDATION_EMAIL: &str = "this.validation.email";
    pub const VALIDATION_URL: &str = "this.validation.url";
    pub const VALIDATION_READ_ONLY: &str = "this.validation.read_only";
    pub const VALIDATION_IMMUTABLE: &str = "this.validation.immutable";
    pub const VALIDATION_UNKNOWN_FIELD: &str = "this.validation.unknown_field";
    pub const VALIDATION_NOT_OBJECT: &str = "this.validation.not_object";
    pub const VALIDATION_CUSTOM: &str = "this.validation.custom";
    pub const QUERY_MALFORMED_FILTER: &str = "this.query.malformed_filter";
    pub const QUERY_FIELD_NOT_FILTERABLE: &str = "this.query.field_not_filterable";
    pub const QUERY_OP_NOT_ALLOWED: &str = "this.query.op_not_allowed";
    pub const QUERY_FIELD_NOT_SORTABLE: &str = "this.query.field_not_sortable";
    pub const QUERY_MALFORMED_SORT: &str = "this.query.malformed_sort";
    pub const QUERY_PAGE_OUT_OF_RANGE: &str = "this.query.page_out_of_range";
    pub const QUERY_TOO_LARGE: &str = "this.query.too_large";
    pub const AUTH_UNAUTHENTICATED: &str = "this.auth.unauthenticated";
    pub const AUTH_FORBIDDEN: &str = "this.auth.forbidden";
    pub const AUTH_RESOLVER_UNKNOWN: &str = "this.auth.resolver_unknown";
    pub const AUTH_POLICY_INVALID: &str = "this.auth.policy_invalid";
    pub const ENTITY_NOT_FOUND: &str = "this.entity.not_found";
    pub const ENTITY_CONFLICT: &str = "this.entity.conflict";
    pub const MODULE_DEPENDENCY_MISSING: &str = "this.module.dependency_missing";
    pub const MODULE_DEPENDENCY_VERSION: &str = "this.module.dependency_version";
    pub const MODULE_CYCLE: &str = "this.module.cycle";
    pub const MODULE_DUPLICATE_MODULE: &str = "this.module.duplicate_module";
    pub const MODULE_DUPLICATE_ENTITY: &str = "this.module.duplicate_entity";
    pub const MODULE_DUPLICATE_PLURAL: &str = "this.module.duplicate_plural";
    pub const MODULE_DUPLICATE_ERROR_CODE: &str = "this.module.duplicate_error_code";
    pub const MODULE_PLURAL_MISMATCH: &str = "this.module.plural_mismatch";
    pub const MODULE_IDENTITY_MISMATCH: &str = "this.module.identity_mismatch";
    pub const MODULE_HAS_DEPENDENTS: &str = "this.module.has_dependents";
    pub const MODULE_FORMAT_UNSUPPORTED: &str = "this.module.format_unsupported";
    pub const MODULE_UNAVAILABLE: &str = "this.module.unavailable";
    pub const MODULE_HOT_LOAD_DISABLED: &str = "this.module.hot_load_disabled";
    pub const MODULE_ILLEGAL_TRANSITION: &str = "this.module.illegal_transition";
    /// A stored module carries no signature while the host requires one.
    pub const MODULE_UNSIGNED: &str = "this.module.unsigned";
    /// The manifest is signed by a key the host does not trust.
    pub const MODULE_UNTRUSTED_KEY: &str = "this.module.untrusted_key";
    /// The signature does not match the manifest bytes.
    pub const MODULE_BAD_SIGNATURE: &str = "this.module.bad_signature";
    pub const MODULE_REFERENCE_UNKNOWN: &str = "this.module.reference_unknown";
    pub const MODULE_NOT_FOUND: &str = "this.module.not_found";
    pub const MODULE_CONFIG_CONFLICT: &str = "this.module.config_conflict";
    pub const DESCRIPTOR_INVALID: &str = "this.descriptor.invalid";
    pub const DESCRIPTOR_PARSE: &str = "this.descriptor.parse";
    pub const DESCRIPTOR_TAG_REUSED: &str = "this.descriptor.tag_reused";
    pub const DESCRIPTOR_DECLARED_TWICE: &str = "this.descriptor.declared_twice";
    pub const VFS_NOT_FOUND: &str = "this.vfs.not_found";
    pub const VFS_PRECONDITION_FAILED: &str = "this.vfs.precondition_failed";
    pub const VFS_READ_ONLY: &str = "this.vfs.read_only";
    pub const VFS_INVALID_PATH: &str = "this.vfs.invalid_path";
    pub const TRANSPORT_BAD_REQUEST: &str = "this.transport.bad_request";
    pub const TRANSPORT_MISSING_ARG: &str = "this.transport.missing_arg";
    pub const TRANSPORT_UNAVAILABLE: &str = "this.transport.unavailable";
    pub const TRANSPORT_NOT_FOUND: &str = "this.transport.not_found";
    pub const TRANSPORT_METHOD_NOT_ALLOWED: &str = "this.transport.method_not_allowed";
    pub const NOT_IMPLEMENTED: &str = "this.not_implemented";
    pub const INTERNAL: &str = "this.internal";
}

use crate::error::ErrorKind as K;
use codes::*;

/// (code, kind, params, French, English). THE list of framework codes.
pub const FRAMEWORK_ERRORS: &[(&str, ErrorKind, &[&str], &str, &str)] = &[
    (
        VALIDATION_FAILED,
        K::Validation,
        &[],
        "Validation échouée",
        "Validation failed",
    ),
    (
        VALIDATION_REQUIRED,
        K::Validation,
        &["field"],
        "Le champ '{field}' est requis",
        "Field '{field}' is required",
    ),
    (
        VALIDATION_TYPE_MISMATCH,
        K::Validation,
        &["field", "expected"],
        "'{field}' doit être de type {expected}",
        "'{field}' must be of type {expected}",
    ),
    (
        VALIDATION_POSITIVE,
        K::Validation,
        &["field", "value"],
        "Le champ '{field}' doit être positif (valeur: {value})",
        "Field '{field}' must be positive (value: {value})",
    ),
    (
        VALIDATION_STRING_LENGTH,
        K::Validation,
        &["field", "min", "max", "len"],
        "'{field}' doit avoir entre {min} et {max} caractères (actuellement: {len})",
        "'{field}' must have between {min} and {max} characters (currently: {len})",
    ),
    (
        VALIDATION_MIN_VALUE,
        K::Validation,
        &["field", "min", "value"],
        "'{field}' doit être au moins {min} (valeur: {value})",
        "'{field}' must be at least {min} (value: {value})",
    ),
    (
        VALIDATION_MAX_VALUE,
        K::Validation,
        &["field", "max", "value"],
        "'{field}' ne doit pas dépasser {max} (valeur: {value})",
        "'{field}' must not exceed {max} (value: {value})",
    ),
    (
        VALIDATION_IN_LIST,
        K::Validation,
        &["field", "allowed", "value"],
        "'{field}' doit être l'une des valeurs: {allowed} (valeur actuelle: {value})",
        "'{field}' must be one of: {allowed} (current value: {value})",
    ),
    (
        VALIDATION_DATE_FORMAT,
        K::Validation,
        &["field", "format", "value"],
        "'{field}' doit être au format {format} (valeur actuelle: {value})",
        "'{field}' must use the format {format} (current value: {value})",
    ),
    (
        VALIDATION_PATTERN,
        K::Validation,
        &["field", "pattern"],
        "'{field}' ne respecte pas le motif {pattern}",
        "'{field}' does not match {pattern}",
    ),
    (
        VALIDATION_EMAIL,
        K::Validation,
        &["field"],
        "'{field}' doit être une adresse e-mail",
        "'{field}' must be an e-mail address",
    ),
    (
        VALIDATION_URL,
        K::Validation,
        &["field"],
        "'{field}' doit être une URL http(s)",
        "'{field}' must be an http(s) URL",
    ),
    (
        VALIDATION_READ_ONLY,
        K::Validation,
        &["field"],
        "'{field}' est en lecture seule",
        "'{field}' is read-only",
    ),
    (
        VALIDATION_IMMUTABLE,
        K::Validation,
        &["field"],
        "'{field}' ne peut pas être modifié après la création",
        "'{field}' cannot be changed after creation",
    ),
    (
        VALIDATION_UNKNOWN_FIELD,
        K::Validation,
        &["field"],
        "Champ inconnu: '{field}'",
        "Unknown field: '{field}'",
    ),
    (
        VALIDATION_NOT_OBJECT,
        K::Validation,
        &[],
        "Le corps doit être un objet JSON",
        "The body must be a JSON object",
    ),
    (
        VALIDATION_CUSTOM,
        K::Validation,
        &["field", "rule"],
        "'{field}' refusé par la règle {rule}",
        "'{field}' rejected by rule {rule}",
    ),
    (
        QUERY_MALFORMED_FILTER,
        K::BadRequest,
        &["reason"],
        "Filtre mal formé: {reason}",
        "Malformed filter: {reason}",
    ),
    (
        QUERY_FIELD_NOT_FILTERABLE,
        K::BadRequest,
        &["field"],
        "Le champ '{field}' n'est pas filtrable",
        "Field '{field}' is not filterable",
    ),
    (
        QUERY_OP_NOT_ALLOWED,
        K::BadRequest,
        &["field", "op"],
        "Opérateur {op} interdit sur '{field}'",
        "Operator {op} not allowed on '{field}'",
    ),
    (
        QUERY_FIELD_NOT_SORTABLE,
        K::BadRequest,
        &["field"],
        "Le champ '{field}' n'est pas triable",
        "Field '{field}' is not sortable",
    ),
    (
        QUERY_MALFORMED_SORT,
        K::BadRequest,
        &["reason"],
        "Tri mal formé: {reason}",
        "Malformed sort: {reason}",
    ),
    (
        QUERY_PAGE_OUT_OF_RANGE,
        K::BadRequest,
        &["max"],
        "Pagination hors limites (max {max})",
        "Page out of range (max {max})",
    ),
    (
        QUERY_TOO_LARGE,
        K::BadRequest,
        &["max"],
        "Table trop volumineuse pour un filtrage en mémoire (max {max} lignes): affinez la requête ou utilisez un backend qui exécute la requête nativement",
        "Too many rows for an in-memory query (max {max}): narrow the query or use a backend that runs it natively",
    ),
    (
        AUTH_UNAUTHENTICATED,
        K::Unauthenticated,
        &[],
        "Authentification requise",
        "Authentication required",
    ),
    (
        AUTH_FORBIDDEN,
        K::PermissionDenied,
        &["operation"],
        "Opération refusée: {operation}",
        "Operation denied: {operation}",
    ),
    (
        AUTH_RESOLVER_UNKNOWN,
        K::PermissionDenied,
        &["resolver"],
        "Résolveur inconnu: {resolver}",
        "Unknown resolver: {resolver}",
    ),
    (
        AUTH_POLICY_INVALID,
        K::Internal,
        &["policy"],
        "Politique invalide: {policy}",
        "Invalid policy: {policy}",
    ),
    (
        ENTITY_NOT_FOUND,
        K::NotFound,
        &["entity", "id"],
        "{entity} introuvable: {id}",
        "{entity} not found: {id}",
    ),
    (
        ENTITY_CONFLICT,
        K::Conflict,
        &["entity"],
        "{entity} existe déjà",
        "{entity} already exists",
    ),
    (
        MODULE_DEPENDENCY_MISSING,
        K::PreconditionFailed,
        &["module", "dependency"],
        "Le module {module} requiert {dependency}, absent",
        "Module {module} requires {dependency}, which is missing",
    ),
    (
        MODULE_DEPENDENCY_VERSION,
        K::PreconditionFailed,
        &["module", "dependency", "required", "found"],
        "Le module {module} requiert {dependency} {required}, trouvé {found}",
        "Module {module} requires {dependency} {required}, found {found}",
    ),
    (
        MODULE_CYCLE,
        K::PreconditionFailed,
        &["modules"],
        "Dépendances cycliques: {modules}",
        "Dependency cycle: {modules}",
    ),
    (
        MODULE_DUPLICATE_MODULE,
        K::Conflict,
        &["module"],
        "Module déclaré deux fois: {module}",
        "Module declared twice: {module}",
    ),
    (
        MODULE_DUPLICATE_ENTITY,
        K::Conflict,
        &["entity", "modules"],
        "Entité {entity} fournie par plusieurs modules: {modules}",
        "Entity {entity} provided by several modules: {modules}",
    ),
    (
        MODULE_DUPLICATE_PLURAL,
        K::Conflict,
        &["plural"],
        "Pluriel déjà utilisé: {plural}",
        "Plural already used: {plural}",
    ),
    (
        MODULE_DUPLICATE_ERROR_CODE,
        K::Conflict,
        &["code"],
        "Code d'erreur déclaré deux fois: {code}",
        "Error code declared twice: {code}",
    ),
    (
        MODULE_PLURAL_MISMATCH,
        K::PreconditionFailed,
        &["entity", "sources"],
        "Pluriels divergents pour {entity}: {sources}",
        "Diverging plurals for {entity}: {sources}",
    ),
    (
        MODULE_IDENTITY_MISMATCH,
        K::PreconditionFailed,
        &["module", "what"],
        "Le module {module} diffère de son descripteur: {what}",
        "Module {module} differs from its descriptor: {what}",
    ),
    (
        MODULE_HAS_DEPENDENTS,
        K::PreconditionFailed,
        &["module", "dependents"],
        "{module} est requis par {dependents}",
        "{module} is required by {dependents}",
    ),
    (
        MODULE_FORMAT_UNSUPPORTED,
        K::BadRequest,
        &["format"],
        "Format de descripteur non supporté: {format}",
        "Unsupported descriptor format: {format}",
    ),
    (
        MODULE_UNAVAILABLE,
        K::Unavailable,
        &["module"],
        "Module indisponible: {module}",
        "Module unavailable: {module}",
    ),
    (
        MODULE_HOT_LOAD_DISABLED,
        K::PermissionDenied,
        &["operation"],
        "Chargement à chaud désactivé sur cet hôte (opération: {operation})",
        "Hot loading is disabled on this host (operation: {operation})",
    ),
    (
        MODULE_ILLEGAL_TRANSITION,
        K::Internal,
        &["from", "to"],
        "Transition interdite: {from} -> {to}",
        "Illegal transition: {from} -> {to}",
    ),
    (
        MODULE_REFERENCE_UNKNOWN,
        K::PreconditionFailed,
        &["module", "entity"],
        "{module} référence {entity}, ni local ni requis",
        "{module} references {entity}, neither local nor required",
    ),
    (
        MODULE_NOT_FOUND,
        K::NotFound,
        &["module"],
        "Module introuvable: {module}",
        "Module not found: {module}",
    ),
    (
        MODULE_CONFIG_CONFLICT,
        K::Conflict,
        &["what", "key"],
        "Configuration en conflit ({what}): {key}",
        "Conflicting configuration ({what}): {key}",
    ),
    (
        DESCRIPTOR_INVALID,
        K::BadRequest,
        &["issues"],
        "Descripteur invalide: {issues}",
        "Invalid descriptor: {issues}",
    ),
    (
        DESCRIPTOR_PARSE,
        K::BadRequest,
        &["reason"],
        "Descripteur illisible: {reason}",
        "Unreadable descriptor: {reason}",
    ),
    (
        DESCRIPTOR_DECLARED_TWICE,
        K::BadRequest,
        &["what"],
        "Déclaré deux fois: {what}",
        "Declared twice: {what}",
    ),
    (
        DESCRIPTOR_TAG_REUSED,
        K::Conflict,
        &["entity", "field", "tag"],
        "Tag {tag} réutilisé ou déplacé ({entity}.{field})",
        "Tag {tag} reused or moved ({entity}.{field})",
    ),
    (
        VFS_NOT_FOUND,
        K::NotFound,
        &["path"],
        "Introuvable: {path}",
        "Not found: {path}",
    ),
    (
        VFS_PRECONDITION_FAILED,
        K::Aborted,
        &["path"],
        "Version modifiée entre-temps: {path}",
        "Version changed concurrently: {path}",
    ),
    (
        VFS_READ_ONLY,
        K::PermissionDenied,
        &[],
        "Stockage en lecture seule",
        "Read-only storage",
    ),
    (
        VFS_INVALID_PATH,
        K::BadRequest,
        &["path"],
        "Chemin invalide: {path}",
        "Invalid path: {path}",
    ),
    (
        TRANSPORT_BAD_REQUEST,
        K::BadRequest,
        &["reason"],
        "Requête invalide: {reason}",
        "Bad request: {reason}",
    ),
    (
        TRANSPORT_MISSING_ARG,
        K::Validation,
        &["field"],
        "Argument manquant: {field}",
        "Missing argument: {field}",
    ),
    (
        TRANSPORT_NOT_FOUND,
        K::NotFound,
        &["path"],
        "Route introuvable: {path}",
        "Route not found: {path}",
    ),
    (
        TRANSPORT_METHOD_NOT_ALLOWED,
        K::BadRequest,
        &["method"],
        "Méthode non autorisée: {method}",
        "Method not allowed: {method}",
    ),
    (
        TRANSPORT_UNAVAILABLE,
        K::Unavailable,
        &["reason"],
        "Service injoignable: {reason}",
        "Service unreachable: {reason}",
    ),
    (
        NOT_IMPLEMENTED,
        K::NotImplemented,
        &["what"],
        "Non implémenté: {what}",
        "Not implemented: {what}",
    ),
    (
        INTERNAL,
        K::Internal,
        &[],
        "Erreur interne",
        "Internal error",
    ),
];

/// Kind of a framework code, `None` if the code is not a framework code.
pub fn kind_of(code: &str) -> Option<ErrorKind> {
    FRAMEWORK_ERRORS
        .iter()
        .find(|(c, ..)| *c == code)
        .map(|(_, k, ..)| *k)
}

/// Default (French) rendering of a framework code.
pub fn default_message(code: &str, params: &BTreeMap<String, serde_json::Value>) -> String {
    match FRAMEWORK_ERRORS.iter().find(|(c, ..)| *c == code) {
        Some((_, _, _, fr, _)) => crate::engine::i18n::substitute(fr, params),
        None => code.to_string(),
    }
}

/// `ErrorDesc` of every framework code.
pub fn errors() -> Vec<ErrorDesc> {
    FRAMEWORK_ERRORS
        .iter()
        .map(|(code, kind, params, ..)| ErrorDesc {
            code: ErrorCode::new(*code),
            kind: *kind,
            message_key: MessageKey::new(*code),
            params: params.iter().map(|p| p.to_string()).collect(),
            description: None,
        })
        .collect()
}

/// Bundle of the framework messages in `fr` or `en` (`None` for another locale).
pub fn bundle(locale: &Locale) -> Option<MessageBundle> {
    let french = match locale.language().as_str() {
        "fr" => true,
        "en" => false,
        _ => return None,
    };
    Some(MessageBundle {
        module: FRAMEWORK_MODULE.into(),
        locale: Locale::new(locale.language()),
        syntax: MessageSyntax::Simple,
        messages: FRAMEWORK_ERRORS
            .iter()
            .map(|e| {
                (
                    MessageKey::new(e.0),
                    if french { e.3 } else { e.4 }.to_string(),
                )
            })
            .collect(),
    })
}

/// The framework as a descriptor (namespace `this`): errors and message keys.
pub fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        format: DESCRIPTOR_FORMAT,
        name: FRAMEWORK_MODULE.into(),
        version: "0.1.0".into(),
        requires: vec![],
        entities: vec![],
        links: vec![],
        errors: errors(),
        i18n: I18nDecl {
            default_locale: Locale::new("fr"),
            locales: vec![Locale::new("fr"), Locale::new("en")],
            keys: FRAMEWORK_ERRORS
                .iter()
                .map(|(code, _, params, ..)| MessageDecl {
                    key: MessageKey::new(*code),
                    params: params.iter().map(|p| p.to_string()).collect(),
                })
                .collect(),
        },
        custom: vec![],
        runtime: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_codes_are_unique_namespaced_and_translated() {
        let mut seen = std::collections::BTreeSet::new();
        for (code, _, params, fr, en) in FRAMEWORK_ERRORS {
            assert!(code.starts_with("this."), "{code}");
            assert!(seen.insert(*code), "duplicate {code}");
            assert!(!fr.is_empty() && !en.is_empty());
            for p in *params {
                let ph = format!("{{{p}}}");
                assert!(fr.contains(&ph) && en.contains(&ph), "{code} misses {ph}");
            }
        }
    }

    #[test]
    fn all_emitted_codes_are_declared() {
        // Codes that framework code emits through `codes::*` must all be in the table.
        for c in [
            VALIDATION_FAILED,
            VALIDATION_REQUIRED,
            VALIDATION_TYPE_MISMATCH,
            QUERY_MALFORMED_FILTER,
            MODULE_CYCLE,
            NOT_IMPLEMENTED,
            VFS_PRECONDITION_FAILED,
            MODULE_HOT_LOAD_DISABLED,
            TRANSPORT_MISSING_ARG,
        ] {
            assert!(kind_of(c).is_some(), "{c}");
        }
        assert_eq!(kind_of(VFS_PRECONDITION_FAILED), Some(ErrorKind::Aborted));
        assert_eq!(
            kind_of(MODULE_HOT_LOAD_DISABLED),
            Some(ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn framework_descriptor_passes_check_as_framework() {
        let d = descriptor();
        d.check(
            &crate::custom::CustomRegistry::default(),
            crate::check::CheckProfile::Framework,
        )
        .expect("framework descriptor is valid");
    }
}
