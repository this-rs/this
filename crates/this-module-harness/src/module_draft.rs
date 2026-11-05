//! Ask a model for a **complete feature**, not just a program.
//!
//! ## What "complete" means here
//!
//! A module is a [`ModuleDescriptor`]: entities with their fields, links between
//! them, business errors, and their messages. That is what the app already knows
//! how to render — its navigation, lists and forms are derived from the
//! descriptor. A `fn run` program is a capability; a descriptor is an interface.
//!
//! So this drafter produces a descriptor, and the framework **validates** it before
//! returning: the same guarantee as for a program. A module that would not load is
//! refused here, with the validator's reasons, rather than at the user's screen.
//!
//! ## Why the prompt is a schema, not a wish
//!
//! Asking for "a JSON module" gets something approximately shaped, which is then
//! repaired by hand — and hand-repair is how a demo drifts from what the framework
//! accepts. The prompt names every section the validator checks, and a minimal
//! example that passes it. What does not fit is rejected by the real checker.

use crate::Error;
use this_descriptor::{CheckProfile, CustomRegistry, ModuleDescriptor};
use this_llm::Completion;

/// A module a model wrote, already validated.
#[derive(Debug)]
pub struct ModuleDraft {
    pub descriptor: ModuleDescriptor,
}

/// Asks a model for a module, and refuses what would not load.
pub struct ModuleDrafter<C: Completion> {
    completion: C,
}

impl<C: Completion> ModuleDrafter<C> {
    pub fn new(completion: C) -> Self {
        Self { completion }
    }

    /// Draft a module for `request`, or explain why nothing usable came back.
    ///
    /// Validation uses `CheckProfile::Stored`: a drafted module is an *uploaded*
    /// one, so every operation of every entity must carry an explicit policy. That
    /// is stricter than for a compiled module, and it is the honest bar — a module
    /// arriving from a model is exactly the case the strict profile exists for.
    pub async fn draft(&self, request: &str) -> Result<ModuleDraft, Error> {
        let answer = self
            .completion
            .complete(module_system_prompt(), request)
            .await
            .map_err(Error::Model)?;
        let json = extract_json(&answer).ok_or_else(|| Error::NoProgram(answer.clone()))?;

        let descriptor: ModuleDescriptor = serde_json::from_str(&json)
            .map_err(|error| Error::InvalidDescriptor(error.to_string()))?;

        descriptor
            .check(
                &CustomRegistry::default(),
                CheckProfile::Stored {
                    allow_public: false,
                },
            )
            .map_err(|issues| Error::Rejected {
                issues: issues
                    .iter()
                    .map(|issue| format!("{} : {}", issue.path, issue.message))
                    .collect(),
            })?;

        Ok(ModuleDraft { descriptor })
    }
}

/// The description a model receives: the shapes the validator actually checks.
pub fn module_system_prompt() -> &'static str {
    r#"You design a feature for a this-rs application. Answer with ONE fenced ```json block, nothing else.

A module describes entities and the links between them. The application derives its
navigation, lists and forms from it — so the descriptor IS the interface.

TOP LEVEL
  format   : 1
  name     : snake_case, lowercase (e.g. "inventory")
  version  : semantic version, e.g. "0.1.0"
  entities : list (see below)
  links    : list (see below)
  errors   : list (see below)

ENTITY
  name       : snake_case singular (e.g. "item")
  plural     : snake_case plural (e.g. "items")
  kind       : "data"
  fields     : list of
                 name    : snake_case
                 ty      : "string" | "text" | "bool" | "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | "uuid"
                 optional: true | false
  permissions: a map from operation name to a policy STRING, e.g.
                 "permissions": { "list": "authenticated", "get": "authenticated",
                                  "create": "authenticated", "update": "authenticated",
                                  "delete": "deny" }
               operation names are exactly:
                 list, get, create, update, delete,
                 list_links, get_link, create_link, update_link, delete_link
               policy strings are exactly:
                 "authenticated"  "deny"
                 "role:<name>"  "owner_or_role:<name>"
                 "all:<p>"  "any:<p>"  "not:<p>"
               NEVER use "public": a stored module may not grant public access — the
               validator refuses it, and rightly so. Use "authenticated" instead.
               a STORED module needs an EXPLICIT policy for each operation it exposes.
               <policy> is one of:
                 {"kind": "deny"}
                 {"kind": "authenticated"}
                 {"kind": "requires", "capability": "<name>"}

