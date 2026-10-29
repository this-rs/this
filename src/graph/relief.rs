//! Relief d'obrain, décrit par introspection.
//!
//! # Doctrine : introspecté ou déclaré, jamais deviné
//!
//! Ce module ne tient **aucune** liste de procédures à la main. La section
//! `procedures` est engendrée depuis le registre du moteur lui-même :
//! `obrain_engine::procedures::registre().list()` rend, pour chaque procédure
//! enregistrée, son nom qualifié, sa description, ses paramètres (nom, type,
//! requis, défaut) et ses colonnes de sortie. Le relief suit donc obrain sans
//! qu'une ligne ne bouge ici quand obrain ajoute ou retire un algorithme.
//!
//! Mais une partie du relief **n'est pas** introspectable, et prétendre le
//! contraire serait la faute la plus coûteuse de ce module. D'où deux
//! marques explicites sur chaque élément :
//!
//! - [`Origin::Introspected`] — lu à l'exécution depuis le moteur. Fait.
//! - [`Origin::Declared`] — affirmé par this-rs, **parce qu'obrain n'offre
//!   aucune introspection**. À vérifier à la main si obrain change.
//!
//! Les familles de dispatch (`obrain.cognitive.*`, `obrain.engrams.*`,
//! `obrain.projection.*`, `obrain.window_*`, `db.*`, `obrain.search_*`) sont
//! dans le second cas : elles ne passent **pas** par le registre, donc
//! `registre().list()` — et `CALL obrain.procedures()` — les ignorent. Cette
//! liste est donc déclarée ici, et marquée comme telle.
//!
//! # Preuve empirique de la feature `algos`
//!
//! `obrain` n'expose pas ses features compilées à l'exécution. Un fait, en
//! revanche, est vérifiable : `obrain-engine::procedures` est gaté derrière
//! `algos` (`obrain-engine/src/lib.rs:44-45`). Donc **si `registre().list()`
//! rend des procédures, `algos` est compilé** — c'est une preuve, pas une
//! déclaration. Les autres features (`cognitive`, `cdc`, les dialectes) ne
//! sont pas prouvables de l'intérieur : elles restent déclarées, avec leur
//! provenance.
//!
//! # Coût
//!
//! Tout est O(1) ou O(registre) **sauf** `counts`, qui est un balayage
//! complet : obrain ne tient aucun compteur de simplexes (vérifié — aucun
//! `simplex_count` / `nb_simplexes` / `simplex_stats` dans `obrain-substrate`).
//! Le seul comptage disponible est `iter_simplices_kind(kind)?.len()`. Il est
//! donc **opt-in** ([`ReliefOptions::include_counts`]) et signalé comme tel.

use serde::{Deserialize, Serialize};

use crate::storage::ObrainStore;
use crate::storage::permission::ReadScope;

/// Identifiant de schéma du document — à faire évoluer si la forme change.
pub const RELIEF_SCHEMA: &str = "this-rs/graph-relief/1";

/// Features d'`obrain-engine` que ce build de this-rs active.
///
/// **Déclaré**, pas introspecté (cf. la doctrine du module) : cette liste
/// reflète `this/Cargo.toml` — les features par défaut d'`obrain-engine`
/// (`gql, parallel, wal, spill, mmap, regex`) plus `algos`, que this-rs
/// demande explicitement pour le registre et les algorithmes structurels.
/// Si ce fichier et le Cargo.toml divergent, c'est le Cargo.toml qui a raison.
pub const OBRAIN_ENGINE_FEATURES_DECLARED: &[&str] =
    &["gql", "parallel", "wal", "spill", "mmap", "regex", "algos"];

/// D'où vient une affirmation du document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Lu à l'exécution depuis le moteur obrain.
    Introspected,
    /// Affirmé par this-rs faute d'introspection disponible.
    Declared,
}

/// Options de construction du document.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReliefOptions {
    /// Calculer les compteurs de simplexes — **balayage complet**, opt-in.
    pub include_counts: bool,
}

/// Ce que le store sait de l'**identité canonique** des entités.
///
/// Le contrat **complet** — chaîne de motifs, repli, compteurs par label — est
/// publié par `GET /graph/identity`, qui dispose du registre d'identité. Ici on
/// ne publie que ce que le store sait vraiment : l'état de son index. Publier
/// les motifs depuis le store serait les **inventer**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRelief {
    /// Schéma d'identité en vigueur ([`crate::core::identity::ENTITY_ID_SCHEME`]).
    pub scheme: String,
    /// Entités actuellement indexées.
    pub index_len: usize,
    /// Espace de l'index (`None` = graphe par défaut).
    pub index_space: Option<String>,
    /// L'index porte-t-il sur l'espace **courant** ?
    ///
    /// `false` = l'index a été bâti dans un autre espace, ou jamais : ses
    /// entrées ne concernent **pas** le graphe interrogé. Sans ce champ, un
    /// `index_len` non nul dans le mauvais espace se lirait comme un fait.
    pub index_in_current_space: bool,
    /// Où lire le contrat complet.
    pub full_contract_at: String,
}

/// Bail d'écriture et fencing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRelief {
    /// Le fencing est-il **armé** ? `false` = comportement historique, aucun
    /// arbitrage : un second écrivain n'est pas refusé par this-rs.
    pub armed: bool,
    /// L'époque détenue — le jeton de fencing.
    pub epoch: Option<u64>,
    /// La clé du shard couvert (store **et** espace).
    pub shard: Option<String>,
    /// Qui délivre les baux.
    pub authority: Option<crate::storage::writer_lease::AuthorityKind>,
    /// Le contrôle a-t-il lieu **en entrée d'opération** ?
    ///
    /// Toujours `true` dans ce build, et c'est le point : côté obrain le WAL est
    /// écrit **avant** la mutation mmap, donc un contrôle au commit laisserait
    /// la trace du writer périmé déjà sur le disque.
    pub checked_at_operation_entry: bool,
}

/// Journal ordonné.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRelief {
    /// Un journal **durable** est-il possible sur ce store ?
    ///
    /// Conditionné à l'adossement au substrat : la zone journal est une zone du
    /// substrat.
    pub durable_available: bool,
    /// Le **transport** de réplication existe-t-il dans ce build ?
    ///
    /// `false` aujourd'hui : le journal est ordonné, idempotent et reprenable,
    /// mais rien n'expédie ses entrées vers un autre nœud. Publié pour qu'un
    /// lecteur ne confonde pas « journal là » avec « réplication en place ».
    pub transport_available: bool,
}

/// IAM matérialisé — **et le contrat de données pour le lire sans this-rs**.
///
/// Un tableau de bord obrain, un harnais, n'importe quel programme qui parle
/// obrain doit pouvoir lire l'état d'autorisation **sans lier this-rs** : les
/// décisions sont des nœuds et des arêtes ordinaires. Cette section publie donc
/// le schéma — labels, propriétés, arêtes, sens de chacun — au lieu de le
/// laisser deviner par lecture du code.
///
/// Ce qui est publié est un **contrat** : les noms ci-dessous sont ceux que le
/// framework écrit, et ils sont figés par des tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamRelief {
    /// Version du schéma des décisions ([`crate::storage::iam::IAM_SCHEMA`]).
    pub schema: String,
    /// L'index de décision (`acl_key`) existe-t-il sur ce store ?
    ///
    /// C'est la preuve qu'une décision d'autorisation peut être lue **par
    /// index** au lieu d'être recalculée par traversée.
    pub acl_index: bool,
    /// Nombre de décisions présentes (`Acl`), dans l'**espace actif**.
    pub decisions: usize,
    /// La propriété que l'application des politiques lit à chaque requête.
    ///
    /// Un tableau de bord qui veut montrer « ce que le framework regarde
    /// réellement » lit celle-ci, pas la liste JSON.
    pub hot_path_property: String,
    /// Les labels que le framework écrit.
    pub labels: Vec<IamSchemaEntry>,
    /// Les propriétés d'un nœud de décision.
    pub properties: Vec<IamSchemaEntry>,
    /// Les arêtes du graphe d'administration.
    pub edges: Vec<IamSchemaEntry>,
    /// La frontière de confiance, en clair.
    pub trust: Vec<String>,
    /// **Le vocabulaire des permissions granulaires** — la façon de donner un
    /// droit sur *ce* label, *ce* type de relation, *cet* espace.
    pub vocabulary: Vec<PermissionSpec>,
    /// La règle de correspondance, en une phrase.
    pub matching: String,
}

/// Une famille de permissions, publiée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSpec {
    /// La famille (`graph`, `node`, `edge`, `space`).
    pub family: String,
    /// Les actions reconnues.
    pub actions: Vec<String>,
    /// Ce que la cible désigne (`label`, `type d'arête`, `espace`, ou rien).
    pub target: String,
    /// Un exemple, prêt à recopier.
    pub example: String,
}

