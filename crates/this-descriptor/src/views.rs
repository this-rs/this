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
//! - `chat` : une brique `chat` (fil de messages et saisie) ; la brique `composer` déclare ce
//!   que la saisie du fil offre (invite, pièces jointes, actions du module).
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
use crate::field::FieldType;
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
    ///
    /// Un hôte qui trouve **au moins une** vue `sidebar` déclarée dessine son menu avec
    /// elles, dans l'ordre déclaré, et n'y ajoute **rien de lui-même** : un module décide
    /// donc de ce que le menu offre — les conversations, les entrées, le pied — et chacune
    /// avec l'action qu'elle déclenche. Sans aucune vue `sidebar`, l'hôte garde son menu
    /// d'avant, mot pour mot.
    ///
    /// Ce qui reste à l'hôte est ce qu'aucune vue ne peut dire : le geste de bord, le bouton
    /// qui ouvre le menu, l'ouverture et la fermeture du panneau. Une vue `sidebar` qui
    /// remplace une autre (voir [`ViewDesc::replaces`]) la retire : c'est ainsi qu'un module
    /// prend la place d'un autre dans le menu sans le nommer.
    Sidebar,
    /// L'écran principal : ce qu'on voit en ouvrant le module.
    #[default]
    Main,
    /// Un panneau : à côté du contenu principal, ou par-dessus.
    Panel,
    /// L'en-tête : une barre fine ; seules les petites briques y ont leur place.
    Header,
    /// Le fil de conversation : la surface où l'échange entre la personne, l'agent et les
    /// modules se déroule. C'est la même que les autres — mêmes briques, mêmes actions —
    /// posée dans le chat au lieu d'un écran. Sans elle, le fil restait le seul endroit de
    /// l'app qu'aucun module ne pouvait dessiner, ni l'utilisateur modifier.
    Chat,

    // ---- Les surfaces d'un AUTRE hôte ------------------------------------------------
    //
    // Les quatre ci-dessus sont l'écran de l'app : elle décide où la vue se place. Celles
    // qui suivent ne se dessinent PAS dans l'app : un autre processus s'en charge, avec le
    // même vocabulaire de briques. C'est ce qui rend un module capable de décrire un widget
    // ou une activité sans savoir qui le peindra — et l'hôte, lui, dit franchement ce qu'il
    // sait rendre (voir la carte des capacités).
    /// Le widget de l'écran d'accueil (WidgetKit sur iOS, App Widget sur Android).
    Widget,
    /// Le widget de l'écran de verrouillage (iOS 16+).
    LockWidget,
    /// Une Live Activity : l'état poussé par l'app, pas une vue de l'écran.
    Activity,
    /// La Dynamic Island, qui est un jeu de dispositions d'une Live Activity — et non une
    /// surface à part : les tailles (compacte, minimale, étendue) sont imposées par l'OS.
    Island,
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
    /// Une vue VIVANTE : l'hôte rafraîchit ses lignes à cette cadence tant qu'elle est affichée.
    /// Absent = comportement inchangé (rafraîchie sur écriture, jamais par minuterie).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<Live>,
    /// L'arbre dessiné.
    pub root: Node,
}

/// Borne basse de [`Live::every_secs`].
pub const LIVE_MIN_SECS: u32 = 5;
/// Borne haute de [`Live::every_secs`].
pub const LIVE_MAX_SECS: u32 = 3600;

/// La cadence d'une vue vivante : `{ "every_secs": 30 }`.
///
/// **C'est un SONDAGE, pas du push.** L'hôte relit les lignes de la vue toutes les
/// `every_secs` secondes, seulement quand elle est affichée et que l'app est au premier plan,
/// sans chevaucher deux lectures ; un échec est journalisé et la cadence continue.
///
/// Pour du VRAI temps réel il faudrait : un flux (SSE/WebSocket) côté hôte, un port `Watch`
/// dans le framework, et un serveur qui émet. Rien de cela n'existe : le graphe est local, la
/// forge est HTTP ponctuelle, aucun module n'émet de flux. `live` ne doit donc jamais être lu
/// comme une promesse de push.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Live {
    pub every_secs: u32,
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
    /// Une page web que le MODULE livre : un fichier de son dossier VFS, montée par l'hôte
    /// dans une iframe sandboxée (sans `allow-same-origin`).
    ///
    /// C'est la porte ILLIMITÉE de l'interface : tout ce que les briques ne disent pas, le
    /// module l'écrit en HTML/CSS/JS. Le prix : plus de thème imposé, plus de contraste
    /// vérifié, plus de cible tactile de 44 px — et un intérieur que l'hôte ne peut pas
    /// mesurer. Le pont est explicite : l'hôte envoie `{type:"this:data", rows, theme,
    /// locale}`, la page renvoie `{type:"this:intent", …}`, validé comme une intention.
    ///
    /// `path` est relatif au module : `ui/index.html`, ou le chemin complet
    /// `/modules/<nom>/<version>/ui/index.html`. Hors du module, `..`, extension autre que
    /// `.html`/`.htm` (un `.svg` est refusé : il peut porter ses propres scripts), ou
    /// placement dans un `header` : refusés, nommément.
    Html {
        path: String,
        /// Hauteur en px. Absente = hauteur intrinsèque ; l'hôte borne à
        /// [`HTML_MAX_HEIGHT`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        /// Les données que la page demande, EXPLICITEMENT : au plus [`HTML_DATA_MAX_GRANTS`]
        /// demandes, une par entité. Sans elles, la page ne reçoit que ce que les AUTRES
        /// nœuds de la vue lisent — donc rien si elle est seule. Avec elles, `rows` porte
        /// exactement ces entités, filtrées et projetées : voir [`DataGrant`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        data: Vec<DataGrant>,
    },
    /// « Montre `child` seulement si » : une condition de visibilité posée sur UN nœud — une
    /// brique, ou un groupe entier.
    ///
    /// La condition se dit comme un filtre de brique : `entity` et `filter` (voir
    /// [`FilterExpr`]). L'hôte l'évalue contre ce qu'il sait déjà, **avant** de dessiner :
    /// `child` est dessiné si et seulement si au moins une ligne de `entity` satisfait
    /// `filter`. Rien ne ressemble à un état :
    ///
    /// - faux (aucune ligne ne satisfait) ⇒ `child` n'est pas dessiné ;
    /// - donnée absente (entité pas encore lue, ou aucune ligne) ⇒ pas dessiné : une
    ///   condition qu'on ne peut pas évaluer ne montre pas un bouton au hasard ;
    /// - lecture en échec ou refusée ⇒ pas dessiné, et l'hôte nomme le refus à son journal.
    ///
    /// Un nœud masqué ne compte pas dans le verdict de la vue (voir `ViewState`) : un arbre
    /// dont tous les nœuds conditionnels sont masqués se dessine, vide, sans erreur.
    When {
        entity: String,
        filter: FilterExpr,
        child: Box<Node>,
    },
}

/// Une demande de données d'un nœud [`Node::Html`] : ce que la page lira dans `rows`.
///
/// ```json
/// {"entity":"tache","filter":{"cmp":{"field":"fait","op":"eq","value":false}},
///  "fields":["titre","fait"],"limit":200}
/// ```
///
/// - `entity` (requis) : une entité du module, ou d'un module qu'il requiert ;
/// - `filter` : la forme d'un filtre de liste ([`FilterExpr`]) ;
/// - `fields` : les champs DÉCLARÉS envoyés, plus `id` si on le demande. Absent = tous les
///   champs déclarés de l'entité, sans `id` ni champ de base. Voir [`html_field_verdict`] ;
/// - `limit` : 1..=[`HTML_DATA_MAX_LIMIT`], défaut [`HTML_DATA_DEFAULT_LIMIT`].
///
/// C'est la frontière de sécurité de la page : ce qui n'est pas demandé ici ne franchit pas
/// l'iframe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataGrant {
    pub entity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Les champs de base d'une ligne, posés par l'hôte et non déclarés par le module.
pub const HTML_BASE_FIELDS: [&str; 4] = ["id", "created_at", "updated_at", "deleted_at"];

/// Ce qu'une demande `fields` d'un nœud [`Node::Html`] peut nommer, et pourquoi.
///
/// - un champ DÉCLARÉ de l'entité : accordé ;
/// - `id` : accordé **s'il est demandé**, jamais implicitement. Il sert à CIBLER : sans lui
///   une page ne peut pas émettre `Update { id }` ni `Delete { id }` sur la ligne qu'elle
///   affiche. Un `fields` absent ne le porte pas : la règle reste « on ne reçoit que ce
///   qu'on demande » ;
/// - tout autre champ de base (`created_at`, `updated_at`, `deleted_at`) : refusé,
///   `html_data_field_reserved`. C'est une métadonnée de l'hôte, qui ne sert à cibler rien
///   et n'a pas à franchir l'iframe ;
/// - le reste : `html_data_field_unknown`.
pub fn html_field_verdict(field: &str, declared: &[String]) -> Result<(), &'static str> {
    if declared.iter().any(|d| d == field) || field == "id" {
        Ok(())
    } else if HTML_BASE_FIELDS.contains(&field) {
        Err("html_data_field_reserved")
    } else {
        Err("html_data_field_unknown")
    }
}

/// Le nombre maximal de demandes de données d'un nœud [`Node::Html`].
pub const HTML_DATA_MAX_GRANTS: usize = 5;
/// La limite de lignes d'une demande qui n'en dit pas.
pub const HTML_DATA_DEFAULT_LIMIT: u32 = 200;
/// La limite de lignes la plus haute qu'une demande puisse porter.
pub const HTML_DATA_MAX_LIMIT: u32 = 1000;

impl DataGrant {
    /// La limite effective : celle de la demande, ou le défaut, **bornée** à
    /// `1..=`[`HTML_DATA_MAX_LIMIT`] (un descripteur qui aurait contourné la validation ne
    /// sort pas du cadre).
    pub fn effective_limit(&self) -> usize {
        self.limit
            .unwrap_or(HTML_DATA_DEFAULT_LIMIT)
            .clamp(1, HTML_DATA_MAX_LIMIT) as usize
    }
}

/// La hauteur maximale d'un nœud [`Node::Html`], en px.
pub const HTML_MAX_HEIGHT: u32 = 2000;

/// Les extensions qu'un nœud [`Node::Html`] accepte de servir. Un `.svg` n'y est pas : il
/// peut porter ses propres scripts et références externes qu'un `<meta>` CSP ne couvre pas —
/// il est refusé nommément (`html_svg_refused`) et se sert comme image, pas comme page.
pub const HTML_EXTENSIONS: [&str; 2] = [".html", ".htm"];

/// Résout le `path` d'un nœud `html` contre le dossier du module `<name>@<version>` :
/// `Ok(chemin VFS complet)` ou `Err(raison, message)`.
pub fn html_resolve(
    name: &str,
    version: &str,
    path: &str,
) -> Result<String, (&'static str, String)> {
    if path.trim().is_empty() {
        return Err((
            "html_path_empty",
            "an `html` node must name the file it serves, e.g. `ui/index.html`".into(),
        ));
    }
    let racine = format!("/modules/{name}/{version}/");
    let relatif = if let Some(r) = path.strip_prefix(&racine) {
        r
    } else if path.starts_with('/') {
        return Err((
            "html_path_outside_module",
            format!(
                "`{path}` is outside this module; an `html` file lives under `{racine}`. \
                 Use a path relative to the module, e.g. `ui/index.html`"
            ),
        ));
    } else {
        path
    };
    if relatif.is_empty()
        || relatif
            .split('/')
            .any(|seg| seg == ".." || seg == "." || seg.is_empty() || seg.contains('\\'))
    {
        return Err((
            "html_path_outside_module",
            format!(
                "`{path}` climbs out of the module (`..`) or is not a clean relative path; \
                 name a file inside the module folder"
            ),
        ));
    }
    let bas = relatif.to_ascii_lowercase();
    if bas.ends_with(".svg") {
        return Err((
            "html_svg_refused",
            format!(
                "`{path}` is an SVG: it can carry its own scripts and external references, \
                 and a <meta> CSP does not apply in an SVG document; serve it as an image, \
                 not as an `html` page"
            ),
        ));
    }
    if !HTML_EXTENSIONS.iter().any(|e| bas.ends_with(e)) {
        return Err((
            "html_extension_refused",
            format!(
                "`{path}` is not servable: an `html` node serves only {}",
                HTML_EXTENSIONS.join(", ")
            ),
        ));
    }
    Ok(format!("{racine}{relatif}"))
}

