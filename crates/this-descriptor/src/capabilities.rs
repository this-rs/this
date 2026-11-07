//! # Capacités d'un front, calculées depuis le contrat du module
//!
//! Une interface — Leptos dans un webview, une app mobile, un CLI — doit savoir ce qu'elle
//! peut **proposer** avant de le proposer. La réponse est déjà dans le descripteur :
//! chaque entité déclare une politique par opération ([`PermDesc::effective`]). Ce module
//! rejoue cette politique avec l'évaluateur du framework ([`crate::engine::policy`]) et
//! rend un verdict — à **trois** valeurs, parce qu'un front ne sait pas tout :
//!
//! - [`Capability::Allowed`] — la politique s'évalue à `true` ici ;
//! - [`Capability::Denied`] — elle s'évalue à `false` ;
//! - [`Capability::Unknown`] — elle dépend de ce que seul l'hôte peut répondre
//!   (`resolver:is_delegated`, une permission IAM matérialisée). Le front ne doit ni
//!   l'afficher comme autorisée, ni la masquer comme refusée.
//!
//! ```no_run
//! use this_descriptor::{Capabilities, Operation};
//!
//! # fn demo(desc: &this_descriptor::ModuleDescriptor) {
//! // Ce qu'un visiteur non authentifié peut faire, d'après le seul descripteur.
//! let caps = Capabilities::anonymous(desc);
//! let can_create = caps.allows("order", &Operation::Create);
//! println!("création de commande : {}", can_create.label());
//! # }
//! ```
//!
//! **Ce que ce n'est pas** : un contrôle d'accès. Le serveur reste juge (le middleware
//! d'autorisation de this-rs). Ces capacités servent à ne pas proposer un bouton qui sera
//! refusé — et à expliquer pourquoi il est grisé.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::policy::{PolicyEnv, evaluate};
use crate::module::ModuleDescriptor;
use crate::perm::{Operation, PrincipalView};

/// Le verdict d'une politique, du point de vue d'un front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Allowed,
    Denied,
    /// Dépend de l'hôte (resolver nommé, IAM) : ni autorisé, ni refusé.
    Unknown,
}

impl Capability {
    /// Libellé français, prêt à afficher.
    pub fn label(self) -> &'static str {
        match self {
            Capability::Allowed => "autorisé",
            Capability::Denied => "refusé",
            Capability::Unknown => "indéterminé",
        }
    }

    /// Seul `Allowed` justifie d'afficher l'action.
    pub fn is_allowed(self) -> bool {
        matches!(self, Capability::Allowed)
    }

    /// `true` si l'on sait, dans un sens ou dans l'autre.
    pub fn is_known(self) -> bool {
        !matches!(self, Capability::Unknown)
    }
}

impl From<Option<bool>> for Capability {
    /// La règle du framework : seul `Some(true)` autorise ; `None` n'est pas un refus,
    /// c'est une ignorance, et l'interface doit pouvoir le dire.
    fn from(verdict: Option<bool>) -> Self {
        match verdict {
            Some(true) => Capability::Allowed,
            Some(false) => Capability::Denied,
            None => Capability::Unknown,
        }
    }
}

/// Le genre de principal, tel que la surface le déclare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    /// Visiteur non authentifié.
    #[default]
    Anonymous,
    /// Utilisateur authentifié sans privilège particulier.
    User,
    /// Utilisateur authentifié administrateur.
    Admin,
    /// Appel service à service.
    Service,
}

impl SessionKind {
    pub fn is_authenticated(self) -> bool {
        !matches!(self, SessionKind::Anonymous)
    }

    pub fn label(self) -> &'static str {
        match self {
            SessionKind::Anonymous => "visiteur",
            SessionKind::User => "utilisateur",
            SessionKind::Admin => "administrateur",
            SessionKind::Service => "service",
        }
    }

    fn principal(self) -> PrincipalView<'static> {
        match self {
            SessionKind::Anonymous => PrincipalView::default(),
            SessionKind::User => PrincipalView {
                authenticated: true,
                ..Default::default()
            },
            SessionKind::Admin => PrincipalView {
                authenticated: true,
                admin: true,
                ..Default::default()
            },
            SessionKind::Service => PrincipalView {
                authenticated: true,
                service: true,
                ..Default::default()
            },
        }
    }
}