/// Une entrée du contrat publié : un nom, ce qu'il veut dire, et **qui l'écrit**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamSchemaEntry {
    /// Le nom écrit dans le store (label, propriété ou type d'arête).
    pub name: String,
    /// Sa forme ou ses extrémités (`text`, `json`, `int`, `Principal → Role`…).
    pub form: String,
    /// Ce qu'il signifie — et ce qu'un lecteur peut en conclure.
    pub meaning: String,
    /// `true` : le framework l'**écrit**. `false` : il le **lit** quand il est là
    /// (c'est alors à l'application de le tenir).
    pub written: bool,
}

/// **Application des politiques, par surface.**
///
/// Le framework déclare ici ce qu'il applique — et ce qu'il n'applique pas. Une
/// surface hors REST (GraphQL, gRPC, WebSocket) n'est pas résolue par route :
/// aucune politique `auth:` ne peut s'y rattacher, et le framework **refuse**
/// donc de la monter quand les politiques sont armées. C'est une **déclaration**
/// ([`Origin::Declared`]) : le relief ne peut pas savoir ce que l'application a
/// réellement monté, il publie ce qu'il en coûte de le monter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRelief {
    /// La surface (`rest`, `graphql`, `grpc`, `websocket`).
    pub surface: String,
    /// Les chemins qu'elle sert.
    pub mounted_at: Vec<String>,
    /// Applique-t-elle les politiques `auth:` ?
    pub enforces_policies: bool,
    /// Comment, ou pourquoi pas.
    pub mechanism: String,
    /// Ce qui la protège **à défaut** de politique.
    pub otherwise: String,
    /// Provenance de l'affirmation.
    pub origin: Origin,
}

/// **La forme du store** : ce que la base contient, pas seulement ce qu'elle sait faire.
///
/// Le relief publiait les **capacités** du moteur (procédures, familles,
/// simplexes) et les **contrats** du framework (identité, bail, journal, IAM) —
/// mais pas la **matière** : quels labels existent et combien de nœuds chacun
/// porte, quels types d'arêtes et en quelle quantité, quelles clés de propriétés,
/// quels index. C'est pourtant la première chose qu'on demande à une base.
///
/// Les valeurs viennent de l'introspection du moteur (`all_labels`,
/// `node_count_by_label`, `edge_counts_by_type`, `all_property_keys`,
/// `list_indexes`) — donc de la **base réelle**, pas d'un catalogue tenu à la
/// main. Ce qui n'est pas introspectable est **dit absent**, jamais estimé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreRelief {
    /// Nœuds, dans l'espace actif.
    pub nodes: usize,
    /// Arêtes, dans l'espace actif.
    pub edges: usize,
    /// Les labels et leur nombre de nœuds — du plus peuplé au moins peuplé.
    pub labels: Vec<CountedName>,
    /// Les types d'arêtes et leur nombre.
    pub edge_types: Vec<CountedName>,
    /// Le catalogue des clés de propriétés.
    pub property_keys: Vec<String>,
    /// Les index déclarés.
    pub indexes: Vec<IndexRelief>,
    /// Provenance : **introspecté** quand le moteur répond, sinon absent.
    pub origin: Origin,
    /// Labels **masqués** par le périmètre de lecture du demandeur.
    #[serde(default)]
    pub hidden_labels: usize,
    /// Types d'arêtes **masqués** par ce périmètre.
    #[serde(default)]
    pub hidden_edge_types: usize,
}

/// Un nom et son compte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountedName {
    /// Le nom (label ou type d'arête).
    pub name: String,
    /// Combien il y en a.
    pub count: usize,
}

/// Un index déclaré.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexRelief {
    /// Son nom.
    pub name: String,
    /// Son type (`hash`, `btree`, `fulltext`…).
    pub index_type: String,
    /// Sa cible (`Label:property`).
    pub target: String,
    /// Est-il unique ?
    pub unique: bool,
}

/// **Comment brancher un moteur d'inférence sur cette base.**
///
/// Un modèle — ou l'agent qui le pilote — n'a pas à deviner le protocole : il
/// lit ce manifeste, puis appelle. Il dit **comment s'authentifier**, **quoi
/// lire**, **quelles bornes** s'appliquent, et **sous quel régime** ces lectures
/// sont jugées.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceRelief {
    /// Ce qu'un lecteur doit présenter (`Authorization: Bearer <jeton>`), et ce
    /// que l'authentification en fait : un principal et une portée.
    pub authentication: String,
    /// Le mapping d'identité : comment `(principal, portée)` est dérivé.
    pub identity: String,
    /// **Les lectures, dans l'ordre où on les découvre.**
    pub reads: Vec<InferenceRead>,
    /// Les bornes appliquées à chaque lecture.
    pub limits: Vec<String>,
    /// **Le régime d'autorisation** de ces lectures.
    pub authorization: String,
    /// Les règles à respecter côté appelant.
    pub rules: Vec<String>,
}

/// Une lecture du manifeste.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceRead {
    /// Le chemin.
    pub path: String,
    /// Ce qu'elle rend — en une phrase, pour un modèle.
    pub returns: String,
    /// Pourquoi on l'appelle.
    pub purpose: String,
}

/// Le relief complet, tel que publié.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relief {
    /// Identifiant de schéma ([`RELIEF_SCHEMA`]).
    pub schema: String,
    /// Faits du moteur et du substrat.
    pub engine: EngineRelief,
    /// Espace (graphe nommé) actif et espaces connus.
    pub space: SpaceRelief,
    /// Procédures du registre — **introspectées**.
    pub procedures: Vec<ProcedureSpec>,
    /// Familles de dispatch hors registre — **déclarées**.
    pub families: Vec<FamilySpec>,
    /// Identité canonique des entités — ce que l'**index** sait, et rien de plus.
    pub identity: IdentityRelief,
    /// Bail d'écriture et jeton de fencing.
    pub lease: LeaseRelief,
    /// Journal ordonné.
    pub journal: JournalRelief,
    /// **La matière** : labels, arêtes, propriétés, index — la forme du store.
    pub store: StoreRelief,
    /// **Comment un moteur d'inférence se branche sur cette base.**
    pub inference: InferenceRelief,
    /// IAM matérialisé.
    pub iam: IamRelief,
    /// Application des politiques, surface par surface.
    pub surfaces: Vec<SurfaceRelief>,
    /// Limites et pièges connus, adossés à `fichier:ligne`.
    pub traps: Vec<Trap>,
    /// Notes de lecture (provenance, features, coûts).
    pub notes: Vec<String>,
}