LINK
  link_type     : snake_case (e.g. "contains")
  source        : an entity name of this module
  target        : an entity name of this module
  forward_route : snake_case path segment
  reverse_route : snake_case path segment
  fields        : optional list, same shape as entity fields
  permissions   : same shape as for entities (a map operation -> policy string)

ERROR
  code        : "<module name>.<reason>" — it MUST start with the module name
                (e.g. "inventory.out_of_stock", NOT "module.inventory.…")
  kind        : "validation" | "conflict" | "not_found" | "forbidden"
  message_key : an i18n key (e.g. "inventory.out_of_stock")
  params      : list of field names the message interpolates (may be empty)

WHAT IS CHECKED — a module failing any of these is refused
  every link source and target names an entity declared in this module
  every entity declares all four permissions
  names are snake_case; the module name does not collide with "this"
  no field is named like a reserved word of the framework

EXAMPLE — a module that passes validation
```json
{
  "format": 1,
  "name": "inventory",
  "version": "0.1.0",
  "entities": [
    {
      "name": "item",
      "plural": "items",
      "kind": "data",
      "fields": [
        { "name": "label", "ty": "string", "optional": false },
        { "name": "quantity", "ty": "i64", "optional": false }
      ],
      "permissions": {
        "list": "authenticated",
        "get": "authenticated",
        "create": "authenticated",
        "update": "authenticated",
        "delete": "deny"
      }
    }
  ],
  "links": [],
  "errors": [
    {
      "code": "inventory.out_of_stock",
      "kind": "validation",
      "message_key": "inventory.out_of_stock",
      "params": ["item"]
    }
  ]
}
```"#
}

