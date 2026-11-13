//! Permissions: operations, the policy AST and its string grammar.
//!
//! The string grammar is a strict superset of the YAML grammar of
//! `AuthPolicy::parse_policy` (this-rs), with three differences, all fail-closed:
//! - an unknown token is an ERROR (`parse_policy` falls back to `authenticated`);
//! - `perm:<permission>` (materialized IAM) is part of the AST;
//! - a nestable function form `all(..)`, `any(..)`, `not(..)` exists, and `Display` always
//!   prints it for compositions, so ONE canonical syntax round-trips.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
                    .map(|r| {
                        // `role:a|role:b` se lit naturellement comme « a ou b », mais donnait
                        // les rôles `a` et `role:b` (le `:` est permis dans un nom) : un membre
                        // perdait l'accès sans aucune erreur. La forme juste est `role:a|b`.
                        if r.starts_with("role:") {
                            return Err(perr(
                                input,
                                format!("repeated `role:` prefix in `{s}`: write `role:a|b`"),
                            ));
                        }
                        named(r, "role")
                    })
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

    /// Les permissions IAM mentionnées par la politique (`perm:…`), sans doublon.
    ///
    /// Symétrique de [`PolicyExpr::resolvers`] : un hôte peut savoir ce qu'il faudrait
    /// accorder pour rendre la politique décidable — c'est la base d'une demande
    /// d'autorisation in-app.
    pub fn permissions(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        self.walk(&mut |e| match e {
            PolicyExpr::Perm(name) if !out.contains(&name.as_str()) => out.push(name.as_str()),
            _ => {}
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

// ---------------------------------------------------------------------------------------------
// Rôles donnés par le propriétaire d'un service, et décision à deux valeurs pour le maillage.
// ---------------------------------------------------------------------------------------------

/// Les noms de rôle que mentionnent les `HasRole` de toutes les opérations de `perm`, en
/// descendant dans `And`, `Or` et `Not`. C'est la liste des « rôles que ce service peut
/// donner » : elle se lit dans les politiques, on ne la déclare pas une seconde fois.
///
/// Un rôle qui n'apparaît que sous un `Not` y figure aussi : il est nommé par le service,
/// même si l'accorder RETIRE un droit.
pub fn roles_de(perm: &PermDesc) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for expr in perm.ops.values() {
        expr.walk(&mut |e| {
            if let PolicyExpr::HasRole(rs) = e {
                out.extend(rs.iter().cloned());
            }
        });
    }
    out
}

/// L'appelant, vu par le maillage : sans identité de service, sans admin, sans résolveur.
#[derive(Debug, Clone, Copy)]
pub struct Sujet<'a> {
    /// L'appelant est le propriétaire de la ressource (pour le maillage : un appareil du
    /// lien « moi »).
    pub proprietaire: bool,
    /// L'appelant est authentifié (un pair appairé l'est).
    pub authentifie: bool,
    /// Les rôles accordés à cet appelant sur CE service.
    pub roles: &'a BTreeSet<String>,
    /// Les permissions matérialisées (souvent vide). Comparaison EXACTE du nom :
    /// `perm:a:b.c` et `a.b.c` ne sont pas rapprochés ici (voir [`canonical_permission`]).
    pub perms: &'a BTreeSet<String>,
}

/// Environnement du maillage pour [`crate::engine::policy::evaluate`] : les permissions
/// viennent du sujet, et AUCUN résolveur n'existe.
struct EnvMaillage<'a>(&'a BTreeSet<String>);

impl crate::engine::policy::PolicyEnv for EnvMaillage<'_> {
    /// Le maillage n'a pas de résolveur : `resolver:x` est INÉVALUABLE, donc refusé à la
    /// décision, et `not:resolver:x` l'est aussi (la négation d'un inévaluable reste
    /// inévaluable). Un hôte qui a des résolveurs ne passe pas par [`autorise`] : il appelle
    /// [`crate::engine::policy::evaluate`] avec son propre [`crate::engine::policy::PolicyEnv`].
    fn resolver(&self, _: &str) -> Option<bool> {
        None
    }
    fn permission(&self, permission: &str) -> Option<bool> {
        Some(self.0.contains(permission))
    }
}

/// `true` si `expr` contient un `And` vide, quelle que soit la profondeur.
fn a_un_and_vide(expr: &PolicyExpr) -> bool {
    let mut vu = false;
    expr.walk(&mut |e| vu |= matches!(e, PolicyExpr::And(v) if v.is_empty()));
    vu
}

/// Copie de `expr` où chaque `And` vide devient `Deny`. Le moteur commun
/// ([`crate::engine::policy::evaluate`]) rend `Some(true)` pour un `And` vide (élément neutre
/// de la conjonction) ; ici un `And` vide n'accorde rien.
fn and_vide_en_deny(expr: &PolicyExpr) -> PolicyExpr {
    match expr {
        PolicyExpr::And(v) if v.is_empty() => PolicyExpr::Deny,
        PolicyExpr::And(v) => PolicyExpr::And(v.iter().map(and_vide_en_deny).collect()),
        PolicyExpr::Or(v) => PolicyExpr::Or(v.iter().map(and_vide_en_deny).collect()),
        PolicyExpr::Not(e) => PolicyExpr::Not(Box::new(and_vide_en_deny(e))),
        e => e.clone(),
    }
}