/// **Le manifeste d'inférence** : le protocole, dit une fois, pour un moteur.
fn describe_inference() -> InferenceRelief {
    InferenceRelief {
        authentication: "`Authorization: Bearer <jeton>` — l'application décide du format ;                          le framework en dérive un principal et une portée (voir `identity`)"
            .to_string(),
        identity: "`(principal, portée)` : le mappage de l'application s'il en a fourni un,                    sinon le modèle de this-rs (`user_id`, `tenant_id`)"
            .to_string(),
        reads: vec![
            InferenceRead {
                path: "/graph".to_string(),
                returns: "le relief : la forme du store, les capacités du moteur, le contrat IAM"
                    .to_string(),
                purpose: "savoir ce que la base contient AVANT de la parcourir".to_string(),
            },
            InferenceRead {
                path: "/graph/hierarchy/self?echelle=canvas".to_string(),
                returns: "la descente depuis le SUJET de votre état d'interface, à SON échelle"
                    .to_string(),
                purpose: "lire ce que l'utilisateur a sous les yeux, sans qu'il traduise — le \
                          plafond de sa décision IAM s'applique"
                    .to_string(),
            },
            InferenceRead {
                path: "/graph/hierarchy/{id}?echelle=cosmos|monde|groupe|noeud".to_string(),
                returns: "les sous-groupes, à l'échelle demandée".to_string(),
                purpose: "descendre d'un cran à la fois — et demander une INTENTION plutôt \
                          qu'un nombre de niveaux".to_string(),
            },
            InferenceRead {
                path: "/graph/hierarchy/{id}?depth=N&limit=M".to_string(),
                returns: "la même descente, bornée explicitement".to_string(),
                purpose: "quand la profondeur exacte compte plus que l'échelle".to_string(),
            },
            InferenceRead {
                path: "/graph/hierarchy/{id}?format=outline".to_string(),
                returns: "la même vue en texte indenté, avec les adresses pour continuer"
                    .to_string(),
                purpose: "la forme qu'un modèle lit le mieux".to_string(),
            },
            InferenceRead {
                path: "/graph/identity".to_string(),
                returns: "le contrat d'identité ; `?counts=true` pour un comptage borné"
                    .to_string(),
                purpose: "reconnaître la même entité d'un moteur à l'autre".to_string(),
            },
            InferenceRead {
                path: "/graph/identity/{uuid}".to_string(),
                returns: "la chaîne d'identité d'une entité, octets canoniques compris"
                    .to_string(),
                purpose: "recalculer un identifiant de façon indépendante".to_string(),
            },
        ],
        limits: vec![
            "`depth` ≤ 32, `limit` ≤ 1000 par niveau (les valeurs hors bornes sont ramenées)"
                .to_string(),
            "`echelle` : `cosmos` (l'ensemble, agrégé) · `monde` (le graphe entier) ·              `groupe` (une branche et ses sous-groupes) · `noeud` (un nœud et son voisinage)              — une échelle inconnue est REFUSÉE, avec la liste des valides"
                .to_string(),
            "chaque niveau publie `total` et `truncated` : une coupe n'est JAMAIS silencieuse"
                .to_string(),
            "un cycle est signalé (`cycles`), jamais parcouru : la descente se termine toujours"
                .to_string(),
            "`echelle=canvas` et `self` lisent l'ÉTAT D'INTERFACE du demandeur : l'échelle qu'il \
             affiche et le sujet qu'il regarde. Refusés explicitement (401/404/501) quand \
             l'interface n'existe pas ou n'a rien déclaré — jamais interprétés comme un défaut"
                .to_string(),
            "le coût suit ce qui est montré, pas la taille de la base (parcours par adjacence)"
                .to_string(),
        ],
        authorization: "ces lectures s'ABSTIENNENT par défaut (descriptives, publiques). Si                         l'application déclare `graph.policy` (config/server.yaml), elles sont                         JUGÉES — 403 sans droit — ET FILTRÉES par la décision IAM du                         demandeur : il ne voit que les labels, les types de relations et les                         échelles que ses permissions couvrent"
            .to_string(),
        rules: vec![
            "n'énumérez jamais le graphe entier : descendez par `depth`, et arrêtez-vous quand              `children` vaut 0 ou quand `truncated` est vrai — il dit alors combien il reste"
                .to_string(),
            "traitez `children` comme la carte : il dit ce qui est en dessous SANS le charger"
                .to_string(),
            "une permission refusée est un refus, pas une absence de données : ne réessayez pas              sans droit"
                .to_string(),
            "ce que vous ne voyez pas est COMPTÉ quand c'est un nœud masqué par son label, et              seulement SIGNALÉ quand c'est une relation dont vous ne lisez pas le type : dans              ce cas le nombre n'est pas publié, il décrirait une structure hors périmètre"
                .to_string(),
        ],
    }
}

/// **La forme du store, sous un périmètre de lecture.**
///
/// Un lecteur ne voit que les labels dont il a le droit de lire les nœuds, et
/// les types d'arêtes dont il a le droit de lire les relations. Ce qui est
/// masqué est **compté** : même règle que la vue multi-échelle — le masquage est
/// visible, jamais déduit. Les totaux (`nodes`, `edges`) restent ceux de la base :
/// ils ne disent rien d'un label en particulier.
fn describe_store_scoped(store: &ObrainStore, scope: &ReadScope) -> StoreRelief {
    let active = store.active_store();

    let mut labels: Vec<CountedName> = active
        .all_labels()
        .into_iter()
        .map(|label| CountedName {
            count: active.node_count_by_label(&label),
            name: label,
        })
        .collect();
    labels.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));

    // **Le périmètre de lecture** : un label non lisible n'est pas publié — mais
    // il est compté, pour que le lecteur sache qu'on lui cache quelque chose.
    let mut hidden_labels = 0usize;
    if !scope.is_unrestricted() {
        let avant = labels.len();
        labels.retain(|l| scope.may_read_node(&l.name));
        hidden_labels = avant - labels.len();
    }

    // Les comptes d'arêtes ne sont pas garantis par tous les moteurs : quand le
    // moteur ne sait pas, on publie une liste vide — jamais une estimation.
    let mut edge_types: Vec<CountedName> = active
        .edge_counts_by_type()
        .map(|counts| {
            counts
                .into_iter()
                .map(|(name, count)| CountedName { name, count })
                .collect()
        })
        .unwrap_or_default();
    edge_types.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));

    let mut hidden_edge_types = 0usize;
    if !scope.is_unrestricted() {
        let avant = edge_types.len();
        edge_types.retain(|e| scope.may_read_edge(&e.name));
        hidden_edge_types = avant - edge_types.len();
    }

    let property_keys = active.all_property_keys();

    let indexes = store
        .db()
        .list_indexes()
        .into_iter()
        .map(|index| IndexRelief {
            name: index.name,
            index_type: index.index_type,
            target: index.target,
            unique: index.unique,
        })
        .collect();

    StoreRelief {
        nodes: active.node_count(),
        edges: active.edge_count(),
        labels,
        edge_types,
        property_keys,
        indexes,
        origin: Origin::Introspected,
        hidden_labels,
        hidden_edge_types,
    }
}

/// **Ce que le framework applique, surface par surface.**
///
/// Déclaré, jamais introspecté : le relief ne voit pas les routeurs que
/// l'application monte. Ce qu'il publie, c'est la règle du framework — et un
/// consommateur qui voit `enforces_policies: false` sait exactement ce qu'il
/// reste à sa charge.
fn describe_surfaces() -> Vec<SurfaceRelief> {
    let declared = |surface: &str,
                    mounted_at: &[&str],
                    enforces_policies: bool,
                    mechanism: &str,
                    otherwise: &str| SurfaceRelief {
        surface: surface.to_string(),
        mounted_at: mounted_at.iter().map(|s| (*s).to_string()).collect(),
        enforces_policies,
        mechanism: mechanism.to_string(),
        otherwise: otherwise.to_string(),
        origin: Origin::Declared,
    };

    vec![
        declared(
            "rest",
            &[
                "/{plural}",
                "/{plural}/{id}",
                "/{plural}/{id}/{route}",
                "/{plural}/{id}/{cible}/{id}",
                "/links/{id}",
                "/events/stream",
                "/notifications/{user_id}/…",
                "/device-tokens/{user_id}",
                "/graph…",
                "/graph/hierarchy/{id}",
                "/gdpr/…",
                "/auth/…",
                "/ui/state/{portee}",
                "/health",
            ],
            true,
            "middleware d'autorisation : pluriel + méthode + nombre de segments → opération → politique",
            "les chemins hors routes d'entité s'abstiennent : /health et /auth/* sont publics par nature, /graph/* est descriptif SAUF si `graph.policy` est déclarée (config/server.yaml) — il est alors jugé comme une route d'entité, ce qui permet à un moteur d'inférence de lire la base sous IAM, /gdpr/* exige un administrateur dans le handler, /notifications/* et /device-tokens/* comparent l'identifiant du chemin à celui de l'appelant, /ui/state/* de même (l'état d'interface est une ressource PERSONNELLE : la portée du chemin n'est jamais crue sur parole), /events/stream est borné au locataire de l'appelant",
        ),
        declared(
            "graphql",
            &[
                "/graphql",
                "/graphql/playground",
                "/graphql/schema",
                "/graphql/ws",
            ],
            false,
            "aucune : les opérations GraphQL ne sont pas résolues par route",
            "le framework REFUSE de monter la surface quand les politiques sont armées ; l'application doit accepter explicitement (`build_router_unenforced`), et l'acceptation est journalisée",
        ),
        declared(
            "grpc",
            &["/* (tonic)", "/grpc/*"],
            false,
            "aucune : les services gRPC ne sont pas résolus par route",
            "même refus, même acceptation explicite (`build_router_unenforced`, `build_with_grpc_unenforced`)",
        ),
        declared(
            "websocket",
            &["/ws"],
            false,
            "aucune : un canal n'est pas une route",
            "même refus, même acceptation explicite (`build_router_unenforced`)",
        ),
        declared(
            "frontend",
            &["/* (repli)", "assets, SPA, /__dev/*"],
            false,
            "aucune : c'est un repli, il n'est pas résolu par route",
            "ATTENTION À L'ORDRE : `Frontend::attach(app)` remplace le repli, donc s'il est appelé APRÈS vos couches, ni l'authentification ni l'autorisation ne voient le SPA ni `/__dev` (mesuré : `tests/frontend_surface.rs`). Montez le repli AVANT d'appliquer les couches. Les fichiers statiques sont publics par nature",
        ),
        declared(
            "dev_bridge",
            &["/__dev", "/__dev/command*"],
            false,
            "aucune — et c'est un pont de COMMANDES : il exécute la commande de reconstruction sur demande HTTP",
            "FERMÉ par défaut : `THIS_DEV_COMMANDS` doit valoir vrai pour l'ouvrir (auparavant `THIS_DEV=true` suffisait), et l'ouverture est journalisée en avertissement. À réserver au développement local",
        ),
    ]
}