/// Résout le `path` d'une source d'image contre le dossier du module `<name>@<version>` :
/// `Ok(chemin VFS complet)` ou `Err(raison, message)`. Une URL distante (`http(s)://`, `//`)
/// est refusée avant tout : aucune requête réseau ne doit sortir d'une vue.
pub fn image_resolve(
    name: &str,
    version: &str,
    path: &str,
) -> Result<String, (&'static str, String)> {
    let path = path.trim();
    if path.is_empty() {
        return Err((
            "image_vfs_outside_module",
            "an image `vfs` must name the file it serves, e.g. `assets/logo.png`".into(),
        ));
    }
    // Une URL distante n'est pas un chemin de module : elle fuiterait une requête.
    let bas = path.to_ascii_lowercase();
    if bas.starts_with("http://") || bas.starts_with("https://") || path.starts_with("//") {
        return Err((
            "image_remote_refused",
            format!(
                "`{path}` is a remote URL; an image serves a module file or a data URL,                  never a network request"
            ),
        ));
    }
    let racine = format!("/modules/{name}/{version}/");
    let relatif = if let Some(r) = path.strip_prefix(&racine) {
        r
    } else if path.starts_with('/') {
        return Err((
            "image_vfs_outside_module",
            format!(
                "`{path}` is outside this module; an image file lives under `{racine}`.                  Use a path relative to the module, e.g. `assets/logo.png`"
            ),
        ));
    } else {
        path
    };
    if relatif.is_empty()
        || relatif
            .split('/')
            .any(|seg| seg == ".." || seg == "." || seg.is_empty() || seg.contains('\\'))
    {
        return Err((
            "image_vfs_outside_module",
            format!(
                "`{path}` climbs out of the module (`..`) or is not a clean relative path;                  name a file inside the module folder"
            ),
        ));
    }
    Ok(format!("{racine}{relatif}"))
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
    /// Une ligne secondaire : le champ `field` de la première ligne du filtre, petit, atténué,
    /// sans cadre ni fond, sur une seule ligne (tronqué à l'ellipse). Aucune action.
    Caption {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
    },
    /// Un graph borné : une série numérique, UN point par ligne du filtre (dans l'ordre de la
    /// lecture), dessinée en SVG en ligne aux couleurs du thème. `kind` : `bar` ou `line`
    /// (rien d'autre : deux formes qui se dessinent proprement). `field` doit être déclaré
    /// numérique. `height` : [`CHART_HEIGHT_DEFAULT`] par défaut, `80..=400`.
    Chart {
        entity: String,
        field: String,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        /// Le titre du graph : petit, au-dessus.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
    },
    /// Un bouton qui déclenche une action déclarée par le module.
    Button {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        action: String,
        /// Le libellé du bouton, en texte d'interface (« Épingler cette clé »). Il gagne sur
        /// le libellé de l'action ([`Action::label`]), qui gagne sur le nom de l'action
        /// humanisé par l'hôte. Pas d'identifiant, pas de `_`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Une bannière : un ton fermé et un texte déclaré (clé de message ou texte).
    ///
    /// Pour ce que l'hôte peut annoncer sans rien lire. Quand le texte vient d'une LIGNE —
    /// le motif d'arrêt d'un tour, une panne, une phrase d'état — c'est [`Brick::Notice`] :
    /// une bannière au texte lu serait une bannière dont le texte change sans qu'on puisse
    /// le vérifier sur la déclaration.
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
        /// Des actions de ce module, posées en boutons DANS la rangée, chacune exécutée
        /// avec l'identifiant de la ligne. Un clic sur l'un de ces boutons n'ouvre pas la
        /// ligne : `on_click` ne se déclenche pas en même temps.
        ///
        /// C'est le chaînon qui manquait à [`Click`] : un bouton qui VISE une ligne doit
        /// dire LAQUELLE, et une action de ligne produit une intention qui porte l'`id` de
        /// la ligne et le nom de l'action. Seules les actions qui ont besoin d'une ligne y
        /// ont leur place (modifier, supprimer, opération sur l'entité).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        row_actions: Vec<String>,
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
    /// Une bulle par ligne lue : l'auteur (rôle) et le contenu. Filtrée sur UNE ligne, c'est
    /// un message ; filtrée sur la conversation ouverte (`{"$context": "conversation"}`,
    /// voir [`crate::FilterExpr`]), c'est le fil — sans la saisie, que `Chat` porte. C'est la
    /// brique qui permet à un module de redessiner CE QU'UNE BULLE DIT et à quoi elle
    /// ressemble, là où `Chat` ne savait rendre qu'une liste brute de l'entité du module.
    Message {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        /// Le contenu d'une bulle est du **markdown**, rendu par l'hôte.
        ///
        /// Absent (le défaut), le contenu est du texte : ce qu'on lit est ce qui est écrit.
        /// C'est un choix que le module fait, pas une convention que l'hôte impose — un
        /// module qui affiche du JSON, du code ou une phrase contenant `*` garde son texte
        /// intact, et un module qui affiche de la prose d'agent demande le rendu.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        markdown: bool,
        /// La taille de la fenêtre du fil. Absente = tout le fil.
        ///
        /// Un fil se lit par le bas : la brique dessine les `page_size` **derniers**
        /// messages, et un bouton qui élargit la fenêtre d'autant, vers le haut. C'est
        /// l'inverse d'une `list` (qui pagine du début) parce que c'est l'inverse d'un
        /// besoin : personne ne lit une conversation en commençant par la première ligne.
        /// Les messages déjà dessinés ne bougent pas quand la fenêtre s'élargit.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page_size: Option<u32>,
    },
    /// Une étape d'outil, lue sur UNE ligne : son état est un champ.
    ///
    /// Pour un fil dont les étapes sont des lignes à part. Quand une bulle porte ses étapes
    /// dans un champ à elle (une liste JSON), c'est [`Brick::Message`] qui les dessine : ce
    /// vocabulaire-ci n'a pas de « la ligne de la bulle en cours », et une étape ne peut pas
    /// dire à quelle bulle elle appartient si les deux ne sont pas la même ligne.
    ToolStep {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        field: String,
    },
    /// Un avis lu dans une ligne : un ton et un champ.
    ///
    /// C'est la brique des choses qu'un hôte annonce sur l'état d'un tour — le motif d'un
    /// arrêt, une panne, le compte de contexte, une coupure — et qu'il ne peut pas écrire à
    /// l'avance. Elle lit la première ligne du filtre et annonce son champ.
    ///
    /// **Un champ vide n'annonce rien** : la brique ne se dessine pas. C'est la seule façon,
    /// dans ce vocabulaire, de faire apparaître et disparaître un avis : les états d'une
    /// brique (`rows`, `empty`, `failed`, `denied`) parlent de la LECTURE, et aucune
    /// expression de filtre ne dit « si ce champ est vide ». Une lecture en ÉCHEC, elle, se
    /// dit toujours, en ton d'erreur — un avis muet serait un trou.
    ///
    /// `action` pose un bouton dans l'avis : c'est ainsi qu'un bandeau d'arrêt offre
    /// « Continuer ». L'annonce et le geste viennent de la même ligne et disparaissent
    /// ensemble ; deux briques séparées ne pourraient pas le garantir, parce qu'un bouton
    /// ne sait pas encore ne pas se dessiner quand sa ligne est vide.
    Notice {
        tone: Tone,
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        /// Le champ lu : son texte est ce que l'avis annonce.
        field: String,
        /// Une action de ce module, posée en bouton dans l'avis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        /// Le libellé de ce bouton de secours. Même précédence que [`Brick::Button`] :
        /// celui-ci, puis le libellé de l'action, puis son nom humanisé.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// La carte d'une question que l'agent pose et attend, au milieu du fil.
    ///
    /// Une question sans guichet est un silence : l'agent s'arrête, la personne ne sait pas
    /// qu'il attend, et le tour meurt à la fin de son budget. Cette brique déclare la carte
    /// qui rend la question visible — son texte, ses choix, la réponse déjà donnée.
    ///
    /// Elle lit l'entité que le module nomme, et **c'est l'hôte qui la remplit** : les
    /// questions en attente ne sont dans aucune table, elles vivent dans le tour en cours.
    /// L'hôte fournit donc, sous l'entité déclarée ici et dans les champs déclarés ici, une
    /// ligne par question — `id` porte l'identifiant du guichet, par lequel la réponse
    /// revient à l'agent. Le module choisit les noms ; l'hôte les lit dans la brique.
    ///
    /// Un clic sur un choix émet une intention de réponse que l'hôte remet au guichet.
    /// Aucune action du module n'est appelée : répondre à l'agent est le geste de la
    /// personne, pas celui du module.
    Question {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FilterExpr>,
        /// Le champ qui porte la question, en une phrase.
        text: String,
        /// Le champ qui porte les réponses possibles : une liste de textes.
        options: String,
        /// Le champ qui porte la réponse déjà donnée. Absent = la carte n'en montre pas.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
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
    /// Le composeur du fil : ce que la zone de saisie OFFRE, déclaré par le module.
    ///
    /// Ne lit aucune ligne. Le module dit le texte d'invite, s'il accepte des pièces jointes
    /// et quelles de SES actions se posent autour du champ. L'hôte garde ce qui ne peut pas
    /// être déclaratif : le champ réel et son clavier, l'envoi, l'arrêt d'un run, le
    /// glisser-déposer et la file des pièces. Sans cette brique, la saisie était le seul
    /// morceau du fil qu'aucun module ne pouvait toucher.
    Composer {
        /// Le texte d'invite du champ. Absent = celui de l'hôte ; vide = refusé.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        /// Des actions déclarées par ce module, posées en boutons autour du champ, dans
        /// l'ordre. Une action qui exige une ligne (modifier, supprimer, opération sur une
        /// entité) ou une décision d'appareil est refusée : le composeur n'en a pas.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        actions: Vec<String>,
        /// Le composeur accepte-t-il des pièces jointes (bouton `+`, dépôt, collage) ?
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        attachments: bool,
    },
    /// Un écran de réglages : des sections titrées, et dans chacune des lignes qui déclenchent
    /// une action du module (ouvrir un sous-écran, appeler une opération, demander un secret).
    ///
    /// Ne lit aucune ligne. Les autres briques lisent une entité ; les réglages d'une app
    /// (moteurs, autorisations, comptes) n'en sont pas, et aucune brique ne porte un titre de
    /// section ni le libellé d'une action. Un secret n'y a jamais de valeur : la ligne ne
    /// fait que demander à l'hôte de le recueillir (`request_secret`).
    Settings { sections: Vec<SettingsSection> },
    /// Une saisie de secret : la personne tape, la valeur va directement au coffre de
    /// l'hôte.
    ///
    /// L'arbre ne porte QUE le nom du secret (et de quoi l'afficher) : jamais une valeur.
    /// La valeur tapée ne traverse aucune intention de module, aucun `Debug`, aucun journal
    /// — elle n'existe que sur le chemin qui va du champ masqué au coffre de l'hôte.
    ///
    /// C'est le pendant **saisie** de [`Action::RequestSecret`] : l'action demande à l'hôte
    /// de recueillir un secret, la brique est le champ où la personne le tape. Une action
    /// porte un nom de secret fixé à l'avance ; un formulaire de réglages a besoin d'un
    /// champ libre, et ce champ doit rester hors de portée du module — d'où une brique à
    /// part, sans entité et sans action.
    Secret {
        /// Le nom du secret, tel que le coffre de l'hôte le connaît.
        name: String,
        /// Le libellé affiché. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Une aide affichée sous le champ.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
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
    /// Une image : un fichier du module, ou une data URL lue dans une ligne.
    ///
    /// Deux sources, et jamais un réseau :
    /// - `"source": {"vfs": "assets/logo.png"}` : un fichier du module, résolu dans son
    ///   dossier (refusé s'il en sort) ; l'hôte lit les octets et fabrique une data URL (ou
    ///   un Blob URL révoqué au démontage) ;
    /// - `"source": {"entity": "produit", "field": "photo"}` : le champ d'une ligne porte
    ///   une **data URL** (`data:image/png;base64,…`). Jamais une URL distante : le réseau
    ///   est interdit aux vues, et une image distante fuiterait une requête
    ///   (`image_remote_refused`).
    ///
    /// `alt` est requis (une image sans texte alternatif est refusée nommément) ; `height`
    /// est borné `24..=600` ; `fit` est `cover` (défaut) ou `contain`.
    Image {
        /// La source : un fichier du module, ou une data URL lue dans une ligne. Exactement
        /// une des deux (`image_source_missing` si aucune, `image_source_ambiguous` si les deux).
        source: ImageSource,
        /// Le texte alternatif. Requis : une image sans `alt` est un refus nommé.
        alt: String,
        /// Hauteur en px, bornée `24..=600`. Absente = hauteur intrinsèque.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        /// `cover` (défaut) ou `contain`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<ImageFit>,
    },
}

impl Brick {
    /// L'entité que la brique lit ; `None` pour les briques d'état (bannière, demande de
    /// capacité, indicateur de connexion sans entité).
    pub fn entity(&self) -> Option<&str> {
        match self {
            Brick::Banner { .. }
            | Brick::CapabilityRequest { .. }
            | Brick::Composer { .. }
            | Brick::Settings { .. }
            | Brick::Secret { .. } => None,
            Brick::Indicator { entity, .. } => entity.as_deref(),
            Brick::Image { source, .. } => source.entity.as_deref(),
            Brick::Text { entity, .. }
            | Brick::Caption { entity, .. }
            | Brick::Chart { entity, .. }
            | Brick::Button { entity, .. }
            | Brick::Relation { entity, .. }
            | Brick::List { entity, .. }
            | Brick::Card { entity, .. }
            | Brick::Form { entity, .. }
            | Brick::Field { entity, .. }
            | Brick::Chat { entity, .. }
            | Brick::Message { entity, .. }
            | Brick::ToolStep { entity, .. }
            | Brick::Question { entity, .. }
            | Brick::Notice { entity, .. }
            | Brick::Queue { entity, .. }
            | Brick::CodeBlock { entity, .. }
            | Brick::Result { entity, .. }
            | Brick::Metrics { entity, .. }
            | Brick::Program { entity, .. } => Some(entity),
        }
    }
}

/// La source d'une [`Brick::Image`] : un fichier du module, ou une data URL lue dans une
/// ligne. Exactement une des deux — jamais les deux, jamais aucune, jamais une URL distante.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSource {
    /// Le chemin du fichier, relatif au module : `assets/logo.png`, ou le chemin complet
    /// `/modules/<nom>/<version>/assets/logo.png`. Un `..`, un chemin absolu hors du module,
    /// une URL (`http(s)://`, `//`) sont refusés nommément.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vfs: Option<String>,
    /// L'entité dont la première ligne porte la data URL. Exige `field`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// Le champ qui porte la data URL. Exige `entity`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// Comment une [`Brick::Image`] remplit son cadre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Recouvre le cadre, quitte à rogner (défaut).
    #[default]
    Cover,
    /// Entre entière dans le cadre, sans rogner.
    Contain,
}

/// Hauteur minimale d'une image, en px.
pub const IMAGE_HEIGHT_MIN: u32 = 24;
/// Hauteur maximale d'une image, en px.
pub const IMAGE_HEIGHT_MAX: u32 = 600;

/// Une section d'un écran de réglages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSection {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub items: Vec<SettingsItem>,
}

/// Une ligne de réglage : un libellé, une aide, l'action qu'elle déclenche.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsItem {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Une action de ce module, qui n'exige aucune ligne.
    pub action: String,
}

/// Les formes d'un [`Brick::Chart`].
pub const CHART_KINDS: [&str; 2] = ["bar", "line"];
/// Hauteur par défaut d'un graph, en px.
pub const CHART_HEIGHT_DEFAULT: u32 = 120;
/// Hauteur minimale d'un graph, en px.
pub const CHART_HEIGHT_MIN: u32 = 80;
/// Hauteur maximale d'un graph, en px.
pub const CHART_HEIGHT_MAX: u32 = 400;

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
    /// A numeric entry. The field must be declared with a numeric type.
    Number,
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
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        entity: String,
        fields: Vec<String>,
    },
    /// Modifie les champs `fields` d'une ligne de `entity`.
    Update {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        entity: String,
        fields: Vec<String>,
    },
    /// Supprime une ligne de `entity`.
    Delete {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        entity: String,
    },
    /// Ouvre la vue `view` de ce module — ou, avec `module`, une vue d'un AUTRE module.
    ///
    /// `module` doit être déclaré dans `requires[]` du module appelant : un module n'ouvre
    /// pas l'écran d'un module dont il ne dit pas dépendre. La vue d'un autre module ne se
    /// vérifie PAS ici (on n'a pas son descripteur) : c'est l'hôte qui la résout à
    /// l'exécution, et qui refuse avec un message si le module n'est pas installé ou n'a pas
    /// cette vue.
    Navigate {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        view: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        module: Option<String>,
    },
    /// Demande à l'hôte de recueillir le secret `secret` (une clé d'API, un jeton).
    ///
    /// Le module déclare QU'un secret existe et comment il s'appelle ; la saisie, le coffre et
    /// la lecture restent à l'hôte. Aucune valeur ne passe par le module : ni en entrée, ni en
    /// retour, ni dans ce que la vue rend.
    RequestSecret {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        secret: String,
    },
    /// Une opération d'agent : envoyer un message, arrêter un run. Liée à une entité si besoin.
    ///
    /// `send_message` **exige** `text` : l'hôte ne doit pas inventer la demande. Un module
    /// qui dit seulement « envoie un message » obligerait l'hôte à écrire à la place de la
    /// personne, et c'est ainsi qu'on fait dire à un bouton ce que personne n'a demandé.
    AgentTool {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        op: AgentOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Déclenche une opération nommée du module (`operations` de son descripteur).
    ///
    /// C'est le seul bouton qui fait travailler le CODE du module : sans lui, une vue ne
    /// pouvait que créer, modifier, supprimer, naviguer ou parler à l'agent — jamais
    /// exécuter un geste propre. Un refus du module remonte à la personne, il ne devient
    /// pas un silence.
    Invoke {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Nom de l'opération déclarée dans `operations`.
        operation: String,
        /// Entité visée ; `None` = l'opération décide (opération de module).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
    },
    /// Autoriser ou refuser une capacité de l'appareil.
    DeviceCapability {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        capability: DeviceCapability,
        grant: Grant,
    },
    /// Demander à l'hôte d'ouvrir une de SES surfaces (voir [`HostSurface`]).
    ///
    /// Le module ne les possède pas — il les **offre**. C'est lui qui décide si elles
    /// apparaissent dans son menu, sous quel libellé (le nom de l'action, humanisé) et à
    /// quelle place. Sans cette variante, un menu déclaré ne pouvait pas être complet : le
    /// pied du tiroir de l'hôte (modules, modèle, réglages) restait chez l'hôte, et tout
    /// module qui déclarait un menu le faisait disparaître — la personne perdait le chemin
    /// vers ces trois écrans, sans un mot.
    Host {
        name: String,
        /// Le libellé affiché, en texte d'interface. Absent = le nom, humanisé par l'hôte.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        surface: HostSurface,
    },
}

