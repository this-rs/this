//! Composer un arbre de vues en Rust — le compilateur plutôt que la validation à l'exécution.
//!
//! Un module qui **calcule** son arbre (un compteur, un filtre, une brique choisie selon
//! l'état) l'écrit avec ces types ; `export!(M, render = ma_fonction)` le rend à l'hôte.
//! Le JSON produit est celui du contrat `this-descriptor::views` (`ViewDesc`, `Node`, `Brick`) :
//! un test de conformité (`tests/views.rs`) le relit avec les vrais types du contrat.
//!
//! # Pourquoi les types sont redéfinis ici
//!
//! `this-descriptor` tirerait `chrono`, `uuid`, `semver` et `thiserror` dans un crate wasm, et
//! la forge n'a que `serde` et `serde_json` en liste blanche (une dépendance optionnelle
//! suffirait à casser sa résolution hors ligne). On redéfinit donc la **forme** (sérialisation
//! seule, `deny_unknown_fields` côté hôte la vérifie) et on la prouve conforme par test.
//!
//! # Ce que le typage rend impossible
//!
//! * un nom (entité, champ, action, vue) vide ou mal formé : [`Ident`] ne se construit que
//!   par [`ident!`] (vérifié **à la compilation**) ou [`Ident::new`] (un `Result`) ;
//! * un texte blanc : [`Text`], même principe ([`text!`]) ;
//! * un conteneur vide (`Stack`, `Row`, `Drawer`, `Modal`, `Tabs`, formulaire sans champ) :
//!   les constructeurs exigent un premier élément ;
//! * un indicateur de compteur sans entité, d'activité ou de badge sans champ, une référence
//!   sans cible : un constructeur par forme valide, pas de champ optionnel à oublier.
//!
//! # Ce qui reste vérifié à l'exécution (et pourquoi)
//!
//! [`View::new`] refuse ce qu'aucun type ne dit sans alourdir l'écriture : une profondeur au-delà
//! de [`MAX_VIEW_DEPTH`], deux onglets de même libellé dans un conteneur, une brique que
//! l'en-tête n'accepte pas. Et ce qui dépend du **descripteur** (l'entité existe-t-elle ? le champ ?)
//! reste à l'hôte, qui confronte l'arbre rendu au manifeste.
//!
//! ```
//! use this_module_sdk::views::*;
//! use this_module_sdk::{ident, text};
//!
//! let vue = View::new(
//!     ident!("accueil"),
//!     Surface::Main,
//!     Node::stack(
//!         Node::banner(Tone::Info, text!("Bonjour")),
//!         [Node::text(ident!("tache"), ident!("titre"))],
//!     ),
//! )
//! .unwrap();
//! assert_eq!(vue.name().as_str(), "accueil");
//! ```
//!
//! Un nom invalide ne compile pas :
//!
//! ```compile_fail
//! let _ = this_module_sdk::ident!("Pas Un Nom");
//! ```
//!
//! ```compile_fail
//! let _ = this_module_sdk::text!("   ");
//! ```

use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::num::NonZeroU32;

/// Profondeur maximale de conteneurs acceptée par l'hôte (`this-descriptor::views::MAX_VIEW_DEPTH`).
pub const MAX_VIEW_DEPTH: usize = 16;

/// Ce qui empêche de construire un nom, un texte ou une vue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// Un identifiant doit commencer par `a-z`, puis `a-z`, `0-9` ou `_`.
    BadIdent(String),
    /// Un texte ne peut pas être vide ni blanc.
    BlankText,
    /// Des conteneurs imbriqués au-delà de [`MAX_VIEW_DEPTH`].
    TooDeep,
    /// Deux onglets du même conteneur portent ce libellé.
    DuplicateTab(String),
    /// L'en-tête n'accepte que `text`, `banner`, `indicator` et `button`.
    HeaderBrick(&'static str),
    /// Un module qui rend sans rien rendre.
    NoView,
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewError::BadIdent(s) => {
                write!(f, "`{s}` n'est pas un identifiant (a-z puis a-z, 0-9, _)")
            }
            ViewError::BlankText => f.write_str("un texte ne peut pas être vide"),
            ViewError::TooDeep => write!(f, "plus de {MAX_VIEW_DEPTH} niveaux de conteneurs"),
            ViewError::DuplicateTab(l) => write!(f, "l'onglet `{l}` apparaît deux fois"),
            ViewError::HeaderBrick(k) => {
                write!(f, "la brique `{k}` n'a pas sa place dans un en-tête")
            }
            ViewError::NoView => f.write_str("aucune vue rendue"),
        }
    }
}