/// Une entrée du contrat IAM publié.
fn entry(name: &str, form: &str, meaning: &str, written: bool) -> IamSchemaEntry {
    IamSchemaEntry {
        name: name.to_string(),
        form: form.to_string(),
        meaning: meaning.to_string(),
        written,
    }
}

/// Faits du moteur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineRelief {
    /// Le store est-il adossé au substrat (`SubstrateStore`) ?
    ///
    /// C'est la condition de tout le relief topologique : simplexes, courbure,
    /// énergie, HKS, clustering. Surprise vérifiée : depuis le cutover T17,
    /// **même `new_in_memory()` est substrat-backed** — une configuration en
    /// mémoire ouvre un substrat temporaire auto-nettoyé
    /// (`obrain-engine/src/database/mod.rs:799-808`). Ce champ est donc
    /// aujourd'hui toujours `true`, et il le reste par contrat : c'est lui qui
    /// autorise ou non la suite du document.
    pub substrate: bool,
    /// Store ouvert en lecture seule.
    pub read_only: bool,
    /// Le registre de procédures a-t-il rendu au moins une entrée ?
    ///
    /// **Preuve empirique** que la feature `algos` d'`obrain-engine` est
    /// compilée : `procedures` n'existe que sous ce `cfg`.
    pub registry_available: bool,
    /// Couche simpliciale (triangles, cliques, hyperarêtes).
    pub simplex: SimplexRelief,
}

/// État de la couche simpliciale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimplexRelief {
    /// Les passes simpliciales sont-elles configurées sur ce store ?
    ///
    /// `false` = la zone est vide par construction ; les compteurs à `0` ne
    /// signifieraient alors pas « pas de triangle », mais « jamais calculé ».
    pub configured: bool,
    /// La maintenance incrémentale au commit est-elle active ?
    ///
    /// Quand elle l'est, chaque création/suppression d'arête met à jour les
    /// simplexes **dans la même transaction WAL** (`obrain-substrate/src/store.rs:16154`
    /// et `:16342`) : le relief reste frais sous les écritures de this-rs,
    /// sans tâche batch.
    pub maintenance: bool,
    /// Type d'arête des cliques (id de dictionnaire, **pas** un nom : obrain
    /// ne rend pas le dictionnaire par cette API).
    pub clique_edge_type_id: Option<u16>,
    /// Masque des labels autorisés aux extrémités des cliques.
    pub clique_label_mask: u64,
    /// Une passe DISPATCH (hyperarêtes de groupe) est-elle configurée ?
    pub dispatch_configured: bool,
    /// Coefficient de clustering global — **non publié**.
    ///
    /// Obrain l'expose (`SubstrateStore::clustering_global()`) mais l'appel
    /// panique en build debug dès qu'un nœud vivant a un degré nul dans le
    /// type d'arête des cliques (défaut `store.rs:10859`, voir `traps`). Tant
    /// qu'il n'est pas corrigé, le document ne le publie pas plutôt que de
    /// risquer un panic dans une requête HTTP.
    pub clustering_global: Option<f64>,
    /// Compteurs — présents seulement si demandés **et** calculables.
    pub counts: Option<SimplexCounts>,
    /// Pourquoi les compteurs sont absents, le cas échéant.
    pub counts_error: Option<String>,
}

/// Compteurs de simplexes vivants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimplexCounts {
    /// Triangles (3-cliques).
    pub triangles: u64,
    /// Cliques K4.
    pub k4: u64,
    /// Cliques K5.
    pub k5: u64,
    /// Hyperarêtes DISPATCH.
    pub hyperedges: u64,
}

/// Espace (graphe nommé) — la dimension « spacieux ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceRelief {
    /// Espace actif — `None` = graphe par défaut.
    pub current: Option<String>,
    /// Espaces connus, triés.
    pub graphs: Vec<String>,
}

/// Une procédure **du registre**, telle que le moteur la décrit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureSpec {
    /// Nom qualifié pour `CALL`, ex. `obrain.pagerank`.
    pub name: String,
    /// Description issue de l'algorithme.
    pub description: String,
    /// Paramètres déclarés par l'algorithme.
    pub parameters: Vec<ParameterSpec>,
    /// Colonnes de sortie canoniques.
    pub output_columns: Vec<String>,
    /// Toujours [`Origin::Introspected`] ici.
    pub origin: Origin,
    /// Réserves calculées sur cette entrée (cf. `warnings` dans le code).
    pub warnings: Vec<String>,
}

/// Un paramètre de procédure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterSpec {
    /// Nom du paramètre.
    pub name: String,
    /// Type (`integer`, `float`, `string`, `boolean`, `node_id`).
    pub kind: String,
    /// Requis ?
    pub required: bool,
    /// Valeur par défaut, si déclarée.
    pub default: Option<String>,
    /// Description.
    pub description: String,
}

/// Une famille de procédures qui **ne passe pas** par le registre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilySpec {
    /// Nom de la famille, ex. `cognitive`.
    pub family: String,
    /// Noms `CALL` acceptés (préfixe `obrain.` implicite pour la plupart).
    pub names: Vec<String>,
    /// Features d'`obrain-engine` exigées par cette famille.
    pub requires: Vec<String>,
    /// Cette famille est-elle disponible dans ce build ?
    ///
    /// Calculé en confrontant `requires` à
    /// [`OBRAIN_ENGINE_FEATURES_DECLARED`] — donc **déclaré**, pas prouvé.
    pub available: bool,
    /// Toujours [`Origin::Declared`].
    pub origin: Origin,
    /// Réserve éventuelle (ex. noms annoncés mais non implémentés).
    pub note: Option<String>,
}

/// Une limite connue du relief, adossée à une preuve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trap {
    /// Identifiant stable.
    pub id: String,
    /// L'énoncé.
    pub statement: String,
    /// Où c'est vérifiable dans obrain (`fichier:ligne`).
    pub evidence: String,
}

// ── Faits déclarés (hors introspection) ────────────────────────────────

/// Familles de dispatch hors registre.
///
/// Source : `obrain-engine/src/query/planner/lpg/mutation.rs:743-937`, qui
/// ordonne neuf familles dont une seule (le registre, `:891`) est
/// introspectable. Les huit autres sont ci-dessous.
/// Une famille de dispatch : `(nom, préfixes, portées, note)`.
type FamilyRow = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    Option<&'static str>,
);

const FAMILIES: &[FamilyRow] = &[
    (
        "projection",
        &[
            "obrain.projection.create",
            "obrain.projection.drop",
            "obrain.projection.list",
        ],
        &["algos"],
        None,
    ),
    (
        "window",
        &["obrain.window_2d", "obrain.window_2d_energy"],
        &["algos"],
        None,
    ),
    (
        "substrate",
        &[
            "obrain.search_by_role",
            "obrain.search_causes",
            "obrain.search_effects",
            "obrain.reachable_count",
        ],
        &["algos"],
        None,
    ),
    (
        "cdc",
        &[
            "obrain.search_delta",
            "obrain.search_surprise",
            "obrain.relief",
            "obrain.search_conflicts",
        ],
        &["algos", "cdc"],
        None,
    ),
    (
        "cognitive",
        &[
            "obrain.cognitive.spread",
            "obrain.cognitive.distill",
            "obrain.cognitive.search",
            "obrain.cognitive.detect_gaps",
            "obrain.cognitive.synthesize",
            "obrain.cognitive.reinforce",
        ],
        &["algos", "cognitive"],
        None,
    ),
    (
        "engrams",
        &[
            "obrain.engrams.list",
            "obrain.engrams.inspect",
            "obrain.engrams.forget",
        ],
        &["algos", "cognitive", "engram"],
        Some(
            "`obrain.engrams.recall` est souvent supposé exister : il n'est PAS câblé \
             (seuls list/inspect/forget le sont, \
             obrain-engine/src/cognitive_procedures.rs:88-99)",
        ),
    ),
    (
        "catalog",
        &["db.labels", "db.relationshipTypes", "db.propertyKeys"],
        &["algos"],
        Some(
            "`db.schema` et `db.indexes` sont annoncés en docstring \
             (mutation.rs:1069-1070) mais n'ont AUCUNE branche d'implémentation : \
             pour le schéma et les index, passer par ObrainDB::schema() / \
             list_indexes(), pas par CALL",
        ),
    ),
    (
        "user-defined",
        &["<catalogue de procédures utilisateur>"],
        &["algos"],
        None,
    ),
];

