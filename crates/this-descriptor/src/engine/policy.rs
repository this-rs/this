//! Pure, three-valued policy evaluation.
//!
//! Same rules as the REST middleware (`Authorization::evaluate`):
//! - `Some(true)` allow, `Some(false)` deny, `None` UNEVALUABLE;
//! - `And` short-circuits on `Some(false)`, `Or` on `Some(true)`; otherwise any `None`
//!   makes the result `None`;
//! - `Not(None) = None`: a negation never turns "unevaluable" into approval
//!   (`not:perm:blacklisted` without an IAM plan must not admit everyone);
//! - at the edge, only `Some(true)` allows ([`is_allowed`]).
//!
//! One difference with the legacy `AuthPolicy::check_with_resolver`, deliberately
//! stricter: an UNKNOWN resolver is unevaluable (`None`), not `false`, so
//! `not:resolver:unknown` denies (legacy: allowed).

use crate::perm::{PolicyExpr, PrincipalView};

/// What the evaluator needs from its host.
pub trait PolicyEnv {
    /// Named resolver: `Some(verdict)`, or `None` if unknown / not evaluable here.
    fn resolver(&self, name: &str) -> Option<bool>;
    /// Materialized IAM permission: `Some(verdict)`, or `None` without an IAM plan / principal.
    fn permission(&self, permission: &str) -> Option<bool>;
}

/// Environment with no resolver and no IAM: both are unevaluable.
pub struct NoEnv;
impl PolicyEnv for NoEnv {
    fn resolver(&self, _: &str) -> Option<bool> {
        None
    }
    fn permission(&self, _: &str) -> Option<bool> {
        None
    }
}

pub fn evaluate(expr: &PolicyExpr, who: &PrincipalView<'_>, env: &dyn PolicyEnv) -> Option<bool> {
    match expr {
        PolicyExpr::Public => Some(true),
        PolicyExpr::Deny => Some(false),
        PolicyExpr::Authenticated => Some(who.authenticated),
        PolicyExpr::Owner => who.owner,
        PolicyExpr::ServiceOnly => Some(who.service),
        PolicyExpr::AdminOnly => Some(who.admin),
        PolicyExpr::HasRole(roles) => Some(roles.iter().any(|r| who.roles.contains(r))),
        PolicyExpr::Resolver(name) => env.resolver(name),
        PolicyExpr::Perm(p) => env.permission(p),
        PolicyExpr::And(v) => {
            let mut unevaluable = false;
            for e in v {
                match evaluate(e, who, env) {
                    Some(false) => return Some(false),
                    None => unevaluable = true,
                    Some(true) => {}
                }
            }
            if unevaluable { None } else { Some(true) }
        }
        PolicyExpr::Or(v) => {
            let mut unevaluable = false;
            for e in v {
                match evaluate(e, who, env) {
                    Some(true) => return Some(true),
                    None => unevaluable = true,
                    Some(false) => {}
                }
            }
            if unevaluable { None } else { Some(false) }
        }
        PolicyExpr::Not(e) => evaluate(e, who, env).map(|b| !b),
    }
}

/// Edge decision: only an evaluable `true` allows.
pub fn is_allowed(expr: &PolicyExpr, who: &PrincipalView<'_>, env: &dyn PolicyEnv) -> bool {
    evaluate(expr, who, env) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env;
    impl PolicyEnv for Env {
        fn resolver(&self, name: &str) -> Option<bool> {
            match name {
                "yes" => Some(true),
                "no" => Some(false),
                _ => None,
            }
        }
        fn permission(&self, _: &str) -> Option<bool> {
            None
        }
    }

    fn p(s: &str) -> PolicyExpr {
        PolicyExpr::parse(s).unwrap()
    }

    #[test]
    fn not_of_unevaluable_stays_unevaluable() {
        let anon = PrincipalView::default();
        assert_eq!(evaluate(&p("not:perm:blacklisted"), &anon, &Env), None);
        assert!(!is_allowed(&p("not:perm:blacklisted"), &anon, &Env));
        assert_eq!(evaluate(&p("not:resolver:unknown"), &anon, &Env), None);
        assert_eq!(evaluate(&p("not:resolver:no"), &anon, &Env), Some(true));
    }

    /// An unknown resolver never allows: alone, composed, or negated. Unlike the legacy
    /// `check_with_resolver` (where `not:resolver:unknown` was `true`), the negation of an
    /// unevaluable resolver stays unevaluable, hence denied at the edge.
    #[test]
    fn unknown_resolver_denies() {
        let roles = vec!["admin".to_string()];
        let admin_user = PrincipalView {
            authenticated: true,
            roles: &roles,
            admin: true,
            service: true,
            owner: Some(true),
            ..Default::default()
        };
        for s in [
            "resolver:unknown",
            "not:resolver:unknown",
            "not(resolver:unknown)",
            "all:authenticated,resolver:unknown",
            "any:resolver:unknown,resolver:no",
            "owner_or_resolver:unknown_but_owner_is_false",
        ] {
            let expr = p(s);
            let who = if s.starts_with("owner_or") {
                PrincipalView {
                    owner: Some(false),
                    ..admin_user
                }
            } else {
                admin_user
            };
            assert!(
                !is_allowed(&expr, &who, &Env),
                "`{s}` must deny with an unknown resolver"
            );
            assert!(
                !is_allowed(&expr, &who, &NoEnv),
                "`{s}` must deny without env"
            );
        }
        // A known resolver still decides.
        assert!(is_allowed(&p("resolver:yes"), &admin_user, &Env));
        // Short-circuit: an evaluable `true` branch of an `any` still allows.
        assert!(is_allowed(
            &p("any:resolver:unknown,admin_only"),
            &admin_user,
            &Env
        ));
    }

    /// Three-valued table, same verdicts as `Authorization::evaluate` on the shared vocabulary.
    #[test]
    fn three_valued_equivalence_table() {
        let roles = vec!["admin".to_string()];
        let user = PrincipalView {
            authenticated: true,
            roles: &roles,
            owner: Some(false),
            ..Default::default()
        };
        let anon = PrincipalView {
            owner: Some(false),
            ..Default::default()
        };
        let cases: &[(&str, &PrincipalView, Option<bool>)] = &[
            ("public", &anon, Some(true)),
            ("authenticated", &anon, Some(false)),
            ("authenticated", &user, Some(true)),
            ("role:admin", &user, Some(true)),
            ("role:admin", &anon, Some(false)),
            ("all:authenticated,resolver:yes", &user, Some(true)),
            ("all:authenticated,resolver:unknown", &user, None),
            ("all:authenticated,resolver:unknown", &anon, Some(false)), // short-circuit
            ("any:resolver:unknown,role:admin", &user, Some(true)),     // short-circuit
            ("any:resolver:unknown,owner", &user, None),
            ("owner_or_role:admin", &user, Some(true)),
            ("deny", &user, Some(false)),
        ];
        for (s, who, expected) in cases {
            assert_eq!(evaluate(&p(s), who, &Env), *expected, "{s}");
        }
        // Owner not known (List before fetch) is unevaluable, never true.
        assert_eq!(
            evaluate(&PolicyExpr::Owner, &PrincipalView::default(), &Env),
            None
        );
    }
}
