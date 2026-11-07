//! # Les vues : ce qu'un module veut montrer, et les actions qu'il offre
//!
//! Une vue est un **arbre déclaratif** : des conteneurs ([`Node`]) qui rangent des **briques**
//! ([`Brick`]), et des **actions** ([`Action`]) nommées que les briques déclenchent. Chaque
//! brique lit une entité, à travers un filtre optionnel. Le module nomme ce qu'il lit et ce
//! qu'il offre ; l'hôte dessine. Rien à exécuter, donc rien à confiance.
//!
//! ```json
//! { "name": "conversations", "surface": "sidebar", "title": "Conversations",
//!   "root": { "type": "leaf", "brick": { "type": "list", "entity": "conversation",
//!             "label": "titre", "subtitle": "updated_at",
//!             "on_click": { "kind": "open_detail" } } } }
//! ```
//!
//! **Vocabulaire fermé.** Chaque conteneur, brique, action, capacité, entrée et clic est une
//! variante d'énumération. Un type inconnu est refusé à la désérialisation
//! (`deny_unknown_fields` partout) : un hôte qui ne connaît pas une brique ne la devine pas.
//!
//! ## Ce que chaque genre rend
//!
//! Il n'y a plus de « genre » de vue : une vue est un arbre. Les anciens genres sont des
//! compositions connues de l'hôte :
//! - `list` : une brique `list` (lignes cliquables, tri, filtre, pagination) ;
//! - `detail` : une brique `card` (les champs d'une ligne) ;
//! - `form` : une brique `form` (champs énumérés, ordre, libellé, lecture seule) ;
//! - `metrics` : une brique `metrics` (compte, ou somme d'un champ) ;
//! - `chat` : une brique `chat` (fil de messages et saisie).
//!
//! Par emplacement : `header` n'accepte que les petites briques (`text`, `banner`, `indicator`,
//! `metrics`, `button`) ; `sidebar`, `main`, `panel` acceptent tout.
//!
//! ## Ce que la vue ne dit PAS
//!
//! Comment dessiner. Une liste reste une liste ; l'apparence déclarée ([`Look`]) se borne à
//! des choix fermés (accent, disposition, transition), rendus par l'hôte avec son style.
//!
//! [`issues`] est la vérification, appelée par `ModuleDescriptor::check`.

use crate::entity::{EntityDesc, base_fields};
use crate::link::LinkDesc;
use crate::module::ModuleDescriptor;
use crate::naming::is_identifier;
use crate::query::{FilterExpr, SortSpec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Où la vue se dessine. Le nom est celui de l'emplacement, pas d'un composant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// Le menu latéral, quand il y en a un.
    Sidebar,
    /// L'écran principal : ce qu'on voit en ouvrant le module.
    #[default]
    Main,
    /// Un panneau : à côté du contenu principal, ou par-dessus.
    Panel,
    /// L'en-tête : une barre fine ; seules les petites briques y ont leur place.
    Header,
}

/// Apparence déclarée : des choix fermés, pas de CSS. Absente = défauts de l'hôte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    #[serde(default)]
    pub accent: Accent,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub transition: Transition,
}

/// Couleur d'accent, parmi une palette fermée.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Neutral,
    Primary,
    Success,
    Warning,
    Danger,
    Info,
}

/// Disposition des enfants d'un conteneur racine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Stack,
    Grid,
    Compact,
}

/// Transition à l'ouverture de la vue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    #[default]
    None,
    Fade,
    Slide,
}

/// Une vue déclarée par un module : un nom, un emplacement, un arbre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewDesc {
    /// Identifiant de la vue dans le module (`conversations`, `invoice_detail`).
    pub name: String,
    #[serde(default)]
    pub surface: Surface,
    /// Le titre de la section, quand l'hôte en affiche un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Le nom d'une vue de ce module que celle-ci remplace : l'hôte n'affiche que la
    /// remplaçante. Doit désigner une vue existante, différente de soi, remplacée une seule fois.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
    /// L'arbre dessiné.
    pub root: Node,
}

/// Un nœud de l'arbre : un conteneur, ou une feuille qui porte une brique.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Node {
    /// Empile ses enfants verticalement.
    Stack { children: Vec<Node> },
    /// Range ses enfants côte à côte.
    Row { children: Vec<Node> },
    /// Un onglet par entrée ; chaque onglet a son propre arbre.
    Tabs { tabs: Vec<Tab> },
    /// Un tiroir replié sous un libellé.
    Drawer { label: String, children: Vec<Node> },
    /// Une feuille modale : ouverte par-dessus, avec un titre.
    Modal { title: String, children: Vec<Node> },
    /// Une feuille : une brique.
    Leaf { brick: Box<Brick> },
}

/// Un onglet de [`Node::Tabs`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub label: String,
    pub children: Vec<Node>,
}