/// Limites vérifiées du relief.
const TRAPS: &[(&str, &str, &str)] = &[
    (
        "identity-index-is-in-memory-and-scoped",
        "L'index d'identité vit EN MÉMOIRE : il repart de zéro à chaque ouverture \
         du store et à chaque bascule d'espace. Un `index_len` non nul dans un \
         espace qui n'est pas le courant ne concerne PAS le graphe interrogé — \
         d'où `index_in_current_space`.",
        "this/src/storage/obrain.rs — invalidate_caches ;          this/src/storage/obrain_identity.rs — rebuild_identity_index_capped",
    ),
    (
        "unarmed-lease-means-no-arbitration",
        "Sans bail armé, RIEN n'empêche deux écrivains sur le même store — pas \
         même dans un seul processus : obrain rend la part de verrou existante, \
         donc deux ouvertures du même répertoire réussissent toutes les deux.",
        "obrain-substrate/src/file.rs:179-207 ; this/src/storage/writer_lease.rs",
    ),
    (
        "journal-has-no-transport",
        "Le journal est ordonné, idempotent et reprenable, mais aucun transport \
         ne l'expédie vers un autre nœud : `transport_available: false`. Ne pas \
         confondre « journal durable là » avec « réplication en place ».",
        "this/src/events/log_obrain.rs — subscribe (sondage, pas de notification)",
    ),
    (
        "registry-is-not-exhaustive",
        "Le registre — et donc `CALL obrain.procedures()` — n'énumère QUE les \
         algorithmes. Les familles de dispatch (cognitive, engrams, projection, \
         window, cdc, substrate, catalog) n'y figurent jamais : il n'existe \
         aucune source unique du relief côté obrain.",
        "obrain-engine/src/procedures.rs:107-120 ; mutation.rs:743-937",
    ),
    (
        "non-literal-arguments-ignored",
        "Les arguments de procédure non constants sont ignorés SILENCIEUSEMENT : \
         `CALL obrain.pagerank({damping: $d})` ne pose aucun paramètre et ne lève \
         aucune erreur. Une surface qui accepte des paramètres doit refuser les \
         paramètres liés, ou injecter les littéraux elle-même.",
        "obrain-engine/src/procedures.rs:301-311 (`_ => {}`)",
    ),
    (
        "kcore-column-mismatch",
        "`obrain.kcore` est incohérent : `KCoreAlgorithm::name()` rend `\"kcore\"` \
         alors que la table de colonnes indexe `\"k_core\"`. Sans `YIELD` \
         explicite, les colonnes annoncées sont donc le repli par défaut \
         (`node_id`, `value`), pas les colonnes réellement produites.",
        "obrain-adapters/.../structure.rs:484 ; obrain-engine/src/procedures.rs:252,268",
    ),
    (
        "no-o1-simplex-counter",
        "obrain ne tient aucun compteur de simplexes. Tout comptage est un \
         balayage complet de la zone simplex : une surface HTTP doit mettre en \
         cache, pas recalculer par requête.",
        "obrain-substrate/src/store.rs:9998 (seul `iter_simplices_kind`)",
    ),
    (
        "no-ricci-getter",
        "Il n'existe aucun accesseur `ricci` sur `SubstrateStore` : la courbure \
         se lit par `writer().read_edge(slot)` puis `EdgeRecord::ricci_f32()`. \
         La courbure de Forman, elle, est CALCULÉE mais NON PERSISTÉE.",
        "obrain-substrate/src/geometric.rs:1007-1013 ; record.rs:341-342",
    ),
    (
        "in-memory-is-substrate-backed",
        "`ObrainDB::new_in_memory()` n'est PAS un moteur sans substrat : depuis le \
         cutover T17, une configuration en mémoire ouvre un substrat temporaire \
         (auto-nettoyé à la destruction). Le relief est donc disponible même sur \
         un store « en mémoire » — l'intuition inverse est fausse. Le vrai \
         interrupteur du relief topologique n'est pas la nature du store, c'est \
         la CONFIGURATION DES PASSES.",
        "obrain-engine/src/database/mod.rs:799-808",
    ),
    (
        "simplex-data-is-config-dependent",
        "Les primitives simpliciales existent toujours, mais les DONNÉES \
         seulement si les passes ont été configurées : \
         `passes_simpliciales_config()` rend `Ok(None)` sinon. Un compteur à 0 \
         doit donc être distingué d'un « jamais calculé ».",
        "obrain-substrate/src/store.rs:10482",
    ),
    (
        "maintenance-is-private",
        "La maintenance incrémentale des simplexes est entièrement privée \
         (hooks internes aux mutations d'arêtes) : elle ne se déclenche pas à la \
         demande. Les seules entrées publiques sont la configuration des passes \
         et la passe complète `materialiser_simplexes()`.",
        "obrain-substrate/src/store.rs:10090, 10231 (privés) ; :16154, :16342 (appelants)",
    ),
    (
        "clustering-global-panics-in-debug",
        "`SubstrateStore::clustering_global()` n'est pas appelable sans risque : il \
         calcule `Σ_v k(v)·(k(v)−1)/2` et **panique en build debug** dès qu'un nœud \
         vivant a un degré nul dans le type d'arête des cliques — `k − 1` \
         sous-déborde alors un `u64`. En release, le même calcul passe par un wrap \
         et rend un résultat correct par accident, ce qui masque le défaut. C'est \
         pourquoi ce document ne publie PAS le clustering (champ laissé à `null`) : \
         un panic dans une requête HTTP coûterait plus cher que l'information.",
        "obrain-substrate/src/store.rs:10859 (`triplets += k * (k - 1) / 2`) — \
         panic reproductible ; garde `k == 0` absente",
    ),
    (
        "obrain-cli-is-bin-only",
        "`obrain-cli` n'a AUCUNE cible `lib` : tout son code — dont les outils \
         MCP `structural_roles`, `node_curvature`, `viz_topo` et les extracteurs \
         substrate — est inatteignable depuis un autre crate. Ce qui n'est pas \
         dans `obrain-engine` / `-substrate` / `-core` / `-viz` / `-indexer` est \
         à réécrire, pas à importer.",
        "obrain-cli/Cargo.toml:11-13 (pas de src/lib.rs) ; main.rs:23-33",
    ),
    (
        "features-not-exposed",
        "obrain n'expose pas ses features compilées à l'exécution : la \
         disponibilité des familles hors registre est DÉCLARÉE par this-rs, pas \
         prouvée. Seule la feature `algos` est prouvable — parce que le registre \
         n'existe pas sans elle.",
        "obrain-engine/src/lib.rs:44-45 ; this/src/graph/relief.rs",
    ),
];

/// L'identité, vue par l'index du store.
fn describe_identity(store: &ObrainStore) -> IdentityRelief {
    let index_space = store.identity_space();
    let current = store.space();
    IdentityRelief {
        scheme: crate::core::identity::ENTITY_ID_SCHEME.to_string(),
        index_len: store.identity_len(),
        index_in_current_space: index_space == current,
        index_space,
        full_contract_at: "/graph/identity".to_string(),
    }
}

/// Le bail d'écriture, s'il est armé.
fn describe_lease(store: &ObrainStore) -> LeaseRelief {
    let info = store.lease_info();
    LeaseRelief {
        armed: info.is_some(),
        epoch: info.as_ref().map(|i| i.epoch),
        shard: info.as_ref().map(|i| i.shard.clone()),
        authority: info.and_then(|i| i.authority),
        checked_at_operation_entry: true,
    }
}

impl Relief {
    /// Décrit le relief du store.
    ///
    /// Ne rend jamais d'erreur : un fait indisponible est un fait `false` ou
    /// `None` accompagné de sa raison, jamais une exception. C'est le contrat
    /// d'honnêteté du document — un consommateur doit pouvoir le lire même sur
    /// un store vide, en mémoire, ou en lecture seule.
    pub fn describe(store: &ObrainStore, opts: ReliefOptions) -> Self {
        Self::describe_scoped(store, opts, &ReadScope::all())
    }

