//! **Le périmètre de lecture d'un demandeur**, adossé à l'IAM.
//!
//! # Pourquoi ce module existe
//!
//! Un relief, une vue multi-échelle et une chaîne d'identité disent *ce que la
//! base contient* et *comment elle est reliée*. Les servir à un appelant sans
//! tenir compte de sa décision IAM, c'est publier la structure entière à qui a
//! le droit d'en voir un fragment.
//!
//! Ce module relie les deux : le **contexte d'authentification** de la requête
//! devient `(principal, portée)`, la **décision matérialisée** fournit ses
//! permissions, et celles-ci deviennent un [`ReadScope`] — ce que l'appelant peut
//! lire, label par label, type de relation par type, échelle par échelle.
//!
//! # Le défaut est le refus
//!
//! Sans contexte d'authentification, ou sans décision matérialisée, le périmètre
//! est **vide** : l'appelant ne voit rien, et les compteurs de masquage le
//! disent. Un IAM qui ne répond pas ne doit pas ressembler à un IAM qui autorise.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::core::auth::AuthContext;
use crate::graph::routes::ScopeResolver;
use crate::server::middleware::authorization::PrincipalMapping;
use crate::storage::iam::Iam;
use crate::storage::permission::ReadScope;

/// Résout le périmètre d'un demandeur depuis sa décision IAM.
pub struct IamScopeResolver {
    iam: Iam,
    /// Le mappage d'identité de l'application, s'il en a fourni un.
    mapping: Option<Arc<PrincipalMapping>>,
    /// Le périmètre d'un appelant **sans contexte** — vide par défaut.
    anonymous: ReadScope,
    /// **Périmètres déjà construits**, par `(principal, portée, époque)`.
    ///
    /// Construire un périmètre demande d'**analyser** chaque permission et
    /// d'allouer ses chaînes : le banc mesurait 7,7 µs pour 67 permissions, à
    /// chaque requête. L'époque de la décision rend le cache **sûr** : elle est
    /// monotone, donc une décision modifiée porte une autre époque, donc une
    /// autre clé. Un cache qui ignorerait l'époque servirait un droit révoqué —
    /// le genre d'optimisation qui est une faille.
    cache: RwLock<HashMap<(String, String, u64), Arc<ReadScope>>>,
}