/// Le `sujet` peut-il ce que garde `expr` ? Décision pure, à deux valeurs : seul un `true`
/// évaluable accorde. S'appuie sur [`crate::engine::policy::evaluate`] (le moteur commun,
/// trois valeurs), avec un principal sans service ni admin et un environnement sans résolveur.
///
/// | variante          | résultat                                                        |
/// |-------------------|-----------------------------------------------------------------|
/// | `Public`          | vrai                                                            |
/// | `Authenticated`   | `sujet.authentifie`                                             |
/// | `Owner`           | `sujet.proprietaire` (n'implique AUCUN rôle)                    |
/// | `HasRole(rs)`     | un des `rs` est dans `sujet.roles` ; liste vide : faux          |
/// | `Perm(p)`         | `p` est dans `sujet.perms` (nom exact)                          |
/// | `Deny`            | faux                                                            |
/// | `ServiceOnly`     | faux : le maillage n'a pas de service interne                   |
/// | `AdminOnly`       | faux : le maillage n'a pas d'admin                              |
/// | `Resolver(_)`     | faux (inévaluable) : le maillage n'a pas de résolveur           |
/// | `And(v)`          | tous vrais ; liste vide : FAUX (un `And` vide n'accorde rien)   |
/// | `Or(v)`           | au moins un vrai ; liste vide : faux                            |
/// | `Not(e)`          | négation de `e` ; si `e` est inévaluable, faux                  |
///
/// Refuser est la règle sûre pour ce que le maillage ne connaît pas (`service_only`,
/// `admin_only`, `resolver:`). Un hôte qui a des résolveurs, un admin ou des appels de service
/// doit utiliser son propre chemin ([`crate::engine::policy::evaluate`] avec son
/// [`PrincipalView`] et son environnement), pas cette fonction.
///
/// ATTENTION : `Not` est une vraie négation. `not:deny`, `not:admin_only` et
/// `not:service_only` accordent TOUT, à tout appelant, même anonyme. Seul `not:resolver:x`
/// reste refusé, parce qu'un résolveur absent est inévaluable et non faux.
/// [`PolicyExpr::may_admit_anonymous`] repère ces politiques avant publication.
pub fn autorise(expr: &PolicyExpr, sujet: &Sujet) -> bool {
    let roles: Vec<String> = sujet.roles.iter().cloned().collect();
    let who = PrincipalView {
        authenticated: sujet.authentifie,
        user_id: None,
        roles: &roles,
        admin: false,
        service: false,
        owner: Some(sujet.proprietaire),
    };
    let env = EnvMaillage(sujet.perms);
    if a_un_and_vide(expr) {
        crate::engine::policy::is_allowed(&and_vide_en_deny(expr), &who, &env)
    } else {
        crate::engine::policy::is_allowed(expr, &who, &env)
    }
}

/// [`autorise`] sur la politique DÉCLARÉE de `op`. Une opération absente de `perm` est
/// refusée (refus par défaut) : on n'applique PAS [`PermDesc::default_policy`], qui donnerait
/// `authenticated` aux opérations standard.
pub fn autorise_op(perm: &PermDesc, op: &Operation, sujet: &Sujet) -> bool {
    perm.policy(op).is_some_and(|e| autorise(e, sujet))
}

// ---------------------------------------------------------------------------------------------
// Module-scoped permissions: what a module may REQUIRE, and what it may be GRANTED.
// ---------------------------------------------------------------------------------------------

/// Codes of the module-permission contract. Namespaced (`this.module.*` for the install /
/// load decisions, `this.auth.*` for the call-time refusal), stable, and mapped by
/// [`ContractError::kind`] to HTTP and gRPC like every other `ContractError`.
///
/// They are built with [`ContractError::new`] (the framework table of `framework.rs` belongs
/// to another stream and does not list them yet): the kind is fixed here, once, by
/// [`module_perm_error`].
pub mod permission_codes {
    /// A permission is empty, has an empty segment, a single segment, or characters outside
    /// `[A-Za-z0-9_-]`. Install refusal (422).
    pub const MALFORMED: &str = "this.module.permission_malformed";
    /// A permission lives in the namespace of ANOTHER module (or in none the module owns).
    /// Install refusal (422).
    pub const FOREIGN_NAMESPACE: &str = "this.module.permission_foreign_namespace";
    /// A permission lives in a namespace reserved to the framework (`this`, `iam`, ...).
    /// Install refusal (422).
    pub const RESERVED_NAMESPACE: &str = "this.module.permission_reserved_namespace";
    /// A permission is an administration permission (`admin` segment, wildcard).
    /// Install refusal (422).
    pub const ADMIN: &str = "this.module.permission_admin";
    /// A permission the module requires is granted by NO policy rule. Load refusal (412).
    pub const UNGRANTED: &str = "this.module.permission_ungranted";
    /// The MODULE (not the caller) lacks the permission a route needs. Call refusal (403).
    pub const MODULE_FORBIDDEN: &str = "this.auth.module_forbidden";
}