impl std::error::Error for ViewError {}

const fn is_ident(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_lowercase() {
        return false;
    }
    let mut i = 1;
    while i < b.len() {
        let c = b[i];
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_') {
            return false;
        }
        i += 1;
    }
    true
}

const fn is_blank(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_whitespace() {
            return false;
        }
        i += 1;
    }
    true
}

/// Un nom du contrat : entité, champ, action, vue, lien.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Ident(String);

impl Ident {
    /// Un nom lu à l'exécution : un `Result`, jamais un panic.
    pub fn new(s: impl AsRef<str>) -> Result<Ident, ViewError> {
        let s = s.as_ref();
        if is_ident(s) {
            Ok(Ident(s.to_string()))
        } else {
            Err(ViewError::BadIdent(s.to_string()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[doc(hidden)]
    pub const fn __checked(s: &'static str) -> bool {
        is_ident(s)
    }
    #[doc(hidden)]
    pub fn __trusted(s: &'static str) -> Ident {
        Ident(s.to_string())
    }
}

/// Un texte affiché : jamais vide ni blanc.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Text(String);

impl Text {
    /// Un texte calculé : refusé s'il est blanc.
    pub fn new(s: impl Into<String>) -> Result<Text, ViewError> {
        let s = s.into();
        if is_blank(&s) {
            Err(ViewError::BlankText)
        } else {
            Ok(Text(s))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[doc(hidden)]
    pub const fn __checked(s: &'static str) -> bool {
        !is_blank(s)
    }
    #[doc(hidden)]
    pub fn __trusted(s: &'static str) -> Text {
        Text(s.to_string())
    }
}

/// Un [`Ident`] littéral, **vérifié à la compilation**.
#[macro_export]
macro_rules! ident {
    ($s:literal) => {{
        const {
            assert!(
                $crate::views::Ident::__checked($s),
                "identifiant invalide : a-z puis a-z, 0-9, _"
            )
        };
        $crate::views::Ident::__trusted($s)
    }};
}

/// Un [`Text`] littéral, **vérifié à la compilation**.
#[macro_export]
macro_rules! text {
    ($s:literal) => {{
        const { assert!($crate::views::Text::__checked($s), "texte vide") };
        $crate::views::Text::__trusted($s)
    }};
}

/// Où la vue se dessine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Sidebar,
    Main,
    Panel,
    Header,
    Chat,
    Widget,
    LockWidget,
    Activity,
    Island,
}

/// Ton d'une bannière ou d'un avis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

/// Comparaison d'un filtre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Contains,
    StartsWith,
}

/// Un filtre de lignes : la forme de `this-descriptor::FilterExpr`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Filter(FilterRepr);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FilterRepr {
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
    Cmp { field: Ident, op: Op, value: Value },
}

impl Filter {
    pub fn cmp(field: Ident, op: Op, value: impl Into<Value>) -> Filter {
        Filter(FilterRepr::Cmp {
            field,
            op,
            value: value.into(),
        })
    }
    pub fn eq(field: Ident, value: impl Into<Value>) -> Filter {
        Filter::cmp(field, Op::Eq, value)
    }
    pub fn and(first: Filter, rest: impl IntoIterator<Item = Filter>) -> Filter {
        Filter(FilterRepr::And(
            std::iter::once(first).chain(rest).collect(),
        ))
    }
    pub fn or(first: Filter, rest: impl IntoIterator<Item = Filter>) -> Filter {
        Filter(FilterRepr::Or(std::iter::once(first).chain(rest).collect()))
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(inner: Filter) -> Filter {
        Filter(FilterRepr::Not(Box::new(inner)))
    }
}

/// Une brique du contrat. Elle ne se construit que par les constructeurs ci-dessous.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Brick(BrickRepr);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Signal {
    Connection,
    Activity,
    Counter,
    Badge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Input {
    Text,
    Multiline,
    Number,
    File,
    Reference,
    Toggle,
}

/// Mode d'une relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationMode {
    Show,
    Edit,
}

/// Ce qui se passe au clic d'une ligne.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Click {
    Select,
    OpenDetail,
    Navigate { view: Ident },
}

/// Un champ de formulaire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FormField {
    field: Ident,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<Text>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    read_only: bool,
}

impl FormField {
    pub fn new(field: Ident) -> FormField {
        FormField {
            field,
            label: None,
            read_only: false,
        }
    }
    pub fn label(mut self, label: Text) -> FormField {
        self.label = Some(label);
        self
    }
    pub fn read_only(mut self) -> FormField {
        self.read_only = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrickRepr {
    Text {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        field: Ident,
    },
    Button {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        action: Ident,
    },
    Banner {
        tone: Tone,
        text: Text,
    },
    Indicator {
        signal: Signal,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity: Option<Ident>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        #[serde(skip_serializing_if = "Option::is_none")]
        field: Option<Ident>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<Ident>,
    },
    List {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        label: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        subtitle: Option<Ident>,
        #[serde(skip_serializing_if = "Option::is_none")]
        empty: Option<Text>,
        #[serde(skip_serializing_if = "Option::is_none")]
        page_size: Option<NonZeroU32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        on_click: Option<Click>,
    },
    Card {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        fields: Vec<Ident>,
    },
    Form {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        fields: Vec<FormField>,
        #[serde(skip_serializing_if = "Option::is_none")]
        submit: Option<Ident>,
    },
    Field {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        field: Ident,
        input: Input,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<Ident>,
    },
    Relation {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        link: Ident,
        mode: RelationMode,
    },
    Chat {
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
    },
    Notice {
        tone: Tone,
        entity: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Filter>,
        field: Ident,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<Ident>,
    },
}

impl BrickRepr {
    fn kind(&self) -> &'static str {
        match self {
            BrickRepr::Text { .. } => "text",
            BrickRepr::Button { .. } => "button",
            BrickRepr::Banner { .. } => "banner",
            BrickRepr::Indicator { .. } => "indicator",
            BrickRepr::List { .. } => "list",
            BrickRepr::Card { .. } => "card",
            BrickRepr::Form { .. } => "form",
            BrickRepr::Field { .. } => "field",
            BrickRepr::Relation { .. } => "relation",
            BrickRepr::Chat { .. } => "chat",
            BrickRepr::Notice { .. } => "notice",
        }
    }
    fn header_ok(&self) -> bool {
        matches!(
            self,
            BrickRepr::Text { .. }
                | BrickRepr::Button { .. }
                | BrickRepr::Banner { .. }
                | BrickRepr::Indicator { .. }
        )
    }
    fn filter_slot(&mut self) -> Option<&mut Option<Filter>> {
        match self {
            BrickRepr::Banner { .. } => None,
            BrickRepr::Text { filter, .. }
            | BrickRepr::Button { filter, .. }
            | BrickRepr::Indicator { filter, .. }
            | BrickRepr::List { filter, .. }
            | BrickRepr::Card { filter, .. }
            | BrickRepr::Form { filter, .. }
            | BrickRepr::Field { filter, .. }
            | BrickRepr::Relation { filter, .. }
            | BrickRepr::Chat { filter, .. }
            | BrickRepr::Notice { filter, .. } => Some(filter),
        }
    }
}

impl Brick {
    /// Une valeur : le champ de la première ligne du filtre.
    pub fn text(entity: Ident, field: Ident) -> Brick {
        Brick(BrickRepr::Text {
            entity,
            filter: None,
            field,
        })
    }
    /// Un bouton qui déclenche une action déclarée par le module.
    pub fn button(entity: Ident, action: Ident) -> Brick {
        Brick(BrickRepr::Button {
            entity,
            filter: None,
            action,
        })
    }
    /// Une bannière : un ton, un texte.
    pub fn banner(tone: Tone, text: Text) -> Brick {
        Brick(BrickRepr::Banner { tone, text })
    }
    /// L'état de la connexion (seul indicateur sans entité).
    pub fn connection() -> Brick {
        Brick::indicator(Signal::Connection, None, None)
    }
    /// L'activité : la phase est `field`, la durée `duration`.
    pub fn activity(entity: Ident, phase: Ident, duration: Option<Ident>) -> Brick {
        let mut b = Brick::indicator(Signal::Activity, Some(entity), Some(phase));
        if let BrickRepr::Indicator { duration: d, .. } = &mut b.0 {
            *d = duration;
        }
        b
    }
    /// Un compteur : le nombre de lignes du filtre, ou `field` si donné.
    pub fn counter(entity: Ident, field: Option<Ident>) -> Brick {
        Brick::indicator(Signal::Counter, Some(entity), field)
    }
    /// Un badge : la valeur de `field`.
    pub fn badge(entity: Ident, field: Ident) -> Brick {
        Brick::indicator(Signal::Badge, Some(entity), Some(field))
    }
    fn indicator(signal: Signal, entity: Option<Ident>, field: Option<Ident>) -> Brick {
        Brick(BrickRepr::Indicator {
            signal,
            entity,
            filter: None,
            field,
            duration: None,
        })
    }
    /// Les lignes d'une entité ; `label` est le champ affiché.
    pub fn list(entity: Ident, label: Ident) -> Brick {
        Brick(BrickRepr::List {
            entity,
            filter: None,
            label,
            subtitle: None,
            empty: None,
            page_size: None,
            on_click: None,
        })
    }
    /// Une fiche : au moins un champ.
    pub fn card(entity: Ident, first: Ident, rest: impl IntoIterator<Item = Ident>) -> Brick {
        Brick(BrickRepr::Card {
            entity,
            filter: None,
            fields: std::iter::once(first).chain(rest).collect(),
        })
    }
    /// Un formulaire : au moins un champ.
    pub fn form(
        entity: Ident,
        first: FormField,
        rest: impl IntoIterator<Item = FormField>,
    ) -> Brick {
        Brick(BrickRepr::Form {
            entity,
            filter: None,
            fields: std::iter::once(first).chain(rest).collect(),
            submit: None,
        })
    }
    fn field(entity: Ident, field: Ident, input: Input, target: Option<Ident>) -> Brick {
        Brick(BrickRepr::Field {
            entity,
            filter: None,
            field,
            input,
            target,
        })
    }
    pub fn field_text(entity: Ident, field: Ident) -> Brick {
        Brick::field(entity, field, Input::Text, None)
    }
    pub fn field_multiline(entity: Ident, field: Ident) -> Brick {
        Brick::field(entity, field, Input::Multiline, None)
    }
    pub fn field_number(entity: Ident, field: Ident) -> Brick {
        Brick::field(entity, field, Input::Number, None)
    }
    pub fn field_toggle(entity: Ident, field: Ident) -> Brick {
        Brick::field(entity, field, Input::Toggle, None)
    }
    pub fn field_file(entity: Ident, field: Ident) -> Brick {
        Brick::field(entity, field, Input::File, None)
    }
    /// Une référence vers `target` : la cible est obligatoire, donc exigée ici.
    pub fn field_reference(entity: Ident, field: Ident, target: Ident) -> Brick {
        Brick::field(entity, field, Input::Reference, Some(target))
    }
    /// Une relation à travers un lien déclaré.
    pub fn relation(entity: Ident, link: Ident, mode: RelationMode) -> Brick {
        Brick(BrickRepr::Relation {
            entity,
            filter: None,
            link,
            mode,
        })
    }
    /// Le fil d'une conversation.
    pub fn chat(entity: Ident) -> Brick {
        Brick(BrickRepr::Chat {
            entity,
            filter: None,
        })
    }
    /// Un avis lu dans une ligne ; un champ vide n'annonce rien.
    pub fn notice(tone: Tone, entity: Ident, field: Ident) -> Brick {
        Brick(BrickRepr::Notice {
            tone,
            entity,
            filter: None,
            field,
            action: None,
        })
    }