    /// **Le relief sous un périmètre de lecture.**
    ///
    /// La forme du store n'est alors publiée que pour ce que le demandeur a le
    /// droit de lire : ses permissions décident des labels et des types de
    /// relations visibles, et ce qui est masqué est **compté**. C'est ce que le
    /// handler appelle dès qu'un résolveur de périmètre est branché.
    pub fn describe_scoped(store: &ObrainStore, opts: ReliefOptions, scope: &ReadScope) -> Self {
        let substrate_store = store.db().substrate_handle();
        let substrate = substrate_store.is_some();

        let (simplex, mut notes) = describe_simplex(substrate_store.as_ref(), opts);

        // Les procédures : introspection pure. `registre()` n'existe que sous
        // la feature `algos` d'obrain-engine, donc une liste non vide EST la
        // preuve que cette feature est compilée.
        let procedures: Vec<ProcedureSpec> = obrain_engine::procedures::registre()
            .list()
            .into_iter()
            .map(ProcedureSpec::from_info)
            .collect();
        let registry_available = !procedures.is_empty();

        if !registry_available {
            notes.push(
                "Registre de procédures vide : la feature `algos` d'obrain-engine \
                 n'est pas compilée dans ce build."
                    .to_string(),
            );
        }
        if !substrate {
            notes.push(
                "Store sans `SubstrateStore` : aucun relief topologique \
                 (simplexes, courbure, énergie, clustering) n'est disponible. \
                 Cas défensif — depuis T17, tout store obrain est substrat-backed."
                    .to_string(),
            );
        }

        notes.push(format!(
            "Section `procedures` : introspectée depuis le registre du moteur \
             ({} entrées). Section `families` : déclarée par this-rs, car le \
             registre ne couvre pas les familles de dispatch.",
            procedures.len()
        ));
        notes.push(
            "Les arguments de procédure non littéraux sont ignorés sans erreur par \
             obrain (voir traps)."
                .to_string(),
        );

        notes.push(
            "Section `identity` : l'index vit EN MÉMOIRE et repart de zéro à chaque              ouverture du store, ainsi qu'à chaque bascule d'espace. Le contrat complet              (chaîne de motifs, repli, compteurs par label) est publié par              `GET /graph/identity`, seule surface qui dispose du registre."
                .to_string(),
        );
        let lease = describe_lease(store);
        if !lease.armed {
            notes.push(
                "Section `lease` : fencing NON armé — un second écrivain sur le même                  store (y compris dans ce processus) n'est pas refusé par this-rs.                  Armer avec `ObrainStore::enable_write_fencing_with`."
                    .to_string(),
            );
        }
        if !substrate {
            notes.push(
                "Section `journal` : store sans substrat, donc sans zone journal —                  aucun journal durable n'est possible ici."
                    .to_string(),
            );
        }

        Relief {
            schema: RELIEF_SCHEMA.to_string(),
            engine: EngineRelief {
                substrate,
                read_only: store.is_read_only(),
                registry_available,
                simplex,
            },
            space: SpaceRelief {
                current: store.space(),
                graphs: store.spaces(),
            },
            identity: describe_identity(store),
            lease: describe_lease(store),
            journal: JournalRelief {
                durable_available: substrate,
                transport_available: false,
            },
            store: describe_store_scoped(store, scope),
            inference: describe_inference(),
            iam: {
                use crate::storage::iam;
                IamRelief {
                    schema: iam::IAM_SCHEMA.to_string(),
                    acl_index: store
                        .db()
                        .has_property_index(crate::storage::iam::ACL_KEY_PROP),
                    // **Espace actif** : le comptage doit porter sur ce que le
                    // lecteur voit, pas sur la racine.
                    decisions: store.active_store().node_count_by_label(iam::ACL_LABEL),
                    hot_path_property: iam::ACL_PERMS_BLOB_PROP.to_string(),
                    labels: vec![
                        entry(
                            iam::ACL_LABEL,
                            "nœud de décision",
                            "un nœud par couple (principal, portée) : la décision matérialisée",
                            true,
                        ),
                        entry(
                            iam::PRINCIPAL_LABEL,
                            "nœud",
                            "le principal, pour la traversée et la traçabilité — il ne PORTE pas la décision",
                            true,
                        ),
                        entry(
                            "Role",
                            "nœud",
                            "un rôle nommé — LU si présent (la traversée de référence s'en sert), ÉCRIT par l'application si elle gère ses rôles en graphe",
                            false,
                        ),
                        entry(
                            "Permission",
                            "nœud",
                            "une permission nommée — même contrat que `Role`",
                            false,
                        ),
                    ],
                    properties: vec![
                        entry(
                            iam::ACL_KEY_PROP,
                            "text",
                            "clé du couple (portée, principal), jointe par un US (0x1F) — indexée",
                            true,
                        ),
                        entry(
                            iam::ACL_SCOPE_PROP,
                            "text",
                            "la portée (locataire, organisation…)",
                            true,
                        ),
                        entry(
                            iam::ACL_PRINCIPAL_PROP,
                            "text",
                            "le principal",
                            true,
                        ),
                        entry(
                            iam::ACL_ROLES_PROP,
                            "json (liste de chaînes)",
                            "les rôles effectifs, dédupliqués et triés",
                            true,
                        ),
                        entry(
                            iam::ACL_PERMS_PROP,
                            "json (liste de chaînes)",
                            "les permissions effectives — la forme PUBLIÉE, à lire pour afficher",
                            true,
                        ),
                        entry(
                            iam::ACL_PERMS_BLOB_PROP,
                            "text (permissions encadrées par des US)",
                            "la forme du chemin chaud : ce que l'application des politiques lit à chaque requête",
                            true,
                        ),
                        entry(
                            iam::ACL_EPOCH_PROP,
                            "int",
                            "l'époque, strictement croissante : l'invalidation ne recule jamais",
                            true,
                        ),
                        entry(
                            iam::ACL_DIGEST_PROP,
                            "text (empreinte du contenu)",
                            "empreinte des rôles et permissions ; vidée à l'invalidation",
                            true,
                        ),
                    ],
                    edges: vec![
                        entry(
                            "HAS_ROLE",
                            "Principal → Role",
                            "LU si présent : c'est par ces arêtes que la traversée de référence calcule les permissions. La décision matérialisée, elle, ne les écrit pas — elle porte les rôles en propriété",
                            false,
                        ),
                        entry(
                            "GRANTS",
                            "Role → Permission",
                            "LU si présent : ce que le rôle accorde",
                            false,
                        ),
                    ],
                    vocabulary: vec![
                    PermissionSpec {
                        family: "graph".to_string(),
                        actions: vec!["read".to_string()],
                        target: "aucune (la base entière)".to_string(),
                        example: "graph:read".to_string(),
                    },
                    PermissionSpec {
                        family: "node".to_string(),
                        actions: vec![
                            "read".to_string(),
                            "write".to_string(),
                            "delete".to_string(),
                        ],
                        target: "un label, ou `*`".to_string(),
                        example: "node:read:Page".to_string(),
                    },
                    PermissionSpec {
                        family: "edge".to_string(),
                        actions: vec![
                            "read".to_string(),
                            "create".to_string(),
                            "delete".to_string(),
                        ],
                        target: "un type d'arête, ou `*`".to_string(),
                        example: "edge:create:CITES  (TISSER : le droit de relier)".to_string(),
                    },
                    PermissionSpec {
                        family: "echelle".to_string(),
                        actions: vec!["read".to_string()],
                        target: "une échelle : cosmos | monde | groupe | noeud".to_string(),
                        example: "echelle:read:groupe  (voir JUSQU'AU groupe, pas le détail d'un \
                                  nœud)"
                            .to_string(),
                    },
                    PermissionSpec {
                        family: "space".to_string(),
                        actions: vec!["use".to_string(), "create".to_string(), "drop".to_string()],
                        target: "un espace nommé".to_string(),
                        example: "space:use:lab".to_string(),
                    },
                ],
                matching: "une permission DÉTENUE accorde une permission DEMANDÉE si les familles                            et les actions sont égales, et si la cible détenue est `*` ou égale à                            la cible demandée. `*` accorde tout ; `famille:*` accorde toutes les                            actions de cette famille ; une permission sans cible ne couvre PAS une                            demande avec cible (`node:read` n'accorde pas `node:read:Page`). Ce qui                            n'est pas reconnu n'accorde rien. Un droit d'ÉCHELLE est un \
                           PLAFOND : `echelle:read:groupe` accorde cosmos, monde et groupe, \
                           mais pas `noeud`."
                    .to_string(),
                trust: vec![
                        "Qui peut écrire dans le store peut écrire une décision, donc accorder des permissions : la protection est la maîtrise du store et le bail d'écriture, pas une signature.".to_string(),
                        "Deux nœuds portant la même `acl_key` rendent la décision ambiguë : elle est REFUSÉE, jamais résolue par l'ordre de l'index.".to_string(),
                        "Lire ce graphe demande un droit de lecture sur le store : la liste des décisions dit qui peut quoi.".to_string(),
                    ],
                }
            },
            surfaces: describe_surfaces(),
            procedures,
            families: describe_families(),
            traps: TRAPS
                .iter()
                .map(|(id, statement, evidence)| Trap {
                    id: (*id).to_string(),
                    statement: (*statement).to_string(),
                    evidence: (*evidence).to_string(),
                })
                .collect(),
            notes,
        }
    }

    /// Nombre de procédures introspectées.
    pub fn procedure_count(&self) -> usize {
        self.procedures.len()
    }