/// Namespaces a module can never use: they belong to the framework and to its IAM.
pub const RESERVED_PERMISSION_NAMESPACES: [&str; 7] = [
    "this",
    "iam",
    "wami",
    "admin",
    "system",
    "framework",
    "root",
];

/// Permission segments that mean administration. A module cannot grant itself one, whatever
/// the namespace in front of it (`billing.admin`, `billing.invoice.root`).
pub const ADMIN_PERMISSION_SEGMENTS: [&str; 3] = ["admin", "root", "superuser"];

/// `perm:billing:invoice.read` and `billing.invoice.read` are the same permission (the
/// separators `:` and `.` are interchangeable, as in `wami_action_for` and the token scopes).
/// Returns the canonical dotted form without the optional `perm:` prefix.
pub fn canonical_permission(raw: &str) -> String {
    raw.strip_prefix("perm:")
        .unwrap_or(raw)
        .trim()
        .replace(':', ".")
}

/// Namespace of a permission: its first segment (`billing` for `billing.invoice.read`).
pub fn permission_namespace(raw: &str) -> &str {
    raw.strip_prefix("perm:")
        .unwrap_or(raw)
        .trim()
        .split(['.', ':'])
        .next()
        .unwrap_or("")
}

/// Error of the module-permission contract, with its kind fixed in one place.
fn module_perm_error(
    code: &str,
    kind: crate::error::ErrorKind,
    message: String,
) -> crate::error::ContractError {
    crate::error::ContractError::new(code, kind).with_message(message)
}

/// Is `perm` an acceptable requirement for module `module`? The single rule behind install,
/// load and call time. `Err` carries a named, parameterized [`ContractError`]
/// (`module`, `permission`, `namespace`).
pub fn check_permission_for_module(
    module: &str,
    perm: &str,
) -> Result<(), crate::error::ContractError> {
    use crate::error::ErrorKind;
    let canonical = canonical_permission(perm);
    let segments: Vec<&str> = canonical.split('.').collect();
    let namespace = segments.first().copied().unwrap_or("");
    let fail = |code: &str, why: String| {
        module_perm_error(code, ErrorKind::Validation, why)
            .with_param("module", module)
            .with_param("permission", &canonical)
            .with_param("namespace", namespace)
    };
    // A wildcard is a blanket grant: administration, not a name.
    if segments.iter().any(|s| s.contains('*')) {
        return Err(fail(
            permission_codes::ADMIN,
            format!(
                "module `{module}` requires the wildcard permission `{canonical}`: a wildcard is an administration permission a module cannot give itself"
            ),
        ));
    }
    let well_formed = segments.len() >= 2
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if !well_formed {
        return Err(fail(
            permission_codes::MALFORMED,
            format!(
                "module `{module}` requires the malformed permission `{canonical}`: expected `{module}.<action>` (at least two non-empty segments of letters, digits, `_`, `-`)"
            ),
        ));
    }
    if RESERVED_PERMISSION_NAMESPACES.contains(&namespace) {
        return Err(fail(
            permission_codes::RESERVED_NAMESPACE,
            format!(
                "module `{module}` requires `{canonical}`: namespace `{namespace}` is reserved to the framework"
            ),
        ));
    }
    if segments
        .iter()
        .any(|s| ADMIN_PERMISSION_SEGMENTS.contains(&s.to_ascii_lowercase().as_str()))
    {
        return Err(fail(
            permission_codes::ADMIN,
            format!(
                "module `{module}` requires the administration permission `{canonical}`: a module cannot give itself one"
            ),
        ));
    }
    if namespace != module {
        return Err(fail(
            permission_codes::FOREIGN_NAMESPACE,
            format!(
                "module `{module}` requires `{canonical}`, which belongs to namespace `{namespace}`: a module may only require permissions of its own namespace (`{module}.*`)"
            ),
        ));
    }
    Ok(())
}

/// Every `(where, permission)` the module's policies name through `perm:`, in declaration
/// order (entities, then links), duplicates kept: `where` locates it for the operator.
pub fn declared_permissions(m: &crate::module::ModuleDescriptor) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut scan = |place: String, perms: &PermDesc| {
        for (op, expr) in &perms.ops {
            for p in expr.permissions() {
                out.push((format!("{place}.{op}"), p.to_string()));
            }
        }
    };
    for e in &m.entities {
        scan(format!("entity `{}`", e.name), &e.permissions);
    }
    for l in &m.links {
        scan(
            format!("link `{}({}→{})`", l.link_type, l.source, l.target),
            &l.permissions,
        );
    }
    out
}

/// The distinct permissions (canonical form) the module requires, sorted.
pub fn required_permissions(m: &crate::module::ModuleDescriptor) -> Vec<String> {
    let set: std::collections::BTreeSet<String> = declared_permissions(m)
        .into_iter()
        .map(|(_, p)| canonical_permission(&p))
        .collect();
    set.into_iter().collect()
}