/// Un principal **sérialisable**, tel qu'une surface le transmet à un front.
///
/// Ce que la surface transmet décide de ce que le front peut calculer — et surtout de ce
/// qu'il ne peut pas :
///
/// | Transmis | Décidable côté front | Reste indéterminé |
/// |---|---|---|
/// | `kind` seul | `public`, `authenticated`, `admin_only`, `service_only`, `owner` | `role:…`, `perm:…` |
/// | `+ roles` | idem + `role:…` | `perm:…` |
/// | `+ grants` (`Some`) | idem + `perm:…` | `resolver:…` |
///
/// `grants: None` veut dire « la surface ne les transmet pas » : les politiques `perm:…`
/// restent alors **indéterminées**, jamais « refusées ». `grants: Some([])` veut dire
/// « ce principal n'en a aucune », ce qui est une information, pas une ignorance.
///
/// Asymétrie assumée avec `roles` : un rôle fait partie de l'identité et la surface doit
/// transmettre la liste **complète** (vide = aucun rôle). Les permissions IAM, elles, peuvent
/// être volumineuses ou calculées côté hôte : ne pas les transmettre est un choix, et
/// `PolicyExpr::Perm` devient alors indéterminé.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(default)]
    pub kind: SessionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants: Option<Vec<String>>,
}

impl Session {
    pub fn anonymous() -> Self {
        Self::default()
    }

    pub fn user() -> Self {
        Self {
            kind: SessionKind::User,
            ..Self::default()
        }
    }

    pub fn admin() -> Self {
        Self {
            kind: SessionKind::Admin,
            ..Self::default()
        }
    }

    pub fn service() -> Self {
        Self {
            kind: SessionKind::Service,
            ..Self::default()
        }
    }

    pub fn with_roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Déclare la liste **complète** des permissions IAM du principal.
    pub fn with_grants(mut self, grants: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.grants = Some(grants.into_iter().map(Into::into).collect());
        self
    }

    /// Ajoute une permission. Renvoie `false` si la surface ne transmet pas les permissions
    /// (on ne complète pas ce qu'on ne connaît pas) ou si elle était déjà là.
    pub fn grant(&mut self, permission: impl Into<String>) -> bool {
        match &mut self.grants {
            Some(grants) => {
                let permission = permission.into();
                if grants.contains(&permission) {
                    false
                } else {
                    grants.push(permission);
                    true
                }
            }
            None => false,
        }
    }

    pub fn has_grant(&self, permission: &str) -> Option<bool> {
        self.grants
            .as_ref()
            .map(|grants| grants.iter().any(|p| p == permission))
    }

    pub fn is_authenticated(&self) -> bool {
        self.kind.is_authenticated()
    }

    pub fn label(&self) -> &'static str {
        self.kind.label()
    }

    /// La vue pure du principal, empruntant les rôles de la session.
    pub fn principal(&self) -> PrincipalView<'_> {
        let mut who = self.kind.principal();
        who.roles = &self.roles;
        who
    }
}

/// Ce que la session permet de répondre, et rien de plus.
struct SessionEnv<'a> {
    grants: Option<&'a [String]>,
}

impl PolicyEnv for SessionEnv<'_> {
    fn resolver(&self, _: &str) -> Option<bool> {
        // Un resolver est une question posée à l'hôte : un front n'y répond jamais.
        None
    }

    fn permission(&self, permission: &str) -> Option<bool> {
        self.grants
            .map(|grants| grants.iter().any(|p| p == permission))
    }
}

/// Ce qu'une action exige de la surface : pouvoir fonctionner sans elle, ou pas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Requirement {
    /// Sans cette autorisation l'écran ne remplit pas sa fonction : on la demande avant.
    #[default]
    Required,
    /// L'écran fonctionne sans ; l'autorisation ajoute une action.
    Optional,
}

impl Requirement {
    pub fn label(self) -> &'static str {
        match self {
            Requirement::Required => "requis",
            Requirement::Optional => "facultatif",
        }
    }
}

/// Une capacité manquante, telle que l'interface doit la présenter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub entity: String,
    pub op: Operation,
    pub requirement: Requirement,
    pub verdict: Capability,
}