    /// Cherche une procédure du registre par nom qualifié (`obrain.pagerank`).
    pub fn procedure(&self, name: &str) -> Option<&ProcedureSpec> {
        self.procedures.iter().find(|p| p.name == name)
    }
}

impl ProcedureSpec {
    /// Convertit une entrée du registre en spec publiable.
    fn from_info(info: obrain_engine::procedures::ProcedureInfo) -> Self {
        let name = info.name;
        let output_columns = info.output_columns;

        // Réserve calculée : `output_columns_for` (obrain-engine/src/procedures.rs:192)
        // rend `["node_id", "value"]` en repli quand `algo.name()` n'est pas
        // dans sa table. On ne peut pas décider depuis ici si c'est le repli ou
        // une vraie paire — mais on peut le signaler, ce qui vaut mieux qu'un
        // document qui affirme des colonnes fausses (cas réel : `obrain.kcore`).
        let mut warnings = Vec::new();
        if output_columns.len() == 2
            && output_columns[0] == "node_id"
            && output_columns[1] == "value"
        {
            warnings.push(
                "Les colonnes annoncées sont celles du repli par défaut : l'algorithme \
                 peut en produire d'autres. Utiliser un `YIELD` explicite."
                    .to_string(),
            );
        }
        if name == "obrain.kcore" {
            warnings.push(
                "Cas connu : `KCoreAlgorithm::name()` rend `kcore` alors que la table \
                 de colonnes indexe `k_core` (structure.rs:484 vs procedures.rs:252)."
                    .to_string(),
            );
        }

        ProcedureSpec {
            name,
            description: info.description,
            parameters: info
                .parameters
                .into_iter()
                .map(|p| ParameterSpec {
                    name: p.name,
                    kind: param_kind(p.param_type),
                    required: p.required,
                    default: p.default,
                    description: p.description,
                })
                .collect(),
            output_columns,
            origin: Origin::Introspected,
            warnings,
        }
    }
}

/// Nom lisible d'un type de paramètre.
fn param_kind(t: obrain_adapters::plugins::ParameterType) -> String {
    use obrain_adapters::plugins::ParameterType as P;
    match t {
        P::Integer => "integer",
        P::Float => "float",
        P::String => "string",
        P::Boolean => "boolean",
        P::NodeId => "node_id",
    }
    .to_string()
}

/// Construit la liste des familles, en confrontant leurs exigences aux
/// features déclarées.
fn describe_families() -> Vec<FamilySpec> {
    FAMILIES
        .iter()
        .map(|(family, names, requires, note)| {
            let available = requires
                .iter()
                .all(|r| OBRAIN_ENGINE_FEATURES_DECLARED.contains(r));
            FamilySpec {
                family: (*family).to_string(),
                names: names.iter().map(|n| (*n).to_string()).collect(),
                requires: requires.iter().map(|r| (*r).to_string()).collect(),
                available,
                origin: Origin::Declared,
                note: note.map(str::to_string),
            }
        })
        .collect()
}

/// Lit l'état de la couche simpliciale, et — si demandé — ses compteurs.
fn describe_simplex(
    substrate: Option<&std::sync::Arc<obrain_substrate::SubstrateStore>>,
    opts: ReliefOptions,
) -> (SimplexRelief, Vec<String>) {
    let mut notes = Vec::new();

    let Some(store) = substrate else {
        return (
            SimplexRelief {
                configured: false,
                maintenance: false,
                clique_edge_type_id: None,
                clique_label_mask: 0,
                dispatch_configured: false,
                clustering_global: None,
                counts: None,
                counts_error: Some("store non adossé au substrat".to_string()),
            },
            notes,
        );
    };

    let cfg = match store.passes_simpliciales_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            notes.push(format!("passes_simpliciales_config() a échoué : {e}"));
            None
        }
    };

    let mut relief = SimplexRelief {
        configured: cfg.is_some(),
        maintenance: cfg.as_ref().is_some_and(|c| c.maintenance_active()),
        clique_edge_type_id: cfg.as_ref().map(|c| c.clique_edge_type),
        clique_label_mask: cfg.as_ref().map_or(0, |c| c.clique_label_mask),
        dispatch_configured: cfg.as_ref().is_some_and(|c| c.dispatch.is_some()),
        clustering_global: None,
        counts: None,
        counts_error: None,
    };

    if opts.include_counts {
        if !relief.configured {
            relief.counts_error = Some(
                "passes simpliciales non configurées : la zone est vide par \
                 construction (un compteur à 0 ne signifierait pas « aucun \
                 triangle » mais « jamais calculé »)"
                    .to_string(),
            );
        } else {
            match count_simplexes(store) {
                Ok(counts) => {
                    notes.push(
                        "Compteurs obtenus par balayage complet de la zone simplex : \
                         obrain ne tient aucun compteur O(1)."
                            .to_string(),
                    );
                    relief.counts = Some(counts);
                }
                Err(e) => relief.counts_error = Some(format!("balayage simplex échoué : {e}")),
            }
        }
    }

    (relief, notes)
}