/// INSTALL-time check: every permission the module's policies name must be in the module's own
/// namespace and not an administration permission. Returns ALL violations (not only the first),
/// each located (`where`) so the operator can fix the descriptor in one pass. Empty = valid;
/// a module that declares no `perm:` is valid (nothing to steal).
pub fn check_module_permissions(
    m: &crate::module::ModuleDescriptor,
) -> Vec<crate::error::ContractError> {
    declared_permissions(m)
        .into_iter()
        .filter_map(|(place, p)| {
            check_permission_for_module(&m.name, &p)
                .err()
                .map(|e| e.with_param("declared_at", place))
        })
        .collect()
}

/// Who grants a module its permissions: the host's policy rules, seen from the module.
///
/// Three-valued like `perm:` everywhere else: `Some(true)` granted, `Some(false)` refused,
/// `None` unevaluable. Only `Some(true)` is a grant; the other two refuse.
pub trait ModulePermissionSource: Send + Sync {
    /// May `module` itself exercise `permission` (canonical dotted form)?
    fn module_allows(&self, module: &str, permission: &str) -> Option<bool>;
}

impl<F> ModulePermissionSource for F
where
    F: Fn(&str, &str) -> Option<bool> + Send + Sync,
{
    fn module_allows(&self, module: &str, permission: &str) -> Option<bool> {
        self(module, permission)
    }
}

/// In-memory policy of module grants: `Allow`/`Deny` patterns per module, deny-overrides, and
/// NO inheritance: a module only ever sees ITS OWN rules, and never a permission outside its own
/// namespace, whatever a rule says. Nothing is granted by a module being loaded.
///
/// A pattern is an exact permission (`billing.invoice.read`) or a family of the module's own
/// namespace (`billing.invoice.*`, `billing.*`). The bare `*` matches nothing.
#[derive(Debug, Clone, Default)]
pub struct ModuleGrants {
    allow: BTreeMap<String, Vec<String>>,
    deny: BTreeMap<String, Vec<String>>,
}

impl ModuleGrants {
    pub fn new() -> Self {
        Self::default()
    }
    /// Allow `pattern` to `module`.
    pub fn allow(mut self, module: &str, pattern: &str) -> Self {
        self.allow
            .entry(module.to_string())
            .or_default()
            .push(canonical_permission(pattern));
        self
    }
    /// Deny `pattern` to `module` (wins over any allow).
    pub fn deny(mut self, module: &str, pattern: &str) -> Self {
        self.deny
            .entry(module.to_string())
            .or_default()
            .push(canonical_permission(pattern));
        self
    }

    fn matches(pattern: &str, permission: &str) -> bool {
        if pattern == permission {
            return true;
        }
        pattern.strip_suffix(".*").is_some_and(|family| {
            !family.is_empty() && permission.starts_with(&format!("{family}."))
        })
    }
}

impl ModulePermissionSource for ModuleGrants {
    fn module_allows(&self, module: &str, permission: &str) -> Option<bool> {
        let permission = canonical_permission(permission);
        // No inheritance between modules, whatever the rule table contains.
        if permission_namespace(&permission) != module {
            return Some(false);
        }
        let hit = |t: &BTreeMap<String, Vec<String>>| {
            t.get(module)
                .is_some_and(|v| v.iter().any(|p| Self::matches(p, &permission)))
        };
        if hit(&self.deny) {
            return Some(false);
        }
        Some(hit(&self.allow))
    }
}

/// What to do with a module whose required permissions are not all granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadPosture {
    /// Refuse to load: `this.module.permission_ungranted` (default, fail-closed).
    #[default]
    Strict,
    /// Load, and SAY SO: the verdict lists the ungranted permissions, and every route whose
    /// policy names one of them is refused at call time (`this.auth.module_forbidden`). The
    /// rest of the module works.
    Degraded,
}

/// The documented outcome of a load decision.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct LoadVerdict {
    pub module: String,
    /// Required and granted.
    pub granted: Vec<String>,
    /// Required and NOT granted (non-empty only under [`LoadPosture::Degraded`]).
    pub ungranted: Vec<String>,
}

impl LoadVerdict {
    /// Was the module admitted with missing permissions?
    pub fn is_degraded(&self) -> bool {
        !self.ungranted.is_empty()
    }
}