/// Ce qu'une feuille dessine. Les briques de données lisent une entité, à travers un filtre
/// optionnel (absent = toutes les lignes). Les briques d'état (`banner`, `indicator` de
/// connexion, `capability_request`) ne lisent pas une ligne.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Brick {
    /// Une valeur : le champ `field` de la première ligne du filtre.
    Text {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
    },
    /// Un bouton qui déclenche une action déclarée par le module.
    Button {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        action: String,
    },
    /// Une bannière d'alerte : un ton fermé et un texte (clé de message ou texte).
    Banner { tone: Tone, text: String },
    /// Un indicateur : connexion, activité (phase et durée), compteur ou badge.
    Indicator {
        signal: Signal,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        /// Activité : la phase. Compteur ou badge : la valeur.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        /// Activité : la durée écoulée.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<String>,
    },
    /// Les lignes d'une entité : libellé, sous-titre, tri, filtre, pagination, clic.
    List {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        empty: Option<String>,
        /// Tri par défaut, dans l'ordre de la liste.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sort: Vec<SortSpec>,
        /// Champs sur lesquels l'utilisateur peut filtrer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        filterable: Vec<String>,
        /// Taille de page. Absente = pas de pagination.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page_size: Option<u32>,
        /// Ce qui se passe quand on clique une ligne. Absent = rien.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_click: Option<Click>,
    },
    /// Une fiche : plusieurs champs d'une même ligne (le genre `detail`).
    Card {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        fields: Vec<String>,
    },
    /// Un formulaire : ses champs, énumérés dans l'ordre, avec libellé et lecture seule.
    Form {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        fields: Vec<FormField>,
        /// L'action de soumission (une création ou une modification).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        submit: Option<String>,
    },
    /// Un champ éditable. `input` choisit le contrôle ; `target` est l'entité d'une référence.
    Field {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
        #[serde(default)]
        input: Input,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    /// Une relation entre entités : affichée ou éditable, à travers un lien déclaré.
    Relation {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        /// Le `link_type` d'un lien dont `entity` est la source.
        link: String,
        mode: RelationMode,
    },
    /// Le fil d'une conversation : les messages dans l'ordre, et la saisie.
    Chat {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
    },
    /// Une étape d'outil dans un fil de messages : son état est un champ.
    ToolStep {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
    },
    /// Une file d'attente : les éléments à traiter, dans l'ordre de `sort`.
    Queue {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        label: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sort: Vec<SortSpec>,
    },
    /// Un bloc de code : le texte d'un champ, avec son langage fixe.
    CodeBlock {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
    /// Le résultat d'un run : un champ, affiché tel quel.
    Result {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
    },
    /// Une carte de demande de capacité d'appareil ; l'action porte la décision.
    CapabilityRequest {
        capability: DeviceCapability,
        /// Une action `device_capability` de ce module.
        action: String,
    },
    /// Des nombres : sans `field`, le nombre de lignes ; avec, la somme du champ.
    Metrics {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
    /// Un programme : arguments typés, exécution fermée, colonnes calculées.
    Program {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<ProgramArg>,
        run: Run,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<Column>,
    },
}

impl Brick {
    /// L'entité que la brique lit ; `None` pour les briques d'état (bannière, demande de
    /// capacité, indicateur de connexion sans entité).
    pub fn entity(&self) -> Option<&str> {
        match self {
            Brick::Banner { .. } | Brick::CapabilityRequest { .. } => None,
            Brick::Indicator { entity, .. } => entity.as_deref(),
            Brick::Text { entity, .. }
            | Brick::Button { entity, .. }
            | Brick::Relation { entity, .. }
            | Brick::List { entity, .. }
            | Brick::Card { entity, .. }
            | Brick::Form { entity, .. }
            | Brick::Field { entity, .. }
            | Brick::Chat { entity, .. }
            | Brick::ToolStep { entity, .. }
            | Brick::Queue { entity, .. }
            | Brick::CodeBlock { entity, .. }
            | Brick::Result { entity, .. }
            | Brick::Metrics { entity, .. }
            | Brick::Program { entity, .. } => Some(entity),
        }
    }
}

/// Ton d'une bannière.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Connection,
    Activity,
    Counter,
    Badge,
}

/// Le contrôle d'un champ éditable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Input {
    #[default]
    Text,
    Multiline,
    File,
    /// Une référence vers une autre entité, avec sélecteur : exige `target`.
    Reference,
    /// Une bascule actif / inactif.
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationMode {
    Show,
    Edit,
}

/// Un champ d'un formulaire : son ordre est celui de la liste.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}

/// Ce qui se passe quand on clique une ligne.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Click {
    /// Sélectionne la ligne, sans quitter la vue.
    Select,
    /// Ouvre le détail de la ligne.
    OpenDetail,
    /// Ouvre une autre vue de ce module.
    Navigate { view: String },
}

/// Un argument de programme : nom et type fermé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramArg {
    pub name: String,
    pub kind: ArgKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgKind {
    Text,
    Number,
    Bool,
    Date,
}

/// Quand un programme s'exécute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Run {
    /// À la demande, par un bouton ou un outil.
    Manual,
    /// À chaque changement des lignes lues.
    OnChange,
}

/// Une colonne calculée d'un programme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    pub source: ColumnSource,
}

/// La source d'une colonne calculée : un champ, un compte, une somme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ColumnSource {
    Field { field: String },
    Count,
    Sum { field: String },
}

/// Une action déclarée par le module, nommée, et que les briques peuvent appeler.
/// Les champs écrits sont ceux que l'entité déclare (jamais les champs de base).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Crée une ligne de `entity` avec les champs `fields`.
    Create {
        name: String,
        entity: String,
        fields: Vec<String>,
    },
    /// Modifie les champs `fields` d'une ligne de `entity`.
    Update {
        name: String,
        entity: String,
        fields: Vec<String>,
    },
    /// Supprime une ligne de `entity`.
    Delete { name: String, entity: String },
    /// Ouvre la vue `view` de ce module.
    Navigate { name: String, view: String },
    /// Une opération d'agent : envoyer un message, arrêter un run. Liée à une entité si besoin.
    AgentTool {
        name: String,
        op: AgentOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
    },
    /// Autoriser ou refuser une capacité de l'appareil.
    DeviceCapability {
        name: String,
        capability: DeviceCapability,
        grant: Grant,
    },
}

impl Action {
    /// Le nom de l'action, celui que les briques référencent.
    pub fn name(&self) -> &str {
        match self {
            Action::Create { name, .. }
            | Action::Update { name, .. }
            | Action::Delete { name, .. }
            | Action::Navigate { name, .. }
            | Action::AgentTool { name, .. }
            | Action::DeviceCapability { name, .. } => name,
        }
    }
}

/// Les opérations d'agent exposées. Fermé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOp {
    SendMessage,
    StopRun,
}

/// Décision sur une capacité d'appareil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    Allow,
    Deny,
}

/// Les capacités de l'appareil. Fermé, comme le reste.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCapability {
    Camera,
    Microphone,
    Location,
    Screen,
    DeviceMotion,
}

/// Les problèmes de vues et d'actions d'un module, sous forme `(raison, chemin, message)`.
/// `ModuleDescriptor::check` les rend en `this.descriptor.<raison>`.
pub fn issues(m: &ModuleDescriptor) -> Vec<(&'static str, String, String)> {
    let mut c = Checks::new(m);
    c.actions();
    c.views();
    c.replacements();
    c.out
}