/// Compte les simplexes vivants, par balayage.
fn count_simplexes(store: &obrain_substrate::SubstrateStore) -> Result<SimplexCounts, String> {
    use obrain_substrate::simplex_kind;
    let count = |kind: u8| -> Result<u64, String> {
        store
            .iter_simplices_kind(kind)
            .map(|v| v.len() as u64)
            .map_err(|e| e.to_string())
    };
    Ok(SimplexCounts {
        triangles: count(simplex_kind::TRIANGLE)?,
        k4: count(simplex_kind::K4)?,
        k5: count(simplex_kind::K5)?,
        hyperedges: count(simplex_kind::HYPEREDGE)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le document doit se construire sur un store en mémoire SANS paniquer —
    /// et dire la vérité vérifiée : ce store EST substrat-backed (T17), mais sa
    /// couche simpliciale n'est pas configurée.
    #[test]
    fn in_memory_store_is_substrate_backed_but_unconfigured() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());

        assert_eq!(relief.schema, RELIEF_SCHEMA);
        assert!(
            relief.engine.substrate,
            "depuis T17, une config en mémoire ouvre un substrat temporaire"
        );
        assert!(
            !relief.engine.simplex.configured,
            "les passes simpliciales ne sont pas configurées par défaut"
        );
        assert!(
            !relief.engine.simplex.maintenance,
            "sans configuration, aucune maintenance n'est active"
        );
        assert!(
            relief
                .notes
                .iter()
                .all(|n| !n.contains("sans `SubstrateStore`")),
            "la note « pas de substrat » ne doit pas apparaître ici : elle serait fausse"
        );
    }

    /// Le compteur demandé sur un store non configuré doit être absent AVEC sa
    /// raison — jamais un `0` qu'on lirait comme « aucun triangle ».
    #[test]
    fn counts_on_unconfigured_simplex_layer_are_explained() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(
            &store,
            ReliefOptions {
                include_counts: true,
            },
        );

        assert!(relief.engine.simplex.counts.is_none());
        let reason = relief
            .engine
            .simplex
            .counts_error
            .as_deref()
            .expect("la raison doit être publiée");
        assert!(
            reason.contains("non configurées"),
            "la raison doit nommer la configuration des passes, pas un compteur nul : {reason}"
        );
    }

    /// Aucune liste de procédures n'est tenue à la main : elle vient du
    /// registre. Si cette assertion échoue, c'est que la feature `algos`
    /// d'obrain-engine n'est plus compilée (voir Cargo.toml).
    #[test]
    fn procedures_are_introspected_from_the_engine_registry() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());

        let engine_registry = obrain_engine::procedures::registre().list().len();
        assert_eq!(
            relief.procedures.len(),
            engine_registry,
            "le relief doit refléter exactement le registre du moteur"
        );
        assert!(
            relief.engine.registry_available,
            "registre vide : la feature `algos` est absente"
        );
        assert!(
            relief
                .procedures
                .iter()
                .all(|p| p.origin == Origin::Introspected),
            "toute procédure du registre est introspectée"
        );
        assert!(
            relief
                .procedures
                .iter()
                .all(|p| p.name.starts_with("obrain.")),
            "les noms du registre sont qualifiés `obrain.`"
        );
    }

    /// Les procédures connues sont présentes, avec leurs colonnes — et le cas
    /// `kcore` est signalé comme limite plutôt qu'affirmé.
    #[test]
    fn known_procedures_carry_columns_and_warnings() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());

        let pagerank = relief
            .procedure("obrain.pagerank")
            .expect("obrain.pagerank doit être au registre");
        assert_eq!(pagerank.output_columns, vec!["node_id", "score"]);
        assert!(
            pagerank.parameters.iter().any(|p| p.name == "damping"),
            "pagerank déclare `damping`"
        );

        let leiden = relief
            .procedure("obrain.leiden")
            .expect("obrain.leiden doit être au registre");
        assert_eq!(
            leiden.output_columns,
            vec!["node_id", "community_id", "modularity"]
        );

        let kcore = relief
            .procedure("obrain.kcore")
            .expect("obrain.kcore doit être au registre");
        assert!(
            !kcore.warnings.is_empty(),
            "l'incohérence kcore/k_core doit être signalée dans le document"
        );
    }

    /// Les familles hors registre sont déclarées, et leur disponibilité suit
    /// les features déclarées — pas une supposition.
    #[test]
    fn families_are_declared_with_their_gates() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());

        assert!(!relief.families.is_empty());
        assert!(
            relief.families.iter().all(|f| f.origin == Origin::Declared),
            "les familles hors registre sont déclarées, jamais présentées comme introspectées"
        );

        let cognitive = relief
            .families
            .iter()
            .find(|f| f.family == "cognitive")
            .expect("la famille cognitive doit être déclarée");
        assert!(
            !cognitive.available,
            "la feature `cognitive` d'obrain-engine n'est pas activée par this-rs"
        );
        assert!(cognitive.requires.contains(&"cognitive".to_string()));

        let projection = relief
            .families
            .iter()
            .find(|f| f.family == "projection")
            .expect("la famille projection doit être déclarée");
        assert!(
            projection.available,
            "projection ne demande que `algos`, qui est activé"
        );
    }

    /// Les pièges sont publiés avec leur preuve, et les cas vérifiés sont là.
    #[test]
    fn traps_are_published_with_evidence() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());

        assert!(relief.traps.len() >= 5);
        assert!(
            relief.traps.iter().all(|t| !t.evidence.is_empty()),
            "un piège sans preuve vérifiable n'a pas sa place ici"
        );
        let ids: Vec<&str> = relief.traps.iter().map(|t| t.id.as_str()).collect();
        for expected in [
            "registry-is-not-exhaustive",
            "non-literal-arguments-ignored",
            "no-o1-simplex-counter",
            "obrain-cli-is-bin-only",
        ] {
            assert!(ids.contains(&expected), "piège manquant : {expected}");
        }
    }

    /// Le document est sérialisable — c'est un artefact publiable, pas un
    /// objet interne.
    #[test]
    fn relief_is_serializable() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());
        let json = serde_json::to_value(&relief).expect("sérialisation");
        assert_eq!(json["schema"], RELIEF_SCHEMA);
        assert!(json["procedures"].is_array());
        assert!(json["families"].is_array());
        assert!(json["traps"].is_array());
        // Les origines sont des chaînes stables, pas des entiers d'enum.
        assert_eq!(json["procedures"][0]["origin"], "introspected");
    }
    // ── Sections neuves : identité, bail, journal, IAM ───────────────────

    #[test]
    fn the_identity_section_publishes_what_the_store_actually_knows() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());

        assert_eq!(
            relief.identity.scheme,
            crate::core::identity::ENTITY_ID_SCHEME
        );
        assert_eq!(relief.identity.index_len, 0, "aucun balayage n'a eu lieu");
        assert_eq!(relief.identity.index_space, None);
        assert!(
            relief.identity.index_in_current_space,
            "aucun espace n'est actif : l'index (vide) porte bien sur le courant"
        );
        assert_eq!(relief.identity.full_contract_at, "/graph/identity");
        // Le contrat COMPLET n'est pas inventé ici : il n'est pas dans ce document.
        let json = serde_json::to_value(&relief).expect("sérialisation");
        assert!(json["identity"].get("motives").is_none());
    }

    #[test]
    fn an_unarmed_lease_says_so_instead_of_looking_safe() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());
        assert!(!relief.lease.armed);
        assert_eq!(relief.lease.epoch, None);
        assert_eq!(relief.lease.authority, None);
        assert!(
            relief.lease.checked_at_operation_entry,
            "la conception est publiée : le contrôle est en entrée d'opération"
        );
        assert!(
            relief.notes.iter().any(|n| n.contains("fencing NON armé")),
            "un bail désarmé doit être DIT, pas laissé silencieux"
        );
        assert!(
            relief
                .traps
                .iter()
                .any(|t| t.id == "unarmed-lease-means-no-arbitration")
        );
    }

    #[test]
    fn an_armed_lease_publishes_its_epoch_shard_and_authority() {
        use crate::storage::writer_lease::FileAuthority;

        let dir = std::env::temp_dir().join(format!("this_relief_lease_{}", uuid::Uuid::new_v4()));
        let store = ObrainStore::open(&dir).unwrap();
        let authority = FileAuthority::new(&dir);
        store.enable_write_fencing_with(&authority).unwrap();

        let relief = Relief::describe(&store, ReliefOptions::default());
        assert!(relief.lease.armed);
        assert_eq!(relief.lease.epoch, Some(1));
        assert_eq!(
            relief.lease.authority,
            Some(crate::storage::writer_lease::AuthorityKind::File)
        );
        assert!(
            relief.lease.shard.as_ref().is_some_and(|s| s.len() == 16),
            "la clé de shard est publiée : {:?}",
            relief.lease.shard
        );
        assert!(
            !relief.notes.iter().any(|n| n.contains("fencing NON armé")),
            "armé : la note d'alerte disparaît"
        );

        store.release_write_fencing();
        let after = Relief::describe(&store, ReliefOptions::default());
        assert!(!after.lease.armed, "relâcher se voit dans le relief");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_journal_section_admits_there_is_no_transport() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());
        assert!(
            relief.journal.durable_available,
            "un store substrat-backed peut porter un journal durable"
        );
        assert!(
            !relief.journal.transport_available,
            "aucun transport n'existe : le dire, plutôt que le laisser croire"
        );
        assert!(
            relief
                .traps
                .iter()
                .any(|t| t.id == "journal-has-no-transport")
        );
    }

    #[test]
    fn the_iam_section_reports_the_decision_index() {
        use crate::storage::iam::ACL_KEY_PROP;

        let store = ObrainStore::new_in_memory();
        let before = Relief::describe(&store, ReliefOptions::default());
        assert!(
            !before.iam.acl_index,
            "sans plan IAM, aucune décision indexée n'est possible"
        );

        let _iam = store.iam();
        let after = Relief::describe(&store, ReliefOptions::default());
        assert!(
            after.iam.acl_index,
            "le plan IAM a posé l'index ({ACL_KEY_PROP})"
        );
    }

    #[test]
    fn the_new_sections_survive_a_json_roundtrip() {
        let store = ObrainStore::new_in_memory();
        let relief = Relief::describe(&store, ReliefOptions::default());
        let json = serde_json::to_string(&relief).expect("sérialisation");
        let back: Relief = serde_json::from_str(&json).expect("désérialisation");
        assert_eq!(back, relief);
        for key in ["identity", "lease", "journal", "iam"] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "section {key} absente"
            );
        }
    }

    /// **Le relief lui-même est filtré** : un lecteur ne voit que les labels et
    /// les types de relations qu'il a le droit de lire — et ce qui est masqué est
    /// **compté**, jamais silencieux.
    #[test]
    fn the_store_shape_is_filtered_by_the_read_scope() {
        use crate::storage::permission::ReadScope;

        let store = ObrainStore::new_in_memory();
        {
            let session = store.db().session();
            let a = session.create_node_with_props(
                &["Page"],
                [("qualified_name", obrain_common::types::Value::from("a"))],
            );
            let b = session.create_node_with_props(
                &["Secret"],
                [("qualified_name", obrain_common::types::Value::from("b"))],
            );
            session.create_edge(a, b, "CONTAINS");
            session.create_edge(b, a, "REVELE");
            store.commit_wal().expect("commit");
        }

        // Périmètre : lire les `Page`, et lire la relation `CONTAINS` seulement.
        let scope = ReadScope::from_permissions(["node:read:Page", "edge:read:CONTAINS"]);
        let shape = describe_store_scoped(&store, &scope);

        let labels: Vec<&str> = shape.labels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(labels, vec!["Page"], "seul le label lisible : {labels:?}");
        assert_eq!(shape.hidden_labels, 1, "le label masqué est compté");

        let edges: Vec<&str> = shape.edge_types.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(edges, vec!["CONTAINS"], "seul le type lisible : {edges:?}");
        assert_eq!(shape.hidden_edge_types, 1, "le type masqué est compté");

        // Sans périmètre restrictif, tout est publié — le comportement d'avant.
        let complet = describe_store_scoped(&store, &ReadScope::all());
        assert_eq!(complet.labels.len(), 2);
        assert_eq!(complet.hidden_labels, 0);
        assert_eq!(complet.edge_types.len(), 2);
    }
}
