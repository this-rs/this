//! Permissions: operations, the policy AST and its string grammar.
//!
//! The string grammar is a strict superset of the YAML grammar of
//! `AuthPolicy::parse_policy` (this-rs), with three differences, all fail-closed:
//! - an unknown token is an ERROR (`parse_policy` falls back to `authenticated`);
//! - `perm:<permission>` (materialized IAM) is part of the AST;
//! - a nestable function form `all(..)`, `any(..)`, `not(..)` exists, and `Display` always
//!   prints it for compositions, so ONE canonical syntax round-trips.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// An operation guarded by a policy. Serialized as a string (`"create"`, `"get_link"`,
/// `"invoice.pay"`), so it can be a JSON map key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Operation {
    List,
    Get,
    Create,
    Update,
    Delete,
    ListLinks,
    GetLink,
    CreateLink,
    UpdateLink,
    DeleteLink,
    /// Business operation, dotted name: `invoice.pay`.
    Custom(String),
}

impl Operation {
    pub const STANDARD: [Operation; 10] = [
        Operation::List,
        Operation::Get,
        Operation::Create,
        Operation::Update,
        Operation::Delete,
        Operation::ListLinks,
        Operation::GetLink,
        Operation::CreateLink,
        Operation::UpdateLink,
        Operation::DeleteLink,
    ];
    /// Operations of an entity's CRUD surface.
    pub const CRUD: [Operation; 5] = [
        Operation::List,
        Operation::Get,
        Operation::Create,
        Operation::Update,
        Operation::Delete,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            Operation::List => "list",
            Operation::Get => "get",
            Operation::Create => "create",
            Operation::Update => "update",
            Operation::Delete => "delete",
            Operation::ListLinks => "list_links",
            Operation::GetLink => "get_link",
            Operation::CreateLink => "create_link",
            Operation::UpdateLink => "update_link",
            Operation::DeleteLink => "delete_link",
            Operation::Custom(s) => s,
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Invalid operation name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid operation `{0}`: expected a standard operation or a dotted name like `invoice.pay`"
)]
pub struct InvalidOperation(pub String);

impl FromStr for Operation {
    type Err = InvalidOperation;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(op) = Operation::STANDARD.iter().find(|o| o.as_str() == s) {
            return Ok(op.clone());
        }
        let ok = s.contains('.') && s.split('.').all(crate::naming::is_identifier);
        if ok {
            Ok(Operation::Custom(s.to_string()))
        } else {
            Err(InvalidOperation(s.to_string()))
        }
    }
}
impl From<Operation> for String {
    fn from(o: Operation) -> String {
        o.as_str().to_string()
    }
}
impl TryFrom<String> for Operation {
    type Error = InvalidOperation;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Serializable AST of `AuthPolicy`. `AuthPolicy::Custom(fn)` is not serializable: it is
/// expressed by `Resolver(name)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum PolicyExpr {
    Public,
    Authenticated,
    Owner,
    ServiceOnly,
    AdminOnly,
    /// Always refuses. Default of business operations (`Operation::Custom`).
    Deny,
    /// Any of the roles.
    HasRole(Vec<String>),
    /// Materialized IAM permission (`perm:invoices.read`).
    Perm(String),
    /// Named resolver of the host's `AuthResolverRegistry`.
    Resolver(String),
    And(Vec<PolicyExpr>),
    Or(Vec<PolicyExpr>),
    Not(Box<PolicyExpr>),
}

/// Policy string that is not in the vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid policy `{input}`: {reason}")]
pub struct PolicyParseError {
    pub input: String,
    pub reason: String,
}

fn perr(input: &str, reason: impl Into<String>) -> PolicyParseError {
    PolicyParseError {
        input: input.to_string(),
        reason: reason.into(),
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '|'))
}

/// Split on top-level commas (parentheses balanced).
fn split_top(s: &str, input: &str) -> Result<Vec<String>, PolicyParseError> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(perr(input, "unbalanced `)`"));
                }
                cur.push(c)
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if depth != 0 {
        return Err(perr(input, "unbalanced `(`"));
    }
    out.push(cur.trim().to_string());
    Ok(out)
}

/// Maximum nesting of a policy string (`not:not:..`, `all(any(..))`): the parser and every
/// walker recurse once per level, and a descriptor can come from a store or an admin POST.
pub const MAX_POLICY_DEPTH: usize = 32;

impl PolicyExpr {
    /// Parse one policy. NO trim at the top (as the REST middleware: `" public"` is not
    /// `public`); parts of a composition are trimmed (as `parse_policy`).
    pub fn parse(s: &str) -> Result<Self, PolicyParseError> {
        Self::parse_inner(s, s, 0)
    }