impl Gap {
    /// L'action est-elle indispensable à cet écran ?
    pub fn is_required(&self) -> bool {
        self.requirement == Requirement::Required
    }
}

/// Une demande d'autorisation, telle qu'un front la présente à sa surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub entity: String,
    pub op: Operation,
    #[serde(default)]
    pub requirement: Requirement,
    /// Pourquoi l'écran la demande : c'est ce que lit la personne qui décide.
    pub reason: String,
}

/// Ce que la surface répond à une demande.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Granted,
    Denied,
    /// En attente d'une décision humaine (validation par un administrateur).
    Pending,
}

impl PermissionDecision {
    pub fn granted(self) -> bool {
        matches!(self, PermissionDecision::Granted)
    }

    pub fn label(self) -> &'static str {
        match self {
            PermissionDecision::Granted => "accordée",
            PermissionDecision::Denied => "refusée",
            PermissionDecision::Pending => "en attente",
        }
    }
}

/// Les capacités d'une entité, opération par opération.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityCapabilities {
    pub entity: String,
    ops: BTreeMap<Operation, Capability>,
}

impl EntityCapabilities {
    pub fn get(&self, op: &Operation) -> Capability {
        self.ops.get(op).copied().unwrap_or(Capability::Unknown)
    }

    pub fn allows(&self, op: &Operation) -> bool {
        self.get(op).is_allowed()
    }

    pub fn can_create(&self) -> bool {
        self.allows(&Operation::Create)
    }

    pub fn can_update(&self) -> bool {
        self.allows(&Operation::Update)
    }

    pub fn can_delete(&self) -> bool {
        self.allows(&Operation::Delete)
    }

    /// Au moins une écriture est autorisée : le formulaire a un sens.
    pub fn writable(&self) -> bool {
        self.can_create() || self.can_update()
    }

    /// Tout est explicitement refusé (aucune ignorance) : l'entité est en consultation.
    pub fn is_read_only(&self) -> bool {
        [Operation::Create, Operation::Update, Operation::Delete]
            .iter()
            .all(|op| self.get(op) == Capability::Denied)
    }

    /// Toutes les opérations évaluées, dans l'ordre du vocabulaire.
    pub fn ops(&self) -> impl Iterator<Item = (&Operation, Capability)> {
        self.ops.iter().map(|(op, cap)| (op, *cap))
    }
}

/// Les capacités de toutes les entités d'un module.
///
/// `Default` = aucune entité connue : tout est [`Capability::Unknown`], ce qui est le bon
/// point de départ d'un front avant que le descripteur n'arrive.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    entities: BTreeMap<String, EntityCapabilities>,
}

impl Capabilities {
    /// Rejoue les politiques du descripteur pour un principal donné.
    pub fn of(desc: &ModuleDescriptor, who: &PrincipalView<'_>, env: &dyn PolicyEnv) -> Self {
        let mut entities = BTreeMap::new();
        for entity in &desc.entities {
            let mut ops = BTreeMap::new();
            for op in Operation::STANDARD.iter() {
                let policy = entity.permissions.effective(op);
                ops.insert(op.clone(), Capability::from(evaluate(&policy, who, env)));
            }
            entities.insert(
                entity.name.clone(),
                EntityCapabilities {
                    entity: entity.name.clone(),
                    ops,
                },
            );
        }
        Self { entities }
    }

    /// Un visiteur non authentifié : tout ce qu'un front peut déduire du seul descripteur.
    pub fn anonymous(desc: &ModuleDescriptor) -> Self {
        Self::for_session(desc, &Session::anonymous())
    }

    /// Un utilisateur authentifié, sans rôle ni permission connus.
    pub fn authenticated(desc: &ModuleDescriptor) -> Self {
        Self::for_session(desc, &Session::user())
    }

    /// Un administrateur authentifié.
    pub fn administrator(desc: &ModuleDescriptor) -> Self {
        Self::for_session(desc, &Session::admin())
    }

    /// Les capacités déduites d'une session transmise par la surface.
    ///
    /// Rôles et permissions IAM deviennent décidables si la session les porte ; un resolver
    /// reste indéterminé, parce que seul l'hôte peut y répondre.
    pub fn for_session(desc: &ModuleDescriptor, session: &Session) -> Self {
        let who = session.principal();
        let env = SessionEnv {
            grants: session.grants.as_deref(),
        };
        Self::of(desc, &who, &env)
    }