/// Les surfaces de la coquille qu'un module peut **offrir** sans les posséder.
///
/// Trois choses ne se déclarent pas : le registre des modules installés (il décide de ce qui
/// est chargé), le choix du modèle (il tient le fournisseur en cours) et l'écran de réglages
/// de l'app (l'hôte seul sait quel module le porte). Un module qui déclare le menu doit
/// pouvoir les proposer ; il ne peut pas les dessiner, et il n'en invente pas d'autres : le
/// vocabulaire est fermé, et un hôte qui reçoit une surface qu'il ne connaît pas le dit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSurface {
    /// La liste des modules installés : les activer, les retirer, les ouvrir.
    Modules,
    /// Le choix du modèle en cours (les moteurs configurés).
    Model,
    /// L'écran de réglages de l'app — celui que l'hôte sait ouvrir, déclaré ou non.
    Settings,
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
            | Action::Invoke { name, .. }
            | Action::RequestSecret { name, .. }
            | Action::DeviceCapability { name, .. }
            | Action::Host { name, .. } => name,
        }
    }

    /// Le libellé que le module donne à l'action, quand il en donne un.
    ///
    /// **Précédence** de ce que l'hôte affiche sur un bouton qui déclenche l'action : le
    /// `label` de la brique ([`Brick::Button`], [`Brick::Notice`]), puis ce libellé-ci, puis
    /// le nom de l'action humanisé. Un libellé est du texte d'interface : voir
    /// [`label_problem`].
    pub fn label(&self) -> Option<&str> {
        match self {
            Action::Create { label, .. }
            | Action::Update { label, .. }
            | Action::Delete { label, .. }
            | Action::Navigate { label, .. }
            | Action::AgentTool { label, .. }
            | Action::Invoke { label, .. }
            | Action::RequestSecret { label, .. }
            | Action::DeviceCapability { label, .. }
            | Action::Host { label, .. } => label.as_deref(),
        }
    }
}

/// Pourquoi un libellé n'est pas acceptable, ou `None`. Contrôle minimal : le libellé est du
/// TEXTE d'interface, donc il n'est pas vide, ne porte aucun caractère de contrôle (un
/// retour à la ligne n'est pas un libellé) et aucun `_` — c'est la marque d'un identifiant
/// (`epingler_cle`), que l'hôte humanise déjà sans qu'on ait à le déclarer. Rien d'autre
/// n'est refusé : accents, espaces, ponctuation et casse sont à l'auteur.
pub fn label_problem(label: &str) -> Option<LabelProblem> {
    if label.trim().is_empty() {
        Some(LabelProblem::Empty)
    } else if label.chars().any(char::is_control) || label.contains('_') {
        Some(LabelProblem::NotText)
    } else {
        None
    }
}

/// Le défaut d'un libellé, tel que [`label_problem`] le dit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelProblem {
    /// Vide, ou fait d'espaces.
    Empty,
    /// Un caractère de contrôle, ou un `_` : ce n'est pas du texte d'interface.
    NotText,
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
    // Les capacités du pont d'appareil (`this_platform::capability::Capability`), ajoutées le
    // 08/11/2026. Sans elles, une vue de module ne pouvait demander que cinq accès sur
    // vingt et une : ni Bluetooth, ni NFC, ni contacts, ni calendrier, ni santé, ni photos…
    // `Screen` et `DeviceMotion` n'ont PAS d'équivalent dans le pont : un hôte qui les reçoit
    // ne peut rien demander, et doit le dire plutôt que d'inventer une capacité.
    Contacts,
    Calendar,
    Health,
    Bluetooth,
    Gyroscope,
    MotionActivity,
    Nfc,
    LocationAlways,
    Speech,
    FaceId,
    Photos,
    PhotosPick,
    FilesPick,
    Clipboard,
    Share,
    Notifications,
    LiveActivity,
    LocalNetwork,
}

/// Les problèmes de vues et d'actions d'un module, sous forme `(raison, chemin, message)`.
/// `ModuleDescriptor::check` les rend en `this.descriptor.<raison>`.
pub fn issues(m: &ModuleDescriptor) -> Vec<(&'static str, String, String)> {
    let vues: Vec<&ViewDesc> = m.views.iter().collect();
    issues_over(m, vues)
}

/// The modules `m` requires, transitively, taken from `known` (in `known`'s order).
/// `m` itself is never part of the result, and a `requires` cycle terminates.
pub(crate) fn required_modules<'a>(
    m: &ModuleDescriptor,
    known: &[&'a ModuleDescriptor],
) -> Vec<&'a ModuleDescriptor> {
    let mut reach: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = m.requires.iter().map(|r| r.name.as_str()).collect();
    while let Some(name) = stack.pop() {
        if name == m.name || !reach.insert(name) {
            continue;
        }
        if let Some(dep) = known.iter().find(|k| k.name == name) {
            stack.extend(dep.requires.iter().map(|r| r.name.as_str()));
        }
    }
    known
        .iter()
        .copied()
        .filter(|k| k.name != m.name && reach.contains(k.name.as_str()))
        .collect()
}

/// Same verdict as [`issues`], but the entities a view may name also include those declared by
/// the modules `m` requires (transitively), looked up in `known`. A module that lists an
/// entity owned by a module it does NOT require is still refused (`view_entity_unknown`).
///
/// `known` may hold every module of the host, `m` itself included: only the transitive
/// `requires` of `m` are read, and an entity `m` declares itself wins over a required one.
/// [`issues`] is unchanged: it knows the module alone.
pub fn issues_with_required(m: &ModuleDescriptor, known: &[&ModuleDescriptor]) -> Vec<Issue> {
    let required = required_modules(m, known);
    let vues: Vec<&ViewDesc> = m.views.iter().collect();
    let mut c = Checks::new(m, vues);
    for dep in required {
        for e in &dep.entities {
            c.entities.entry(e.name.as_str()).or_insert(e);
        }
    }
    c.operations();
    c.actions();
    c.views();
    c.replacements();
    c.out
}

/// A reported problem: `(reason, path, message)`.
pub type Issue = (&'static str, String, String);

fn issues_over<'a>(m: &'a ModuleDescriptor, vues: Vec<&'a ViewDesc>) -> Vec<Issue> {
    let mut c = Checks::new(m, vues);
    c.operations();
    c.actions();
    c.views();
    c.replacements();
    c.out
}

/// The merged view list of a module once `rendered` views are in: declared views whose name a
/// rendered view carries are dropped, the rendered ones follow. No clone of the descriptor.
fn merged<'a>(m: &'a ModuleDescriptor, rendered: &'a [ViewDesc]) -> Vec<&'a ViewDesc> {
    let remplacees: BTreeSet<&str> = rendered.iter().map(|v| v.name.as_str()).collect();
    m.views
        .iter()
        .filter(|v| !remplacees.contains(v.name.as_str()))
        .chain(rendered.iter())
        .collect()
}

/// Same verdict as `issues` on a clone of `m` whose views were replaced by `rendered` (same
/// name) and extended with them, without cloning `m`. Revalidates every declared view.
pub fn issues_with_rendered(m: &ModuleDescriptor, rendered: &[ViewDesc]) -> Vec<Issue> {
    issues_over(m, merged(m, rendered))
}

/// What is known about the declared views of one descriptor, computed once: the problems of
/// each declared view, so that a rendered tree is validated without revalidating the N declared
/// views. Bound to the descriptor it was built from: use it only with that descriptor.
///
/// Why this is exact: a view's own problems depend on the entities, the actions and the
/// operations of the module (unchanged by a render) and on the *set of view names* — and only
/// through a reference to a view (`view_target_unknown`). Adding names can only remove such a
/// problem, so a declared view whose baseline problems include one is revalidated every time.
/// Duplicate names among declared views vanish or stay together (a rendered name removes all
/// declared views of that name), so the baseline duplicate flags stay valid.
pub struct ViewBaseline {
    /// Per declared view: its problems, paths relative to `views[i]`, or `None` when the view
    /// must be revalidated each time.
    per_view: Vec<Option<Vec<Issue>>>,
}

impl ViewBaseline {
    pub fn new(m: &ModuleDescriptor) -> Self {
        let vues: Vec<&ViewDesc> = m.views.iter().collect();
        let mut c = Checks::new(m, vues);
        c.operations();
        c.actions();
        let mut per_view = Vec::with_capacity(m.views.len());
        let mut vus = BTreeSet::new();
        for (i, v) in m.views.iter().enumerate() {
            let avant = c.out.len();
            c.view_at(i, v, &mut vus);
            let prefixe = format!("views[{i}]");
            let mine: Vec<Issue> = c.out.drain(avant..).collect();
            let stable = mine
                .iter()
                .all(|(r, p, _)| *r != "view_target_unknown" && p.starts_with(&prefixe));
            per_view.push(stable.then(|| {
                mine.into_iter()
                    .map(|(r, p, msg)| (r, p[prefixe.len()..].to_string(), msg))
                    .collect()
            }));
        }
        Self { per_view }
    }

    /// Same verdict, same order, same text as [`issues_with_rendered`] — and as `issues` on the
    /// cloned merged descriptor — for the descriptor `m` this baseline was built from.
    pub fn issues_with(&self, m: &ModuleDescriptor, rendered: &[ViewDesc]) -> Vec<Issue> {
        if self.per_view.len() != m.views.len() {
            return issues_with_rendered(m, rendered);
        }
        let remplacees: BTreeSet<&str> = rendered.iter().map(|v| v.name.as_str()).collect();
        let vues = merged(m, rendered);
        let mut c = Checks::new(m, vues);
        c.operations();
        c.actions();
        let mut vus = BTreeSet::new();
        let mut position = 0usize;
        for (i, v) in m.views.iter().enumerate() {
            if remplacees.contains(v.name.as_str()) {
                continue;
            }
            match &self.per_view[i] {
                Some(cache) => {
                    vus.insert(v.name.as_str());
                    for (r, suffixe, msg) in cache {
                        c.out
                            .push((r, format!("views[{position}]{suffixe}"), msg.clone()));
                    }
                }
                None => c.view_at(position, v, &mut vus),
            }
            position += 1;
        }
        for v in rendered {
            c.view_at(position, v, &mut vus);
            position += 1;
        }
        c.replacements();
        c.out
    }
}

/// Deepest nesting of containers the checker accepts. A deeper tree is almost certainly a
/// mistake (or hostile) and would make every host recurse through it.
pub const MAX_VIEW_DEPTH: usize = 16;

/// Levenshtein distance, small inputs only (names of entities, fields, actions, views).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The candidate closest to `name`, if it is close enough to be a typo.
pub(crate) fn closest<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (distance(name, c), c))
        .filter(|(d, c)| *d <= 2.max(c.len() / 3) && *d < name.len().max(1))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// `"`x` is not <what> (declared: a, b). Did you mean `a`? <fix>"`: names the candidates and the
/// remedy, so the author does not have to guess.
fn unknown_msg<'a>(
    name: &str,
    what: &str,
    candidates: impl IntoIterator<Item = &'a str> + Clone,
    fix: &str,
) -> String {
    let list: Vec<&str> = candidates.clone().into_iter().collect();
    let mut m = format!("`{name}` is not {what}");
    if list.is_empty() {
        m.push_str(" (none is declared)");
    } else {
        m.push_str(&format!(" (declared: {})", list.join(", ")));
    }
    if let Some(c) = closest(name, candidates) {
        m.push_str(&format!(". Did you mean `{c}`?"));
    } else {
        m.push('.');
    }
    m.push(' ');
    m.push_str(fix);
    m
}

struct Checks<'a> {
    m: &'a ModuleDescriptor,
    entities: BTreeMap<&'a str, &'a EntityDesc>,
    actions: BTreeMap<&'a str, &'a Action>,
    operations: BTreeMap<&'a str, &'a crate::module::OperationDesc>,
    views: BTreeSet<&'a str>,
    /// The views being checked, in order (a merged list for a render).
    vues: Vec<&'a ViewDesc>,
    out: Vec<(&'static str, String, String)>,
}

impl<'a> Checks<'a> {
    fn new(m: &'a ModuleDescriptor, vues: Vec<&'a ViewDesc>) -> Self {
        Self {
            m,
            entities: m.entities.iter().map(|e| (e.name.as_str(), e)).collect(),
            actions: BTreeMap::new(),
            operations: BTreeMap::new(),
            views: vues.iter().map(|v| v.name.as_str()).collect(),
            vues,
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
            let msg = unknown_msg(
                entity,
                "a declared entity",
                self.entities.keys().copied(),
                "Declare it in `entities` or fix the name.",
            );
            self.push(reason, path, msg);
            false
        }
    }

    /// Un libellé déclaré (action, bouton, avis) : du texte d'interface, pas un identifiant.
    fn label_ref(&mut self, path: &str, label: Option<&str>) {
        match label.and_then(label_problem) {
            Some(LabelProblem::Empty) => self.push(
                "label_empty",
                path,
                "a label cannot be empty; drop `label` to let the host humanise the action name",
            ),
            Some(LabelProblem::NotText) => self.push(
                "label_not_text",
                path,
                format!(
                    "`{}` is not interface text (no `_`, no control character): write what the person should read, e.g. \"Pin this key\"",
                    label.unwrap_or_default().escape_debug()
                ),
            ),
            None => {}
        }
    }

    fn action_ref(&mut self, path: &str, name: &str) {
        if !self.actions.contains_key(name) {
            let msg = unknown_msg(
                name,
                "a declared action",
                self.actions.keys().copied(),
                "Add it to the module's `actions` or fix the name.",
            );
            self.push("view_action_unknown", path, msg);
        }
    }

    fn view_ref(&mut self, reason: &'static str, path: &str, name: &str) {
        if !self.views.contains(name) {
            let msg = unknown_msg(
                name,
                "a view of this module",
                self.views.iter().copied(),
                "Declare that view in `views` or fix the name.",
            );
            self.push(reason, path, msg);
        }
    }

    /// `navigate` vers une vue d'un autre module : le module doit être déclaré dans
    /// `requires[]`. La vue elle-même n'est pas vérifiable ici (le descripteur de l'autre
    /// module n'est pas là) : l'hôte la résout à l'exécution et refuse avec un message.
    fn navigate_module(&mut self, chemin: &str, cible: &str, view: &str) {
        if view.trim().is_empty() {
            self.push(
                "view_target_unknown",
                format!("{chemin}.view"),
                "a navigate to another module must name the view to open",
            );
        }
        if cible == self.m.name {
            self.push(
                "navigate_module_is_self",
                format!("{chemin}.module"),
                format!("`{cible}` is this module; drop `module` to open one of its own views"),
            );
        } else if !self.m.requires.iter().any(|r| r.name == cible) {
            let msg = unknown_msg(
                cible,
                "a module this one requires",
                self.m.requires.iter().map(|r| r.name.as_str()),
                "Declare it in `requires[]` (as `this.module.reference_unknown` does for links), or fix the name.",
            );
            self.push(
                "navigate_module_not_required",
                format!("{chemin}.module"),
                msg,
            );
        }
    }