    fn parse_inner(s: &str, input: &str, depth: usize) -> Result<Self, PolicyParseError> {
        if depth > MAX_POLICY_DEPTH {
            return Err(perr(
                input,
                format!("nested deeper than {MAX_POLICY_DEPTH} levels"),
            ));
        }
        let flat = |rest: &str, and: bool| -> Result<PolicyExpr, PolicyParseError> {
            // Legacy flat form: `all:a,b` / `any:a,b` (split on every comma).
            let parts = rest
                .split(',')
                .map(|p| PolicyExpr::parse_inner(p.trim(), input, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match parts.len() {
                1 => parts.into_iter().next().unwrap(),
                _ if and => PolicyExpr::And(parts),
                _ => PolicyExpr::Or(parts),
            })
        };
        let func = |name: &str| -> Option<&str> {
            s.strip_prefix(name)
                .and_then(|r| r.strip_prefix('('))
                .and_then(|r| r.strip_suffix(')'))
        };
        if let Some(inner) = func("all") {
            let parts = split_top(inner, input)?
                .iter()
                .map(|p| PolicyExpr::parse_inner(p, input, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(PolicyExpr::And(parts));
        }
        if let Some(inner) = func("any") {
            let parts = split_top(inner, input)?
                .iter()
                .map(|p| PolicyExpr::parse_inner(p, input, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(PolicyExpr::Or(parts));
        }
        if let Some(inner) = func("not") {
            return Ok(PolicyExpr::Not(Box::new(PolicyExpr::parse_inner(
                inner.trim(),
                input,
                depth + 1,
            )?)));
        }
        let named = |rest: &str, what: &str| -> Result<String, PolicyParseError> {
            if valid_name(rest) {
                Ok(rest.to_string())
            } else {
                Err(perr(
                    input,
                    format!("empty or invalid {what} name `{rest}`"),
                ))
            }
        };
        Ok(match s {
            "public" => PolicyExpr::Public,
            "authenticated" => PolicyExpr::Authenticated,
            "owner" => PolicyExpr::Owner,
            "service_only" => PolicyExpr::ServiceOnly,
            "admin_only" => PolicyExpr::AdminOnly,
            "deny" => PolicyExpr::Deny,
            "owner_or_service" => PolicyExpr::Or(vec![PolicyExpr::Owner, PolicyExpr::ServiceOnly]),
            _ if s.starts_with("all:") => return flat(&s[4..], true),
            _ if s.starts_with("any:") => return flat(&s[4..], false),
            _ if s.starts_with("not:") => PolicyExpr::Not(Box::new(PolicyExpr::parse_inner(
                &s[4..],
                input,
                depth + 1,
            )?)),
            _ if s.starts_with("role:") => PolicyExpr::HasRole(
                s[5..]
                    .split('|')
                    .map(|r| named(r, "role"))
                    .collect::<Result<_, _>>()?,
            ),
            _ if s.starts_with("owner_or_role:") => PolicyExpr::Or(vec![
                PolicyExpr::Owner,
                PolicyExpr::HasRole(vec![named(&s[14..], "role")?]),
            ]),
            _ if s.starts_with("resolver:") => PolicyExpr::Resolver(named(&s[9..], "resolver")?),
            _ if s.starts_with("owner_or_resolver:") => PolicyExpr::Or(vec![
                PolicyExpr::Owner,
                PolicyExpr::Resolver(named(&s[18..], "resolver")?),
            ]),
            _ if s.starts_with("perm:") => PolicyExpr::Perm(named(s[5..].trim(), "permission")?),
            _ => return Err(perr(input, format!("unknown token `{s}`"))),
        })
    }

    /// Resolvers referenced by this policy.
    pub fn resolvers(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let PolicyExpr::Resolver(n) = e {
                out.push(n.as_str())
            }
        });
        out
    }

    fn walk<'a>(&'a self, f: &mut impl FnMut(&'a PolicyExpr)) {
        f(self);
        match self {
            PolicyExpr::And(v) | PolicyExpr::Or(v) => v.iter().for_each(|e| e.walk(f)),
            PolicyExpr::Not(e) => e.walk(f),
            _ => {}
        }
    }

    /// Can this policy admit an ANONYMOUS caller? Sound over-approximation: `public` does,
    /// but so does any unguarded negation (`not:deny`, `not:role:x`, `any:authenticated,
    /// not:authenticated`). Computed over the outcome SET {true,false,unevaluable} of every
    /// sub-expression for a principal with no identity and no role; `owner`, `perm:` and
    /// positive `resolver:` are never `true` for it (host-registered resolvers decide
    /// themselves), but their NEGATION is, which is why `not:resolver:x` needs a guard.
    pub fn may_admit_anonymous(&self) -> bool {
        const T: u8 = 1;
        const F: u8 = 2;
        const N: u8 = 4;
        fn go(e: &PolicyExpr) -> u8 {
            match e {
                PolicyExpr::Public => T,
                PolicyExpr::Deny
                | PolicyExpr::Authenticated
                | PolicyExpr::ServiceOnly
                | PolicyExpr::AdminOnly
                | PolicyExpr::HasRole(_) => F,
                PolicyExpr::Owner | PolicyExpr::Perm(_) | PolicyExpr::Resolver(_) => F | N,
                PolicyExpr::Not(x) => {
                    let s = go(x);
                    (if s & T != 0 { F } else { 0 })
                        | (if s & F != 0 { T } else { 0 })
                        | (if s & N != 0 { N } else { 0 })
                }
                PolicyExpr::And(v) => fold(v, F, T),
                PolicyExpr::Or(v) => fold(v, T, F),
            }
        }
        // `short` = the absorbing value (false for and, true for or), `unit` the neutral one.
        fn fold(v: &[PolicyExpr], short: u8, unit: u8) -> u8 {
            let mut acc = unit;
            for e in v {
                let s = go(e);
                let mut next = 0;
                for a in [T, F, N] {
                    if acc & a == 0 {
                        continue;
                    }
                    for b in [T, F, N] {
                        if s & b == 0 {
                            continue;
                        }
                        next |= if a == short || b == short {
                            short
                        } else if a == N || b == N {
                            N
                        } else {
                            unit
                        };
                    }
                }
                acc = next;
            }
            acc
        }
        go(self) & T != 0
    }

    /// Does the policy admit anonymous callers syntactically (contains `public`)?
    pub fn mentions_public(&self) -> bool {
        let mut found = false;
        self.walk(&mut |e| found |= matches!(e, PolicyExpr::Public));
        found
    }
}

impl fmt::Display for PolicyExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, name: &str, v: &[PolicyExpr]| {
            write!(f, "{name}(")?;
            for (i, e) in v.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{e}")?;
            }
            f.write_str(")")
        };
        match self {
            PolicyExpr::Public => f.write_str("public"),
            PolicyExpr::Authenticated => f.write_str("authenticated"),
            PolicyExpr::Owner => f.write_str("owner"),
            PolicyExpr::ServiceOnly => f.write_str("service_only"),
            PolicyExpr::AdminOnly => f.write_str("admin_only"),
            PolicyExpr::Deny => f.write_str("deny"),
            PolicyExpr::HasRole(r) => write!(f, "role:{}", r.join("|")),
            PolicyExpr::Perm(p) => write!(f, "perm:{p}"),
            PolicyExpr::Resolver(n) => write!(f, "resolver:{n}"),
            PolicyExpr::And(v) => list(f, "all", v),
            PolicyExpr::Or(v) => list(f, "any", v),
            PolicyExpr::Not(e) => write!(f, "not({e})"),
        }
    }
}