    /// Restreint les lignes lues. Sans effet sur une bannière (elle ne lit rien).
    pub fn filter(mut self, f: Filter) -> Brick {
        if let Some(slot) = self.0.filter_slot() {
            *slot = Some(f);
        }
        self
    }
    /// Un sous-titre de ligne (liste seulement ; sans effet ailleurs).
    pub fn subtitle(mut self, field: Ident) -> Brick {
        if let BrickRepr::List { subtitle, .. } = &mut self.0 {
            *subtitle = Some(field);
        }
        self
    }
    /// Le texte d'une liste vide (liste seulement).
    pub fn empty(mut self, text: Text) -> Brick {
        if let BrickRepr::List { empty, .. } = &mut self.0 {
            *empty = Some(text);
        }
        self
    }
    /// Taille de page d'une liste ; non nulle par le type.
    pub fn page_size(mut self, n: NonZeroU32) -> Brick {
        if let BrickRepr::List { page_size, .. } = &mut self.0 {
            *page_size = Some(n);
        }
        self
    }
    /// Le clic d'une ligne (liste seulement).
    pub fn on_click(mut self, c: Click) -> Brick {
        if let BrickRepr::List { on_click, .. } = &mut self.0 {
            *on_click = Some(c);
        }
        self
    }
    /// L'action de soumission d'un formulaire, ou le bouton d'un avis.
    pub fn action(mut self, name: Ident) -> Brick {
        match &mut self.0 {
            BrickRepr::Form { submit, .. } => *submit = Some(name),
            BrickRepr::Notice { action, .. } => *action = Some(name),
            _ => {}
        }
        self
    }
}

/// Un onglet : un libellé et son arbre (au moins un nœud).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tab {
    label: Text,
    children: Vec<Node>,
}

impl Tab {
    pub fn new(label: Text, first: Node, rest: impl IntoIterator<Item = Node>) -> Tab {
        Tab {
            label,
            children: std::iter::once(first).chain(rest).collect(),
        }
    }
}

/// Un nœud de l'arbre : un conteneur non vide, ou une feuille.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Node(NodeRepr);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NodeRepr {
    Stack { children: Vec<Node> },
    Row { children: Vec<Node> },
    Tabs { tabs: Vec<Tab> },
    Drawer { label: Text, children: Vec<Node> },
    Modal { title: Text, children: Vec<Node> },
    Leaf { brick: Box<Brick> },
}