    pub fn entity(&self, name: &str) -> Option<&EntityCapabilities> {
        self.entities.get(name)
    }

    pub fn allows(&self, entity: &str, op: &Operation) -> Capability {
        self.entity(entity)
            .map(|e| e.get(op))
            .unwrap_or(Capability::Unknown)
    }

    /// Ce qui manque, pour une liste d'exigences déclarées par l'écran.
    ///
    /// L'écran dit ce dont il a besoin et à quel point ([`Requirement`]) ; le framework dit
    /// ce qui manque et pourquoi ([`Capability`]). Il faut les deux pour demander la bonne
    /// autorisation au bon moment — et pour ne pas demander celle qui est déjà accordée.
    pub fn gaps(&self, needs: &[(String, Operation, Requirement)]) -> Vec<Gap> {
        needs
            .iter()
            .filter_map(|(entity, op, requirement)| {
                let verdict = self.allows(entity, op);
                (verdict != Capability::Allowed).then(|| Gap {
                    entity: entity.clone(),
                    op: op.clone(),
                    requirement: *requirement,
                    verdict,
                })
            })
            .collect()
    }

    /// Les entités, dans l'ordre alphabétique.
    pub fn entities(&self) -> impl Iterator<Item = &EntityCapabilities> {
        self.entities.values()
    }

    /// Entités où au moins une écriture est autorisée.
    pub fn writable_entities(&self) -> Vec<&str> {
        self.entities
            .values()
            .filter(|e| e.writable())
            .map(|e| e.entity.as_str())
            .collect()
    }

    /// Entités dont la suppression est autorisée.
    pub fn deletable_entities(&self) -> Vec<&str> {
        self.entities
            .values()
            .filter(|e| e.can_delete())
            .map(|e| e.entity.as_str())
            .collect()
    }

    pub fn any_write(&self) -> bool {
        self.entities.values().any(|e| e.writable())
    }

    pub fn any_delete(&self) -> bool {
        self.entities.values().any(|e| e.can_delete())
    }