impl FromStr for PolicyExpr {
    type Err = PolicyParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PolicyExpr::parse(s)
    }
}
impl From<PolicyExpr> for String {
    fn from(p: PolicyExpr) -> String {
        p.to_string()
    }
}
impl TryFrom<String> for PolicyExpr {
    type Error = PolicyParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Policies of an entity or a link, per operation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermDesc {
    pub ops: BTreeMap<Operation, PolicyExpr>,
}

impl PermDesc {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    /// Explicit policy of `op`, if declared.
    pub fn policy(&self, op: &Operation) -> Option<&PolicyExpr> {
        self.ops.get(op)
    }
    /// Default when nothing is declared: `authenticated` for standard operations (= the
    /// historical `default_auth_policy`), `deny` for business operations. Stored modules
    /// get no default at all: `check()` (profile `Stored`) requires explicit policies.
    pub fn default_policy(op: &Operation) -> PolicyExpr {
        match op {
            Operation::Custom(_) => PolicyExpr::Deny,
            _ => PolicyExpr::Authenticated,
        }
    }
    /// Explicit policy or default.
    pub fn effective(&self, op: &Operation) -> PolicyExpr {
        self.policy(op)
            .cloned()
            .unwrap_or_else(|| Self::default_policy(op))
    }
}

/// Pure view of the principal, to evaluate without this-rs (front, tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct PrincipalView<'a> {
    pub authenticated: bool,
    pub user_id: Option<&'a str>,
    pub roles: &'a [String],
    pub admin: bool,
    pub service: bool,
    /// Is the caller the owner of the resource? `None` = not known for this operation (for
    /// example a `List` before rows are fetched): `owner` is then UNEVALUABLE, never true.
    pub owner: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every string of the `parse_policy` documentation and of the middleware vocabulary.
    #[test]
    fn policy_expr_parses_existing_yaml_grammar() {
        use PolicyExpr::*;
        let cases: Vec<(&str, PolicyExpr)> = vec![
            ("public", Public),
            ("authenticated", Authenticated),
            ("owner", Owner),
            ("service_only", ServiceOnly),
            ("admin_only", AdminOnly),
            ("owner_or_service", Or(vec![Owner, ServiceOnly])),
            ("role:admin", HasRole(vec!["admin".into()])),
            (
                "owner_or_role:admin",
                Or(vec![Owner, HasRole(vec!["admin".into()])]),
            ),
            ("resolver:is_delegated", Resolver("is_delegated".into())),
            ("owner_or_resolver:x", Or(vec![Owner, Resolver("x".into())])),
            (
                "all:authenticated,resolver:is_group_member",
                And(vec![Authenticated, Resolver("is_group_member".into())]),
            ),
            (
                "any:owner,resolver:is_delegated,role:admin",
                Or(vec![
                    Owner,
                    Resolver("is_delegated".into()),
                    HasRole(vec!["admin".into()]),
                ]),
            ),
            (
                "all:admin_only,not:resolver:is_readonly_mode",
                And(vec![
                    AdminOnly,
                    Not(Box::new(Resolver("is_readonly_mode".into()))),
                ]),
            ),
            ("all:public", Public),
            ("perm:invoices.read", Perm("invoices.read".into())),
            (
                "not:perm:blacklisted",
                Not(Box::new(Perm("blacklisted".into()))),
            ),
        ];
        for (s, expected) in cases {
            assert_eq!(PolicyExpr::parse(s).unwrap(), expected, "{s}");
        }
    }