    /// L'écran de réglages : des sections titrées, des lignes dont l'action ne veut aucune ligne.
    fn settings(&mut self, chemin: &str, sections: &[SettingsSection]) {
        if sections.is_empty() {
            self.push(
                "settings_empty",
                format!("{chemin}.sections"),
                "a settings brick needs at least one section",
            );
        }
        for (i, sec) in sections.iter().enumerate() {
            let p = format!("{chemin}.sections[{i}]");
            if sec.title.trim().is_empty() {
                self.push(
                    "settings_text_empty",
                    format!("{p}.title"),
                    "a section needs a title",
                );
            }
            if sec.items.is_empty() {
                self.push(
                    "settings_empty",
                    format!("{p}.items"),
                    "a section needs at least one item; remove it otherwise",
                );
            }
            for (j, it) in sec.items.iter().enumerate() {
                let q = format!("{p}.items[{j}]");
                if it.label.trim().is_empty() {
                    self.push(
                        "settings_text_empty",
                        format!("{q}.label"),
                        "an item needs a label",
                    );
                }
                self.action_ref(&format!("{q}.action"), &it.action);
                let refus = match self.actions.get(it.action.as_str()) {
                    Some(Action::Create { .. }) => Some("creates a row from values nobody typed"),
                    Some(Action::Update { .. }) => Some("modifies a row"),
                    Some(Action::Delete { .. }) => Some("deletes a row"),
                    Some(Action::Invoke {
                        entity: Some(_), ..
                    }) => Some("invokes an operation on a row"),
                    _ => None,
                };
                if let Some(pourquoi) = refus {
                    self.push(
                        "settings_action_refused",
                        format!("{q}.action"),
                        format!(
                            "`{}` {pourquoi}, and a settings line has no row; use navigate, request_secret, agent_tool, device_capability, host or a module-level invoke",
                            it.action
                        ),
                    );
                }
            }
        }
    }

    /// Les opérations déclarées : nom, unicité, entité. Un bouton `invoke` ne doit
    /// jamais référencer un geste que le module n'a pas déclaré.
    fn operations(&mut self) {
        let m = self.m;
        for (i, op) in m.operations.iter().enumerate() {
            let p = format!("operations[{i}]");
            if !is_identifier(&op.name) {
                self.push("invalid_name", &p, format!("`{}`", op.name));
            }
            if self.operations.insert(op.name.as_str(), op).is_some() {
                self.push(
                    "duplicate_operation",
                    &p,
                    format!("`{}` declared twice", op.name),
                );
                continue;
            }
            if let Some(e) = &op.entity {
                self.entity_ref("operation_entity_unknown", &format!("{p}.entity"), e);
            }
        }
    }