fn some(first: Node, rest: impl IntoIterator<Item = Node>) -> Vec<Node> {
    std::iter::once(first).chain(rest).collect()
}

impl Node {
    pub fn stack(first: Node, rest: impl IntoIterator<Item = Node>) -> Node {
        Node(NodeRepr::Stack {
            children: some(first, rest),
        })
    }
    pub fn row(first: Node, rest: impl IntoIterator<Item = Node>) -> Node {
        Node(NodeRepr::Row {
            children: some(first, rest),
        })
    }
    pub fn tabs(first: Tab, rest: impl IntoIterator<Item = Tab>) -> Node {
        Node(NodeRepr::Tabs {
            tabs: std::iter::once(first).chain(rest).collect(),
        })
    }
    pub fn drawer(label: Text, first: Node, rest: impl IntoIterator<Item = Node>) -> Node {
        Node(NodeRepr::Drawer {
            label,
            children: some(first, rest),
        })
    }
    pub fn modal(title: Text, first: Node, rest: impl IntoIterator<Item = Node>) -> Node {
        Node(NodeRepr::Modal {
            title,
            children: some(first, rest),
        })
    }
    pub fn leaf(brick: Brick) -> Node {
        Node(NodeRepr::Leaf {
            brick: Box::new(brick),
        })
    }
    /// Raccourcis de feuille.
    pub fn text(entity: Ident, field: Ident) -> Node {
        Node::leaf(Brick::text(entity, field))
    }
    pub fn banner(tone: Tone, text: Text) -> Node {
        Node::leaf(Brick::banner(tone, text))
    }
    pub fn button(entity: Ident, action: Ident) -> Node {
        Node::leaf(Brick::button(entity, action))
    }