    /// Entités dont une capacité est `Unknown` : l'interface doit le signaler plutôt que
    /// de trancher.
    pub fn uncertain_entities(&self) -> Vec<&str> {
        self.entities
            .values()
            .filter(|e| e.ops().any(|(_, cap)| !cap.is_known()))
            .map(|e| e.entity.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::policy::NoEnv;
    use crate::entity::EntityDesc;
    use crate::perm::PolicyExpr;

    fn entity(name: &str, ops: &[(Operation, PolicyExpr)]) -> EntityDesc {
        let mut e = EntityDesc::new(name);
        for (op, policy) in ops {
            e.permissions.ops.insert(op.clone(), policy.clone());
        }
        e
    }

    fn desc(entities: Vec<EntityDesc>) -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new("shop", "0.1.0");
        d.entities = entities;
        d
    }

    fn p(s: &str) -> PolicyExpr {
        PolicyExpr::parse(s).unwrap()
    }

    /// Écritures explicitement refusées : l'entité ne peut rien changer.
    const DENIED_WRITES: [(Operation, PolicyExpr); 3] = [
        (Operation::Create, PolicyExpr::Deny),
        (Operation::Update, PolicyExpr::Deny),
        (Operation::Delete, PolicyExpr::Deny),
    ];

    #[test]
    fn default_policy_is_authenticated_so_anonymous_fronts_hide_writes() {
        // rien de déclaré : `effective` retombe sur `Authenticated`
        let d = desc(vec![EntityDesc::new("order")]);
        let caps = Capabilities::anonymous(&d);
        assert_eq!(caps.allows("order", &Operation::Create), Capability::Denied);
        assert!(!caps.any_write());

        let caps = Capabilities::authenticated(&d);
        assert_eq!(
            caps.allows("order", &Operation::Create),
            Capability::Allowed
        );
        assert!(caps.any_write());
    }

    #[test]
    fn public_operations_are_offered_to_anonymous_visitors() {
        let d = desc(vec![entity(
            "catalog",
            &[
                (Operation::List, p("public")),
                (Operation::Get, p("public")),
            ],
        )]);
        let caps = Capabilities::anonymous(&d);
        assert_eq!(
            caps.allows("catalog", &Operation::List),
            Capability::Allowed
        );
        assert_eq!(
            caps.allows("catalog", &Operation::Delete),
            Capability::Denied
        );
    }

    #[test]
    fn deny_is_denied_even_for_an_admin() {
        let d = desc(vec![entity("ledger", &[(Operation::Delete, p("deny"))])]);
        assert_eq!(
            Capabilities::administrator(&d).allows("ledger", &Operation::Delete),
            Capability::Denied
        );
    }

    #[test]
    fn role_policies_respect_the_principal_roles() {
        let d = desc(vec![entity(
            "invoice",
            &[(Operation::Update, p("role:accountant"))],
        )]);
        let roles = vec!["accountant".to_string()];
        let who = PrincipalView {
            authenticated: true,
            roles: &roles,
            ..Default::default()
        };
        assert_eq!(
            Capabilities::of(&d, &who, &NoEnv).allows("invoice", &Operation::Update),
            Capability::Allowed
        );

        let other = vec!["viewer".to_string()];
        let who = PrincipalView {
            authenticated: true,
            roles: &other,
            ..Default::default()
        };
        assert_eq!(
            Capabilities::of(&d, &who, &NoEnv).allows("invoice", &Operation::Update),
            Capability::Denied
        );
    }

    #[test]
    fn resolver_policies_are_unknown_without_a_host() {
        // un front ne peut pas évaluer `resolver:is_delegated` : il ne doit pas trancher.
        // Les DEUX écritures sont déclarées : sans ça, `Update` non déclaré retomberait
        // sur `Authenticated` et le test mesurerait autre chose que ce qu'il annonce.
        let d = desc(vec![entity(
            "order",
            &[
                (Operation::Create, p("resolver:is_delegated")),
                (Operation::Update, p("resolver:is_delegated")),
            ],
        )]);
        let caps = Capabilities::authenticated(&d);
        assert_eq!(
            caps.allows("order", &Operation::Create),
            Capability::Unknown
        );
        assert_eq!(caps.uncertain_entities(), vec!["order"]);
        // et surtout : « indéterminé » n'est pas « autorisé »
        assert!(!caps.any_write());
    }

    #[test]
    fn a_short_circuiting_public_arm_makes_the_policy_decidable() {
        // `any:public,resolver:x` s'évalue à true sans connaître le resolver
        let d = desc(vec![entity(
            "catalog",
            &[(Operation::List, p("any:public,resolver:x"))],
        )]);
        assert_eq!(
            Capabilities::anonymous(&d).allows("catalog", &Operation::List),
            Capability::Allowed
        );
    }

    #[test]
    fn negation_never_turns_ignorance_into_approval() {
        // règle du framework : `not:resolver:x` sans resolver reste indéterminé
        let d = desc(vec![entity(
            "export",
            &[(Operation::List, p("not:resolver:x"))],
        )]);
        assert_eq!(
            Capabilities::authenticated(&d).allows("export", &Operation::List),
            Capability::Unknown
        );
    }

    #[test]
    fn service_and_admin_policies_are_distinguished() {
        let d = desc(vec![
            entity("job", &[(Operation::Create, p("service_only"))]),
            entity("tenant", &[(Operation::Create, p("admin_only"))]),
        ]);
        let service = PrincipalView {
            service: true,
            ..Default::default()
        };
        let caps = Capabilities::of(&d, &service, &NoEnv);
        assert_eq!(caps.allows("job", &Operation::Create), Capability::Allowed);
        assert_eq!(
            caps.allows("tenant", &Operation::Create),
            Capability::Denied
        );
    }

    #[test]
    fn write_and_delete_aggregations_only_count_allowed_entities() {
        // Toutes les écritures sont déclarées : une opération non déclarée vaudrait
        // `Authenticated`, donc « autorisée » pour un principal connecté, et le test ne
        // mesurerait plus ce qu'il prétend mesurer.
        let d = desc(vec![
            entity("customer", &DENIED_WRITES),
            entity("order", &DENIED_WRITES),
            entity(
                "product",
                &[
                    (Operation::Create, p("authenticated")),
                    (Operation::Update, p("deny")),
                    (Operation::Delete, p("admin_only")),
                ],
            ),
        ]);
        let user = Capabilities::authenticated(&d);
        assert_eq!(user.writable_entities(), vec!["product"]);
        assert!(user.deletable_entities().is_empty());

        let admin = Capabilities::administrator(&d);
        assert_eq!(admin.writable_entities(), vec!["product"]);
        assert_eq!(admin.deletable_entities(), vec!["product"]);
    }

    #[test]
    fn read_only_entities_are_recognised() {
        let d = desc(vec![
            entity(
                "report",
                &[
                    (Operation::Create, p("deny")),
                    (Operation::Update, p("deny")),
                    (Operation::Delete, p("deny")),
                ],
            ),
            EntityDesc::new("order"),
        ]);
        let caps = Capabilities::administrator(&d);
        assert!(caps.entity("report").unwrap().is_read_only());
        assert!(!caps.entity("order").unwrap().is_read_only());
    }

    #[test]
    fn an_unknown_entity_answers_unknown_rather_than_panicking() {
        let d = desc(vec![EntityDesc::new("order")]);
        assert_eq!(
            Capabilities::anonymous(&d).allows("fantome", &Operation::List),
            Capability::Unknown
        );
    }

    #[test]
    fn sessions_map_to_the_expected_principals() {
        let d = desc(vec![
            entity("page", &[(Operation::List, p("public"))]),
            entity("order", &[(Operation::Create, p("authenticated"))]),
            entity("tenant", &[(Operation::Delete, p("admin_only"))]),
            entity("job", &[(Operation::Create, p("service_only"))]),
        ]);

        let anon = Capabilities::for_session(&d, &Session::anonymous());
        assert_eq!(anon.allows("page", &Operation::List), Capability::Allowed);
        assert_eq!(anon.allows("order", &Operation::Create), Capability::Denied);

        let user = Capabilities::for_session(&d, &Session::user());
        assert_eq!(
            user.allows("order", &Operation::Create),
            Capability::Allowed
        );
        assert_eq!(
            user.allows("tenant", &Operation::Delete),
            Capability::Denied
        );

        let admin = Capabilities::for_session(&d, &Session::admin());
        assert_eq!(
            admin.allows("tenant", &Operation::Delete),
            Capability::Allowed
        );

        let service = Capabilities::for_session(&d, &Session::service());
        assert_eq!(
            service.allows("job", &Operation::Create),
            Capability::Allowed
        );
    }

    #[test]
    fn session_is_serializable_for_a_surface_to_transmit() {
        assert!(
            serde_json::to_string(&Session::admin())
                .unwrap()
                .contains("\"admin\"")
        );
        let back: Session = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Session::anonymous());
        assert!(!Session::anonymous().is_authenticated());
        assert!(Session::user().is_authenticated());
    }