    #[test]
    fn policy_expr_rejects_unknown_token() {
        for bad in [
            "admin_onyl",
            "PUBLIC",
            "",
            " public",
            "role:",
            "resolver:",
            "all:public,oops",
            "all(public",
            "any(a))",
        ] {
            assert!(PolicyExpr::parse(bad).is_err(), "`{bad}` must be refused");
        }
    }

    #[test]
    fn policy_display_fromstr_roundtrip_with_nesting() {
        use PolicyExpr::*;
        let e = And(vec![
            Or(vec![Owner, HasRole(vec!["a".into(), "b".into()])]),
            Not(Box::new(Resolver("x".into()))),
            Perm("invoice.pay".into()),
        ]);
        let s = e.to_string();
        assert_eq!(
            s,
            "all(any(owner, role:a|b), not(resolver:x), perm:invoice.pay)"
        );
        assert_eq!(PolicyExpr::parse(&s).unwrap(), e);
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<PolicyExpr>(&json).unwrap(), e);
    }

    /// Review (DoS): a policy string of a few hundred KB (`not:not:not:..`) recursed once per
    /// level and overflowed the stack of a worker thread (process abort). Bounded depth now.
    #[test]
    fn deeply_nested_policy_is_refused_not_a_stack_overflow() {
        let run = || {
            for s in [
                format!("{}deny", "not:".repeat(200_000)),
                format!("{}deny{}", "not(".repeat(100_000), ")".repeat(100_000)),
                format!("{}deny", "all:".repeat(200_000)),
                format!("{}deny{}", "any(".repeat(100_000), ")".repeat(100_000)),
            ] {
                assert!(PolicyExpr::parse(&s).is_err());
            }
            // A realistic nesting still parses.
            assert!(PolicyExpr::parse("all(any(owner, role:a), not(not(resolver:x)))").is_ok());
        };
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn operation_string_form_and_custom_default_deny() {
        assert_eq!("get_link".parse::<Operation>().unwrap(), Operation::GetLink);
        assert_eq!(
            "invoice.pay".parse::<Operation>().unwrap(),
            Operation::Custom("invoice.pay".into())
        );
        assert!("pay".parse::<Operation>().is_err());
        assert!("Invoice.Pay".parse::<Operation>().is_err());
        assert_eq!(
            PermDesc::default().effective(&Operation::Custom("invoice.pay".into())),
            PolicyExpr::Deny
        );
        assert_eq!(
            PermDesc::default().effective(&Operation::Delete),
            PolicyExpr::Authenticated
        );
    }
}