    fn check(&self, depth: usize, header: bool) -> Result<(), ViewError> {
        if depth > MAX_VIEW_DEPTH {
            return Err(ViewError::TooDeep);
        }
        let all = |c: &[Node]| c.iter().try_for_each(|n| n.check(depth + 1, header));
        match &self.0 {
            NodeRepr::Stack { children }
            | NodeRepr::Row { children }
            | NodeRepr::Drawer { children, .. }
            | NodeRepr::Modal { children, .. } => all(children),
            NodeRepr::Tabs { tabs } => {
                for (i, t) in tabs.iter().enumerate() {
                    if tabs[..i].iter().any(|u| u.label == t.label) {
                        return Err(ViewError::DuplicateTab(t.label.0.clone()));
                    }
                    all(&t.children)?;
                }
                Ok(())
            }
            NodeRepr::Leaf { brick } => {
                if header && !brick.0.header_ok() {
                    Err(ViewError::HeaderBrick(brick.0.kind()))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Une vue : un nom, un emplacement, un arbre vérifié.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    name: Ident,
    surface: Surface,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<Text>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces: Option<Ident>,
    root: Node,
}

impl View {
    /// Vérifie ce que le typage ne dit pas : profondeur, onglets distincts, briques d'en-tête.
    pub fn new(name: Ident, surface: Surface, root: Node) -> Result<View, ViewError> {
        root.check(1, surface == Surface::Header)?;
        Ok(View {
            name,
            surface,
            title: None,
            replaces: None,
            root,
        })
    }
    pub fn title(mut self, title: Text) -> View {
        self.title = Some(title);
        self
    }
    /// La vue de ce module que celle-ci remplace.
    pub fn replaces(mut self, view: Ident) -> View {
        self.replaces = Some(view);
        self
    }
    pub fn name(&self) -> &Ident {
        &self.name
    }
}

/// Ce qu'une fonction de rendu peut rendre : une vue, des vues, ou un `Result` de l'un d'eux.
pub trait IntoViews {
    fn into_views(self) -> Result<Vec<View>, String>;
}

impl IntoViews for View {
    fn into_views(self) -> Result<Vec<View>, String> {
        Ok(vec![self])
    }
}

impl IntoViews for Vec<View> {
    fn into_views(self) -> Result<Vec<View>, String> {
        if self.is_empty() {
            Err(ViewError::NoView.to_string())
        } else {
            Ok(self)
        }
    }
}

impl<T: IntoViews, E: fmt::Display> IntoViews for Result<T, E> {
    fn into_views(self) -> Result<Vec<View>, String> {
        match self {
            Ok(v) => v.into_views(),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// La vue que l'hôte reçoit quand le rendu échoue : une bannière d'erreur qui dit pourquoi.
/// C'est un arbre **valide** : un échec de rendu se lit à l'écran, il ne se perd pas en refus muet.
pub fn error_view(message: &str) -> View {
    let text = Text::new(message).unwrap_or_else(|_| Text("erreur de rendu".to_string()));
    View {
        name: Ident("render_error".to_string()),
        surface: Surface::Main,
        title: None,
        replaces: None,
        root: Node::banner(Tone::Error, text),
    }
}