    #[test]
    fn iam_permissions_are_decidable_only_when_the_surface_sends_them() {
        let d = desc(vec![entity(
            "customer",
            &[(Operation::Delete, p("perm:crm.delete"))],
        )]);

        // la surface ne transmet rien : le front ne doit pas conclure « refusé »
        let silent = Capabilities::for_session(&d, &Session::user());
        assert_eq!(
            silent.allows("customer", &Operation::Delete),
            Capability::Unknown
        );

        // liste transmise, permission absente : refus argumenté
        let empty = Capabilities::for_session(&d, &Session::user().with_grants([] as [&str; 0]));
        assert_eq!(
            empty.allows("customer", &Operation::Delete),
            Capability::Denied
        );

        // permission accordée : autorisé
        let granted = Capabilities::for_session(&d, &Session::user().with_grants(["crm.delete"]));
        assert_eq!(
            granted.allows("customer", &Operation::Delete),
            Capability::Allowed
        );
    }

    #[test]
    fn granting_a_permission_changes_the_verdict_without_a_new_descriptor() {
        let d = desc(vec![entity(
            "customer",
            &[(Operation::Delete, p("perm:crm.delete"))],
        )]);
        let mut session = Session::user().with_grants([] as [&str; 0]);
        assert_eq!(
            Capabilities::for_session(&d, &session).allows("customer", &Operation::Delete),
            Capability::Denied
        );

        assert!(session.grant("crm.delete"));
        assert!(!session.grant("crm.delete"), "déjà accordée");
        assert_eq!(
            Capabilities::for_session(&d, &session).allows("customer", &Operation::Delete),
            Capability::Allowed
        );

        // sans liste transmise, on ne peut pas compléter ce qu'on ne connaît pas
        let mut silent = Session::user();
        assert!(!silent.grant("crm.delete"));
        assert_eq!(silent.has_grant("crm.delete"), None);
    }