impl IamScopeResolver {
    /// Le résolveur : la décision du demandeur décide de ce qu'il lit.
    pub fn new(iam: Iam) -> Self {
        Self {
            iam,
            mapping: None,
            anonymous: ReadScope::none(),
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Utilise le mappage d'identité de l'application plutôt que le modèle par
    /// défaut de this-rs (`user_id` / `tenant_id`).
    pub fn with_mapping(mut self, mapping: Arc<PrincipalMapping>) -> Self {
        self.mapping = Some(mapping);
        self
    }

    /// **Change ce qu'un appelant anonyme peut lire.**
    ///
    /// Réservé aux déploiements qui l'assument : par défaut, pas de contexte =
    /// pas de données.
    pub fn with_anonymous_scope(mut self, scope: ReadScope) -> Self {
        self.anonymous = scope;
        self
    }

    /// L'IAM consulté (pour un diagnostic, ou un tableau de bord).
    pub fn iam(&self) -> &Iam {
        &self.iam
    }

    /// **Le périmètre du demandeur.**
    ///
    /// Trois cas, et deux d'entre eux refusent :
    ///
    /// 1. pas de contexte → [`ReadScope::none`] ;
    /// 2. contexte mais pas de décision matérialisée → [`ReadScope::none`] (un
    ///    principal inconnu n'a aucun droit, même s'il est « administrateur ») ;
    /// 3. décision trouvée → ses permissions, **filtrées** : celles que la
    ///    grammaire ne reconnaît pas sont ignorées, jamais interprétées au plus
    ///    large.
    pub fn scope_of(&self, context: Option<&AuthContext>) -> ReadScope {
        let Some(context) = context else {
            return self.anonymous.clone();
        };
        let Some((principal, scope)) = self.identity(context) else {
            return ReadScope::none();
        };
        match self.iam.decision(&principal, &scope) {
            Ok(Some(stamp)) => {
                // **Le cache est keyé par l'époque** : une décision modifiée a une
                // autre époque, donc une autre entrée.
                let cle = (principal.clone(), scope.clone(), stamp.epoch);
                if let Some(deja) = self.cache.read().expect("verrou").get(&cle) {
                    return (**deja).clone();
                }
                let permissions: Vec<&str> = stamp.permissions.iter().map(String::as_str).collect();
                let construit = ReadScope::from_permissions(permissions);
                self.cache
                    .write()
                    .expect("verrou")
                    .insert(cle, Arc::new(construit.clone()));
                construit
            }
            Ok(None) => ReadScope::none(),
            Err(error) => {
                // **Une erreur de lecture n'est pas une autorisation** : on
                // refuse, en le disant.
                tracing::warn!(
                    principal,
                    scope,
                    %error,
                    "décision IAM illisible : périmètre refusé (fail-closed)"
                );
                ReadScope::none()
            }
        }
    }

    /// `(principal, portée)` — le mappage de l'application, ou le modèle par
    /// défaut du framework.
    fn identity(&self, context: &AuthContext) -> Option<(String, String)> {
        if let Some(mapping) = &self.mapping {
            return mapping(context);
        }
        // **Le mappage du middleware**, pas une copie : une seule réponse à
        // « qui est cet appelant », donc une seule autorisation.
        crate::server::middleware::authorization::Authorization::default_iam_identity(context)
    }
}

impl ScopeResolver for IamScopeResolver {
    fn scope_for(&self, context: Option<&AuthContext>) -> ReadScope {
        self.scope_of(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::iam::RoleGrant;

    fn resolveur() -> (IamScopeResolver, uuid::Uuid, uuid::Uuid) {
        let store = crate::storage::ObrainStore::new_in_memory();
        let iam = store.iam();
        let (user, tenant) = (uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(9));
        iam.materialize(
            &user.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "lecteur".into(),
                permissions: vec![
                    "graph:read".into(),
                    "node:read:Group".into(),
                    "edge:read:CONTAINS".into(),
                    "echelle:read:groupe".into(),
                    // Ce qui n'est pas reconnu n'accorde rien.
                    "node:read:".into(),
                ],
            }],
        )
        .expect("matérialisation");
        (IamScopeResolver::new(iam), user, tenant)
    }

    fn contexte(user: uuid::Uuid, tenant: uuid::Uuid) -> AuthContext {
        AuthContext::User {
            user_id: user,
            tenant_id: tenant,
            roles: vec![],
        }
    }

    /// **La décision devient un périmètre** : les permissions granulaires de
    /// l'appelant décident de ce qu'il lit.
    #[test]
    fn the_decision_becomes_a_read_scope() {
        use crate::storage::permission::Scale;

        let (resolveur, user, tenant) = resolveur();
        let scope = resolveur.scope_of(Some(&contexte(user, tenant)));

        assert!(scope.may_read_node("Group"), "le label accordé est lisible");
        assert!(
            !scope.may_read_node("Secret"),
            "un autre label ne l'est pas"
        );
        assert!(scope.may_read_edge("CONTAINS"));
        assert!(!scope.may_create_edge("CONTAINS"), "lire n'est pas tisser");
        assert!(scope.allows_scale(Scale::Groupe));
        assert!(
            !scope.allows_scale(Scale::Noeud),
            "l'échelle est un plafond"
        );
        assert!(
            !scope.is_unrestricted(),
            "et surtout : ce n'est PAS un périmètre illimité"
        );
    }

    /// **Le défaut est le refus**, dans les trois cas qui doivent refuser.
    #[test]
    fn no_context_no_decision_and_an_unreadable_one_all_refuse() {
        let (resolveur, user, tenant) = resolveur();

        // 1. Pas de contexte.
        assert!(resolveur.scope_of(None).is_empty(), "anonyme : rien");

        // 2. Un contexte sans décision matérialisée — même administrateur.
        let inconnu = contexte(uuid::Uuid::from_u128(4242), tenant);
        assert!(
            resolveur.scope_of(Some(&inconnu)).is_empty(),
            "un principal sans décision n'a AUCUN droit"
        );
        // Un administrateur A un principal (`admin_id`) : c'est sa **décision**
        // qui manque, pas son identité. Il ne lit donc rien — comme les autres
        // tant qu'aucune décision ne l'autorise.
        let admin = AuthContext::Admin { admin_id: user };
        assert!(
            resolveur.scope_of(Some(&admin)).is_empty(),
            "sans décision, un administrateur ne lit rien : l'IAM décide, pas le rôle"
        );

        // 3. Une permission mal orthographiée n'ouvre rien de plus.
        let scope = resolveur.scope_of(Some(&contexte(user, tenant)));
        assert!(
            !scope.may_read_node(""),
            "la permission tronquée `node:read:` ne doit rien accorder"
        );
    }

    /// **Le cache ne sert jamais un droit révoqué.**
    ///
    /// C'est le risque de toute optimisation sur un chemin d'autorisation : le
    /// cache est keyé par l'**époque** de la décision, donc une révocation
    /// produit une autre clé. Ce test l'exige — sinon l'optimisation serait une
    /// faille, pas une amélioration.
    #[test]
    fn a_revoked_decision_is_never_served_from_the_cache() {
        let store = crate::storage::ObrainStore::new_in_memory();
        let iam = store.iam();
        let (user, tenant) = (uuid::Uuid::from_u128(5), uuid::Uuid::from_u128(9));
        iam.materialize(
            &user.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "lecteur".into(),
                permissions: vec!["node:read:Page".into()],
            }],
        )
        .expect("matérialisation");
        let resolveur = IamScopeResolver::new(store.iam());
        let ctx = contexte(user, tenant);

        // Première lecture : le droit existe.
        let avant = resolveur.scope_of(Some(&ctx));
        assert!(
            avant.may_read_node("Page"),
            "le droit est accordé au départ"
        );
        assert!(!avant.may_read_node("Secret"));

        // **Révocation** : on rematérialise sans la permission.
        iam.materialize(
            &user.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "lecteur".into(),
                permissions: vec!["node:read:Secret".into()],
            }],
        )
        .expect("rematérialisation");

        let apres = resolveur.scope_of(Some(&ctx));
        assert!(
            !apres.may_read_node("Page"),
            "un droit RÉVOQUÉ ne doit pas survivre au cache : {apres:?}"
        );
        assert!(
            apres.may_read_node("Secret"),
            "et le nouveau droit s'applique"
        );

        // Et une décision **invalidée** n'accorde plus rien : l'invalidation
        // incrémente l'époque et vide les permissions — fail-closed.
        resolveur
            .iam()
            .invalidate(&user.to_string(), &tenant.to_string())
            .expect("invalidation");
        let apres_invalidation = resolveur.scope_of(Some(&ctx));
        assert!(
            !apres_invalidation.may_read_node("Page")
                && !apres_invalidation.may_read_node("Secret"),
            "une décision invalidée ne doit plus rien accorder : {apres_invalidation:?}"
        );
    }

    /// Le mappage de l'application est respecté quand il est fourni.
    #[test]
    fn an_application_mapping_replaces_the_default_identity() {
        let store = crate::storage::ObrainStore::new_in_memory();
        let iam = store.iam();
        // Le principal est désigné par un AUTRE nom que le modèle par défaut.
        iam.materialize(
            "service-de-lecture",
            "portee-x",
            &[RoleGrant {
                role: "lecteur".into(),
                permissions: vec!["node:read:*".into()],
            }],
        )
        .expect("matérialisation");

        let mapping: Arc<PrincipalMapping> = Arc::new(|ctx: &AuthContext| match ctx {
            AuthContext::Service { .. } => {
                Some(("service-de-lecture".to_string(), "portee-x".to_string()))
            }
            _ => None,
        });
        let resolveur = IamScopeResolver::new(iam).with_mapping(mapping);

        let ctx = AuthContext::Service {
            service_name: "autre-nom".to_string(),
            tenant_id: Some(uuid::Uuid::from_u128(9)),
        };
        let scope = resolveur.scope_of(Some(&ctx));
        assert!(
            scope.may_read_node("N'importe"),
            "la décision est trouvée par le mappage de l'application : {scope:?}"
        );
    }

    /// Ce qu'un appelant anonyme peut lire est **décidable**, et vaut « rien »
    /// par défaut.
    #[test]
    fn the_anonymous_scope_is_explicit_and_empty_by_default() {
        let store = crate::storage::ObrainStore::new_in_memory();
        let resolveur = IamScopeResolver::new(store.iam());
        assert!(resolveur.scope_of(None).is_empty());

        let resolveur = IamScopeResolver::new(store.iam())
            .with_anonymous_scope(ReadScope::from_permissions(["node:read:Public"]));
        let scope = resolveur.scope_of(None);
        assert!(scope.may_read_node("Public"));
        assert!(!scope.may_read_node("Privé"), "et rien de plus");
    }
}