/// Pull the JSON object out of the answer.
fn extract_json(answer: &str) -> Option<String> {
    if let Some(start) = answer.find("```") {
        let rest = &answer[start + 3..];
        let body = match rest.find('\n') {
            Some(newline) => &rest[newline + 1..],
            None => rest,
        };
        if let Some(end) = body.find("```") {
            let text = body[..end].trim();
            if text.starts_with('{') {
                return Some(text.to_string());
            }
        }
    }

    let trimmed = answer.trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (end > start).then(|| trimmed[start..=end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str);

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl Completion for Fixed {
        async fn complete(&self, _system: &str, _request: &str) -> Result<String, String> {
            Ok(self.0.to_string())
        }
    }

    async fn draft_from(answer: &'static str) -> Result<ModuleDraft, Error> {
        ModuleDrafter::new(Fixed(answer)).draft("un stock").await
    }

    fn valid_module() -> &'static str {
        r#"```json
{
  "format": 1,
  "name": "inventory",
  "version": "0.1.0",
  "entities": [
    {
      "name": "item",
      "plural": "items",
      "kind": "data",
      "fields": [
        { "name": "label", "ty": "string", "optional": false },
        { "name": "quantity", "ty": "i64", "optional": false }
      ],
      "permissions": {
        "list": "authenticated",
        "get": "authenticated",
        "create": "authenticated",
        "update": "authenticated",
        "delete": "deny"
      }
    }
  ],
  "links": [],
  "errors": []
}
```"#
    }

    #[tokio::test]
    async fn a_valid_module_is_accepted_and_keeps_its_entities() {
        let draft = draft_from(valid_module()).await.expect("module valide");
        assert_eq!(draft.descriptor.name, "inventory");
        assert_eq!(draft.descriptor.entities.len(), 1);
        assert_eq!(draft.descriptor.entities[0].name, "item");
        assert_eq!(draft.descriptor.entities[0].fields.len(), 2);
    }

    #[tokio::test]
    async fn a_link_to_an_absent_entity_is_refused() {
        let answer = valid_module().replace(
            r#""links": []"#,
            r#""links": [{"link_type":"holds","source":"item","target":"warehouse",
                          "forward_route":"holds","reverse_route":"held_by",
                          "permissions":{"list":"deny","get":"deny","create":"deny","update":"deny","delete":"deny",
                                                 "list_links":"deny","get_link":"deny",
                                                 "create_link":"deny","delete_link":"deny"}}]"#,
        );
        let leaked: &'static str = Box::leak(answer.into_boxed_str());
        match draft_from(leaked).await {
            Err(Error::Rejected { issues }) => assert!(
                !issues.is_empty(),
                "le validateur doit nommer ce qui cloche"
            ),
            other => panic!("un lien vers une entité absente doit être refusé, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_module_without_explicit_policies_is_refused() {
        // `Stored` exige une politique explicite pour les quatre opérations.
        let answer = valid_module().replace(
            r#"      "permissions": {
        "list": "authenticated",
        "get": "authenticated",
        "create": "authenticated",
        "update": "authenticated",
        "delete": "deny"
      }"#,
            r#"      "permissions": {}"#,
        );
        let leaked: &'static str = Box::leak(answer.into_boxed_str());
        match draft_from(leaked).await {
            Err(Error::Rejected { issues }) => assert!(!issues.is_empty()),
            other => panic!("des politiques implicites doivent être refusées, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_error_code_outside_the_module_namespace_is_refused() {
        // Le validateur exige que le code commence par le nom du module. Mon
        // premier prompt disait `module.<nom>.<raison>` : le vrai vérificateur l'a
        // refusé, et ce test garde la correction.
        let answer = valid_module().replace(
            r#""errors": []"#,
            r#""errors": [{"code": "module.inventory.out_of_stock", "kind": "validation",
                          "message_key": "inventory.out_of_stock", "params": []}]"#,
        );
        let leaked: &'static str = Box::leak(answer.into_boxed_str());
        match draft_from(leaked).await {
            Err(Error::Rejected { issues }) => assert!(
                issues
                    .iter()
                    .any(|i| i.contains("namespace") || i.contains("must start")),
                "le refus doit nommer la règle de namespace, reçu {issues:?}"
            ),
            other => panic!("un code hors namespace doit être refusé, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_public_permission_is_refused_with_a_readable_message() {
        // Un modèle a réellement écrit "public" : le profil `Stored` le refuse, et
        // le message doit nommer le chemin et la règle, pas déverser un Debug.
        let answer = valid_module().replace(r#""list": "authenticated""#, r#""list": "public""#);
        let leaked: &'static str = Box::leak(answer.into_boxed_str());
        match draft_from(leaked).await {
            Err(Error::Rejected { issues }) => {
                let joined = issues.join(" | ");
                assert!(
                    joined.contains("permissions"),
                    "le chemin doit être nommé : {joined}"
                );
                assert!(
                    joined.contains("public"),
                    "la règle doit être nommée : {joined}"
                );
                assert!(
                    !joined.contains("DescriptorIssue"),
                    "pas de Debug brut : {joined}"
                );
            }
            other => panic!("une permission publique doit être refusée, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_json_is_reported_as_such() {
        match draft_from("```json\n{ \"format\": 1, \"name\": }\n```").await {
            Err(Error::InvalidDescriptor(_)) => {}
            other => panic!("du JSON invalide doit être signalé, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn prose_without_a_module_is_refused() {
        match draft_from("Je ne peux pas concevoir cette fonctionnalité.").await {
            Err(Error::NoProgram(_)) => {}
            other => panic!("une réponse sans module doit être refusée, reçu {other:?}"),
        }
    }

    #[test]
    fn the_prompt_names_the_sections_the_validator_checks() {
        let prompt = module_system_prompt();
        for section in ["entities", "links", "errors", "permissions", "version"] {
            assert!(
                prompt.contains(section),
                "le prompt doit décrire : {section}"
            );
        }
    }
}