    #[test]
    fn gaps_say_what_is_missing_and_how_much_it_matters() {
        let d = desc(vec![
            entity(
                "customer",
                &[
                    (Operation::Delete, p("perm:crm.delete")),
                    (Operation::Update, p("authenticated")),
                ],
            ),
            entity("product", &[(Operation::Delete, p("perm:catalog.delete"))]),
        ]);
        let caps = Capabilities::for_session(&d, &Session::user().with_grants([] as [&str; 0]));
        let needs = vec![
            (
                "customer".to_string(),
                Operation::Delete,
                Requirement::Required,
            ),
            (
                "customer".to_string(),
                Operation::Update,
                Requirement::Optional,
            ),
            (
                "product".to_string(),
                Operation::Delete,
                Requirement::Optional,
            ),
        ];

        let gaps = caps.gaps(&needs);
        assert_eq!(
            gaps.len(),
            2,
            "l'opération déjà autorisée n'est pas un manque"
        );
        assert!(gaps[0].is_required());
        assert_eq!(gaps[0].verdict, Capability::Denied);
        assert!(!gaps[1].is_required());
        assert!(
            gaps.iter()
                .all(|g| !(g.entity == "customer" && g.op == Operation::Update))
        );
    }

    #[test]
    fn every_policy_variant_is_decided_or_declared_unknown() {
        // Une entité par variante du vocabulaire : le front doit savoir trancher partout
        // où il peut, et dire « indéterminé » là où seul l'hôte répond.
        let policies = [
            ("public", p("public"), Capability::Allowed),
            ("authenticated", p("authenticated"), Capability::Allowed),
            ("admin", p("admin_only"), Capability::Denied),
            ("service", p("service_only"), Capability::Denied),
            ("deny", p("deny"), Capability::Denied),
            ("role", p("role:accountant"), Capability::Allowed),
            ("perm", p("perm:crm.delete"), Capability::Allowed),
            ("resolver", p("resolver:is_delegated"), Capability::Unknown),
            ("not_resolver", p("not:resolver:x"), Capability::Unknown),
            ("or_public", p("any:public,resolver:x"), Capability::Allowed),
            (
                "and_perm",
                p("all:authenticated,perm:crm.delete"),
                Capability::Allowed,
            ),
            // `owner` n'est pas connu avant d'avoir chargé la ligne : indéterminé, pas refusé
            ("owner", p("owner"), Capability::Unknown),
        ];

        let d = desc(
            policies
                .iter()
                .map(|(name, policy, _)| entity(name, &[(Operation::Delete, policy.clone())]))
                .collect(),
        );
        let session = Session::user()
            .with_roles(["accountant"])
            .with_grants(["crm.delete"]);
        let caps = Capabilities::for_session(&d, &session);

        for (name, _, expected) in &policies {
            assert_eq!(
                caps.allows(name, &Operation::Delete),
                *expected,
                "variante « {name} »"
            );
        }
    }

    #[test]
    fn a_policy_can_name_the_permissions_it_needs() {
        assert_eq!(p("perm:crm.delete").permissions(), vec!["crm.delete"]);
        assert_eq!(
            p("all:authenticated,perm:crm.delete").permissions(),
            vec!["crm.delete"]
        );
        assert_eq!(
            p("any:perm:a,perm:b,role:x").permissions(),
            vec!["a", "b"],
            "sans doublon, dans l'ordre de lecture"
        );
        assert!(p("authenticated").permissions().is_empty());
    }

    #[test]
    fn capabilities_round_trip_through_json() {
        let d = desc(vec![entity(
            "order",
            &[(Operation::Delete, p("admin_only"))],
        )]);
        let caps = Capabilities::administrator(&d);
        let raw = serde_json::to_string(&caps).unwrap();
        let back: Capabilities = serde_json::from_str(&raw).unwrap();
        assert_eq!(caps, back);
    }
}