    fn actions(&mut self) {
        let m = self.m;
        for (i, a) in m.actions.iter().enumerate() {
            let p = format!("actions[{i}]");
            if !is_identifier(a.name()) {
                self.push("invalid_name", &p, format!("`{}`", a.name()));
            }
            self.label_ref(&format!("{p}.label"), a.label());
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
                    if fields.is_empty() {
                        self.push(
                            "action_fields_empty",
                            format!("{p}.fields"),
                            "a create/update action must name at least one field to write",
                        );
                    }
                    let mut written = BTreeSet::new();
                    for (j, f) in fields.iter().enumerate() {
                        if !written.insert(f.as_str()) {
                            self.push(
                                "duplicate_action_field",
                                format!("{p}.fields[{j}]"),
                                format!("`{f}` is listed twice; list each written field once"),
                            );
                        }
                    }
                    if self.entity_ref("action_entity_unknown", &format!("{p}.entity"), entity) {
                        let ok = self.writable(entity);
                        for (j, f) in fields.iter().enumerate() {
                            if !ok.contains(f) {
                                let msg = unknown_msg(
                                    f,
                                    &format!("a declared field of `{entity}`"),
                                    ok.iter().map(String::as_str),
                                    "Base fields (id, created_at...) are never written by an action.",
                                );
                                self.push("action_field_unknown", format!("{p}.fields[{j}]"), msg);
                            }
                        }
                    }
                }
                Action::Delete { entity, .. } => {
                    self.entity_ref("action_entity_unknown", &format!("{p}.entity"), entity);
                }
                Action::AgentTool {
                    entity, op, text, ..
                } => {
                    if let Some(e) = entity {
                        self.entity_ref("action_entity_unknown", &format!("{p}.entity"), e);
                    }
                    // La règle qui compte : l'hôte n'écrit pas à la place du module.
                    match (op, text.as_deref().map(str::trim)) {
                        (AgentOp::SendMessage, None | Some("")) => self.push(
                            "action_agent_text_missing",
                            format!("{p}.text"),
                            "une action `send_message` doit dire ce qu'elle demande (champ `text`)",
                        ),
                        (AgentOp::StopRun, Some(_)) => self.push(
                            "action_agent_text_unused",
                            format!("{p}.text"),
                            "`stop_run` n'envoie aucun texte : retire le champ `text`",
                        ),
                        _ => {}
                    }
                }
                Action::Navigate { view, module, .. } => match module {
                    None => self.view_ref("view_target_unknown", &format!("{p}.view"), view),
                    Some(cible) => self.navigate_module(&p, cible, view),
                },
                Action::RequestSecret { secret, .. } => {
                    let ok = !secret.is_empty()
                        && secret
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
                    if !ok {
                        self.push(
                            "action_secret_invalid",
                            format!("{p}.secret"),
                            format!(
                                "`{secret}` is not a secret name: use letters, digits, `_`, `-` or `.`, no space"
                            ),
                        );
                    }
                }
                Action::Invoke {
                    operation, entity, ..
                } => {
                    if let Some(e) = entity {
                        self.entity_ref("action_entity_unknown", &format!("{p}.entity"), e);
                    }
                    if !self.operations.contains_key(operation.as_str()) {
                        let msg = unknown_msg(
                            operation,
                            "a declared operation",
                            self.operations.keys().copied(),
                            "Declare it in `operations` or fix the name.",
                        );
                        self.push("action_operation_unknown", format!("{p}.operation"), msg);
                    }
                }
                Action::DeviceCapability { .. } => {}
                // Une surface de l'hôte ne nomme ni entité, ni champ, ni opération : elle
                // n'a rien à vérifier ici, et l'hôte seul sait s'il sait l'ouvrir.
                Action::Host { .. } => {}
            }
        }
    }

    fn views(&mut self) {
        let vues = self.vues.clone();
        let mut vus = BTreeSet::new();
        for (i, v) in vues.into_iter().enumerate() {
            self.view_at(i, v, &mut vus);
        }
    }

    /// One view, at position `i` of the list being checked.
    fn view_at(&mut self, i: usize, v: &'a ViewDesc, vus: &mut BTreeSet<&'a str>) {
        let p = format!("views[{i}]");
        if !is_identifier(&v.name) {
            self.push("invalid_name", &p, format!("`{}`", v.name));
        }
        if !vus.insert(v.name.as_str()) {
            self.push("duplicate_view", &p, format!("`{}` declared twice", v.name));
        }
        let mut briques = Vec::new();
        let mut structure = Vec::new();
        let mut conditions = Vec::new();
        let mut htmls = Vec::new();
        collecte(
            &format!("{p}.root"),
            &v.root,
            1,
            &mut briques,
            &mut structure,
            &mut conditions,
            &mut htmls,
        );
        if let Some(live) = v.live {
            if v.surface == Surface::Header {
                self.push(
                    "live_on_header",
                    format!("{p}.live"),
                    "a header is a thin bar and does not justify a timer; drop `live` or move the view to `main`, `sidebar` or `panel`",
                );
            }
            if live.every_secs < LIVE_MIN_SECS {
                self.push(
                    "live_interval_too_small",
                    format!("{p}.live.every_secs"),
                    format!("`every_secs` is {} ; the minimum is {LIVE_MIN_SECS} (this is polling, not push)", live.every_secs),
                );
            } else if live.every_secs > LIVE_MAX_SECS {
                self.push(
                    "live_interval_too_large",
                    format!("{p}.live.every_secs"),
                    format!(
                        "`every_secs` is {} ; the maximum is {LIVE_MAX_SECS}",
                        live.every_secs
                    ),
                );
            }
        }
        for (chemin, path, data) in htmls {
            self.html_data(&chemin, data);
            if v.surface == Surface::Header {
                self.push(
                    "html_in_header",
                    &chemin,
                    "a header is a thin bar and cannot host an iframe; move the view to the `main`, `sidebar` or `panel` surface",
                );
            }
            if let Err((raison, msg)) = html_resolve(&self.m.name, &self.m.version, path) {
                self.push(raison, format!("{chemin}.path"), msg);
            }
        }
        for (chemin, raison, msg) in structure {
            self.push(raison, chemin, msg);
        }
        for (chemin, entite, filtre) in conditions {
            self.condition(&chemin, entite, filtre);
        }
        for (chemin, b) in briques {
            self.brick(&chemin, b, v.surface);
        }
    }

    /// Les demandes de données d'un nœud `html` : au plus cinq, une par entité, chaque
    /// entité connue (du module ou de ses requêtes), chaque champ déclaré, une limite dans
    /// les bornes. Tout refus nomme la demande.
    fn html_data(&mut self, chemin: &str, data: &[DataGrant]) {
        if data.len() > HTML_DATA_MAX_GRANTS {
            self.push(
                "html_data_too_many",
                format!("{chemin}.data"),
                format!(
                    "{} data requests, the most an `html` node may ask is {HTML_DATA_MAX_GRANTS}; \
                     merge them or split the page",
                    data.len()
                ),
            );
        }
        let mut vues = BTreeSet::new();
        for (i, g) in data.iter().enumerate() {
            let p = format!("{chemin}.data[{i}]");
            if !self.entity_ref(
                "html_data_entity_unknown",
                &format!("{p}.entity"),
                &g.entity,
            ) {
                continue;
            }
            if !vues.insert(g.entity.as_str()) {
                self.push(
                    "html_data_entity_twice",
                    format!("{p}.entity"),
                    format!(
                        "`{}` is requested twice: `rows` is keyed by entity, one request each",
                        g.entity
                    ),
                );
            }
            let declares: BTreeSet<String> = self
                .entities
                .get(g.entity.as_str())
                .map(|e| e.fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();
            for (j, f) in g.fields.iter().flatten().enumerate() {
                let decl: Vec<String> = declares.iter().cloned().collect();
                match html_field_verdict(f, &decl) {
                    Ok(()) => {}
                    Err("html_data_field_reserved") => self.push(
                        "html_data_field_reserved",
                        format!("{p}.fields[{j}]"),
                        format!(
                            "`{f}` is a base field of the host; only `id` may be requested (to target a row)"
                        ),
                    ),
                    Err(_) => {
                        let msg = unknown_msg(
                            f,
                            &format!("a declared field of `{}`", g.entity),
                            declares.iter().map(String::as_str),
                            "Only declared fields (and `id`, to target a row) cross into the page.",
                        );
                        self.push("html_data_field_unknown", format!("{p}.fields[{j}]"), msg);
                    }
                }
            }
            if let Some(filtre) = &g.filter {
                let lis = self.readable(&g.entity);
                let mut lus = Vec::new();
                chemins_du_filtre(filtre, &mut lus);
                for c in lus {
                    if !c.contains('.') && !lis.contains(c) {
                        let msg = unknown_msg(
                            c,
                            &format!("a field of `{}`", g.entity),
                            lis.iter().map(String::as_str),
                            "Declare the field or fix the name.",
                        );
                        self.push("html_data_field_unknown", format!("{p}.filter"), msg);
                    }
                }
            }
            if let Some(l) = g.limit
                && !(1..=HTML_DATA_MAX_LIMIT).contains(&l)
            {
                self.push(
                    "html_data_limit_out_of_range",
                    format!("{p}.limit"),
                    format!("limit {l} is outside 1..={HTML_DATA_MAX_LIMIT}"),
                );
            }
        }
    }

    /// La condition d'un nœud `when` : une entité connue, des champs connus — les règles
    /// d'un filtre de liste, avec les mêmes raisons.
    fn condition(&mut self, chemin: &str, entite: &str, filtre: &FilterExpr) {
        if !self.entity_ref("view_entity_unknown", &format!("{chemin}.entity"), entite) {
            return;
        }
        let lis = self.readable(entite);
        let mut lus = Vec::new();
        chemins_du_filtre(filtre, &mut lus);
        self.reads(&format!("{chemin}.filter"), entite, &lis, &lus);
    }

    fn brick(&mut self, chemin: &str, b: &Brick, surface: Surface) {
        if surface == Surface::Header
            && !matches!(
                b,
                Brick::Text { .. }
                    | Brick::Caption { .. }
                    | Brick::Banner { .. }
                    | Brick::Indicator { .. }
                    | Brick::Metrics { .. }
                    | Brick::Button { .. }
            )
        {
            self.push(
                "surface_brick_refused",
                chemin,
                "this brick does not fit in a header; a header only takes text, caption, banner, \
                 indicator, metrics and button. Move the view to the `main`, `sidebar` or `panel` surface, \
                 or use one of those bricks"
                    .to_string(),
            );
        }
        match b {
            Brick::Banner { .. } => {}
            // La saisie de secret ne lit aucune entité et ne référence aucune action : elle
            // ne porte qu'un NOM, vérifié ici comme celui d'une action `request_secret`.
            // Le refus sur `header` tombe plus haut (une barre fine n'a pas de champ).
            Brick::Secret { name, .. } => {
                if name.is_empty() {
                    self.push(
                        "secret_name_empty",
                        format!("{chemin}.name"),
                        "a secret input must name the secret it writes",
                    );
                } else if !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                {
                    self.push(
                        "secret_name_invalid",
                        format!("{chemin}.name"),
                        format!(
                            "`{name}` is not a secret name: use letters, digits, `_`, `-` or `.`, no space"
                        ),
                    );
                }
            }
            Brick::Settings { sections } => self.settings(chemin, sections),
            Brick::Composer {
                placeholder,
                actions,
                ..
            } => self.composer(chemin, placeholder.as_deref(), actions),
            Brick::CapabilityRequest { capability, action } => {
                let p = format!("{chemin}.action");
                self.action_ref(&p, action);
                match self.actions.get(action.as_str()) {
                    Some(Action::DeviceCapability { capability: c, .. }) if c == capability => {}
                    Some(Action::DeviceCapability { capability: c, .. }) => self.push(
                        "view_action_kind_mismatch",
                        &p,
                        format!(
                            "`{action}` decides {c:?} but this request asks for {capability:?}; \
                             point it at a `device_capability` action for {capability:?}"
                        ),
                    ),
                    Some(_) => self.push(
                        "view_action_kind_mismatch",
                        &p,
                        format!("`{action}` is not a `device_capability` action; a capability request needs one"),
                    ),
                    None => {}
                }
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
                if matches!(signal, Signal::Activity | Signal::Badge) && field.is_none() {
                    self.push(
                        "view_field_required",
                        chemin,
                        format!(
                            "a {signal:?} indicator must name `field` ({})",
                            if *signal == Signal::Activity {
                                "the phase"
                            } else {
                                "the value"
                            }
                        ),
                    );
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
                        unknown_msg(
                            link,
                            &format!("a link whose source is `{entity}`"),
                            self.m
                                .links
                                .iter()
                                .filter(|l| l.source == *entity)
                                .map(|l| l.link_type.as_str()),
                            "Declare the link in `links` with this entity as source, or fix the name.",
                        ),
                    );
                }
            }
            Brick::Chart {
                entity,
                field,
                kind,
                label,
                height,
                ..
            } => {
                if !self.entity_ref("chart_entity_unknown", &format!("{chemin}.entity"), entity) {
                    return;
                }
                let lis = self.readable(entity);
                self.reads(chemin, entity, &lis, &lectures(b));
                self.chart(chemin, entity, field, kind, label.as_deref(), *height);
            }
            Brick::Image { .. } => self.image(chemin, b),
            _ => self.data_brick(chemin, b),
        }
    }

    /// Le composeur : une invite non vide, des actions déclarées, chacune une fois, et
    /// aucune qui exige ce qu'un composeur n'a pas (une ligne, une question d'appareil).
    fn composer(&mut self, chemin: &str, placeholder: Option<&str>, actions: &[String]) {
        if placeholder.is_some_and(|p| p.trim().is_empty()) {
            self.push(
                "composer_placeholder_empty",
                format!("{chemin}.placeholder"),
                "an empty placeholder leaves the field blank; write a short invitation, or \
                 remove `placeholder` to keep the host's",
            );
        }
        let mut vues = BTreeSet::new();
        for (j, a) in actions.iter().enumerate() {
            let p = format!("{chemin}.actions[{j}]");
            if !vues.insert(a.as_str()) {
                self.push(
                    "duplicate_composer_action",
                    &p,
                    format!("`{a}` is listed twice; list each composer action once"),
                );
                continue;
            }
            self.action_ref(&p, a);
            let refus = match self.actions.get(a.as_str()) {
                Some(Action::Update { .. }) => Some("modifies a row"),
                Some(Action::Delete { .. }) => Some("deletes a row"),
                Some(Action::Invoke {
                    entity: Some(_), ..
                }) => Some("invokes an operation on a row"),
                Some(Action::DeviceCapability { .. }) => {
                    Some("decides a device question nobody asked here")
                }
                _ => None,
            };
            if let Some(pourquoi) = refus {
                self.push(
                    "composer_action_refused",
                    &p,
                    format!(
                        "`{a}` {pourquoi}, and a composer has no row or question; use a create, \
                         navigate, agent_tool or module-level invoke action, or put `{a}` on a \
                         `button` of a view that reads the row"
                    ),
                );
            }
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
            Brick::Button { action, label, .. } => {
                self.action_ref(&format!("{chemin}.action"), action);
                self.label_ref(&format!("{chemin}.label"), label.as_deref());
            }
            // L'avis peut porter un geste (« Continuer ») : il doit exister.
            Brick::Notice {
                action: Some(a),
                label,
                ..
            } => {
                self.action_ref(&format!("{chemin}.action"), a);
                self.label_ref(&format!("{chemin}.label"), label.as_deref());
            }
            Brick::Notice {
                action: None,
                label,
                ..
            } => {
                if label.is_some() {
                    self.push(
                        "notice_label_without_action",
                        format!("{chemin}.label"),
                        "a notice `label` names its button: without an `action` there is no button to label",
                    );
                }
            }
            Brick::Field {
                input,
                target,
                field,
                entity,
                ..
            } => {
                self.input_matches_type(chemin, entity, field, *input);
                match (input, target) {
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
                }
            }
            Brick::Form {
                fields,
                submit,
                entity,
                ..
            } => {
                let mut vus = BTreeSet::new();
                for (j, f) in fields.iter().enumerate() {
                    if f.field.is_empty() {
                        self.push(
                            "invalid_name",
                            format!("{chemin}.fields[{j}]"),
                            "empty field name".to_string(),
                        );
                    } else if !vus.insert(f.field.as_str()) {
                        self.push(
                            "duplicate_form_field",
                            format!("{chemin}.fields[{j}]"),
                            format!(
                                "`{}` appears twice in this form; list each field once",
                                f.field
                            ),
                        );
                    }
                }
                if let Some(s) = submit {
                    let p = format!("{chemin}.submit");
                    self.action_ref(&p, s);
                    match self.actions.get(s.as_str()) {
                        Some(Action::Create { entity: e, .. } | Action::Update { entity: e, .. })
                            if e == entity => {}
                        Some(Action::Create { entity: e, .. } | Action::Update { entity: e, .. }) => {
                            self.push(
                                "view_action_kind_mismatch",
                                &p,
                                format!("`{s}` writes `{e}` but this form is on `{entity}`; submit an action on `{entity}`"),
                            );
                        }
                        Some(_) => self.push(
                            "view_action_kind_mismatch",
                            &p,
                            format!("`{s}` is not a create or update action; a form submits one of those"),
                        ),
                        None => {}
                    }
                }
            }
            Brick::List {
                on_click,
                page_size,
                row_actions,
                ..
            } => {
                if *page_size == Some(0) {
                    self.push(
                        "invalid_page_size",
                        format!("{chemin}.page_size"),
                        "a page holds at least one row".to_string(),
                    );
                }
                if page_size.is_some_and(|n| n > crate::query::MAX_LIMIT) {
                    self.push(
                        "invalid_page_size",
                        format!("{chemin}.page_size"),
                        format!(
                            "a page holds at most {} rows (the query clamp)",
                            crate::query::MAX_LIMIT
                        ),
                    );
                }
                if let Some(Click::Navigate { view }) = on_click {
                    self.view_ref(
                        "view_target_unknown",
                        &format!("{chemin}.on_click.view"),
                        view,
                    );
                }
                let mut vus = BTreeSet::new();
                for (j, a) in row_actions.iter().enumerate() {
                    let p = format!("{chemin}.row_actions[{j}]");
                    if !vus.insert(a.as_str()) {
                        self.push(
                            "duplicate_row_action",
                            &p,
                            format!("`{a}` is listed twice; list each row action once"),
                        );
                        continue;
                    }
                    self.action_ref(&p, a);
                    let refus = match self.actions.get(a.as_str()) {
                        Some(Action::Create { .. }) => {
                            Some("creates a row from values nobody typed")
                        }
                        Some(Action::Navigate { .. }) => Some("opens a view"),
                        Some(Action::RequestSecret { .. }) => Some("asks the host for a secret"),
                        Some(Action::AgentTool { .. }) => Some("talks to the agent"),
                        Some(Action::DeviceCapability { .. }) => {
                            Some("decides a device question nobody asked here")
                        }
                        Some(Action::Host { .. }) => Some("opens a host surface"),
                        Some(Action::Invoke { entity: None, .. }) => {
                            Some("runs a module-level operation")
                        }
                        _ => None,
                    };
                    if let Some(pourquoi) = refus {
                        self.push(
                            "row_action_refused",
                            &p,
                            format!(
                                "`{a}` {pourquoi}, and a row action must aim at the row; use an update, delete or entity invoke action"
                            ),
                        );
                    }
                }
            }
            // Un fil se lit par le bas : la fenêtre est un nombre de messages, pas une
            // page du début. Les bornes sont celles du reste (une fenêtre vide ne montre
            // rien, une fenêtre géante n'est pas une fenêtre).
            Brick::Message { page_size, .. } => {
                if let Some(n) = page_size
                    && (*n == 0 || *n > crate::query::MAX_LIMIT)
                {
                    self.push(
                        "invalid_page_size",
                        format!("{chemin}.page_size"),
                        format!(
                            "a thread window holds 1..={} messages: `{n}` would show {}",
                            crate::query::MAX_LIMIT,
                            if *n == 0 {
                                "nothing"
                            } else {
                                "everything at once"
                            }
                        ),
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

    /// A toggle needs a boolean field, a number input a numeric one. Only a DECLARED field is
    /// judged: base fields and unknown fields are reported elsewhere.
    fn input_matches_type(&mut self, chemin: &str, entity: &str, field: &str, input: Input) {
        let Some(e) = self.entities.get(entity) else {
            return;
        };
        let Some(f) = e.fields.iter().find(|f| f.name == field) else {
            return;
        };
        let (ok, want) = match input {
            Input::Toggle => (matches!(f.ty, FieldType::Bool), "a bool field"),
            Input::Number => (f.ty.is_numeric(), "a numeric field"),
            _ => return,
        };
        if !ok {
            self.push(
                "field_input_invalid",
                chemin,
                format!(
                    "a {input:?} input needs {want}, but `{field}` of `{entity}` is {:?}; change the input or the field type",
                    f.ty
                ),
            );
        }
    }

    /// Un graph : une forme connue, un champ DÉCLARÉ numérique (un texte n'est pas une série),
    /// une hauteur dans la plage. Tout est refusé ici, avant qu'un rendu existe.
    fn chart(
        &mut self,
        chemin: &str,
        entite: &str,
        champ: &str,
        kind: &str,
        label: Option<&str>,
        height: Option<u32>,
    ) {
        if !CHART_KINDS.contains(&kind) {
            self.push(
                "chart_kind_unknown",
                format!("{chemin}.kind"),
                unknown_msg(
                    kind,
                    "a chart kind",
                    CHART_KINDS.iter().copied(),
                    "Use `bar` or `line`.",
                ),
            );
        }
        let numerique = self.entities.get(entite).is_some_and(|e| {
            e.fields.iter().any(|f| {
                f.name == champ
                    && matches!(
                        f.ty,
                        FieldType::I32
                            | FieldType::I64
                            | FieldType::U32
                            | FieldType::U64
                            | FieldType::F32
                            | FieldType::F64
                            | FieldType::Decimal { .. }
                    )
            })
        });
        // Un champ inconnu est déjà dit par `view_field_unknown` : ne pas le redire.
        if !numerique && (champ.contains('.') || self.readable(entite).contains(champ)) {
            self.push(
                "chart_field_not_numeric",
                format!("{chemin}.field"),
                format!(
                    "`{champ}` is not a declared numeric field of `{entite}`; a chart plots \
                     i32, i64, u32, u64, f32, f64 or decimal fields, one point per row"
                ),
            );
        }
        if let Some(h) = height
            && !(CHART_HEIGHT_MIN..=CHART_HEIGHT_MAX).contains(&h)
        {
            self.push(
                "chart_height_out_of_range",
                format!("{chemin}.height"),
                format!("height {h} is outside {CHART_HEIGHT_MIN}..={CHART_HEIGHT_MAX}"),
            );
        }
        self.label_ref(&format!("{chemin}.label"), label);
    }

    /// Une image : exactement UNE source, un texte alternatif, une hauteur dans la plage, et
    /// — pour un fichier — un chemin dans le module, jamais une URL distante.
    fn image(&mut self, chemin: &str, b: &Brick) {
        let Brick::Image {
            source,
            alt,
            height,
            ..
        } = b
        else {
            return;
        };
        let vfs = source.vfs.as_deref();
        let a_entite = source.entity.is_some();
        let a_champ = source.field.is_some();
        // Exactement une source : ni les deux, ni aucune.
        match (vfs.is_some(), a_entite || a_champ) {
            (true, true) => self.push(
                "image_source_ambiguous",
                format!("{chemin}.source"),
                "an image has ONE source: either `source.vfs` (a module file) or                  `source.entity`+`source.field` (a data URL), not both",
            ),
            (false, false) => self.push(
                "image_source_missing",
                format!("{chemin}.source"),
                "an image needs a source: `source.vfs` (a module file) or                  `source.entity`+`source.field` (a data URL)",
            ),
            _ => {}
        }
        // Le texte alternatif est requis : une image muette n'existe pas pour un lecteur.
        if alt.trim().is_empty() {
            self.push(
                "image_alt_missing",
                format!("{chemin}.alt"),
                "an image must carry a non-empty `alt`: what the person should hear when it                  cannot load",
            );
        }
        if let Some(h) = height
            && !(IMAGE_HEIGHT_MIN..=IMAGE_HEIGHT_MAX).contains(h)
        {
            self.push(
                "image_height_out_of_range",
                format!("{chemin}.height"),
                format!("height {h} is outside {IMAGE_HEIGHT_MIN}..={IMAGE_HEIGHT_MAX}"),
            );
        }
        match (vfs, source.entity.as_deref(), source.field.as_deref()) {
            // Un fichier du module : le chemin doit rester dedans, et n'être jamais une URL.
            (Some(chemin_vfs), _, _) => {
                if let Err((raison, msg)) = image_resolve(&self.m.name, &self.m.version, chemin_vfs)
                {
                    self.push(raison, format!("{chemin}.source.vfs"), msg);
                }
            }
            // Une data URL lue dans une ligne : l'entité et le champ sont déclarés.
            (None, Some(e), Some(f)) => {
                if !self.entity_ref(
                    "view_entity_unknown",
                    &format!("{chemin}.source.entity"),
                    e,
                ) {
                    return;
                }
                let lis = self.readable(e);
                self.reads(&format!("{chemin}.source.field"), e, &lis, &[f]);
            }
            // Une moitié de source : ni un fichier, ni une ligne complète.
            (None, Some(_), None) => self.push(
                "image_source_missing",
                format!("{chemin}.source.field"),
                "an image reading a row needs both `source.entity` and `source.field`:                  `source.field` is missing",
            ),
            (None, None, Some(_)) => self.push(
                "image_source_missing",
                format!("{chemin}.source.entity"),
                "an image reading a row needs both `source.entity` and `source.field`:                  `source.entity` is missing",
            ),
            (None, None, None) => {}
        }
    }

    /// Vérifie que chaque champ lu existe ; un chemin pointé (`target.name`) est validé ailleurs.
    fn reads(&mut self, chemin: &str, entite: &str, lis: &BTreeSet<String>, lus: &[&str]) {
        for c in lus {
            if !c.contains('.') && !lis.contains(*c) {
                let msg = unknown_msg(
                    c,
                    &format!("a field of `{entite}`"),
                    lis.iter().map(String::as_str),
                    &format!("Declare the field on `{entite}` or fix the name."),
                );
                self.push("view_field_unknown", chemin, msg);
            }
        }
    }

    fn replacements(&mut self) {
        let vues = self.vues.clone();
        let mut remplacees: HashMap<&str, &str> = HashMap::new();
        let mut suivant: HashMap<&str, &str> = HashMap::new();
        for (i, v) in vues.iter().copied().enumerate() {
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
        for (i, v) in vues.iter().copied().enumerate() {
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
        | Brick::Caption { field, filter, .. }
        | Brick::Chart { field, filter, .. }
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
        Brick::Question {
            text,
            options,
            answer,
            filter,
            ..
        } => {
            v.push(text);
            v.push(options);
            v.extend(answer.as_deref());
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Button { filter, .. }
        | Brick::Chat { filter, .. }
        | Brick::Message { filter, .. } => {
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Notice { field, filter, .. } => {
            v.push(field);
            if let Some(f) = filter {
                chemins_du_filtre(f, &mut v);
            }
        }
        Brick::Banner { .. }
        | Brick::Indicator { .. }
        | Brick::Relation { .. }
        | Brick::CapabilityRequest { .. }
        | Brick::Settings { .. }
        | Brick::Composer { .. }
        | Brick::Secret { .. }
        | Brick::Image { .. } => {}
    }
    v
}

/// Un problème de structure : `(chemin, raison, message)`.
type StructureIssue = (String, &'static str, String);

/// Un nœud `html` : le chemin du nœud, son fichier, ses demandes de données.
type HtmlRef<'a> = (String, &'a str, &'a [DataGrant]);

/// Une condition `when` : le chemin du nœud, l'entité lue, le filtre.
type ConditionRef<'a> = (String, &'a str, &'a FilterExpr);

/// Toutes les briques d'un arbre, avec leur chemin (`views[0].root.children[1]`), les
/// conditions `when` et les problèmes de structure rencontrés (profondeur, onglets).
fn collecte<'a>(
    chemin: &str,
    noeud: &'a Node,
    profondeur: usize,
    out: &mut Vec<(String, &'a Brick)>,
    structure: &mut Vec<StructureIssue>,
    conditions: &mut Vec<ConditionRef<'a>>,
    htmls: &mut Vec<HtmlRef<'a>>,
) {
    if profondeur > MAX_VIEW_DEPTH {
        structure.push((
            chemin.to_string(),
            "view_too_deep",
            format!(
                "containers are nested more than {MAX_VIEW_DEPTH} levels deep; flatten the tree or split it into several views"
            ),
        ));
        return;
    }
    match noeud {
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                collecte(
                    &format!("{chemin}.children[{i}]"),
                    c,
                    profondeur + 1,
                    out,
                    structure,
                    conditions,
                    htmls,
                );
            }
        }
        Node::When {
            entity,
            filter,
            child,
        } => {
            conditions.push((chemin.to_string(), entity.as_str(), filter));
            collecte(
                &format!("{chemin}.child"),
                child,
                profondeur + 1,
                out,
                structure,
                conditions,
                htmls,
            );
        }
        Node::Tabs { tabs } => {
            if tabs.is_empty() {
                structure.push((
                    chemin.to_string(),
                    "tabs_empty",
                    "a `tabs` container needs at least one tab; add a tab or use `stack`"
                        .to_string(),
                ));
            }
            let mut labels = BTreeSet::new();
            for (i, t) in tabs.iter().enumerate() {
                if t.label.is_empty() {
                    structure.push((
                        format!("{chemin}.tabs[{i}].label"),
                        "invalid_name",
                        "a tab needs a non-empty label".to_string(),
                    ));
                } else if !labels.insert(t.label.as_str()) {
                    structure.push((
                        format!("{chemin}.tabs[{i}].label"),
                        "duplicate_tab",
                        format!(
                            "tab `{}` appears twice in this container; give each tab its own label",
                            t.label
                        ),
                    ));
                }
                for (j, c) in t.children.iter().enumerate() {
                    collecte(
                        &format!("{chemin}.tabs[{i}].children[{j}]"),
                        c,
                        profondeur + 1,
                        out,
                        structure,
                        conditions,
                        htmls,
                    );
                }
            }
        }
        Node::Html { path, data, .. } => htmls.push((chemin.to_string(), path.as_str(), data)),
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
            live: None,
            name: name.into(),
            surface: Surface::Main,
            title: None,
            replaces: None,
            look: None,
            root,
        }
    }

    fn leaf(json: &str) -> Node {
        Node::Leaf {
            brick: Box::new(brick(json)),
        }
    }

    // ---------- issues_with_required ----------

    /// Un module sans entité dont une vue liste `entite`, qui requiert `requis`.
    fn liste_de(entite: &str, requis: &[&str]) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("menu", "1.0.0");
        m.requires = requis
            .iter()
            .map(|n| crate::module::ModuleReq {
                name: n.to_string(),
                version: "*".into(),
                optional: false,
            })
            .collect();
        m.views.push(view(
            "liste",
            leaf(&format!(
                r#"{{"type":"list","entity":"{entite}","label":"titre"}}"#
            )),
        ));
        m
    }

    fn socle_de(entite: &str) -> ModuleDescriptor {
        let mut s = ModuleDescriptor::new("socle", "1.0.0");
        let mut e = EntityDesc::new(entite);
        e.fields = vec![FieldDesc::new("titre", FieldType::String)];
        s.entities.push(e);
        s
    }

    #[test]
    fn une_vue_qui_liste_l_entite_d_un_module_requis_passe() {
        let socle = socle_de("conversation");
        let menu = liste_de("conversation", &["socle"]);
        // `issues` seul ne connaît que le module : l'entité lui est inconnue (inchangé).
        assert!(!issues(&menu).is_empty());
        assert!(issues_with_required(&menu, &[&socle, &menu]).is_empty());
    }

    #[test]
    fn la_meme_vue_sur_l_entite_d_un_module_non_requis_est_refusee_et_nommee() {
        let socle = socle_de("conversation");
        let menu = liste_de("conversation", &[]);
        let refus = issues_with_required(&menu, &[&socle, &menu]);
        assert!(
            refus.iter().any(|(raison, chemin, message)| {
                *raison == "view_entity_unknown"
                    && chemin.ends_with(".entity")
                    && message.contains("conversation")
            }),
            "{refus:?}"
        );
    }

    #[test]
    fn les_requis_se_suivent_en_transitif_et_un_cycle_ne_boucle_pas() {
        let socle = socle_de("conversation");
        let mut milieu = ModuleDescriptor::new("milieu", "1.0.0");
        milieu.requires = vec![crate::module::ModuleReq {
            name: "socle".into(),
            version: "*".into(),
            optional: false,
        }];
        // `socle` requiert `menu` : un cycle que `resolve` signale, ici on ne doit pas boucler.
        let mut socle_cyclique = socle.clone();
        socle_cyclique.requires = vec![crate::module::ModuleReq {
            name: "menu".into(),
            version: "*".into(),
            optional: false,
        }];
        let menu = liste_de("conversation", &["milieu"]);
        assert!(issues_with_required(&menu, &[&socle, &milieu, &menu]).is_empty());
        assert!(issues_with_required(&menu, &[&socle_cyclique, &milieu, &menu]).is_empty());
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

    /// Un module doit pouvoir décrire ce qui se dessine HORS de l'app : un widget d'écran
    /// d'accueil, un d'écran verrouillé, une Live Activity et ses dispositions d'île. Ce
    /// sont les mêmes briques, un autre hôte — et la carte des capacités dit lequel sait
    /// rendre quoi.
    #[test]
    fn les_surfaces_d_un_autre_hote_sont_declarables() {
        for (json, attendu) in [
            (r#""widget""#, Surface::Widget),
            (r#""lock_widget""#, Surface::LockWidget),
            (r#""activity""#, Surface::Activity),
            (r#""island""#, Surface::Island),
        ] {
            let surface: Surface = serde_json::from_str(json).expect(json);
            assert_eq!(surface, attendu, "{json}");
            // Et le nom repart tel qu'il est venu : le contrat ne se renomme pas.
            assert_eq!(serde_json::to_string(&attendu).unwrap(), json);
        }
    }

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
            live: None,
            surface: Surface::Header,
            ..leaf_view("count", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            live: None,
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

    fn module_live(every_secs: u32, surface: Surface) -> ModuleDescriptor {
        let mut m = module();
        let mut v = view("x", Node::Stack { children: vec![] });
        v.surface = surface;
        v.live = Some(Live { every_secs });
        m.views.push(v);
        m
    }

    #[test]
    fn a_live_view_is_declared_as_polling_and_round_trips() {
        assert_stable::<ViewDesc>(
            r#"{"name":"x","live":{"every_secs":30},"root":{"type":"stack","children":[]}}"#,
        );
        let sans: ViewDesc =
            serde_json::from_str(r#"{"name":"x","root":{"type":"stack","children":[]}}"#).unwrap();
        assert_eq!(sans.live, None);
        assert!(!serde_json::to_string(&sans).unwrap().contains("live"));
        assert!(
            serde_json::from_str::<ViewDesc>(
                r#"{"name":"x","live":{"every_secs":30,"push":true},"root":{"type":"stack","children":[]}}"#
            )
            .is_err()
        );
        for n in [LIVE_MIN_SECS, 30, LIVE_MAX_SECS] {
            assert!(codes(&module_live(n, Surface::Main)).is_empty(), "{n}");
        }
    }

    #[test]
    fn a_live_view_refuses_a_bad_interval_and_a_header() {
        assert_refused(&module_live(0, Surface::Main), "live_interval_too_small");
        assert_refused(
            &module_live(LIVE_MIN_SECS - 1, Surface::Main),
            "live_interval_too_small",
        );
        assert_refused(
            &module_live(LIVE_MAX_SECS + 1, Surface::Main),
            "live_interval_too_large",
        );
        assert_refused(&module_live(30, Surface::Header), "live_on_header");
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
        // capacité — `bluetooth` est VALIDE depuis le 08/11/2026 : le nom doit être inconnu,
        // sinon ce test cesse de tester ce qu'il annonce.
        assert!(
            serde_json::from_str::<Action>(
                r#"{"kind":"device_capability","name":"b","capability":"teleport","grant":"allow"}"#
            )
            .is_err()
        );
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
            label: None,
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
            label: None,
            name: "drop".into(),
            entity: "ghost".into(),
        });
        assert_refused(&m, "action_entity_unknown");
    }

    /// L'hôte n'écrit pas à la place du module : une demande sans texte est refusée.
    #[test]
    fn check_refuses_a_send_message_without_text() {
        let mut m = module();
        m.actions.push(Action::AgentTool {
            label: None,
            name: "demander".into(),
            op: AgentOp::SendMessage,
            entity: None,
            text: None,
        });
        assert_refused(&m, "action_agent_text_missing");
    }

    /// Un texte sur un arrêt ne ferait rien : mieux vaut le dire que l'ignorer.
    #[test]
    fn check_refuses_a_text_on_stop_run() {
        let mut m = module();
        m.actions.push(Action::AgentTool {
            label: None,
            name: "halt".into(),
            op: AgentOp::StopRun,
            entity: None,
            text: Some("arrête".into()),
        });
        assert_refused(&m, "action_agent_text_unused");
    }

    #[test]
    fn check_accepts_a_send_message_that_says_what_it_asks() {
        let mut m = module();
        m.actions.push(Action::AgentTool {
            label: None,
            name: "demander".into(),
            op: AgentOp::SendMessage,
            entity: None,
            text: Some("Quels articles manquent ?".into()),
        });
        assert!(codes(&m).is_empty(), "codes : {:?}", codes(&m));
    }

    /// Un texte fait d'espaces ne dit rien non plus.
    #[test]
    fn check_refuses_a_blank_text() {
        let mut m = module();
        m.actions.push(Action::AgentTool {
            label: None,
            name: "demander".into(),
            op: AgentOp::SendMessage,
            entity: None,
            text: Some("   ".into()),
        });
        assert_refused(&m, "action_agent_text_missing");
    }

    #[test]
    fn check_refuses_an_agent_op_on_an_unknown_entity() {
        let mut m = module();
        m.actions.push(Action::AgentTool {
            label: None,
            name: "halt".into(),
            op: AgentOp::StopRun,
            entity: Some("ghost".into()),
            text: None,
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
            label: None,
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
            label: None,
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

    /// Le composeur du fil : ce qu'il offre se déclare, et chaque refus a son motif.
    fn module_a_composeur(brique: &str) -> ModuleDescriptor {
        let mut m = module();
        for a in [
            r#"{"kind":"agent_tool","name":"resumer","op":"send_message","text":"Résume le fil."}"#,
            r#"{"kind":"create","name":"noter","entity":"invoice","fields":["titre"]}"#,
            r#"{"kind":"update","name":"corriger","entity":"invoice","fields":["titre"]}"#,
            r#"{"kind":"device_capability","name":"micro","capability":"microphone","grant":"allow"}"#,
        ] {
            m.actions.push(serde_json::from_str(a).unwrap());
        }
        m.views.push(ViewDesc {
            live: None,
            surface: Surface::Chat,
            ..leaf_view("saisie", brique)
        });
        m
    }

    #[test]
    fn a_valid_composer_passes_and_round_trips() {
        let json = r#"{"type":"composer","placeholder":"Écris…","actions":["resumer","noter"],"attachments":true}"#;
        assert_eq!(codes(&module_a_composeur(json)), Vec::<String>::new());
        assert_stable::<Brick>(json);
        // Le minimum : rien de déclaré, l'hôte garde ses défauts — et pas de pièces jointes.
        let nu = brick(r#"{"type":"composer"}"#);
        assert_eq!(
            nu,
            Brick::Composer {
                placeholder: None,
                actions: vec![],
                attachments: false
            }
        );
        assert_eq!(nu.entity(), None, "a composer reads no row");
        assert_eq!(
            serde_json::to_string(&nu).unwrap(),
            r#"{"type":"composer"}"#
        );
        assert_eq!(
            codes(&module_a_composeur(r#"{"type":"composer"}"#)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_composer_refuses_an_unknown_action_and_names_the_candidates() {
        let m = module_a_composeur(r#"{"type":"composer","actions":["resumr"]}"#);
        assert_refused(&m, "view_action_unknown");
        let issues = issues(&m);
        let (_, chemin, msg) = &issues[0];
        assert_eq!(chemin, "views[0].root.brick.actions[0]");
        assert!(msg.contains("Did you mean `resumer`?"), "{msg}");
    }

    #[test]
    fn a_composer_refuses_an_empty_or_blank_placeholder() {
        for p in ["", "   "] {
            let json = format!(r#"{{"type":"composer","placeholder":"{p}"}}"#);
            assert_refused(&module_a_composeur(&json), "composer_placeholder_empty");
        }
    }

    #[test]
    fn a_composer_refuses_a_duplicate_and_a_row_or_device_action() {
        assert_refused(
            &module_a_composeur(r#"{"type":"composer","actions":["resumer","resumer"]}"#),
            "duplicate_composer_action",
        );
        for a in ["corriger", "micro"] {
            let json = format!(r#"{{"type":"composer","actions":["{a}"]}}"#);
            assert_refused(&module_a_composeur(&json), "composer_action_refused");
        }
    }

    #[test]
    fn a_composer_does_not_fit_a_header_and_refuses_unknown_keys() {
        let mut m = module_a_composeur(r#"{"type":"composer"}"#);
        m.views[0].surface = Surface::Header;
        assert_refused(&m, "surface_brick_refused");
        assert!(serde_json::from_str::<Brick>(r#"{"type":"composer","send":true}"#).is_err());
    }

    #[test]
    fn check_refuses_a_big_brick_in_a_header() {
        let mut m = module();
        m.views.push(ViewDesc {
            live: None,
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

    // ---------- réglages : navigation inter-modules, secret, brique settings ----------

    fn module_requiring(name: &str) -> ModuleDescriptor {
        let mut m = module();
        m.requires.push(crate::ModuleReq {
            name: name.into(),
            version: "*".into(),
            optional: false,
        });
        m
    }

    #[test]
    fn navigate_to_another_module_needs_it_in_requires_and_not_the_view_locally() {
        let json =
            r#"{"kind":"navigate","name":"ailleurs","view":"providers","module":"reglages"}"#;
        // Required: accepted, and the view is NOT looked up locally.
        let mut m = module_requiring("reglages");
        m.actions.push(serde_json::from_str(json).unwrap());
        assert_eq!(codes(&m), Vec::<String>::new());
        // Not required: refused with a readable reason.
        let mut m = module();
        m.actions.push(serde_json::from_str(json).unwrap());
        assert_refused(&m, "navigate_module_not_required");
        let msg = m
            .check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap_err()
            .remove(0)
            .message;
        assert!(msg.contains("requires[]"), "{msg}");
        // Naming itself is refused too.
        let mut m = module();
        m.actions.push(Action::Navigate {
            label: None,
            name: "x".into(),
            view: "v".into(),
            module: Some(m.name.clone()),
        });
        assert_refused(&m, "navigate_module_is_self");
    }

    #[test]
    fn navigate_module_is_absent_from_the_json_when_unset() {
        let a: Action =
            serde_json::from_str(r#"{"kind":"navigate","name":"go","view":"cockpit"}"#).unwrap();
        assert!(!serde_json::to_string(&a).unwrap().contains("module"));
        assert_stable::<Action>(
            r#"{"kind":"navigate","name":"go","view":"cockpit","module":"reglages"}"#,
        );
    }

    #[test]
    fn a_settings_brick_with_navigate_and_secret_lines_is_accepted() {
        let mut m = module_requiring("reglages");
        m.actions.push(
            serde_json::from_str(
                r#"{"kind":"navigate","name":"ailleurs","view":"providers","module":"reglages"}"#,
            )
            .unwrap(),
        );
        m.actions.push(
            serde_json::from_str(r#"{"kind":"request_secret","name":"cle","secret":"api-key"}"#)
                .unwrap(),
        );
        m.views.push(leaf_view(
            "reglages",
            r#"{"type":"settings","sections":[{"title":"Moteur","hint":"d","items":[
                {"label":"Providers","action":"ailleurs"},
                {"label":"Clé API","hint":"jamais affichée","action":"cle"}]}]}"#,
        ));
        assert_eq!(codes(&m), Vec::<String>::new());
    }

    /// Une surface de l'hôte se déclare et s'offre : un bouton du menu la cite par son nom,
    /// et le contrat ne demande ni entité ni champ — l'hôte seul sait où elle vit.
    #[test]
    fn a_host_surface_is_a_declared_action_a_menu_can_offer() {
        let mut m = module();
        m.actions.push(
            serde_json::from_str(r#"{"kind":"host","name":"modules","surface":"modules"}"#)
                .unwrap(),
        );
        m.actions.push(
            serde_json::from_str(r#"{"kind":"host","name":"modele","surface":"model"}"#).unwrap(),
        );
        m.actions.push(
            serde_json::from_str(r#"{"kind":"host","name":"reglages","surface":"settings"}"#)
                .unwrap(),
        );
        // Un bouton de menu n'a pas de ligne à lire : la brique cite l'action, c'est tout.
        m.views.push(view(
            "menu",
            Node::Stack {
                children: vec![
                    leaf(r#"{"type":"button","entity":"invoice","action":"modules"}"#),
                    leaf(r#"{"type":"button","entity":"invoice","action":"modele"}"#),
                ],
            },
        ));
        assert_eq!(codes(&m), Vec::<String>::new());
        assert_eq!(
            m.actions.iter().map(Action::name).collect::<Vec<_>>(),
            vec!["modules", "modele", "reglages"]
        );
        assert_stable::<Action>(r#"{"kind":"host","name":"modules","surface":"modules"}"#);
        // Une surface inconnue est refusée à la désérialisation : l'hôte n'en invente pas.
        assert!(
            serde_json::from_str::<Action>(r#"{"kind":"host","name":"x","surface":"mystere"}"#)
                .is_err()
        );
    }

    /// Une ligne de réglages peut offrir une surface de l'hôte : elle ne lit aucune ligne.
    #[test]
    fn a_settings_line_can_offer_a_host_surface() {
        let mut m = module();
        m.actions.push(
            serde_json::from_str(r#"{"kind":"host","name":"modules","surface":"modules"}"#)
                .unwrap(),
        );
        m.views.push(leaf_view(
            "reglages",
            r#"{"type":"settings","sections":[{"title":"L'application","items":[
                {"label":"Modules installés","action":"modules"}]}]}"#,
        ));
        assert_eq!(codes(&m), Vec::<String>::new());
    }

    #[test]
    fn a_settings_line_cannot_name_a_row_action_and_a_header_refuses_settings() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"settings","sections":[{"title":"t","items":[{"label":"a","action":"ghost"}]}]}"#,
        ));
        assert_refused(&m, "view_action_unknown");
        let mut m = module();
        m.views.push(view(
            "x",
            Node::Leaf {
                brick: Box::new(brick(
                    r#"{"type":"settings","sections":[{"title":"t","items":[]}]}"#,
                )),
            },
        ));
        m.views[0].surface = Surface::Header;
        assert!(codes(&m).contains(&"this.descriptor.surface_brick_refused".to_string()));
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
            label: None,
            module: None,
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
            label: None,
            name: "d".into(),
            entity: "invoice".into(),
        });
        m.actions.push(Action::Delete {
            label: None,
            name: "d".into(),
            entity: "invoice".into(),
        });
        assert_refused(&m, "duplicate_action");
    }

    #[test]
    fn check_refuses_an_invalid_action_name() {
        let mut m = module();
        m.actions.push(Action::Delete {
            label: None,
            name: "Supprimer tout".into(),
            entity: "invoice".into(),
        });
        assert_refused(&m, "invalid_name");
    }

    #[test]
    fn check_refuses_a_view_replacing_an_unknown_view() {
        let mut m = module();
        m.views.push(ViewDesc {
            live: None,
            replaces: Some("ghost".into()),
            ..leaf_view("new_list", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert_refused(&m, "view_target_unknown");
    }

    #[test]
    fn check_refuses_a_view_replacing_itself() {
        let mut m = module();
        m.views.push(ViewDesc {
            live: None,
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
            live: None,
            replaces: Some("old".into()),
            ..leaf_view("a", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            live: None,
            replaces: Some("old".into()),
            ..leaf_view("b", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert_refused(&m, "view_replaced_twice");
    }

    #[test]
    fn check_refuses_a_replacement_cycle() {
        let mut m = module();
        m.views.push(ViewDesc {
            live: None,
            replaces: Some("b".into()),
            ..leaf_view("a", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            live: None,
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
            live: None,
            replaces: Some("v1".into()),
            ..leaf_view("v2", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        m.views.push(ViewDesc {
            live: None,
            replaces: Some("v2".into()),
            ..leaf_view("v3", r#"{"type":"metrics","entity":"invoice"}"#)
        });
        assert!(
            m.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }

    // ---------- hardening: actionable messages and new rules ----------

    fn messages(m: &ModuleDescriptor) -> Vec<String> {
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.message)
            .collect()
    }

    #[test]
    fn an_unknown_entity_message_lists_the_declared_ones_and_suggests_the_closest() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoce","label":"titre"}"#,
        ));
        let msg = messages(&m).join(" | ");
        assert!(msg.contains("declared: conversation, invoice"), "{msg}");
        assert!(msg.contains("Did you mean `invoice`?"), "{msg}");
        assert!(msg.contains("Declare it in `entities`"), "{msg}");
    }

    #[test]
    fn an_unknown_field_action_and_view_message_say_what_to_fix() {
        let mut m = module();
        m.actions.push(Action::Create {
            label: None,
            name: "new".into(),
            entity: "invoice".into(),
            fields: vec!["titre".into()],
        });
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titr"}"#,
        ));
        m.views.push(leaf_view(
            "y",
            r#"{"type":"button","entity":"invoice","action":"nev"}"#,
        ));
        m.views.push(leaf_view(
            "z",
            r#"{"type":"list","entity":"invoice","label":"titre","on_click":{"kind":"navigate","view":"zz"}}"#,
        ));
        let msg = messages(&m).join(" | ");
        assert!(msg.contains("Did you mean `titre`?"), "{msg}");
        assert!(msg.contains("Did you mean `new`?"), "{msg}");
        assert!(msg.contains("Add it to the module's `actions`"), "{msg}");
        assert!(msg.contains("Did you mean `z`?"), "{msg}");
    }

    #[test]
    fn a_number_input_needs_a_numeric_field_and_a_toggle_a_bool() {
        let mut m = module();
        m.entities[0]
            .fields
            .push(FieldDesc::new("paid", FieldType::Bool));
        m.entities[0]
            .fields
            .push(FieldDesc::new("qty", FieldType::I32));
        for (field, input) in [("qty", "number"), ("paid", "toggle"), ("titre", "text")] {
            let mut ok = m.clone();
            ok.views.push(leaf_view(
                "x",
                &format!(
                    r#"{{"type":"field","entity":"invoice","field":"{field}","input":"{input}"}}"#
                ),
            ));
            assert!(
                ok.check(&CustomRegistry::default(), CheckProfile::Compiled)
                    .is_ok(),
                "{field} as {input}"
            );
        }
        for (field, input) in [("titre", "number"), ("titre", "toggle"), ("paid", "number")] {
            let mut bad = m.clone();
            bad.views.push(leaf_view(
                "x",
                &format!(
                    r#"{{"type":"field","entity":"invoice","field":"{field}","input":"{input}"}}"#
                ),
            ));
            assert_refused(&bad, "field_input_invalid");
        }
    }

    #[test]
    fn a_form_submit_must_be_a_create_or_update_of_its_own_entity() {
        let mut m = module();
        m.actions.push(Action::Create {
            label: None,
            name: "mk_conv".into(),
            entity: "conversation".into(),
            fields: vec![],
        });
        // `fields: []` is itself refused; isolate the submit rule on a valid action.
        m.actions.clear();
        m.actions.push(Action::Delete {
            label: None,
            name: "drop".into(),
            entity: "invoice".into(),
        });
        m.views.push(leaf_view(
            "x",
            r#"{"type":"form","entity":"invoice","fields":[{"field":"titre"}],"submit":"drop"}"#,
        ));
        assert_refused(&m, "view_action_kind_mismatch");

        let mut m = module();
        m.entities[1]
            .fields
            .push(FieldDesc::new("titre", FieldType::String));
        m.actions.push(Action::Create {
            label: None,
            name: "mk_conv".into(),
            entity: "conversation".into(),
            fields: vec!["titre".into()],
        });
        m.views.push(leaf_view(
            "x",
            r#"{"type":"form","entity":"invoice","fields":[{"field":"titre"}],"submit":"mk_conv"}"#,
        ));
        assert_refused(&m, "view_action_kind_mismatch");
    }

    #[test]
    fn a_capability_request_must_point_at_the_matching_device_capability_action() {
        let mut m = module();
        m.actions.push(Action::DeviceCapability {
            label: None,
            name: "cam_ok".into(),
            capability: DeviceCapability::Camera,
            grant: Grant::Allow,
        });
        m.actions.push(Action::Delete {
            label: None,
            name: "drop".into(),
            entity: "invoice".into(),
        });
        let mut bad = m.clone();
        bad.views.push(leaf_view(
            "x",
            r#"{"type":"capability_request","capability":"camera","action":"drop"}"#,
        ));
        assert_refused(&bad, "view_action_kind_mismatch");
        let mut wrong = m.clone();
        wrong.views.push(leaf_view(
            "x",
            r#"{"type":"capability_request","capability":"microphone","action":"cam_ok"}"#,
        ));
        assert_refused(&wrong, "view_action_kind_mismatch");
        let mut ok = m;
        ok.views.push(leaf_view(
            "x",
            r#"{"type":"capability_request","capability":"camera","action":"cam_ok"}"#,
        ));
        assert!(
            ok.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }

    fn html_data(data: Vec<DataGrant>) -> ModuleDescriptor {
        let mut m = module();
        m.views.push(view(
            "x",
            Node::Html {
                path: "ui/index.html".into(),
                height: None,
                data,
            },
        ));
        m
    }

    fn grant(entity: &str) -> DataGrant {
        DataGrant {
            entity: entity.into(),
            filter: None,
            fields: None,
            limit: None,
        }
    }

    #[test]
    fn html_data_roundtrips_in_its_documented_shape() {
        let json = r#"{"type":"html","path":"ui/index.html","data":[{"entity":"invoice","filter":{"cmp":{"field":"amount","op":"eq","value":"1"}},"fields":["titre"],"limit":200}]}"#;
        let n: Node = serde_json::from_str(json).unwrap();
        let Node::Html { data, .. } = &n else {
            panic!("html")
        };
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].fields.as_deref(), Some(&["titre".to_string()][..]));
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
        assert!(
            html_data(vec![grant("invoice")])
                .check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }

    #[test]
    fn html_data_refusals_are_named() {
        assert_refused(&html_data(vec![grant("ghost")]), "html_data_entity_unknown");
        // Un champ non déclaré ; un champ de base autre que `id` a son propre refus.
        for champ in ["nope"] {
            assert_refused(
                &html_data(vec![DataGrant {
                    fields: Some(vec![champ.into()]),
                    ..grant("invoice")
                }]),
                "html_data_field_unknown",
            );
        }
        assert_refused(
            &html_data(vec![DataGrant {
                fields: Some(vec!["created_at".into()]),
                ..grant("invoice")
            }]),
            "html_data_field_reserved",
        );
        assert!(
            codes(&html_data(vec![DataGrant {
                fields: Some(vec!["id".into()]),
                ..grant("invoice")
            }]))
            .iter()
            .all(|c| !c.contains("html_data_field")),
            "`id` est accordable"
        );
        let filtre: FilterExpr =
            serde_json::from_str(r#"{"cmp":{"field":"nope","op":"eq","value":1}}"#).unwrap();
        assert_refused(
            &html_data(vec![DataGrant {
                filter: Some(filtre),
                ..grant("invoice")
            }]),
            "html_data_field_unknown",
        );
        for l in [0, HTML_DATA_MAX_LIMIT + 1] {
            assert_refused(
                &html_data(vec![DataGrant {
                    limit: Some(l),
                    ..grant("invoice")
                }]),
                "html_data_limit_out_of_range",
            );
        }
        assert_refused(
            &html_data(vec![grant("invoice"), grant("invoice")]),
            "html_data_entity_twice",
        );
        let six: Vec<DataGrant> = (0..=HTML_DATA_MAX_GRANTS)
            .map(|_| grant("invoice"))
            .collect();
        assert!(codes(&html_data(six)).contains(&"this.descriptor.html_data_too_many".to_string()));
    }

    #[test]
    fn an_html_data_entity_must_belong_to_the_module_or_one_it_requires() {
        let mut socle = ModuleDescriptor::new("socle", "1.0.0");
        socle.entities.push(EntityDesc::new("tache"));
        let mut menu = html_data(vec![grant("tache")]);
        // Hors du module et de ses requêtes : refusé.
        let refus = issues_with_required(&menu, &[&socle, &menu]);
        assert!(refus.iter().any(|(r, ..)| *r == "html_data_entity_unknown"));
        menu.requires.push(crate::module::ModuleReq {
            name: "socle".into(),
            version: "*".into(),
            optional: false,
        });
        assert!(
            issues_with_required(&menu, &[&socle, &menu])
                .iter()
                .all(|(r, ..)| !r.starts_with("html_data"))
        );
    }

    #[test]
    fn the_effective_limit_is_defaulted_and_clamped() {
        assert_eq!(grant("a").effective_limit(), 200);
        let g = |l| DataGrant {
            limit: Some(l),
            ..grant("a")
        };
        assert_eq!(g(50).effective_limit(), 50);
        assert_eq!(g(5000).effective_limit(), 1000);
        assert_eq!(g(0).effective_limit(), 1);
    }

    /// Un module dont `invoice` porte un entier (`qty`) en plus des deux textes.
    fn module_with_qty() -> ModuleDescriptor {
        let mut m = module();
        m.entities[0]
            .fields
            .push(FieldDesc::new("qty", FieldType::I32));
        m
    }

    fn chart_module(brick: &str) -> ModuleDescriptor {
        let mut m = module_with_qty();
        m.views.push(leaf_view("x", brick));
        m
    }

    #[test]
    fn chart_roundtrips_and_a_valid_one_passes() {
        let json = r#"{"type":"chart","entity":"invoice","field":"qty","kind":"line","label":"Quantités","height":160}"#;
        assert_stable::<Brick>(json);
        let minimal = r#"{"type":"chart","entity":"invoice","field":"qty","kind":"bar"}"#;
        assert_stable::<Brick>(minimal);
        assert_eq!(
            serde_json::to_value(brick(minimal)).unwrap(),
            serde_json::json!({"type":"chart","entity":"invoice","field":"qty","kind":"bar"})
        );
        for ok in [json, minimal] {
            assert!(
                chart_module(ok)
                    .check(&CustomRegistry::default(), CheckProfile::Compiled)
                    .is_ok(),
                "{ok}"
            );
        }
        // Les bornes de hauteur sont incluses.
        for h in [80, 400] {
            let b = format!(
                r#"{{"type":"chart","entity":"invoice","field":"qty","kind":"bar","height":{h}}}"#
            );
            assert!(
                chart_module(&b)
                    .check(&CustomRegistry::default(), CheckProfile::Compiled)
                    .is_ok(),
                "{h}"
            );
        }
    }

    #[test]
    fn chart_names_its_five_refusals() {
        assert_refused(
            &chart_module(r#"{"type":"chart","entity":"invoice","field":"qty","kind":"pie"}"#),
            "chart_kind_unknown",
        );
        // Un texte n'est pas une série.
        assert_refused(
            &chart_module(r#"{"type":"chart","entity":"invoice","field":"titre","kind":"bar"}"#),
            "chart_field_not_numeric",
        );
        for h in [79, 401] {
            let b = format!(
                r#"{{"type":"chart","entity":"invoice","field":"qty","kind":"bar","height":{h}}}"#
            );
            assert_refused(&chart_module(&b), "chart_height_out_of_range");
        }
        assert_refused(
            &chart_module(r#"{"type":"chart","entity":"ghost","field":"qty","kind":"bar"}"#),
            "chart_entity_unknown",
        );
        // Placement : pas dans un `header`.
        let mut m = module_with_qty();
        m.views.push(ViewDesc {
            live: None,
            surface: Surface::Header,
            ..leaf_view(
                "x",
                r#"{"type":"chart","entity":"invoice","field":"qty","kind":"bar"}"#,
            )
        });
        assert_refused(&m, "surface_brick_refused");
    }

    #[test]
    fn html_node_roundtrips_and_names_its_five_refusals() {
        let n: Node =
            serde_json::from_str(r#"{"type":"html","path":"ui/index.html","height":320}"#).unwrap();
        assert_eq!(
            n,
            Node::Html {
                path: "ui/index.html".into(),
                height: Some(320),
                data: vec![]
            }
        );
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::json!({"type":"html","path":"ui/index.html","height":320})
        );
        let sans: Node = serde_json::from_str(r#"{"type":"html","path":"ui/a.svg"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&sans).unwrap(),
            serde_json::json!({"type":"html","path":"ui/a.svg"})
        );
        let racine = |p: &str| {
            let mut m = module();
            m.views.push(view(
                "x",
                Node::Html {
                    path: p.into(),
                    height: None,
                    data: vec![],
                },
            ));
            m
        };
        // Acceptés : relatif, et chemin complet du module.
        for ok in ["ui/index.html", "/modules/billing/1.0.0/ui/index.htm"] {
            assert!(
                racine(ok)
                    .check(&CustomRegistry::default(), CheckProfile::Compiled)
                    .is_ok(),
                "{ok}"
            );
        }
        assert_refused(&racine(""), "html_path_empty");
        assert_refused(&racine("   "), "html_path_empty");
        assert_refused(&racine("../../etc/passwd.html"), "html_path_outside_module");
        assert_refused(&racine("ui/../../x.html"), "html_path_outside_module");
        assert_refused(&racine("/etc/passwd.html"), "html_path_outside_module");
        assert_refused(
            &racine("/modules/autre/1.0.0/ui/index.html"),
            "html_path_outside_module",
        );
        assert_refused(&racine("ui/app.js"), "html_extension_refused");
        assert_refused(&racine("ui/index"), "html_extension_refused");
        assert_refused(&racine("ui/logo.svg"), "html_svg_refused");
        assert_refused(&racine("a/b.SVG"), "html_svg_refused");
        let mut h = racine("ui/index.html");
        h.views[0].surface = Surface::Header;
        assert_refused(&h, "html_in_header");
    }

    #[test]
    fn image_roundtrips_and_names_its_six_refusals() {
        // Un fichier du module, puis une data URL lue dans une ligne.
        assert_stable::<Brick>(
            r#"{"type":"image","source":{"vfs":"assets/logo.png"},"alt":"Logo"}"#,
        );
        assert_stable::<Brick>(
            r#"{"type":"image","source":{"vfs":"assets/logo.png"},"alt":"Logo","height":160,"fit":"contain"}"#,
        );
        assert_stable::<Brick>(
            r#"{"type":"image","source":{"entity":"invoice","field":"titre"},"alt":"Photo"}"#,
        );
        let racine = |brick: &str| {
            let mut m = module();
            m.views.push(leaf_view("x", brick));
            m
        };
        // Acceptés : fichier propre, data URL, chemin complet du module, bornes de hauteur.
        for ok in [
            r#"{"type":"image","source":{"vfs":"assets/logo.png"},"alt":"Logo"}"#,
            r#"{"type":"image","source":{"entity":"invoice","field":"titre"},"alt":"Photo"}"#,
            r#"{"type":"image","source":{"vfs":"/modules/billing/1.0.0/assets/logo.png"},"alt":"Logo","height":600}"#,
            r#"{"type":"image","source":{"vfs":"assets/logo.png"},"alt":"Logo","height":24}"#,
        ] {
            assert!(
                racine(ok)
                    .check(&CustomRegistry::default(), CheckProfile::Compiled)
                    .is_ok(),
                "{ok}"
            );
        }
        // Les six refus, nommément.
        assert_refused(
            &racine(r#"{"type":"image","source":{},"alt":"Logo"}"#),
            "image_source_missing",
        );
        assert_refused(
            &racine(
                r#"{"type":"image","source":{"vfs":"a.png","entity":"invoice","field":"titre"},"alt":"L"}"#,
            ),
            "image_source_ambiguous",
        );
        assert_refused(
            &racine(r#"{"type":"image","source":{"vfs":"a.png"},"alt":""}"#),
            "image_alt_missing",
        );
        for h in [23, 601] {
            let b =
                format!(r#"{{"type":"image","source":{{"vfs":"a.png"}},"alt":"L","height":{h}}}"#);
            assert_refused(&racine(&b), "image_height_out_of_range");
        }
        assert_refused(
            &racine(r#"{"type":"image","source":{"vfs":"../../secret.png"},"alt":"L"}"#),
            "image_vfs_outside_module",
        );
        assert_refused(
            &racine(r#"{"type":"image","source":{"vfs":"https://evil.example/x.png"},"alt":"L"}"#),
            "image_remote_refused",
        );
        assert_refused(
            &racine(r#"{"type":"image","source":{"vfs":"//evil.example/x.png"},"alt":"L"}"#),
            "image_remote_refused",
        );
        // Un chemin d'un AUTRE module est refusé, comme pour `html`.
        assert_refused(
            &racine(r#"{"type":"image","source":{"vfs":"/modules/autre/1.0.0/a.png"},"alt":"L"}"#),
            "image_vfs_outside_module",
        );
        // Placement : pas dans un header.
        let mut h = racine(r#"{"type":"image","source":{"vfs":"a.png"},"alt":"L"}"#);
        h.views[0].surface = Surface::Header;
        assert_refused(&h, "surface_brick_refused");
    }

    #[test]
    fn duplicate_form_fields_tabs_and_action_fields_are_refused() {
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"form","entity":"invoice","fields":[{"field":"titre"},{"field":"titre","label":"again"}]}"#,
        ));
        assert_refused(&m, "duplicate_form_field");

        let mut m = module();
        m.views.push(view(
            "x",
            serde_json::from_str(
                r#"{"type":"tabs","tabs":[{"label":"A","children":[]},{"label":"A","children":[]}]}"#,
            )
            .unwrap(),
        ));
        assert_refused(&m, "duplicate_tab");

        let mut m = module();
        m.actions.push(Action::Update {
            label: None,
            name: "fix".into(),
            entity: "invoice".into(),
            fields: vec!["amount".into(), "amount".into()],
        });
        assert_refused(&m, "duplicate_action_field");
    }

    #[test]
    fn empty_tabs_and_a_too_deep_tree_are_refused() {
        let mut m = module();
        m.views.push(view("x", Node::Tabs { tabs: vec![] }));
        assert_refused(&m, "tabs_empty");

        let mut deep = Node::Stack { children: vec![] };
        for _ in 0..MAX_VIEW_DEPTH {
            deep = Node::Stack {
                children: vec![deep],
            };
        }
        let mut m = module();
        m.views.push(view("x", deep));
        assert_refused(&m, "view_too_deep");

        // Exactly the maximum depth is accepted.
        let mut ok = Node::Stack { children: vec![] };
        for _ in 0..MAX_VIEW_DEPTH - 1 {
            ok = Node::Stack { children: vec![ok] };
        }
        let mut m = module();
        m.views.push(view("x", ok));
        assert!(
            m.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok()
        );
    }

    #[test]
    fn activity_and_badge_indicators_need_a_field_and_page_size_has_an_upper_bound() {
        for signal in ["activity", "badge"] {
            let mut m = module();
            m.views.push(leaf_view(
                "x",
                &format!(r#"{{"type":"indicator","signal":"{signal}","entity":"invoice"}}"#),
            ));
            assert_refused(&m, "view_field_required");
        }
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titre","page_size":100}"#,
        ));
        assert!(
            m.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .is_ok(),
            "100 is the clamp and is accepted"
        );
        let mut m = module();
        m.views.push(leaf_view(
            "x",
            r#"{"type":"list","entity":"invoice","label":"titre","page_size":101}"#,
        ));
        assert_refused(&m, "invalid_page_size");
    }

    #[test]
    fn a_three_view_replacement_cycle_is_reported_on_each_member() {
        let mut m = module();
        for (n, t) in [("a", "b"), ("b", "c"), ("c", "a")] {
            m.views.push(ViewDesc {
                live: None,
                replaces: Some(t.into()),
                ..leaf_view(n, r#"{"type":"metrics","entity":"invoice"}"#)
            });
        }
        let cycles = codes(&m)
            .into_iter()
            .filter(|c| c == "this.descriptor.view_replaces_cycle")
            .count();
        assert_eq!(cycles, 3);
    }

    #[test]
    fn a_view_leading_into_a_cycle_does_not_loop_and_the_cycle_is_still_reported() {
        let mut m = module();
        for (n, t) in [("tail", "a"), ("a", "b"), ("b", "a")] {
            m.views.push(ViewDesc {
                live: None,
                replaces: Some(t.into()),
                ..leaf_view(n, r#"{"type":"metrics","entity":"invoice"}"#)
            });
        }
        let got = codes(&m);
        let cycles = got
            .iter()
            .filter(|c| *c == "this.descriptor.view_replaces_cycle")
            .count();
        assert_eq!(cycles, 2, "only a and b are on the cycle: {got:?}");
    }

    #[test]
    fn the_number_input_round_trips() {
        assert_stable::<Brick>(
            r#"{"type":"field","entity":"invoice","field":"amount","input":"number"}"#,
        );
    }

    // ---------- libellés et conditions ----------

    #[test]
    fn a_label_is_interface_text_and_nothing_else_is_refused() {
        assert_eq!(label_problem("Épingler cette clé"), None);
        assert_eq!(label_problem("Se connecter — vite !"), None);
        assert_eq!(label_problem(""), Some(LabelProblem::Empty));
        assert_eq!(label_problem("  "), Some(LabelProblem::Empty));
        assert_eq!(label_problem("epingler_cle"), Some(LabelProblem::NotText));
        assert_eq!(label_problem("Deux\nlignes"), Some(LabelProblem::NotText));
    }

    #[test]
    fn a_label_on_a_brick_an_action_or_a_notice_is_checked_and_round_trips() {
        assert_stable::<Brick>(
            r#"{"type":"button","entity":"invoice","action":"new","label":"Nouvelle facture"}"#,
        );
        assert_stable::<Action>(
            r#"{"kind":"delete","name":"revoquer","label":"Révoquer","entity":"invoice"}"#,
        );
        let mut m = module();
        m.actions.push(Action::Delete {
            name: "revoquer".into(),
            label: Some("Révoquer".into()),
            entity: "invoice".into(),
        });
        m.views.push(leaf_view(
            "ok",
            r#"{"type":"button","entity":"invoice","action":"revoquer","label":"Retirer la clé"}"#,
        ));
        assert!(
            !codes(&m).iter().any(|c| c.contains("label")),
            "{:?}",
            codes(&m)
        );
        assert_eq!(m.actions.last().and_then(Action::label), Some("Révoquer"));

        m.views.push(leaf_view(
            "bad",
            r#"{"type":"button","entity":"invoice","action":"revoquer","label":"retirer_cle"}"#,
        ));
        m.actions.push(Action::Delete {
            name: "autre".into(),
            label: Some("".into()),
            entity: "invoice".into(),
        });
        let got = codes(&m);
        assert!(
            got.contains(&"this.descriptor.label_not_text".to_string()),
            "{got:?}"
        );
        assert!(
            got.contains(&"this.descriptor.label_empty".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn a_when_node_round_trips_and_wraps_any_node() {
        let json = r#"{"type":"when","entity":"invoice","filter":{"cmp":{"field":"titre","op":"eq","value":"A"}},"child":{"type":"stack","children":[]}}"#;
        let n: Node = serde_json::from_str(json).expect("when parses");
        assert_eq!(serde_json::to_string(&n).expect("ser"), json);
    }

    #[test]
    fn a_when_condition_names_a_known_entity_and_known_fields() {
        let ok = r#"{"type":"when","entity":"invoice","filter":{"cmp":{"field":"titre","op":"eq","value":"A"}},"child":{"type":"leaf","brick":{"type":"metrics","entity":"invoice"}}}"#;
        let bad_entity = ok.replace(
            r#""entity":"invoice","filter""#,
            r#""entity":"ghost","filter""#,
        );
        let bad_field = ok.replace(r#""field":"titre""#, r#""field":"nom""#);
        for (json, want) in [
            (ok.to_string(), None),
            (bad_entity, Some("this.descriptor.view_entity_unknown")),
            (bad_field, Some("this.descriptor.view_field_unknown")),
        ] {
            let mut m = module();
            m.views
                .push(view("c", serde_json::from_str(&json).expect("node")));
            let got = codes(&m);
            match want {
                None => assert!(got.is_empty(), "{got:?}"),
                Some(code) => assert!(got.contains(&code.to_string()), "{got:?}"),
            }
        }
    }

    #[test]
    fn a_when_node_counts_as_a_nesting_level_and_its_child_is_still_checked() {
        let mut m = module();
        let child = r#"{"type":"leaf","brick":{"type":"list","entity":"invoice","label":"nom"}}"#;
        let json = format!(
            r#"{{"type":"when","entity":"invoice","filter":{{"cmp":{{"field":"titre","op":"eq","value":"A"}}}},"child":{child}}}"#
        );
        m.views
            .push(view("c", serde_json::from_str(&json).expect("node")));
        assert!(
            codes(&m).contains(&"this.descriptor.view_field_unknown".to_string()),
            "a brick under a `when` is checked like any other"
        );
    }
}
