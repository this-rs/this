//! Links between entities.

use crate::entity::EntityDesc;
use crate::field::FieldDesc;
use crate::naming::Pluralizer;
use crate::perm::{Operation, PermDesc, PolicyExpr};
use serde::{Deserialize, Serialize};

/// A link type between two entities. One `LinkDesc` per (source, target) couple: a legacy
/// `validation_rules: { owner: [{ source: user, targets: [car, house] }] }` is derived as
/// the set of `LinkDesc` sharing `link_type` and `source`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkDesc {
    pub link_type: String,
    /// Entity of this module or of a required module.
    pub source: String,
    pub target: String,
    /// Default: `LinkDefinition::default_forward_route_name` = `<targets>-<link_types>`.
    #[serde(default)]
    pub forward_route: String,
    /// Default: `<sources>-<link_types>`.
    #[serde(default)]
    pub reverse_route: String,
    /// Link attributes (replaces `required_fields: Option<Vec<String>>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldDesc>,
    /// Operations `list`, `get_link`, `create_link`, `update_link`, `delete_link`.
    #[serde(default, skip_serializing_if = "PermDesc::is_empty")]
    pub permissions: PermDesc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl LinkDesc {
    pub fn new(link_type: &str, source: &str, target: &str) -> Self {
        let mut l = Self {
            link_type: link_type.into(),
            source: source.into(),
            target: target.into(),
            forward_route: String::new(),
            reverse_route: String::new(),
            fields: vec![],
            permissions: PermDesc::default(),
            description: None,
        };
        l.normalize();
        l
    }

    /// Fill default route names (same formula as `LinkDefinition::default_*_route_name`).
    pub fn normalize(&mut self) {
        if self.forward_route.is_empty() {
            self.forward_route = format!(
                "{}-{}",
                Pluralizer::pluralize(&self.target),
                Pluralizer::pluralize(&self.link_type)
            );
        }
        if self.reverse_route.is_empty() {
            self.reverse_route = format!(
                "{}-{}",
                Pluralizer::pluralize(&self.source),
                Pluralizer::pluralize(&self.link_type)
            );
        }
    }

    /// Effective policy of a link operation, same precedence as the REST middleware
    /// (`declared_link_policy`): the link's own `permissions`, then the entity of the scope
    /// (`list_links`/`create_link`/... of the source or target entity), then the default.
    ///
    /// Difference with the legacy YAML conversion: a PARTIAL link permission table does not
    /// fill the missing operations with `authenticated`; they fall through to the entity.
    pub fn effective_policy(&self, scope: Option<&EntityDesc>, op: &Operation) -> PolicyExpr {
        if let Some(p) = self.permissions.policy(op) {
            return p.clone();
        }
        if let Some(p) = scope.and_then(|e| e.permissions.policy(op)) {
            return p.clone();
        }
        PermDesc::default_policy(op)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_routes_default_like_link_definition() {
        let l = LinkDesc::new("owner", "user", "car");
        assert_eq!(l.forward_route, "cars-owners");
        assert_eq!(l.reverse_route, "users-owners");
    }

    #[test]
    fn link_policy_precedence_link_then_entity_then_default() {
        let mut l = LinkDesc::new("owner", "user", "car");
        let mut user = EntityDesc::new("user");
        assert_eq!(
            l.effective_policy(Some(&user), &Operation::CreateLink),
            PolicyExpr::Authenticated
        );
        user.permissions
            .ops
            .insert(Operation::CreateLink, PolicyExpr::AdminOnly);
        assert_eq!(
            l.effective_policy(Some(&user), &Operation::CreateLink),
            PolicyExpr::AdminOnly
        );
        l.permissions
            .ops
            .insert(Operation::CreateLink, PolicyExpr::Public);
        assert_eq!(
            l.effective_policy(Some(&user), &Operation::CreateLink),
            PolicyExpr::Public
        );
        // Partial link table: GetLink is not declared on the link -> entity -> default.
        assert_eq!(
            l.effective_policy(Some(&user), &Operation::GetLink),
            PolicyExpr::Authenticated
        );
    }
}