/// LOAD-time check: is every permission the module requires granted to IT by `source`?
/// `Strict`: any gap refuses, naming the module and the missing permissions. `Degraded`: the gap
/// is reported in the verdict (never silent). The install rules ([`check_module_permissions`])
/// are NOT repeated here; use [`admit_module`] for both.
pub fn check_load(
    m: &crate::module::ModuleDescriptor,
    source: &dyn ModulePermissionSource,
    posture: LoadPosture,
) -> Result<LoadVerdict, crate::error::ContractError> {
    let mut verdict = LoadVerdict {
        module: m.name.clone(),
        ..LoadVerdict::default()
    };
    for p in required_permissions(m) {
        if source.module_allows(&m.name, &p) == Some(true) {
            verdict.granted.push(p);
        } else {
            verdict.ungranted.push(p);
        }
    }
    if verdict.is_degraded() && posture == LoadPosture::Strict {
        return Err(module_perm_error(
            permission_codes::UNGRANTED,
            crate::error::ErrorKind::PreconditionFailed,
            format!(
                "module `{}` is not loaded: no policy rule grants it {}",
                m.name,
                verdict
                    .ungranted
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with_param("module", &m.name)
        .with_param("permissions", &verdict.ungranted)
        .with_param("posture", "strict"));
    }
    Ok(verdict)
}

/// Install check THEN load check: the single call N3 makes before registering a module.
/// Install violations come first and are all returned; the load verdict only exists for a
/// module that passed them.
pub fn admit_module(
    m: &crate::module::ModuleDescriptor,
    source: &dyn ModulePermissionSource,
    posture: LoadPosture,
) -> Result<LoadVerdict, Vec<crate::error::ContractError>> {
    let violations = check_module_permissions(m);
    if !violations.is_empty() {
        return Err(violations);
    }
    check_load(m, source, posture).map_err(|e| vec![e])
}

/// CALL-time check on the MODULE side: may `module` exercise `permission`? Called IN ADDITION
/// to the caller's own check; both must pass. Re-validates the namespace (a descriptor that
/// reached the runtime without `check_module_permissions` cannot borrow another module's
/// grants). The error names the module and the permission; the surface adds caller and resource.
pub fn check_module_call(
    module: &str,
    permission: &str,
    source: &dyn ModulePermissionSource,
) -> Result<(), crate::error::ContractError> {
    let canonical = canonical_permission(permission);
    let refuse = |reason: String| {
        module_perm_error(
            permission_codes::MODULE_FORBIDDEN,
            crate::error::ErrorKind::PermissionDenied,
            format!("module `{module}` may not exercise `{canonical}`: {reason}"),
        )
        .with_param("module", module)
        .with_param("permission", &canonical)
    };
    if let Err(e) = check_permission_for_module(module, &canonical) {
        return Err(refuse(format!(
            "not a permission of its own namespace ({})",
            e.code
        ))
        .with_param("cause", e.code.as_str()));
    }
    match source.module_allows(module, &canonical) {
        Some(true) => Ok(()),
        Some(false) => Err(refuse("no policy rule grants it to the module".to_string())
            .with_param("cause", "not_granted")),
        None => Err(
            refuse("the module's grant could not be evaluated (fail-closed)".to_string())
                .with_param("cause", "unevaluable"),
        ),
    }
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

    // ----- module-scoped permissions -----

    use crate::entity::EntityDesc;
    use crate::module::ModuleDescriptor;

    /// A module whose single entity guards `get` and `create` with the given policies.
    fn module(name: &str, policies: &[(&str, &str)]) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new(name, "1.0.0");
        let mut e = EntityDesc::new("invoice");
        for (op, pol) in policies {
            e.permissions
                .ops
                .insert(op.parse().unwrap(), PolicyExpr::parse(pol).unwrap());
        }
        m.entities.push(e);
        m
    }

    fn codes_of(errs: &[crate::error::ContractError]) -> Vec<&str> {
        errs.iter().map(|e| e.code.as_str()).collect()
    }

    #[test]
    fn module_permissions_in_own_namespace_are_accepted() {
        let m = module(
            "billing",
            &[
                ("get", "perm:billing.invoice.read"),
                ("create", "all(authenticated, perm:billing:invoice.write)"),
            ],
        );
        assert!(check_module_permissions(&m).is_empty());
        assert_eq!(
            required_permissions(&m),
            vec!["billing.invoice.read", "billing.invoice.write"]
        );
    }

    #[test]
    fn a_module_without_perm_policies_has_nothing_to_validate() {
        let m = module("billing", &[("get", "authenticated"), ("list", "public")]);
        assert!(check_module_permissions(&m).is_empty());
        assert!(required_permissions(&m).is_empty());
    }

    #[test]
    fn stolen_namespace_is_refused_with_a_named_code_and_location() {
        let m = module(
            "billing",
            &[
                ("get", "perm:catalog.product.read"),
                ("create", "perm:billing.invoice.write"),
            ],
        );
        let errs = check_module_permissions(&m);
        assert_eq!(codes_of(&errs), vec![permission_codes::FOREIGN_NAMESPACE]);
        let e = &errs[0];
        assert_eq!(e.kind, crate::error::ErrorKind::Validation);
        assert_eq!(e.params["module"], "billing");
        assert_eq!(e.params["permission"], "catalog.product.read");
        assert_eq!(e.params["namespace"], "catalog");
        assert_eq!(e.params["declared_at"], "entity `invoice`.get");
        assert!(e.message.contains("billing") && e.message.contains("catalog"));
    }

    #[test]
    fn a_namespace_that_merely_starts_like_the_module_is_foreign() {
        let m = module("billing", &[("get", "perm:billing_admin_tools.read")]);
        assert_eq!(
            codes_of(&check_module_permissions(&m)),
            vec![permission_codes::FOREIGN_NAMESPACE]
        );
    }

    #[test]
    fn framework_reserved_namespaces_are_refused() {
        for ns in ["this", "iam", "wami", "admin", "system"] {
            let m = module("billing", &[("get", &format!("perm:{ns}.users.read"))]);
            assert_eq!(
                codes_of(&check_module_permissions(&m)),
                vec![permission_codes::RESERVED_NAMESPACE],
                "{ns}"
            );
        }
    }

    #[test]
    fn administration_permissions_are_refused_even_in_the_own_namespace() {
        for p in [
            "billing.admin",
            "billing.invoice.admin",
            "billing.root",
            "billing.*",
            "*",
            "billing.invoice.*",
        ] {
            let m = module("billing", &[("delete", &format!("perm:{p}"))]);
            let errs = check_module_permissions(&m);
            assert_eq!(codes_of(&errs), vec![permission_codes::ADMIN], "{p}");
            assert_eq!(errs[0].kind, crate::error::ErrorKind::Validation);
        }
    }

    #[test]
    fn malformed_permissions_are_refused() {
        // `perm:` parsing already rejects empty/space names; the rest reaches the check.
        for p in ["billing", "billing..read", "billing.re/ad", ".billing.read"] {
            let err = check_permission_for_module("billing", p).unwrap_err();
            assert_eq!(err.code.as_str(), permission_codes::MALFORMED, "{p}");
        }
    }

    #[test]
    fn every_violation_is_reported_in_one_pass_including_negated_ones() {
        let m = module(
            "billing",
            &[
                ("get", "perm:catalog.read"),
                ("update", "not(perm:this.users.read)"),
                (
                    "delete",
                    "any(perm:billing.admin, perm:billing.invoice.read)",
                ),
            ],
        );
        let errs = check_module_permissions(&m);
        assert_eq!(
            codes_of(&errs),
            vec![
                // Operation order: get, update, delete.
                permission_codes::FOREIGN_NAMESPACE,
                permission_codes::RESERVED_NAMESPACE,
                permission_codes::ADMIN
            ]
        );
    }

    #[test]
    fn link_permissions_are_checked_like_entity_ones() {
        let mut m = module("billing", &[]);
        let mut l = crate::link::LinkDesc::new("billed_to", "invoice", "user");
        l.permissions.ops.insert(
            Operation::CreateLink,
            PolicyExpr::parse("perm:iam.attach").unwrap(),
        );
        m.links.push(l);
        let errs = check_module_permissions(&m);
        assert_eq!(codes_of(&errs), vec![permission_codes::RESERVED_NAMESPACE]);
        assert!(
            errs[0].params["declared_at"]
                .as_str()
                .unwrap()
                .contains("billed_to")
        );
    }

    #[test]
    fn load_is_granted_when_every_required_permission_is_allowed() {
        let m = module("billing", &[("get", "perm:billing.invoice.read")]);
        let grants = ModuleGrants::new().allow("billing", "billing.invoice.*");
        let v = check_load(&m, &grants, LoadPosture::Strict).unwrap();
        assert_eq!(v.granted, vec!["billing.invoice.read"]);
        assert!(!v.is_degraded());
    }

    #[test]
    fn load_is_refused_with_a_named_code_when_no_rule_grants_a_permission() {
        let m = module(
            "billing",
            &[
                ("get", "perm:billing.invoice.read"),
                ("create", "perm:billing.invoice.write"),
            ],
        );
        let grants = ModuleGrants::new().allow("billing", "billing.invoice.read");
        let e = check_load(&m, &grants, LoadPosture::Strict).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::UNGRANTED);
        assert_eq!(e.kind, crate::error::ErrorKind::PreconditionFailed);
        assert_eq!(e.params["module"], "billing");
        assert_eq!(e.params["permissions"][0], "billing.invoice.write");
        assert!(e.message.contains("billing.invoice.write"));
    }

    #[test]
    fn degraded_load_reports_the_gap_instead_of_hiding_it() {
        let m = module(
            "billing",
            &[
                ("get", "perm:billing.invoice.read"),
                ("create", "perm:billing.invoice.write"),
            ],
        );
        let grants = ModuleGrants::new().allow("billing", "billing.invoice.read");
        let v = check_load(&m, &grants, LoadPosture::Degraded).unwrap();
        assert!(v.is_degraded());
        assert_eq!(v.ungranted, vec!["billing.invoice.write"]);
        assert_eq!(v.granted, vec!["billing.invoice.read"]);
    }

    #[test]
    fn a_module_with_no_rule_at_all_is_not_granted_by_being_present() {
        let m = module("billing", &[("get", "perm:billing.invoice.read")]);
        let e = check_load(&m, &ModuleGrants::new(), LoadPosture::Strict).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::UNGRANTED);
    }

    #[test]
    fn admit_module_reports_install_violations_before_load_ones() {
        let m = module("billing", &[("get", "perm:catalog.read")]);
        let errs = admit_module(&m, &ModuleGrants::new(), LoadPosture::Strict).unwrap_err();
        assert_eq!(codes_of(&errs), vec![permission_codes::FOREIGN_NAMESPACE]);
    }

    #[test]
    fn module_grants_never_cross_namespaces_and_deny_overrides() {
        // A rule table that (wrongly) hands billing a catalog permission and a blanket `*`.
        let g = ModuleGrants::new()
            .allow("billing", "catalog.product.read")
            .allow("billing", "*")
            .allow("billing", "billing.*")
            .deny("billing", "billing.invoice.delete");
        assert_eq!(
            g.module_allows("billing", "catalog.product.read"),
            Some(false)
        );
        assert_eq!(
            g.module_allows("billing", "billing.invoice.read"),
            Some(true)
        );
        assert_eq!(
            g.module_allows("billing", "billing.invoice.delete"),
            Some(false)
        );
        // catalog has no rules of its own: billing's rules are not inherited.
        assert_eq!(
            g.module_allows("catalog", "catalog.product.read"),
            Some(false)
        );
    }

    #[test]
    fn module_call_check_is_granted_refused_or_unevaluable_and_names_everything() {
        let g = ModuleGrants::new().allow("billing", "billing.invoice.read");
        assert!(check_module_call("billing", "billing.invoice.read", &g).is_ok());
        assert!(check_module_call("billing", "perm:billing:invoice.read", &g).is_ok());

        let e = check_module_call("billing", "billing.invoice.write", &g).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.kind, crate::error::ErrorKind::PermissionDenied);
        assert_eq!(e.kind.http_status(), 403);
        assert_eq!(e.kind.grpc_code(), 7);
        assert_eq!(e.params["module"], "billing");
        assert_eq!(e.params["permission"], "billing.invoice.write");
        assert_eq!(e.params["cause"], "not_granted");

        let unknown = |_: &str, _: &str| -> Option<bool> { None };
        let e = check_module_call("billing", "billing.invoice.read", &unknown).unwrap_err();
        assert_eq!(e.params["cause"], "unevaluable");
    }

    #[test]
    fn module_call_check_refuses_a_foreign_permission_even_if_the_source_says_yes() {
        let yes = |_: &str, _: &str| -> Option<bool> { Some(true) };
        let e = check_module_call("billing", "catalog.product.read", &yes).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.params["cause"], permission_codes::FOREIGN_NAMESPACE);
        let e = check_module_call("billing", "this.users.read", &yes).unwrap_err();
        assert_eq!(e.params["cause"], permission_codes::RESERVED_NAMESPACE);
    }

    // ----- rôles et décision du maillage -----

    fn ens(noms: &[&str]) -> BTreeSet<String> {
        noms.iter().map(|s| s.to_string()).collect()
    }

    fn role(r: &str) -> PolicyExpr {
        PolicyExpr::HasRole(vec![r.into()])
    }

    #[test]
    fn roles_de_descend_dans_and_or_not() {
        use PolicyExpr::*;
        let mut perm = PermDesc::default();
        perm.ops.insert(
            Operation::Get,
            And(vec![
                role("a"),
                Or(vec![role("b"), Not(Box::new(role("c")))]),
            ]),
        );
        assert_eq!(roles_de(&perm), ens(&["a", "b", "c"]));

        // Union sur toutes les opérations, sans doublon ; une liste `role:x|y` compte pour deux.
        perm.ops
            .insert(Operation::Delete, PolicyExpr::parse("role:a|d").unwrap());
        perm.ops.insert(Operation::List, Public);
        assert_eq!(roles_de(&perm), ens(&["a", "b", "c", "d"]));
        assert!(roles_de(&PermDesc::default()).is_empty());
    }

    #[test]
    fn autorise_chaque_variante() {
        use PolicyExpr::*;
        let vide = BTreeSet::new();
        let roles = ens(&["lecteur"]);
        let perms = ens(&["liste.exporter"]);
        let anonyme = Sujet {
            proprietaire: false,
            authentifie: false,
            roles: &vide,
            perms: &vide,
        };
        let tout = Sujet {
            proprietaire: true,
            authentifie: true,
            roles: &roles,
            perms: &perms,
        };

        assert!(autorise(&Public, &anonyme));

        assert!(!autorise(&Authenticated, &anonyme));
        assert!(autorise(&Authenticated, &tout));

        assert!(!autorise(&Owner, &anonyme));
        assert!(autorise(&Owner, &tout));

        assert!(autorise(&role("lecteur"), &tout));
        assert!(!autorise(&role("membre"), &tout));
        assert!(autorise(
            &HasRole(vec!["membre".into(), "lecteur".into()]),
            &tout
        ));
        assert!(!autorise(&HasRole(vec![]), &tout), "HasRole vide : faux");

        assert!(autorise(&Perm("liste.exporter".into()), &tout));
        assert!(!autorise(&Perm("liste.vider".into()), &tout));
        assert!(!autorise(&Perm("liste.exporter".into()), &anonyme));

        // Tout ce que le maillage ne connaît pas est refusé, même au sujet le plus fort.
        for e in [Deny, ServiceOnly, AdminOnly, Resolver("x".into())] {
            assert!(!autorise(&e, &tout), "{e}");
        }

        assert!(autorise(&And(vec![Authenticated, Owner]), &tout));
        assert!(!autorise(&And(vec![Authenticated, Deny]), &tout));
        assert!(!autorise(&And(vec![]), &tout), "un And vide n'accorde rien");
        assert!(
            autorise(&Or(vec![Public, And(vec![])]), &anonyme),
            "un And vide imbriqué ne casse pas le reste"
        );
        assert!(!autorise(&Or(vec![Owner, And(vec![])]), &anonyme));
        assert!(autorise(&Not(Box::new(And(vec![]))), &anonyme));

        assert!(autorise(&Or(vec![Deny, Owner]), &tout));
        assert!(!autorise(&Or(vec![Deny, Owner]), &anonyme));
        assert!(!autorise(&Or(vec![]), &tout), "Or vide : faux");

        assert!(autorise(&Not(Box::new(Owner)), &anonyme));
        assert!(!autorise(&Not(Box::new(Owner)), &tout));
    }

    /// PIÈGE documenté : `Not` est une vraie négation. `not:deny` (et `not:admin_only`,
    /// `not:service_only`) accorde TOUT, même à un anonyme. Seul `not:resolver:x` reste refusé,
    /// un résolveur absent étant inévaluable.
    #[test]
    fn not_deny_accorde_tout_meme_a_un_anonyme() {
        let vide = BTreeSet::new();
        let anonyme = Sujet {
            proprietaire: false,
            authentifie: false,
            roles: &vide,
            perms: &vide,
        };
        for s in ["not:deny", "not:admin_only", "not:service_only"] {
            let e = PolicyExpr::parse(s).unwrap();
            assert!(autorise(&e, &anonyme), "{s}");
            assert!(
                e.may_admit_anonymous(),
                "{s} est repérable avant publication"
            );
        }
        assert!(!autorise(
            &PolicyExpr::parse("not:resolver:x").unwrap(),
            &anonyme
        ));
    }

    #[test]
    fn autorise_op_refuse_une_operation_absente() {
        let vide = BTreeSet::new();
        let tout = Sujet {
            proprietaire: true,
            authentifie: true,
            roles: &vide,
            perms: &vide,
        };
        let mut perm = PermDesc::default();
        perm.ops.insert(Operation::Get, PolicyExpr::Public);
        assert!(autorise_op(&perm, &Operation::Get, &tout));
        // Ni `authenticated` (défaut des opérations standard), ni rien : refus.
        assert!(!autorise_op(&perm, &Operation::Delete, &tout));
        assert!(!autorise_op(
            &perm,
            &Operation::Custom("liste.lire".into()),
            &tout
        ));
    }

    /// Le cas « liste de courses ». Le « ou » de rôles s'écrit `role:lecteur|membre` (un seul
    /// préfixe `role:`, noms séparés par `|`), ou `any(role:lecteur, role:membre)`.
    ///
    /// `role:lecteur|role:membre` donnait les rôles `lecteur` et `role:membre` (le `:` est
    /// permis dans un nom), donc un membre perdait la lecture sans erreur : c'est maintenant
    /// refusé à l'analyse, avec la forme juste dans le message.
    #[test]
    fn liste_de_courses() {
        let err = PolicyExpr::parse("role:lecteur|role:membre").unwrap_err();
        assert!(err.reason.contains("role:a|b"), "{err}");
        assert_eq!(
            PolicyExpr::parse("any(role:lecteur, role:membre)").unwrap(),
            PolicyExpr::Or(vec![role("lecteur"), role("membre")])
        );

        let op = |s: &str| Operation::Custom(s.into());
        let mut perm = PermDesc::default();
        perm.ops.insert(
            op("liste.lire"),
            PolicyExpr::parse("role:lecteur|membre").unwrap(),
        );
        perm.ops.insert(
            op("liste.ecrire"),
            PolicyExpr::parse("role:membre").unwrap(),
        );
        perm.ops
            .insert(op("liste.vider"), PolicyExpr::parse("owner").unwrap());
        assert_eq!(
            perm.ops[&op("liste.lire")],
            PolicyExpr::HasRole(vec!["lecteur".into(), "membre".into()])
        );
        assert_eq!(roles_de(&perm), ens(&["lecteur", "membre"]));

        let vide = BTreeSet::new();
        let lecteurs = ens(&["lecteur"]);
        let membres = ens(&["membre"]);
        let sujet = |proprietaire: bool, roles| Sujet {
            proprietaire,
            authentifie: true,
            roles,
            perms: &vide,
        };
        let peut = |s: &Sujet, o: &str| autorise_op(&perm, &op(o), s);

        let lecteur = sujet(false, &lecteurs);
        assert!(peut(&lecteur, "liste.lire"));
        assert!(!peut(&lecteur, "liste.ecrire"));
        assert!(!peut(&lecteur, "liste.vider"));

        let membre = sujet(false, &membres);
        assert!(peut(&membre, "liste.lire"));
        assert!(peut(&membre, "liste.ecrire"));
        assert!(!peut(&membre, "liste.vider"));

        // `owner` n'implique AUCUN rôle : le propriétaire sans rôle vide la liste mais ne la
        // lit pas. Un service qui veut que son propriétaire lise écrit
        // `any(owner, role:lecteur|membre)`.
        let proprietaire = sujet(true, &vide);
        assert!(peut(&proprietaire, "liste.vider"));
        assert!(!peut(&proprietaire, "liste.lire"));
        assert!(!peut(&proprietaire, "liste.ecrire"));
        let lire_aussi = PolicyExpr::parse("any(owner, role:lecteur|membre)").unwrap();
        assert!(autorise(&lire_aussi, &proprietaire));
    }
}