struct Checks<'a> {
    m: &'a ModuleDescriptor,
    entities: BTreeMap<&'a str, &'a EntityDesc>,
    actions: BTreeMap<&'a str, &'a Action>,
    views: BTreeSet<&'a str>,
    out: Vec<(&'static str, String, String)>,
}

impl<'a> Checks<'a> {
    fn new(m: &'a ModuleDescriptor) -> Self {
        Self {
            m,
            entities: m.entities.iter().map(|e| (e.name.as_str(), e)).collect(),
            actions: BTreeMap::new(),
            views: m.views.iter().map(|v| v.name.as_str()).collect(),
            out: Vec::new(),
        }
    }

    fn push(&mut self, reason: &'static str, path: impl Into<String>, msg: impl Into<String>) {
        self.out.push((reason, path.into(), msg.into()));
    }

    /// Champs lisibles : déclarés et de base.
    fn readable(&self, entity: &str) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        if let Some(e) = self.entities.get(entity) {
            s.extend(e.fields.iter().map(|f| f.name.clone()));
            s.extend(base_fields(e.kind).iter().map(|f| f.name.clone()));
        }
        s
    }

    /// Champs écrivables : déclarés seulement.
    fn writable(&self, entity: &str) -> BTreeSet<String> {
        self.entities
            .get(entity)
            .map(|e| e.fields.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default()
    }

    fn entity_ref(&mut self, reason: &'static str, path: &str, entity: &str) -> bool {
        if self.entities.contains_key(entity) {
            true
        } else {
            self.push(reason, path, format!("`{entity}` is not a declared entity"));
            false
        }
    }

    fn action_ref(&mut self, path: &str, name: &str) {
        if !self.actions.contains_key(name) {
            self.push(
                "view_action_unknown",
                path,
                format!("`{name}` is not a declared action"),
            );
        }
    }

    fn view_ref(&mut self, reason: &'static str, path: &str, name: &str) {
        if !self.views.contains(name) {
            self.push(
                reason,
                path,
                format!("`{name}` is not a view of this module"),
            );
        }
    }

    fn actions(&mut self) {
        let m = self.m;
        for (i, a) in m.actions.iter().enumerate() {
            let p = format!("actions[{i}]");
            if !is_identifier(a.name()) {
                self.push("invalid_name", &p, format!("`{}`", a.name()));
            }
            if self.actions.contains_key(a.name()) {
                self.push(
                    "duplicate_action",
                    &p,
                    format!("`{}` declared twice", a.name()),
                );
            } else {
                self.actions.insert(a.name(), a);
            }
            match a {
                Action::Create { entity, fields, .. } | Action::Update { entity, fields, .. } => {
                    if self.entity_ref("action_entity_unknown", &format!("{p}.entity"), entity) {
                        let ok = self.writable(entity);
                        for (j, f) in fields.iter().enumerate() {
                            if !ok.contains(f) {
                                self.push(
                                    "action_field_unknown",
                                    format!("{p}.fields[{j}]"),
                                    format!("`{f}` is not a declared field of `{entity}`"),
                                );
                            }
                        }
                    }
                }
                Action::Delete { entity, .. } => {
                    self.entity_ref("action_entity_unknown", &format!("{p}.entity"), entity);
                }
                Action::AgentTool {
                    entity: Some(e), ..
                } => {
                    self.entity_ref("action_entity_unknown", &format!("{p}.entity"), e);
                }
                Action::AgentTool { entity: None, .. } => {}
                Action::Navigate { view, .. } => {
                    self.view_ref("view_target_unknown", &format!("{p}.view"), view);
                }
                Action::DeviceCapability { .. } => {}
            }
        }
    }

    fn views(&mut self) {
        let m = self.m;
        let mut vus = BTreeSet::new();
        for (i, v) in m.views.iter().enumerate() {
            let p = format!("views[{i}]");
            if !is_identifier(&v.name) {
                self.push("invalid_name", &p, format!("`{}`", v.name));
            }
            if !vus.insert(v.name.as_str()) {
                self.push("duplicate_view", &p, format!("`{}` declared twice", v.name));
            }
            let mut briques = Vec::new();
            collecte(&format!("{p}.root"), &v.root, &mut briques);
            for (chemin, b) in briques {
                self.brick(&chemin, b, v.surface);
            }
        }
    }

    fn brick(&mut self, chemin: &str, b: &Brick, surface: Surface) {
        if surface == Surface::Header
            && !matches!(
                b,
                Brick::Text { .. }
                    | Brick::Banner { .. }
                    | Brick::Indicator { .. }
                    | Brick::Metrics { .. }
                    | Brick::Button { .. }
            )
        {
            self.push(
                "surface_brick_refused",
                chemin,
                "this brick does not fit in a header".to_string(),
            );
        }
        match b {
            Brick::Banner { .. } => {}
            Brick::CapabilityRequest { action, .. } => {
                self.action_ref(&format!("{chemin}.action"), action);
            }
            Brick::Indicator {
                signal,
                entity,
                filter,
                field,
                duration,
            } => {
                let Some(e) = entity.as_deref() else {
                    if *signal != Signal::Connection {
                        self.push(
                            "view_entity_required",
                            chemin,
                            format!("a {signal:?} indicator must name its entity"),
                        );
                    }
                    return;
                };
                if !self.entity_ref("view_entity_unknown", &format!("{chemin}.entity"), e) {
                    return;
                }
                let lis = self.readable(e);
                let mut lus: Vec<&str> = field
                    .iter()
                    .chain(duration.iter())
                    .map(String::as_str)
                    .collect();
                if let Some(f) = filter {
                    chemins_du_filtre(f, &mut lus);
                }
                self.reads(chemin, e, &lis, &lus);
            }
            Brick::Relation {
                entity,
                filter,
                link,
                ..
            } => {
                if !self.entity_ref("view_entity_unknown", &format!("{chemin}.entity"), entity) {
                    return;
                }
                let mut lus = Vec::new();
                if let Some(f) = filter {
                    chemins_du_filtre(f, &mut lus);
                }
                let lis = self.readable(entity);
                self.reads(chemin, entity, &lis, &lus);
                let lien = self
                    .m
                    .links
                    .iter()
                    .any(|l: &LinkDesc| l.link_type == *link && l.source == *entity);
                if !lien {
                    self.push(
                        "view_relation_unknown",
                        format!("{chemin}.link"),
                        format!("`{link}` is not a link from `{entity}`"),
                    );
                }
            }
            _ => self.data_brick(chemin, b),
        }
    }

    /// Briques qui lisent une entité : entité, champs, filtre, puis le spécifique.
    fn data_brick(&mut self, chemin: &str, b: &Brick) {
        let Some(entite) = b.entity() else {
            return;
        };
        if !self.entity_ref("view_entity_unknown", &format!("{chemin}.entity"), entite) {
            return;
        }
        let lis = self.readable(entite);
        let lus = lectures(b);
        self.reads(chemin, entite, &lis, &lus);

        match b {
            Brick::Button { action, .. } => self.action_ref(&format!("{chemin}.action"), action),
            Brick::Field { input, target, .. } => match (input, target) {
                (Input::Reference, None) => self.push(
                    "field_input_invalid",
                    chemin,
                    "a reference input must name its target entity".to_string(),
                ),
                (Input::Reference, Some(t)) => {
                    self.entity_ref("view_entity_unknown", &format!("{chemin}.target"), t);
                }
                (_, Some(_)) => self.push(
                    "field_input_invalid",
                    chemin,
                    "only a reference input takes a target".to_string(),
                ),
                (_, None) => {}
            },
            Brick::Form { fields, submit, .. } => {
                for (j, f) in fields.iter().enumerate() {
                    if f.field.is_empty() {
                        self.push(
                            "invalid_name",
                            format!("{chemin}.fields[{j}]"),
                            "empty field name".to_string(),
                        );
                    }
                }
                if let Some(s) = submit {
                    self.action_ref(&format!("{chemin}.submit"), s);
                }
            }
            Brick::List {
                on_click,
                page_size,
                ..
            } => {
                if *page_size == Some(0) {
                    self.push(
                        "invalid_page_size",
                        format!("{chemin}.page_size"),
                        "a page holds at least one row".to_string(),
                    );
                }
                if let Some(Click::Navigate { view }) = on_click {
                    self.view_ref(
                        "view_target_unknown",
                        &format!("{chemin}.on_click.view"),
                        view,
                    );
                }
            }
            Brick::Program { args, columns, .. } => {
                let mut noms = BTreeSet::new();
                for (j, a) in args.iter().enumerate() {
                    let p = format!("{chemin}.args[{j}]");
                    if !is_identifier(&a.name) {
                        self.push("invalid_name", &p, format!("`{}`", a.name));
                    }
                    if !noms.insert(a.name.as_str()) {
                        self.push(
                            "program_name_duplicate",
                            &p,
                            format!("`{}` declared twice", a.name),
                        );
                    }
                }
                let mut colonnes = BTreeSet::new();
                for (j, c) in columns.iter().enumerate() {
                    let p = format!("{chemin}.columns[{j}]");
                    if !is_identifier(&c.name) {
                        self.push("invalid_name", &p, format!("`{}`", c.name));
                    }
                    if !colonnes.insert(c.name.as_str()) {
                        self.push(
                            "program_name_duplicate",
                            &p,
                            format!("`{}` declared twice", c.name),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Vérifie que chaque champ lu existe ; un chemin pointé (`target.name`) est validé ailleurs.
    fn reads(&mut self, chemin: &str, entite: &str, lis: &BTreeSet<String>, lus: &[&str]) {
        for c in lus {
            if !c.contains('.') && !lis.contains(*c) {
                self.push(
                    "view_field_unknown",
                    chemin,
                    format!("`{c}` is not a field of `{entite}`"),
                );
            }
        }
    }

    fn replacements(&mut self) {
        let m = self.m;
        let mut remplacees: HashMap<&str, &str> = HashMap::new();
        let mut suivant: HashMap<&str, &str> = HashMap::new();
        for (i, v) in m.views.iter().enumerate() {
            let Some(cible) = v.replaces.as_deref() else {
                continue;
            };
            let p = format!("views[{i}].replaces");
            if cible == v.name {
                self.push(
                    "view_replaces_self",
                    &p,
                    format!("`{cible}` replaces itself"),
                );
                continue;
            }
            if !self.views.contains(cible) {
                self.push(
                    "view_target_unknown",
                    &p,
                    format!("`{cible}` is not a view of this module"),
                );
                continue;
            }
            if remplacees.insert(cible, v.name.as_str()).is_some() {
                self.push(
                    "view_replaced_twice",
                    &p,
                    format!("`{cible}` is replaced by more than one view"),
                );
            }
            suivant.insert(v.name.as_str(), cible);
        }
        for (i, v) in m.views.iter().enumerate() {
            let mut courant = v.name.as_str();
            let mut vu = BTreeSet::new();
            while let Some(&n) = suivant.get(courant) {
                if n == v.name {
                    self.push(
                        "view_replaces_cycle",
                        format!("views[{i}].replaces"),
                        format!("`{}` replaces itself through a chain", v.name),
                    );
                    break;
                }
                if !vu.insert(n) {
                    break; // cycle qui ne passe pas par `v` : signalé sur ses propres membres.
                }
                courant = n;
            }
        }
    }
}

/// Les chemins de champ qu'une brique de données lit (hors entité et filtre) : champs affichés,
/// tri, filtres, champs de formulaire, colonnes.
fn lectures(b: &Brick) -> Vec<&str> {
    let mut v: Vec<&str> = Vec::new();
    match b {
        Brick::Text { field, filter, .. }
        | Brick::Field { field, filter, .. }
        | Brick::ToolStep { field, filter, .. }
        | Brick::CodeBlock { field, filter, .. }
        | Brick::Result { field, filter, .. } => {
            v.push(field);
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Metrics { field, filter, .. } => {
            v.extend(field.as_deref());
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::List {
            label,
            subtitle,
            sort,
            filterable,
            filter,
            ..
        } => {
            v.push(label);
            v.extend(subtitle.as_deref());
            v.extend(sort.iter().map(|s| s.field.as_str()));
            v.extend(filterable.iter().map(String::as_str));
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Queue {
            label,
            sort,
            filter,
            ..
        } => {
            v.push(label);
            v.extend(sort.iter().map(|s| s.field.as_str()));
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Card { fields, filter, .. } => {
            v.extend(fields.iter().map(String::as_str));
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Form { fields, filter, .. } => {
            v.extend(fields.iter().map(|f| f.field.as_str()));
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Program {
            columns, filter, ..
        } => {
            for c in columns {
                match &c.source {
                    ColumnSource::Field { field } | ColumnSource::Sum { field } => v.push(field),
                    ColumnSource::Count => {}
                }
            }
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Button { filter, .. } | Brick::Chat { filter, .. } => {
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Banner { .. }
        | Brick::Indicator { .. }
        | Brick::Relation { .. }
        | Brick::CapabilityRequest { .. } => {}
    }
    v
}

/// Toutes les briques d'un arbre, avec leur chemin (`views[0].root.children[1]`).
fn collecte<'a>(chemin: &str, noeud: &'a Node, out: &mut Vec<(String, &'a Brick)>) {
    match noeud {
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                collecte(&format!("{chemin}.children[{i}]"), c, out);
            }
        }
        Node::Tabs { tabs } => {
            for (i, t) in tabs.iter().enumerate() {
                for (j, c) in t.children.iter().enumerate() {
                    collecte(&format!("{chemin}.tabs[{i}].children[{j}]"), c, out);
                }
            }
        }
        Node::Leaf { brick } => out.push((format!("{chemin}.brick"), brick.as_ref())),
    }
}

/// Tous les chemins de champ d'un filtre (`amount`, `target.name`).
fn chemins_du_filtre<'a>(f: &'a FilterExpr, out: &mut Vec<&'a str>) {
    match f {
        FilterExpr::And(fs) | FilterExpr::Or(fs) => {
            for g in fs {
                chemins_du_filtre(g, out);
            }
        }
        FilterExpr::Not(g) => chemins_du_filtre(g, out),
        FilterExpr::Cmp { field, .. } => out.push(field.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::CheckProfile;
    use crate::custom::CustomRegistry;
    use crate::field::{FieldDesc, FieldType};

    // ---------- fixtures ----------

    /// Un module : `invoice` (amount, titre), `conversation`, un lien invoice -> conversation.
    fn module() -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        let mut invoice = EntityDesc::new("invoice");
        invoice.fields = vec![
            FieldDesc::new("amount", FieldType::String),
            FieldDesc::new("titre", FieldType::String),
        ];
        m.entities.push(invoice);
        m.entities.push(EntityDesc::new("conversation"));
        m.links.push(LinkDesc::new(
            "invoice_conversation",
            "invoice",
            "conversation",
        ));
        m
    }

    fn brick(json: &str) -> Brick {
        serde_json::from_str(json).unwrap()
    }

    fn view(name: &str, root: Node) -> ViewDesc {
        ViewDesc {
            name: name.into(),
            surface: Surface::Main,
            title: None,
            replaces: None,
            look: None,
            root,
        }
    }

    fn leaf_view(name: &str, json: &str) -> ViewDesc {
        view(
            name,
            Node::Leaf {
                brick: Box::new(brick(json)),
            },
        )
    }

    fn codes(m: &ModuleDescriptor) -> Vec<String> {
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.code.0)
            .collect()
    }

    fn assert_refused(m: &ModuleDescriptor, reason: &str) {
        assert_eq!(codes(m), vec![format!("this.descriptor.{reason}")]);
    }

    fn assert_stable<T>(json: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let v: T = serde_json::from_str(json).unwrap();
        let once = serde_json::to_string(&v).unwrap();
        let back: T = serde_json::from_str(&once).unwrap();
        assert_eq!(back, v);
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            once,
            "output must be stable"
        );
    }

    // ---------- JSON : fermé et stable, une famille à la fois ----------

    #[test]
    fn a_view_defaults_to_the_main_surface() {
        let v: ViewDesc =
            serde_json::from_str(r#"{"name":"x","root":{"type":"stack","children":[]}}"#).unwrap();
        assert_eq!(v.surface, Surface::Main);
        assert!(v.replaces.is_none() && v.look.is_none());
    }

    #[test]
    fn family_a_actions_are_representable() {
        let json = r#"[
          {"kind":"create","name":"new","entity":"invoice","fields":["titre"]},
          {"kind":"update","name":"edit","entity":"invoice","fields":["amount"]},
          {"kind":"delete","name":"drop","entity":"invoice"},
          {"kind":"navigate","name":"go","view":"cockpit"},
          {"kind":"agent_tool","name":"say","op":"send_message","entity":"conversation"},
          {"kind":"agent_tool","name":"halt","op":"stop_run"},
          {"kind":"device_capability","name":"cam_ok","capability":"camera","grant":"allow"},
          {"kind":"device_capability","name":"mic_no","capability":"microphone","grant":"deny"},
          {"kind":"device_capability","name":"where","capability":"location","grant":"allow"},
          {"kind":"device_capability","name":"shot","capability":"screen","grant":"allow"},
          {"kind":"device_capability","name":"move","capability":"device_motion","grant":"deny"}
        ]"#;
        let actions: Vec<Action> = serde_json::from_str(json).unwrap();
        let s = serde_json::to_string(&actions).unwrap();
        let again: Vec<Action> = serde_json::from_str(&s).unwrap();
        assert_eq!(again, actions);
        assert_eq!(serde_json::to_string(&again).unwrap(), s);
    }

    #[test]
    fn family_b_containers_are_representable() {
        assert_stable::<Node>(
            r#"{"type":"stack","children":[
                {"type":"row","children":[{"type":"leaf","brick":{"type":"text","entity":"invoice","field":"titre"}}]},
                {"type":"tabs","tabs":[{"label":"Liste","children":[]}]},
                {"type":"drawer","label":"Détails","children":[]},
                {"type":"modal","title":"Confirmer","children":[]}
            ]}"#,
        );
    }

    #[test]
    fn family_c_base_bricks_are_representable() {
        for json in [
            r#"{"type":"button","entity":"invoice","action":"new"}"#,
            r#"{"type":"text","entity":"invoice","field":"titre"}"#,
            r#"{"type":"banner","tone":"warning","text":"alerts.offline"}"#,
            r#"{"type":"indicator","signal":"connection"}"#,
            r#"{"type":"indicator","signal":"activity","entity":"invoice","field":"phase","duration":"amount"}"#,
            r#"{"type":"indicator","signal":"counter","entity":"invoice"}"#,
            r#"{"type":"indicator","signal":"badge","entity":"invoice","field":"titre"}"#,
            r#"{"type":"field","entity":"invoice","field":"titre","input":"multiline"}"#,
            r#"{"type":"field","entity":"invoice","field":"titre","input":"file"}"#,
            r#"{"type":"field","entity":"invoice","field":"titre","input":"reference","target":"conversation"}"#,
            r#"{"type":"field","entity":"invoice","field":"titre","input":"toggle"}"#,
            r#"{"type":"capability_request","capability":"camera","action":"cam_ok"}"#,
            r#"{"type":"tool_step","entity":"conversation","field":"titre"}"#,
            r#"{"type":"queue","entity":"invoice","label":"titre"}"#,
            r#"{"type":"code_block","entity":"invoice","field":"titre","language":"rust"}"#,
            r#"{"type":"result","entity":"invoice","field":"titre"}"#,
            r#"{"type":"metrics","entity":"invoice"}"#,
            r#"{"type":"chat","entity":"conversation"}"#,
        ] {
            assert_stable::<Brick>(json);
        }
    }

    #[test]
    fn family_d_the_legacy_genres_are_compositions_of_bricks() {
        // detail -> card, form -> form, metrics -> metrics, chat -> chat : chacun se décrit.
        assert_stable::<Brick>(r#"{"type":"card","entity":"invoice","fields":["titre"]}"#);
        assert_stable::<Brick>(
            r#"{"type":"form","entity":"invoice","fields":[{"field":"titre"}]}"#,
        );
        // Header : une vue d'en-tête avec un compteur est valide.
        let mut m = module();
        m.views.push(ViewDesc {
            surface: Surface::Header,
            ..leaf_view("count", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            surface: Surface::Panel,
            ..leaf_view("thread", r#"{"type":"chat","entity":"conversation"}"#)
        });
        assert!(
            m.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }

    #[test]
    fn family_e_a_form_enumerates_its_fields_in_order() {
        assert_stable::<Brick>(
            r#"{"type":"form","entity":"invoice","fields":[
                {"field":"titre","label":"Titre"},
                {"field":"amount","read_only":true},
                {"field":"id","label":"Réf","read_only":true}
            ],"submit":"new"}"#,
        );
        let f: Brick = serde_json::from_str(
            r#"{"type":"form","entity":"invoice","fields":[{"field":"b"},{"field":"a"}]}"#,
        )
        .unwrap();
        match f {
            Brick::Form { fields, .. } => {
                assert_eq!(fields[0].field, "b", "order is the declared order");
                assert!(!fields[0].read_only);
            }
            _ => panic!("expected a form"),
        }
    }

    #[test]
    fn family_f_row_click_is_declared() {
        for click in [
            r#"{"kind":"select"}"#,
            r#"{"kind":"open_detail"}"#,
            r#"{"kind":"navigate","view":"cockpit"}"#,
        ] {
            assert_stable::<Brick>(&format!(
                r#"{{"type":"list","entity":"invoice","label":"titre","on_click":{click}}}"#
            ));
        }
    }

    #[test]
    fn family_g_sort_filter_and_pagination_are_declared() {
        assert_stable::<Brick>(
            r#"{"type":"list","entity":"invoice","label":"titre",
                "sort":[{"field":"amount","dir":"desc"}],
                "filterable":["titre"],"page_size":25,
                "filter":{"cmp":{"field":"amount","op":"gt","value":0}}}"#,
        );
    }

    #[test]
    fn family_h_look_is_declared_with_closed_choices() {
        assert_stable::<ViewDesc>(
            r#"{"name":"x","look":{"accent":"primary","layout":"grid","transition":"slide"},
                "root":{"type":"stack","children":[]}}"#,
        );
        assert!(serde_json::from_str::<ViewDesc>(
            r##"{"name":"x","look":{"accent":"#ff00ff"},"root":{"type":"stack","children":[]}}"##
        )
        .is_err());
    }

    #[test]
    fn family_i_programs_are_representable() {
        assert_stable::<Brick>(
            r#"{"type":"program","entity":"invoice",
                "args":[{"name":"from","kind":"date"},{"name":"verbose","kind":"bool"}],
                "run":"on_change",
                "columns":[{"name":"montant","source":{"op":"sum","field":"amount"}},
                           {"name":"n","source":{"op":"count"}},
                           {"name":"t","source":{"op":"field","field":"titre"}}]}"#,
        );
    }

    #[test]
    fn family_j_relations_are_representable() {
        assert_stable::<Brick>(
            r#"{"type":"relation","entity":"invoice","link":"invoice_conversation","mode":"edit"}"#,
        );
        assert_stable::<Brick>(
            r#"{"type":"relation","entity":"invoice","link":"invoice_conversation","mode":"show"}"#,
        );
    }

    #[test]
    fn an_unknown_container_is_refused_by_deserialization() {
        assert!(serde_json::from_str::<Node>(r#"{"type":"grid","children":[]}"#).is_err());
    }

    #[test]
    fn an_unknown_brick_is_refused_by_deserialization() {
        assert!(serde_json::from_str::<Brick>(r#"{"type":"video","entity":"invoice"}"#).is_err());
    }

    #[test]
    fn an_unknown_field_is_refused_on_views_bricks_actions_and_looks() {
        // Ancien format plat : `kind` à la racine d'une vue n'existe plus.
        assert!(
            serde_json::from_str::<ViewDesc>(
                r#"{"name":"x","kind":"list","root":{"type":"stack","children":[]}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Brick>(
                r#"{"type":"text","entity":"invoice","field":"a","colour":"red"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Action>(
                r#"{"kind":"delete","name":"d","entity":"invoice","force":true}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<Look>(r#"{"accent":"primary","font":"x"}"#).is_err());
    }

    #[test]
    fn unknown_kinds_are_refused() {
        // action
        assert!(
            serde_json::from_str::<Action>(r#"{"kind":"publish","name":"p","entity":"invoice"}"#)
                .is_err()
        );
        // capacité
        assert!(serde_json::from_str::<Action>(
            r#"{"kind":"device_capability","name":"b","capability":"bluetooth","grant":"allow"}"#
        )
        .is_err());
        // opération d'agent libre
        assert!(
            serde_json::from_str::<Action>(r#"{"kind":"agent_tool","name":"a","op":"rm -rf"}"#)
                .is_err()
        );
        // clic
        assert!(serde_json::from_str::<Click>(r#"{"kind":"drag"}"#).is_err());
        // type de champ
        assert!(
            serde_json::from_str::<Brick>(
                r#"{"type":"field","entity":"invoice","field":"a","input":"colour_picker"}"#
            )
            .is_err()
        );
        // emplacement
        assert!(
            serde_json::from_str::<ViewDesc>(
                r#"{"name":"x","surface":"toolbar","root":{"type":"stack","children":[]}}"#
            )
            .is_err()
        );
    }

    // ---------- check : le cas sain ----------

    #[test]
    fn a_coherent_module_passes_check() {
        let mut m = module();
        m.actions.push(Action::Create {
            name: "new".into(),
            entity: "invoice".into(),
            fields: vec!["titre".into()],
        });
        m.views.push(view(
            "invoices",
            Node::Stack {
                children: vec![
                    Node::Leaf {
                        brick: Box::new(brick(r#"{"type":"list","entity":"invoice","label":"titre","subtitle":"id","on_click":{"kind":"open_detail"}}"#)),
                    },
                    Node::Leaf {
                        brick: Box::new(brick(r#"{"type":"button","entity":"invoice","action":"new"}"#)),
                    },
                    Node::Leaf {
                        brick: Box::new(brick(r#"{"type":"field","entity":"invoice","field":"titre","input":"reference","target":"conversation"}"#)),
                    },
                    Node::Leaf {
                        brick: Box::new(brick(r#"{"type":"relation","entity":"invoice","link":"invoice_conversation","mode":"edit"}"#)),
                    },
                ],
            },
        ));
        assert!(
            m.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }

    // ---------- check : entité absente ----------

    #[test]
    fn check_refuses_a_brick_on_an_unknown_entity() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"ghost","label":"titre"}"#,
        ));
        assert_refused(&m, "view_entity_unknown");
    }

    #[test]
    fn check_refuses_an_indicator_that_needs_an_entity_and_names_a_ghost() {
        let mut m = module();
        m.views
            .push(leaf_view("x", r#"{"type":"indicator","signal":"counter"}"#));
        assert_refused(&m, "view_entity_required");

        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"indicator","signal":"badge","entity":"ghost"}"#,
        ));
        assert_refused(&m, "view_entity_unknown");
    }

    #[test]
    fn check_refuses_an_action_on_an_unknown_entity() {
        let mut m = module();
        m.actions.push(Action::Delete {
            name: "drop".into(),
            entity: "ghost".into(),
        });
        assert_refused(&m, "action_entity_unknown");
    }

    #[test]
    fn check_refuses_an_agent_op_on_an_unknown_entity() {
        let mut m = module();
        m.actions.push(Action::AgentTool {
            name: "halt".into(),
            op: AgentOp::StopRun,
            entity: Some("ghost".into()),
        });
        assert_refused(&m, "action_entity_unknown");
    }

    #[test]
    fn check_refuses_a_reference_to_an_unknown_target_entity() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"field","entity":"invoice","field":"titre","input":"reference","target":"ghost"}"#,
        ));
        assert_refused(&m, "view_entity_unknown");
    }

    // ---------- check : champ absent ----------

    #[test]
    fn check_refuses_a_list_label_that_is_not_a_field() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"nom"}"#,
        ));
        assert_refused(&m, "view_field_unknown");
    }

    #[test]
    fn check_refuses_an_unknown_field_in_every_read_position() {
        for brick in [
            r#"{"type":"text","entity":"invoice","field":"nom"}"#,
            r#"{"type":"card","entity":"invoice","fields":["titre","nom"]}"#,
            r#"{"type":"metrics","entity":"invoice","field":"nom"}"#,
            r#"{"type":"field","entity":"invoice","field":"nom"}"#,
            r#"{"type":"list","entity":"invoice","label":"titre","sort":[{"field":"nom"}]}"#,
            r#"{"type":"list","entity":"invoice","label":"titre","filterable":["nom"]}"#,
            r#"{"type":"form","entity":"invoice","fields":[{"field":"nom"}]}"#,
            r#"{"type":"queue","entity":"invoice","label":"nom"}"#,
            r#"{"type":"tool_step","entity":"invoice","field":"nom"}"#,
            r#"{"type":"code_block","entity":"invoice","field":"nom"}"#,
            r#"{"type":"result","entity":"invoice","field":"nom"}"#,
            r#"{"type":"indicator","signal":"activity","entity":"invoice","field":"titre","duration":"nom"}"#,
            r#"{"type":"program","entity":"invoice","run":"manual","columns":[{"name":"s","source":{"op":"sum","field":"nom"}}]}"#,
            r#"{"type":"program","entity":"invoice","run":"manual","columns":[{"name":"s","source":{"op":"field","field":"nom"}}]}"#,
        ] {
            let mut m = module();
            m.views.push(leaf_view("x", brick));
            assert_refused(&m, "view_field_unknown");
        }
    }

    #[test]
    fn check_refuses_a_filter_on_an_unknown_field() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titre",
                "filter":{"and":[{"cmp":{"field":"amount","op":"gt","value":0}},
                                 {"cmp":{"field":"ghost","op":"eq","value":1}}]}}"#,
        ));
        assert_refused(&m, "view_field_unknown");
    }

    #[test]
    fn a_base_field_is_readable_but_not_writable_by_an_action() {
        let mut lu = module();
        lu.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titre","subtitle":"id"}"#,
        ));
        assert!(
            lu.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );

        let mut m = module();
        m.actions.push(Action::Update {
            name: "fix".into(),
            entity: "invoice".into(),
            fields: vec!["id".into()],
        });
        assert_refused(&m, "action_field_unknown");
    }

    #[test]
    fn check_refuses_an_action_writing_an_unknown_field() {
        let mut m = module();
        m.actions.push(Action::Create {
            name: "new".into(),
            entity: "invoice".into(),
            fields: vec!["ghost".into()],
        });
        assert_refused(&m, "action_field_unknown");
    }

    #[test]
    fn check_refuses_a_relation_that_is_not_a_link_of_the_entity() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"relation","entity":"conversation","link":"invoice_conversation","mode":"show"}"#,
        ));
        assert_refused(&m, "view_relation_unknown");
    }

    #[test]
    fn check_refuses_a_reference_input_without_target_and_a_target_on_another_input() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"field","entity":"invoice","field":"titre","input":"reference"}"#,
        ));
        assert_refused(&m, "field_input_invalid");

        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"field","entity":"invoice","field":"titre","input":"text","target":"conversation"}"#,
        ));
        assert_refused(&m, "field_input_invalid");
    }

    #[test]
    fn check_refuses_a_big_brick_in_a_header() {
        let mut m = module();
        m.views.push(ViewDesc {
            surface: Surface::Header,
            ..leaf_view("x", r#"{"type":"chat","entity":"conversation"}"#)
        });
        assert_refused(&m, "surface_brick_refused");
    }

    #[test]
    fn check_refuses_a_program_with_zero_or_bad_names() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"program","entity":"invoice","run":"manual",
                "args":[{"name":"a","kind":"text"},{"name":"a","kind":"number"}]}"#,
        ));
        assert_refused(&m, "program_name_duplicate");

        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"program","entity":"invoice","run":"manual",
                "columns":[{"name":"Total €","source":{"op":"count"}}]}"#,
        ));
        assert_refused(&m, "invalid_name");
    }

    #[test]
    fn check_refuses_a_zero_page_size() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titre","page_size":0}"#,
        ));
        assert_refused(&m, "invalid_page_size");
    }

    // ---------- check : action absente, cible absente ----------

    #[test]
    fn check_refuses_a_button_or_a_capability_request_on_an_unknown_action() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"button","entity":"invoice","action":"ghost"}"#,
        ));
        assert_refused(&m, "view_action_unknown");

        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"capability_request","capability":"camera","action":"ghost"}"#,
        ));
        assert_refused(&m, "view_action_unknown");
    }

    #[test]
    fn check_refuses_a_form_submitting_an_unknown_action() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"form","entity":"invoice","fields":[{"field":"titre"}],"submit":"ghost"}"#,
        ));
        assert_refused(&m, "view_action_unknown");
    }

    #[test]
    fn check_refuses_a_click_or_a_navigate_to_an_unknown_view() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titre","on_click":{"kind":"navigate","view":"ghost"}}"#,
        ));
        assert_refused(&m, "view_target_unknown");

        let mut m = module();
        m.actions.push(Action::Navigate {
            name: "go".into(),
            view: "ghost".into(),
        });
        assert_refused(&m, "view_target_unknown");
    }

    // ---------- check : identité et remplacement ----------

    #[test]
    fn check_refuses_a_duplicate_view_name() {
        let mut m = module();
        m.views
            .push(leaf_view("x", r#"{"type":"metrics","entity":"invoice"}"#));
        m.views
            .push(leaf_view("x", r#"{"type":"metrics","entity":"invoice"}"#));
        assert_refused(&m, "duplicate_view");
    }

    #[test]
    fn check_refuses_a_duplicate_action_name() {
        let mut m = module();
        m.actions.push(Action::Delete {
            name: "d".into(),
            entity: "invoice".into(),
        });
        m.actions.push(Action::Delete {
            name: "d".into(),
            entity: "invoice".into(),
        });
        assert_refused(&m, "duplicate_action");
    }

    #[test]
    fn check_refuses_an_invalid_action_name() {
        let mut m = module();
        m.actions.push(Action::Delete {
            name: "Supprimer tout".into(),
            entity: "invoice".into(),
        });
        assert_refused(&m, "invalid_name");
    }

    #[test]
    fn check_refuses_a_view_replacing_an_unknown_view() {
        let mut m = module();
        m.views.push(ViewDesc {
            replaces: Some("ghost".into()),
            ..leaf_view("new_list", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert_refused(&m, "view_target_unknown");
    }

    #[test]
    fn check_refuses_a_view_replacing_itself() {
        let mut m = module();
        m.views.push(ViewDesc {
            replaces: Some("x".into()),
            ..leaf_view("x", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert_refused(&m, "view_replaces_self");
    }

    #[test]
    fn check_refuses_two_views_replacing_the_same_one() {
        let mut m = module();
        m.views
            .push(leaf_view("old", r#"{"type":"metrics","entity":"invoice"}"#));
        m.views.push(ViewDesc {
            replaces: Some("old".into()),
            ..leaf_view("a", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            replaces: Some("old".into()),
            ..leaf_view("b", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert_refused(&m, "view_replaced_twice");
    }

    #[test]
    fn check_refuses_a_replacement_cycle() {
        let mut m = module();
        m.views.push(ViewDesc {
            replaces: Some("b".into()),
            ..leaf_view("a", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            replaces: Some("a".into()),
            ..leaf_view("b", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert!(codes(&m).contains(&"this.descriptor.view_replaces_cycle".to_string()));
    }

    #[test]
    fn a_replacement_chain_without_cycle_passes() {
        let mut m = module();
        m.views
            .push(leaf_view("v1", r#"{"type":"metrics","entity":"invoice"}"#));
        m.views.push(ViewDesc {
            replaces: Some("v1".into()),
            ..leaf_view("v2", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            replaces: Some("v2".into()),
            ..leaf_view("v3", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert!(
            m.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }
}
