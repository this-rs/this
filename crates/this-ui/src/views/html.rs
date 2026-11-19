//! # `Node::Html` : la page que le module livre, dans une iframe sandboxée
//!
//! C'est la porte illimitée de l'interface. Les briques disent ce qu'elles montrent ; une
//! page HTML écrit tout le reste — et sort du contrat : plus de thème imposé, plus de
//! contraste vérifié, plus de cible de 44 px. L'hôte ne garde que deux choses : l'**isolement**
//! et le **pont**.
//!
//! ## L'isolement
//!
//! `<iframe sandbox="allow-scripts" src="blob:…">`, et JAMAIS `allow-same-origin` : une
//! iframe qui garde l'origine de l'hôte lirait `window.parent.__TAURI__` et appellerait les
//! commandes natives. Sans elle, l'origine est opaque. [`SANDBOX`] est l'unique source de
//! l'attribut ; un test refuse tout fichier qui écrirait ce mot ailleurs que dans ses tests.
//!
//! ## Le pont (explicite, dans les deux sens)
//!
//! - hôte → page : `{type:"this:data", rows:{entité:[…]}, theme, locale}`. Si le nœud porte
//!   `data` (des [`DataGrant`]), `rows` contient EXACTEMENT ces entités — filtrées, bornées
//!   par `limit`, projetées sur les champs demandés — et rien d'autre ; une lecture refusée
//!   ou en échec n'envoie aucune ligne et dit pourquoi au journal ([`grants_message`]).
//!   Sans `data`, uniquement les entités que la vue déclare (celles que ses autres nœuds
//!   lisent). Le message est renvoyé à chaque changement des données ;
//! - page → hôte : `{type:"this:intent", kind, action, id?, values?, request?}`, validé par
//!   [`page_demand`] : le genre est l'un de [`INTENT_KINDS`], l'action est DÉCLARÉE par le
//!   module et de ce genre. Le reste est refusé : l'hôte journalise et n'exécute rien ;
//! - page → hôte : `{type:"this:height", px}` règle la hauteur (bornée à [`MAX_HEIGHT`]) ;
//! - hôte → page : `{type:"this:result", id, ok:true}` ou `{type:"this:refused", id, reason}`,
//!   seulement si la demande portait `request` (voir « La réponse »).
//!
//! ## Ce qu'une page atteint : tout ce que son module déclare, sous contrôle
//!
//! Il n'y a plus de genre interdit à une page. Une page émet tout genre d'action que son
//! module DÉCLARE — ce qu'une brique du même module peut déclencher, ni plus ni moins. Mais
//! les genres ne se valent pas, et [`INTENT_KINDS`] les sépare :
//!
//! - **ordinaires** (`create`, `update`, `delete`, `invoke`, `navigate`) : remis à l'hôte
//!   dès qu'ils sont validés, comme avant ;
//! - **sensibles** (`secret`, `tool`, `capability`, `host`) : un secret, un outil d'agent,
//!   une capacité de l'appareil, une surface de l'hôte. **Fermés par défaut** : ils ne sont
//!   remis à l'hôte que si celui-ci fournit une porte ([`HtmlHost::gate`]) ET qu'elle répond
//!   [`GateAnswer::Approved`]. Sans porte, le refus est nommé (`no_gate`) et rien n'est
//!   exécuté : un hôte qui ne connaît pas la porte se comporte exactement comme avant.
//!
//! La raison de la porte : une brique ne s'active que par un geste de la personne sur un
//! contrôle que l'hôte a dessiné ; le script d'une page émet seul, sans geste. L'approbation
//! remet la personne dans la boucle. La porte reçoit une [`GateRequest`] (module, genre,
//! action déclarée) : de quoi écrire « le module X veut utiliser le micro ». L'écran
//! d'approbation est celui de l'hôte, pas de cette crate.
//!
//! Le chemin est trois fonctions pures, testées sans navigateur, et le montage n'en est que
//! le câblage : [`page_demand`] (valider) → [`admit`] (porte présente ? débit ?) →
//! [`settle`] (la réponse de la porte).
//!
//! ### L'origine
//!
//! Une intention née d'une page est remise dans une enveloppe [`Emitted`] qui porte
//! [`Origin::Page`], à [`HtmlHost::on_page_intent`] si l'hôte le fournit (sinon, comme avant,
//! l'intention nue va au `on_intent` de la vue). L'origine est une enveloppe et pas un champ
//! de chaque variante d'[`Intent`] : elle décrit le chemin d'arrivée, pas le contenu —
//! l'intention d'une page reste égale à celle d'une brique pour la même action, et aucun
//! hôte existant n'a de `match` à réécrire. La [`GateRequest`] porte aussi l'origine.
//!
//! ### La réponse
//!
//! Si le message porte `request` (une chaîne d'au plus [`CORRELATION_MAX_LEN`] caractères ou
//! un nombre — `id` est déjà pris : c'est la ligne visée), l'hôte répond à la fenêtre de
//! CETTE iframe, et à elle seule, avec `id` = la valeur de `request` :
//! `{type:"this:result", id, ok:true}` quand l'intention a été REMISE à l'hôte (ce n'est pas
//! la preuve que l'hôte a réussi : `this-ui` n'exécute rien), ou
//! `{type:"this:refused", id, reason}` où `reason` est un code stable de [`Reason`]. La
//! réponse ne porte jamais de valeur : ni secret, ni ligne. Sans `request`, rien n'est
//! répondu — le comportement d'avant.
//!
//! ### Le débit
//!
//! Un script ne doit pas inonder la porte : au plus [`SENSITIVE_MAX_PER_WINDOW`] demandes
//! sensibles par fenêtre glissante de [`SENSITIVE_WINDOW_MS`] ms et par page montée
//! ([`RateWindow`]). Au-delà, refus `rate_limited`, la porte n'est pas appelée.
//!
//! ### Ce qui ne bouge pas
//!
//! Une action non déclarée ne passe jamais ; une action émise sous un autre genre que le
//! sien est refusée ; la valeur d'un secret ne transite jamais par le pont (une demande
//! `secret` porte un NOM, ses `values` sont ignorées, et `secret_entry` n'est pas un genre
//! qu'une page peut émettre).

use std::collections::{BTreeSet, VecDeque};

use leptos::prelude::*;
use serde_json::{Map, Value, json};
use this_descriptor::ModuleDescriptor;
use this_descriptor::engine::query::matches;
use this_descriptor::views::{
    DataGrant, HTML_DATA_MAX_LIMIT, Node, html_field_verdict, html_resolve,
};

use crate::views::data::Snapshot;
use this_descriptor::views::Action;

use crate::views::intent::{Emitted, FieldValue, Intent, Origin, Values};
use crate::views::render::{self, Ctx};

/// Les droits de l'iframe : des scripts, rien d'autre. Surtout pas l'origine de l'hôte.
pub const SANDBOX: &str = "allow-scripts";

/// La hauteur maximale d'une iframe, en px.
pub const MAX_HEIGHT: u32 = this_descriptor::views::HTML_MAX_HEIGHT;

/// La hauteur avant que la page n'ait dit la sienne (`this:height`).
pub const DEFAULT_HEIGHT: u32 = 320;

/// Au-delà, le fichier n'est pas monté : un Blob de cette taille vit en mémoire le temps de
/// la vue.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// La lecture d'un fichier : les octets, ou rien s'il n'existe pas.
pub type ReadFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<u8>>>>>;

/// La réponse de la porte, plus tard : la personne lit et décide.
pub type GateFuture = std::pin::Pin<Box<dyn std::future::Future<Output = GateAnswer>>>;

/// Ce que la porte de l'hôte répond à une demande sensible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAnswer {
    /// La personne (ou la politique de l'hôte) a dit oui : l'intention est remise.
    Approved,
    /// Non : rien n'est exécuté, la page reçoit `denied`.
    Denied,
}

/// Ce que la porte reçoit : qui demande quoi. De quoi écrire « le module X veut utiliser le
/// micro » sans rien deviner.
#[derive(Clone, Debug, PartialEq)]
pub struct GateRequest {
    /// Le module dont la page émet la demande.
    pub module: String,
    /// Le genre sensible, tel que [`Intent::kind`] le nomme : `secret`, `tool`,
    /// `capability` ou `host`.
    pub kind: &'static str,
    /// Le nom de l'action déclarée.
    pub name: String,
    /// L'action telle que le module la déclare (son libellé, sa capacité, sa surface…).
    pub action: Action,
    /// L'intention qui sera remise à l'hôte si la porte dit oui — celle qu'une brique aurait
    /// produite pour cette action.
    pub intent: Intent,
    /// D'où vient la demande. Aujourd'hui toujours [`Origin::Page`] : les briques ne passent
    /// pas par la porte.
    pub origin: Origin,
}

/// Ce que l'hôte fournit (par le contexte Leptos) pour monter une page : lire un fichier du
/// VFS, dire le thème et la langue. Sans lui, le nœud `html` dit pourquoi il est vide.
///
/// `gate` et `on_page_intent` sont optionnels : `None` pour les deux, c'est exactement le
/// comportement d'avant la porte ([`HtmlHost::new`]).
#[derive(Clone, Copy)]
pub struct HtmlHost {
    /// Lit les octets d'un fichier du VFS (chemin complet `/modules/<nom>/<version>/…`).
    /// Asynchrone : le VFS d'une app l'est.
    pub read: Callback<String, ReadFuture>,
    /// Le thème courant, tel que l'hôte le nomme (`light`, `dark`).
    pub theme: Signal<String>,
    /// La langue courante (`fr`, `en`).
    pub locale: Signal<String>,
    /// La porte des genres sensibles : l'hôte demande à la personne et répond. `None` =
    /// fermé : toute demande sensible d'une page est refusée (`no_gate`), rien n'est exécuté.
    pub gate: Option<Callback<GateRequest, GateFuture>>,
    /// Où vont les intentions nées d'une page, avec leur origine. `None` = l'intention nue
    /// va au `on_intent` de la vue, comme celle d'une brique.
    pub on_page_intent: Option<Callback<Emitted>>,
}

impl HtmlHost {
    /// Un hôte sans porte : les genres sensibles restent fermés aux pages.
    pub fn new(
        read: Callback<String, ReadFuture>,
        theme: Signal<String>,
        locale: Signal<String>,
    ) -> Self {
        Self {
            read,
            theme,
            locale,
            gate: None,
            on_page_intent: None,
        }
    }

    /// Avec la porte des genres sensibles.
    pub fn with_gate(mut self, gate: Callback<GateRequest, GateFuture>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// Avec un récepteur des intentions de page qui en connaît l'origine.
    pub fn with_page_intents(mut self, on_page_intent: Callback<Emitted>) -> Self {
        self.on_page_intent = Some(on_page_intent);
        self
    }
}

/// La hauteur d'une iframe : celle déclarée, bornée à `1..=MAX_HEIGHT`, sinon la hauteur par
/// défaut.
pub fn height_px(declared: Option<u32>) -> u32 {
    declared.unwrap_or(DEFAULT_HEIGHT).clamp(1, MAX_HEIGHT)
}

/// Le type de contenu d'un fichier servi, d'après son extension.
pub fn content_type(path: &str) -> &'static str {
    if path.to_ascii_lowercase().ends_with(".svg") {
        "image/svg+xml"
    } else {
        "text/html;charset=utf-8"
    }
}

/// La politique que l'hôte impose à chaque page qu'il sert. `default-src 'none'` ferme tout ce
/// que la politique ne nomme pas ; `connect-src 'none'` coupe `fetch`, `XHR`, `sendBeacon` et
/// `WebSocket` — rien ne sort. `script-src`/`style-src 'unsafe-inline'` gardent la liberté du
/// module (CSS et JS en ligne) ; `img-src`/`font-src` n'autorisent que les données embarquées.
pub const HOST_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; script-src 'unsafe-inline'; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'";

/// La ligne de journal d'un montage : la politique de l'hôte a été imposée.
pub fn csp_imposed_line(module: &str, view: &str) -> String {
    format!("html : {module}/{view} — CSP imposée (default-src 'none')")
}

/// La réécriture que l'hôte impose aux octets servis, avant le Blob. Un `.svg` est refusé
/// nommément : il peut porter ses propres scripts et références externes, et un `<meta>` CSP ne
/// s'applique pas dans un document SVG. Sinon la CSP de l'hôte est injectée, intersectée avec
/// celle du module — la plus stricte gagne, un module ne peut pas relâcher la règle de l'hôte.
pub fn rewrite_served(path: &str, bytes: Vec<u8>) -> Result<Vec<u8>, (&'static str, String)> {
    if content_type(path) == "image/svg+xml" {
        return Err((
            "html_svg_refused",
            format!(
                "`{path}` est un SVG : il peut porter ses propres scripts et références \
                 externes, et un <meta> CSP ne s'applique pas dans un document SVG — \
                 servez-le comme image, pas comme page"
            ),
        ));
    }
    let html = String::from_utf8(bytes).map_err(|_| {
        (
            "html_not_utf8",
            format!("`{path}` n'est pas du texte UTF-8 ; un nœud `html` ne sert que du HTML"),
        )
    })?;
    Ok(inject_link_guard(&inject_csp(&html)).into_bytes())
}

/// Injecte la politique de l'hôte (intersectée avec celle du module) dans le HTML : dans le
/// `<head>` s'il existe, sinon après le doctype, sinon tout en tête — le parseur HTML hisse
/// toujours un `<meta>` de tête dans le `head`, aucun contenu du module ne peut le précéder.
pub fn inject_csp(html: &str) -> String {
    let (stripped, module_policy) = strip_csp_metas(html);
    let policy = intersect_policies(&module_policy);
    inject_meta(&stripped, &csp_meta(&policy))
}

/// La garde de navigation : un `<script>` en phase de capture sur `window`, posé AVANT tout
/// script du module. `navigate-to` n'est pas appliqué par les navigateurs et le sandbox ne
/// bloque pas l'auto-navigation d'un lien : un `<a href="https://…?d=…">` cliqué ferait
/// partir la donnée dans l'URL. La garde ferme ce dernier chemin : les ancres internes
/// (`#…`), les `blob:` et les pages `data:text/html` restent libres — navigation DANS la page.
const LINK_GUARD: &str = r#"<script>addEventListener('click', function (e) {
  var a = e.target && e.target.closest && e.target.closest('a[href]');
  if (!a) return;
  var h = a.getAttribute('href') || '';
  if (h.startsWith('#') || h.startsWith('blob:') || h.startsWith('data:text/html')) return;
  e.preventDefault(); e.stopImmediatePropagation();
  parent.postMessage({type:'this:intent', kind:'link_refused', href:h.slice(0,120)}, '*');
}, true);</script>"#;

/// Décide si un `href` est un lien sortant que l'hôte bloque — la traduction Rust exacte de
/// la condition de [`LINK_GUARD`]. Les ancres internes (`#…`), les `blob:` et les pages
/// `data:text/html` restent libres ; tout le reste est un chemin de sortie.
pub fn blocks_link(href: &str) -> bool {
    !href.starts_with('#') && !href.starts_with("blob:") && !href.starts_with("data:text/html")
}

/// La ligne de journal d'un lien sortant bloqué par la garde. `href` est déjà tronqué côté
/// page (`slice(0,120)`) : c'est le début d'URL, jamais la donnée entière.
pub fn link_refused_line(href: &str) -> String {
    format!("html : lien sortant refusé — {href}")
}

/// Injecte la garde de navigation dans le HTML : posée AVANT le premier `<script>` du module
/// (un script qui s'exécute avant la garde pourrait la neutraliser), sinon dans le `<head>`
/// (ou après le doctype, sinon en tête) comme la CSP.
pub fn inject_link_guard(html: &str) -> String {
    match first_script_index(html) {
        Some(at) => splice(html, at, LINK_GUARD),
        None => inject_meta(html, LINK_GUARD),
    }
}

/// L'index du `<` du premier `<script>` réel de la page, en sautant les commentaires. `None`
/// si la page n'en porte pas.
fn first_script_index(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < html.len() {
        let rel = lower[i..].find('<')?;
        let lt = i + rel;
        if lower[lt..].starts_with("<!--") {
            let off = lower[lt + 4..].find("-->")?;
            i = lt + 4 + off + 3;
            continue;
        }
        let after = &lower[lt + 1..];
        let name_end = after
            .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
            .unwrap_or(after.len());
        if &after[..name_end] == "script" {
            return Some(lt);
        }
        i = close_of_tag(html, lt)?;
    }
    None
}

/// Une directive d'une politique : `default-src` → `'none'`.
type Directive = (String, String);

/// Découpe une politique en directives (nom en minuscules, valeur).
fn parse_policy(policy: &str) -> Vec<Directive> {
    policy
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .filter_map(|d| {
            let mut it = d.splitn(2, char::is_whitespace);
            let name = it.next()?.to_ascii_lowercase();
            let value = it.next()?.trim().to_owned();
            Some((name, value))
        })
        .collect()
}

/// `list` contient le `token` exact, séparé par des espaces.
fn has_token(list: &str, token: &str) -> bool {
    list.split_whitespace().any(|t| t == token)
}

/// L'intersection de deux listes de sources : la plus stricte gagne. `'none'` (l'ensemble vide)
/// écrase tout ; `*` cède à l'autre côté ; deux listes concrètes différentes ne prouvent pas
/// une intersection non vide, donc échouent fermées.
fn intersect_sources(a: &str, b: &str) -> String {
    if has_token(a, "'none'") || has_token(b, "'none'") {
        return "'none'".into();
    }
    if has_token(a, "*") {
        return b.to_owned();
    }
    if has_token(b, "*") {
        return a.to_owned();
    }
    let ta: BTreeSet<_> = a.split_whitespace().collect();
    let tb: BTreeSet<_> = b.split_whitespace().collect();
    if ta == tb {
        return a.to_owned();
    }
    "'none'".into()
}

/// L'intersection de la politique de l'hôte avec celle du module, directive par directive. Le
/// `default-src 'none'` de l'hôte est le repli de toute directive qu'il ne nomme pas : une
/// directive du module sans contrepartie de l'hôte est déjà fermée par ce repli.
fn intersect_policies(module: &str) -> String {
    let host = parse_policy(HOST_CSP);
    let module = parse_policy(module);
    let module_value = |name: &str| -> Option<String> {
        module
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .or_else(|| {
                module
                    .iter()
                    .find(|(n, _)| n == "default-src")
                    .map(|(_, v)| v.clone())
            })
    };
    let mut out: Vec<String> = Vec::new();
    for (name, host_value) in host {
        let value = match module_value(&name) {
            Some(mv) => intersect_sources(&host_value, &mv),
            None => host_value.clone(),
        };
        out.push(format!("{name} {value}"));
    }
    out.join("; ")
}

/// Le `<meta>` qui porte une politique.
fn csp_meta(policy: &str) -> String {
    format!(r#"<meta http-equiv="Content-Security-Policy" content="{policy}">"#)
}

/// Retire chaque `<meta http-equiv="Content-Security-Policy">` de `html` et rend le HTML sans
/// elles, plus la concaténation de leurs politiques.
fn strip_csp_metas(html: &str) -> (String, String) {
    let mut out = String::with_capacity(html.len());
    let mut policies: Vec<String> = Vec::new();
    let mut rest = html;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(open) = lower.find("<meta") else {
            out.push_str(rest);
            break;
        };
        let is_tag = match rest.as_bytes().get(open + 5).copied() {
            Some(b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>') | None => true,
            Some(_) => false,
        };
        if !is_tag {
            out.push_str(&rest[..open + 5]);
            rest = &rest[open + 5..];
            continue;
        }
        let Some(close) = close_of_tag(rest, open) else {
            out.push_str(rest);
            break;
        };
        let tag = &rest[open..close];
        let inner = &tag[1..tag.len() - 1];
        let attrs = attributes(inner);
        let is_csp = attrs
            .iter()
            .any(|(n, v)| n == "http-equiv" && v.eq_ignore_ascii_case("content-security-policy"));
        if is_csp {
            if let Some((_, content)) = attrs.iter().find(|(n, _)| n == "content") {
                policies.push(content.clone());
            }
            out.push_str(&rest[..open]);
        } else {
            out.push_str(&rest[..close]);
        }
        rest = &rest[close..];
    }
    (out, policies.join("; "))
}

/// L'index juste après le `>` qui ferme la balise ouverte à `open` (l'index du `<`), en
/// respectant les valeurs d'attribut entre guillemets.
fn close_of_tag(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
        } else {
            match b {
                b'\'' | b'"' => quote = Some(b),
                b'>' => return Some(i + 1),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Les attributs d'une balise (le texte entre `<` et `>`), nom en minuscules, valeur telle
/// quelle.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let name = tag[name_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let value = if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                let value = tag[start..i].to_owned();
                if i < bytes.len() {
                    i += 1;
                }
                value
            } else {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                tag[start..i].to_owned()
            };
            out.push((name, value));
        } else {
            out.push((name, String::new()));
        }
    }
    out
}

/// Insère la politique dans le `<head>` réel — ou, sans lui, après le doctype, sinon tout en
/// tête. Le `head` est cherché en sautant commentaires, `<script>` et `<style>`, et jamais
/// après `<body>` : une balise `<head>` posée dans le corps n'en est pas une.
fn inject_meta(html: &str, meta: &str) -> String {
    if let Some(end) = real_head_end(html) {
        return splice(html, end, meta);
    }
    if let Some(end) = tag_end_after(html, "<!doctype") {
        return splice(html, end, meta);
    }
    splice(html, 0, meta)
}

fn splice(html: &str, at: usize, insert: &str) -> String {
    let mut out = String::with_capacity(html.len() + insert.len());
    out.push_str(&html[..at]);
    out.push_str(insert);
    out.push_str(&html[at..]);
    out
}

/// L'index juste après le `>` du `<head>` que la page porte réellement.
fn real_head_end(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    let mut seen_body = false;
    while i < bytes.len() {
        let rel = lower[i..].find('<')?;
        let lt = i + rel;
        if lower[lt..].starts_with("<!--") {
            let off = lower[lt + 4..].find("-->")?;
            i = lt + 4 + off + 3;
            continue;
        }
        let after = &lower[lt + 1..];
        let name_end = after
            .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
            .unwrap_or(after.len());
        let name = &after[..name_end];
        match name {
            "script" | "style" => {
                let close = format!("</{name}");
                let off = lower[lt..].find(&close)?;
                i = lt + off;
                continue;
            }
            "head" if !seen_body => return close_of_tag(html, lt),
            "body" => seen_body = true,
            _ => {}
        }
        i = close_of_tag(html, lt)?;
    }
    None
}

/// L'index juste après le `>` de la balise ouverte par le préfixe `prefix` (minuscule), s'il
/// en existe une.
fn tag_end_after(html: &str, prefix: &str) -> Option<usize> {
    let open = html.to_ascii_lowercase().find(prefix)?;
    close_of_tag(html, open)
}

/// Les entités que l'arbre lit : ce que `this:data` peut porter.
pub fn declared_entities(node: &Node) -> BTreeSet<String> {
    fn walk(node: &Node, out: &mut BTreeSet<String>) {
        match node {
            Node::Leaf { brick } => {
                if let Some(e) = brick.entity() {
                    out.insert(e.to_owned());
                }
            }
            Node::Stack { children }
            | Node::Row { children }
            | Node::Drawer { children, .. }
            | Node::Modal { children, .. } => children.iter().for_each(|c| walk(c, out)),
            Node::Tabs { tabs } => tabs
                .iter()
                .flat_map(|t| t.children.iter())
                .for_each(|c| walk(c, out)),
            Node::When { entity, child, .. } => {
                out.insert(entity.clone());
                walk(child, out);
            }
            Node::Html { .. } => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(node, &mut out);
    out
}

/// Le message hôte → page : les lignes des entités déclarées, le thème, la langue.
pub fn data_message(
    snapshot: &Snapshot,
    entities: &BTreeSet<String>,
    theme: &str,
    locale: &str,
) -> Value {
    let mut rows = Map::new();
    for entity in entities {
        if let Some(r) = snapshot.rows(entity) {
            rows.insert(entity.clone(), Value::Array(r.to_vec()));
        }
    }
    json!({"type": "this:data", "rows": rows, "theme": theme, "locale": locale})
}

/// Les champs DÉCLARÉS d'une entité du module, dans l'ordre : ce qu'une demande sans `fields`
/// envoie. Jamais les champs de base.
fn declared_fields(module: &ModuleDescriptor, entity: &str) -> Option<Vec<String>> {
    module
        .entities
        .iter()
        .find(|e| e.name == entity)
        .map(|e| e.fields.iter().map(|f| f.name.clone()).collect())
}

/// Le message hôte → page d'un nœud qui porte des demandes `data` : `(message, lignes de
/// journal)`.
///
/// `rows` porte, par entité DEMANDÉE, les lignes qui satisfont le filtre (au plus `limit`,
/// bornée à [`HTML_DATA_MAX_LIMIT`]), réduites aux champs demandés — ni plus, ni moins : c'est
/// la frontière de sécurité de la page. Une demande refusée (entité hors du module, champ
/// non déclaré, lecture refusée ou en échec, entité que l'hôte ne fournit pas) n'envoie
/// AUCUNE ligne et laisse une ligne de journal qui la nomme ; une lecture pas encore revenue
/// n'envoie rien et ne dit rien (elle arrive au prochain message).
pub fn grants_message(
    snapshot: &Snapshot,
    grants: &[DataGrant],
    module: &ModuleDescriptor,
    theme: &str,
    locale: &str,
) -> (Value, Vec<String>) {
    use crate::views::data::Feed;
    let mut rows = Map::new();
    let mut lines = Vec::new();
    for g in grants {
        let e = g.entity.as_str();
        let declared = declared_fields(module, e);
        if declared.is_none() && module.requires.is_empty() {
            lines.push(format!(
                "html data : html_data_entity_unknown — `{e}` n'est pas une entité de ce module"
            ));
            continue;
        }
        let fields = match (&g.fields, &declared) {
            (Some(f), Some(d)) => {
                // `id` est accordable SEULEMENT s'il est demandé (il sert à cibler une ligne) ;
                // `created_at` & co restent à l'hôte : refus nommé.
                if let Some((bad, why)) = f
                    .iter()
                    .find_map(|x| html_field_verdict(x, d).err().map(|w| (x, w)))
                {
                    lines.push(format!(
                        "html data : {why} — `{bad}` n'est pas un champ accordable de `{e}`"
                    ));
                    continue;
                }
                f.clone()
            }
            (Some(f), None) => f.clone(),
            (None, Some(d)) => d.clone(),
            (None, None) => {
                lines.push(format!(
                    "html data : `{e}` vient d'un module requis : nommer `fields`"
                ));
                continue;
            }
        };
        let source = match snapshot.feeds.get(e) {
            Some(Feed::Ready(r)) => r,
            Some(Feed::Idle) | Some(Feed::Loading) => continue,
            Some(Feed::Denied(why)) => {
                lines.push(format!(
                    "html data : lecture de `{e}` refusée ({why}) — aucune ligne envoyée"
                ));
                continue;
            }
            Some(Feed::Failed(why)) => {
                lines.push(format!(
                    "html data : lecture de `{e}` en échec ({why}) — aucune ligne envoyée"
                ));
                continue;
            }
            None => {
                lines.push(format!(
                    "html data : l'hôte ne fournit pas `{e}` — aucune ligne envoyée"
                ));
                continue;
            }
        };
        let out_of_range = g.limit.filter(|l| !(1..=HTML_DATA_MAX_LIMIT).contains(l));
        if let Some(asked) = out_of_range {
            lines.push(format!(
                "html data : limit {asked} de `{e}` hors 1..={HTML_DATA_MAX_LIMIT} — bornée à {}",
                g.effective_limit()
            ));
        }
        let filter = snapshot.resolve(g.filter.as_ref());
        let out: Vec<Value> = source
            .iter()
            .filter(|r| filter.as_deref().is_none_or(|f| matches(f, r)))
            .take(g.effective_limit())
            .map(|r| {
                let mut o = Map::new();
                for f in &fields {
                    if let Some(v) = r.get(f) {
                        o.insert(f.clone(), v.clone());
                    }
                }
                Value::Object(o)
            })
            .collect();
        rows.insert(e.to_owned(), Value::Array(out));
    }
    (
        json!({"type": "this:data", "rows": rows, "theme": theme, "locale": locale}),
        lines,
    )
}

/// La classe d'un genre qu'une page peut émettre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindClass {
    /// Remis à l'hôte dès que validé.
    Ordinary,
    /// Remis à l'hôte seulement si sa porte dit oui ([`HtmlHost::gate`]).
    Sensitive,
}

/// Les genres d'intention qu'une page a le droit d'émettre, et leur classe : l'UNIQUE liste.
/// Les noms sont ceux que [`Intent::kind`] rend pour les actions qu'un module déclare ; un
/// test vérifie que chaque variante d'`Action` y tombe. Un genre absent d'ici est inconnu.
pub const INTENT_KINDS: [(&str, KindClass); 9] = [
    ("create", KindClass::Ordinary),
    ("update", KindClass::Ordinary),
    ("delete", KindClass::Ordinary),
    ("invoke", KindClass::Ordinary),
    ("navigate", KindClass::Ordinary),
    ("secret", KindClass::Sensitive),
    ("tool", KindClass::Sensitive),
    ("capability", KindClass::Sensitive),
    ("host", KindClass::Sensitive),
];

/// La classe d'un genre, ou `None` s'il n'est pas de ceux qu'une page émet.
pub fn kind_class(kind: &str) -> Option<KindClass> {
    INTENT_KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, class)| *class)
}

/// Les genres d'une classe, dans l'ordre de [`INTENT_KINDS`].
pub fn kinds_of(class: KindClass) -> impl Iterator<Item = &'static str> {
    INTENT_KINDS
        .iter()
        .filter(move |(_, c)| *c == class)
        .map(|(name, _)| *name)
}

/// Pourquoi une demande de page n'est pas exécutée. Les codes ([`Reason::code`]) sont
/// stables : c'est ce que la page lit dans `this:refused`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Le message n'a pas de `kind` ou pas d'`action`.
    Malformed,
    /// Le genre n'est pas dans [`INTENT_KINDS`].
    UnknownKind,
    /// Le module ne déclare pas d'action de ce nom.
    UndeclaredAction,
    /// L'action existe, mais elle est d'un autre genre que celui annoncé.
    WrongKind,
    /// Genre sensible, et l'hôte ne fournit pas de porte.
    NoGate,
    /// La porte a dit non.
    Denied,
    /// Trop de demandes sensibles dans la fenêtre ([`SENSITIVE_MAX_PER_WINDOW`]).
    RateLimited,
}

impl Reason {
    /// Toutes les raisons, pour qui documente ou teste le vocabulaire.
    pub const ALL: [Reason; 7] = [
        Reason::Malformed,
        Reason::UnknownKind,
        Reason::UndeclaredAction,
        Reason::WrongKind,
        Reason::NoGate,
        Reason::Denied,
        Reason::RateLimited,
    ];

    /// Le code stable, celui de `this:refused`.
    pub fn code(self) -> &'static str {
        match self {
            Reason::Malformed => "malformed",
            Reason::UnknownKind => "unknown_kind",
            Reason::UndeclaredAction => "undeclared_action",
            Reason::WrongKind => "wrong_kind",
            Reason::NoGate => "no_gate",
            Reason::Denied => "denied",
            Reason::RateLimited => "rate_limited",
        }
    }
}

/// Un refus : la raison (pour la page) et le détail (pour le journal de l'hôte).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// Le code stable.
    pub reason: Reason,
    /// La phrase du journal. Elle ne part pas vers la page.
    pub detail: String,
}

impl Refusal {
    fn new(reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Une demande de page validée : l'action est déclarée, et du genre annoncé.
#[derive(Clone, Debug, PartialEq)]
pub enum Demand {
    /// Un genre ordinaire : prêt à être remis.
    Ordinary(Emitted),
    /// Un genre sensible : il lui faut encore la porte.
    Sensitive(GateRequest),
}

/// Valide un message page → hôte : le genre est connu, l'action est DÉCLARÉE par le module et
/// de ce genre. Rien n'est exécuté ici, et un `Demand::Sensitive` n'est pas une autorisation :
/// il doit encore passer [`admit`] puis [`settle`].
///
/// L'intention est construite par la même fonction que pour une brique : une page n'obtient
/// rien qu'une brique du même module n'aurait obtenu. Les `values` du message ne sont lues
/// que par les genres qui en portent (`create`, `update`, `invoke`) : une demande `secret`
/// ne porte qu'un nom.
pub fn page_demand(module: &ModuleDescriptor, message: &Value) -> Result<Demand, Refusal> {
    let kind = message
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal::new(Reason::Malformed, "intention sans `kind`"))?;
    let class = kind_class(kind).ok_or_else(|| {
        Refusal::new(
            Reason::UnknownKind,
            format!("genre d'intention inconnu `{kind}`"),
        )
    })?;
    let name = message
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal::new(Reason::Malformed, "intention sans `action`"))?;
    let declared = module
        .actions
        .iter()
        .find(|a| a.name() == name)
        .cloned()
        .ok_or_else(|| {
            Refusal::new(
                Reason::UndeclaredAction,
                format!("action `{name}` non déclarée par le module"),
            )
        })?;
    let id = message.get("id").and_then(Value::as_str).map(str::to_owned);
    let mut values: Values = Vec::new();
    if let Some(Value::Object(map)) = message.get("values") {
        for (field, value) in map {
            let v = match value {
                Value::String(s) => FieldValue::Text(s.clone()),
                Value::Bool(b) => FieldValue::Flag(*b),
                Value::Null => FieldValue::Empty,
                other => FieldValue::Text(other.to_string()),
            };
            values.push((field.clone(), v));
        }
    }
    let intent = render::intent_of(Some(declared.clone()), name, None, id, values);
    if intent.kind() != kind {
        return Err(Refusal::new(
            Reason::WrongKind,
            format!(
                "`{name}` est une action `{}`, la page l'a émise comme `{kind}`",
                intent.kind()
            ),
        ));
    }
    Ok(match class {
        KindClass::Ordinary => Demand::Ordinary(Emitted {
            intent,
            origin: Origin::Page,
        }),
        KindClass::Sensitive => Demand::Sensitive(GateRequest {
            module: module.name.clone(),
            kind: intent.kind(),
            name: name.to_owned(),
            action: declared,
            intent,
            origin: Origin::Page,
        }),
    })
}

/// Valide un message page → hôte et en tire l'intention ORDINAIRE, ou dit pourquoi non.
///
/// Rien n'est exécuté ici : `Ok` rend une [`Intent`] que l'hôte traite comme n'importe
/// quelle autre ; `Err` est une ligne de journal. Cette fonction ne rend jamais un genre
/// sensible (secret, outil d'agent, capacité d'appareil, surface de l'hôte) : une page peut
/// désormais les demander, mais ils passent par la porte ([`page_demand`] → [`admit`] →
/// [`settle`]), pas par ici. Un hôte qui appelle encore cette fonction seule reste donc
/// fermé, comme avant.
pub fn intent_from_message(module: &ModuleDescriptor, message: &Value) -> Result<Intent, String> {
    match page_demand(module, message) {
        Ok(Demand::Ordinary(emitted)) => Ok(emitted.intent),
        Ok(Demand::Sensitive(request)) => Err(format!(
            "genre sensible `{}` : `{}` ne passe que par la porte de l'hôte",
            request.kind, request.name
        )),
        Err(refusal) => Err(refusal.detail),
    }
}

/// Le nombre de demandes sensibles qu'une page montée peut porter à la porte par fenêtre.
pub const SENSITIVE_MAX_PER_WINDOW: usize = 5;

/// La fenêtre glissante du débit, en millisecondes.
pub const SENSITIVE_WINDOW_MS: u64 = 60_000;

/// Le compteur de débit d'UNE page montée : les instants des demandes sensibles admises.
/// Pur : le temps est un argument, rien n'est lu de l'horloge.
#[derive(Clone, Debug, Default)]
pub struct RateWindow {
    admitted: VecDeque<u64>,
}

impl RateWindow {
    /// Admet une demande à l'instant `now_ms` (millisecondes, horloge quelconque mais la
    /// même d'un appel à l'autre), ou la refuse si [`SENSITIVE_MAX_PER_WINDOW`] demandes ont
    /// déjà été admises depuis moins de [`SENSITIVE_WINDOW_MS`]. Une demande refusée ne
    /// compte pas : insister ne rallonge pas l'attente, et ne l'abrège pas non plus. Une
    /// horloge qui recule ne libère rien (les instants « futurs » restent dans la fenêtre).
    pub fn admit(&mut self, now_ms: u64) -> bool {
        while self
            .admitted
            .front()
            .is_some_and(|at| now_ms.saturating_sub(*at) >= SENSITIVE_WINDOW_MS)
        {
            self.admitted.pop_front();
        }
        if self.admitted.len() >= SENSITIVE_MAX_PER_WINDOW {
            return false;
        }
        self.admitted.push_back(now_ms);
        true
    }
}

/// Ce que l'hôte fait d'une demande validée.
#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    /// Remettre l'intention maintenant.
    Run(Emitted),
    /// Poser la question à la porte ; la suite est [`settle`].
    Ask(GateRequest),
}

/// Décide du sort d'une demande validée. Une demande ordinaire est remise. Une demande
/// sensible n'est JAMAIS rendue comme exécutable : sans porte (`gate_present == false`) elle
/// est refusée `no_gate` ; avec, elle consomme une place du débit de la page (`rate`,
/// `now_ms`) ou est refusée `rate_limited`, et ne rend qu'une question à poser.
pub fn admit(
    demand: Demand,
    gate_present: bool,
    rate: &mut RateWindow,
    now_ms: u64,
) -> Result<Admission, Refusal> {
    match demand {
        Demand::Ordinary(emitted) => Ok(Admission::Run(emitted)),
        Demand::Sensitive(request) if !gate_present => Err(Refusal::new(
            Reason::NoGate,
            format!(
                "`{}` est une action `{}` : l'hôte ne fournit pas de porte d'approbation",
                request.name, request.kind
            ),
        )),
        Demand::Sensitive(request) => {
            if rate.admit(now_ms) {
                Ok(Admission::Ask(request))
            } else {
                Err(Refusal::new(
                    Reason::RateLimited,
                    format!(
                        "`{}` : plus de {SENSITIVE_MAX_PER_WINDOW} demandes sensibles en \
                         {SENSITIVE_WINDOW_MS} ms, la porte n'est pas sollicitée",
                        request.name
                    ),
                ))
            }
        }
    }
}

/// La suite d'une question posée à la porte : oui rend l'intention à remettre (celle de la
/// demande, d'origine page), non la refuse `denied`.
pub fn settle(request: GateRequest, answer: GateAnswer) -> Result<Emitted, Refusal> {
    match answer {
        GateAnswer::Approved => Ok(Emitted {
            intent: request.intent,
            origin: request.origin,
        }),
        GateAnswer::Denied => Err(Refusal::new(
            Reason::Denied,
            format!(
                "`{}` (`{}`) refusée par la porte de l'hôte",
                request.name, request.kind
            ),
        )),
    }
}

/// Le champ d'un `this:intent` qui porte l'identifiant de corrélation. Pas `id` : dans ce
/// message, `id` est la ligne visée par l'action.
pub const CORRELATION_FIELD: &str = "request";

/// La longueur maximale (en caractères) d'un identifiant de corrélation en chaîne. Au-delà,
/// il est ignoré : l'hôte ne renvoie pas à la page un texte arbitrairement long.
pub const CORRELATION_MAX_LEN: usize = 64;

/// L'identifiant de corrélation d'un message, s'il en porte un recevable : une chaîne non
/// vide d'au plus [`CORRELATION_MAX_LEN`] caractères, ou un nombre. Sinon `None` : la demande
/// est traitée, mais sans réponse.
pub fn correlation(message: &Value) -> Option<Value> {
    match message.get(CORRELATION_FIELD)? {
        Value::String(s) if !s.is_empty() && s.chars().count() <= CORRELATION_MAX_LEN => {
            Some(Value::String(s.clone()))
        }
        Value::Number(n) => Some(Value::Number(n.clone())),
        _ => None,
    }
}

/// La réponse hôte → page d'une demande REMISE à l'hôte. `ok:true` dit « transmis », pas
/// « réussi » : `this-ui` n'exécute rien et ne connaît pas l'issue.
pub fn result_message(id: &Value) -> Value {
    json!({"type": "this:result", "id": id, "ok": true})
}

/// La réponse hôte → page d'une demande refusée : le code stable, rien d'autre.
pub fn refused_message(id: &Value, reason: Reason) -> Value {
    json!({"type": "this:refused", "id": id, "reason": reason.code()})
}

/// La ligne de journal d'une intention refusée : ce que l'hôte écrit, jamais exécute.
pub fn refusal_line(why: &str) -> String {
    format!("html : intention ignorée ({why})")
}

/// Le nœud `html` : monte la page, ou dit pourquoi il ne le fait pas.
pub fn render_html(ctx: Ctx, path: &str, height: Option<u32>, data: &[DataGrant]) -> AnyView {
    let full = ctx
        .module
        .try_with_value(|m| html_resolve(&m.name, &m.version, path))
        .unwrap_or_else(|| Err(("html_path_empty", "module indisponible".into())));
    let full = match full {
        Ok(full) => full,
        Err((reason, why)) => return render::fallback_public(&format!("html {reason} : {why}")),
    };
    let Some(host) = use_context::<HtmlHost>() else {
        return render::fallback_public("html : l'hôte ne livre pas les fichiers du module");
    };
    mount(ctx, host, full, height_px(height), data.to_vec())
}

#[cfg(not(target_arch = "wasm32"))]
fn mount(_ctx: Ctx, _host: HtmlHost, full: String, _height: u32, _data: Vec<DataGrant>) -> AnyView {
    render::fallback_public(&format!("html : {full} ne se monte que dans le navigateur"))
}

#[cfg(target_arch = "wasm32")]
fn mount(ctx: Ctx, host: HtmlHost, full: String, height: u32, data: Vec<DataGrant>) -> AnyView {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    // L'état du montage : rien (lecture en cours), l'URL du Blob, ou la raison d'un refus.
    let state = RwSignal::new(None::<Result<String, String>>);
    let created = StoredValue::new_local(None::<String>);
    let module_name = ctx
        .module
        .try_with_value(|m| m.name.clone())
        .unwrap_or_default();
    let view_name = ctx.view.try_get_value().unwrap_or_default();
    {
        let full = full.clone();
        let read = host.read.run(full.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let verdict = match read.await {
                None => Err(format!("fichier introuvable ({full})")),
                Some(bytes) if bytes.len() > MAX_BYTES => Err(format!(
                    "{full} fait {} octets, la limite est {MAX_BYTES}",
                    bytes.len()
                )),
                Some(bytes) => match rewrite_served(&full, bytes) {
                    Err((reason, why)) => {
                        render::log_refusal(&format!("html {reason} : {why}"));
                        Err(why)
                    }
                    Ok(served) => {
                        render::log_line(&csp_imposed_line(&module_name, &view_name));
                        let parts = js_sys::Array::new();
                        parts.push(&js_sys::Uint8Array::from(served.as_slice()));
                        let opts = web_sys::BlobPropertyBag::new();
                        opts.set_type(content_type(&full));
                        web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
                            .and_then(|b| web_sys::Url::create_object_url_with_blob(&b))
                            .map_err(|_| "impossible de créer le Blob".to_owned())
                    }
                },
            };
            if let Ok(url) = &verdict {
                // `try_set` rend `Some(valeur)` quand le signal est déjà démonté : personne
                // ne révoquera cette URL, on le fait ici.
                match state.try_set(Some(verdict.clone())) {
                    Some(_) => {
                        let _ = web_sys::Url::revoke_object_url(url);
                    }
                    None => created.set_value(Some(url.clone())),
                }
            } else {
                let _ = state.try_set(Some(verdict));
            }
        });
    }
    // Révoqué au démontage : pas de fuite.
    on_cleanup(move || {
        if let Some(Some(url)) = created.try_get_value() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });

    let frame = NodeRef::<leptos::html::Iframe>::new();
    let entities = ctx
        .view
        .try_with_value(|view| {
            ctx.module.try_with_value(|m| {
                m.views
                    .iter()
                    .find(|v| &v.name == view)
                    .map(|v| declared_entities(&v.root))
                    .unwrap_or_default()
            })
        })
        .flatten()
        .unwrap_or_default();
    let entities = StoredValue::new(entities);
    let tall = RwSignal::new(height);
    let refused = StoredValue::new(Vec::<String>::new());
    let data = StoredValue::new(data);

    let post = move || {
        let Some(el) = frame.get() else { return };
        let Some(win) = el.content_window() else {
            return;
        };
        let (theme, locale) = (host.theme.get(), host.locale.get());
        let msg = if data.with_value(|d| d.is_empty()) {
            ctx.data
                .with(|s| entities.with_value(|e| data_message(s, e, &theme, &locale)))
        } else {
            let (msg, lines) = ctx.data.with(|s| {
                ctx.module
                    .try_with_value(|m| {
                        data.with_value(|d| grants_message(s, d, m, &theme, &locale))
                    })
                    .unwrap_or_else(|| {
                        (
                            json!({"type":"this:data","rows":{},"theme":theme,"locale":locale}),
                            vec![],
                        )
                    })
            });
            // Un refus se dit une fois, pas à chaque message : on ne journalise que quand
            // l'ensemble des lignes change.
            if refused.get_value() != lines {
                for l in &lines {
                    render::log_refusal(l);
                }
                refused.set_value(lines);
            }
            msg
        };
        if let Ok(js) = js_sys::JSON::parse(&msg.to_string()) {
            let _ = win.post_message(&js, "*");
        }
    };
    // Rejoue à chaque changement de données, de thème ou de langue.
    let post_again = post;
    Effect::new(move |_| {
        ctx.data.track();
        host.theme.track();
        host.locale.track();
        post_again();
    });

    // Le débit des demandes sensibles de CETTE page montée.
    let rate = std::rc::Rc::new(std::cell::RefCell::new(RateWindow::default()));
    // Remet une intention de page à l'hôte : avec son origine s'il la demande, nue sinon.
    // `false` si plus personne n'écoute (vue démontée) : rien n'a été remis.
    let deliver = move |emitted: Emitted| match host.on_page_intent {
        Some(sink) => sink.try_run(emitted).is_some(),
        None => ctx.on_intent.try_run(emitted.intent).is_some(),
    };
    // Journalise un refus, et répond à la page si elle a donné un identifiant. `win` est la
    // fenêtre de l'iframe dont le message vient d'être reconnu : la réponse ne va qu'à elle
    // (cible `*` parce que son origine est opaque, pas parce que le destinataire est large).
    fn answer(win: &web_sys::Window, id: Option<&Value>, verdict: Result<(), &Refusal>) {
        if let Err(refusal) = verdict {
            render::log_refusal(&refusal_line(&refusal.detail));
        }
        let Some(id) = id else { return };
        let message = match verdict {
            Ok(()) => result_message(id),
            Err(refusal) => refused_message(id, refusal.reason),
        };
        if let Ok(js) = js_sys::JSON::parse(&message.to_string()) {
            let _ = win.post_message(&js, "*");
        }
    }

    // Le pont retour. On ne croit que la fenêtre de CETTE iframe.
    let listener =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let Some(el) = frame.get_untracked() else {
                return;
            };
            let Some(win) = el.content_window() else {
                return;
            };
            let from_us = ev
                .source()
                .map(|s| js_sys::Object::is(&s, win.as_ref()))
                .unwrap_or(false);
            if !from_us {
                return;
            }
            let Some(text) = js_sys::JSON::stringify(&ev.data())
                .ok()
                .and_then(|s| s.as_string())
            else {
                return;
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else {
                return;
            };
            match msg.get("type").and_then(Value::as_str) {
                Some("this:intent") => {
                    // Un lien sortant, bloqué par la garde : ce n'est pas une action du
                    // module, on le journalise nommément au lieu de le rejeter comme
                    // « genre inconnu ».
                    if msg.get("kind").and_then(Value::as_str) == Some("link_refused") {
                        let href = msg.get("href").and_then(Value::as_str).unwrap_or("");
                        render::log_refusal(&link_refused_line(href));
                    } else {
                        let Some(demand) = ctx.module.try_with_value(|m| page_demand(m, &msg))
                        else {
                            return;
                        };
                        let id = correlation(&msg);
                        let now_ms = js_sys::Date::now().max(0.0) as u64;
                        let admission = demand.and_then(|d| {
                            admit(d, host.gate.is_some(), &mut rate.borrow_mut(), now_ms)
                        });
                        match admission {
                            Ok(Admission::Run(emitted)) => {
                                if deliver(emitted) {
                                    answer(&win, id.as_ref(), Ok(()));
                                }
                            }
                            // Genre sensible : la porte décide, plus tard. Rien n'est remis
                            // avant son oui ; si la vue est démontée entre-temps, `deliver`
                            // ne trouve plus personne et rien n'est exécuté.
                            Ok(Admission::Ask(request)) => {
                                match host.gate.and_then(|g| g.try_run(request.clone())) {
                                    Some(asked) => wasm_bindgen_futures::spawn_local(async move {
                                        match settle(request, asked.await) {
                                            Ok(emitted) => {
                                                if deliver(emitted) {
                                                    answer(&win, id.as_ref(), Ok(()));
                                                }
                                            }
                                            Err(refusal) => {
                                                answer(&win, id.as_ref(), Err(&refusal))
                                            }
                                        }
                                    }),
                                    None => answer(
                                        &win,
                                        id.as_ref(),
                                        Err(&Refusal::new(
                                            Reason::NoGate,
                                            format!(
                                                "`{}` : la porte de l'hôte n'est plus là",
                                                request.name
                                            ),
                                        )),
                                    ),
                                }
                            }
                            Err(refusal) => answer(&win, id.as_ref(), Err(&refusal)),
                        }
                    }
                }
                Some("this:height") => {
                    if let Some(px) = msg.get("px").and_then(Value::as_u64) {
                        tall.set(height_px(Some(px.min(u64::from(MAX_HEIGHT)) as u32)));
                    }
                }
                Some("this:ready") => post(),
                other => render::log_refusal(&format!("html : message ignoré (type {other:?})")),
            }
        });
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("message", listener.as_ref().unchecked_ref());
        let target = window.clone();
        let cb: js_sys::Function = listener
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();
        on_cleanup(move || {
            let _ = target.remove_event_listener_with_callback("message", &cb);
        });
    }
    listener.forget();

    view! {
        {move || match state.get() {
            None => view! { <div class="vw-html-loading" role="status">"Chargement…"</div> }.into_any(),
            Some(Err(why)) => render::fallback_public(&format!("html : {why}")),
            Some(Ok(url)) => view! {
                <iframe
                    node_ref=frame
                    class="vw-html"
                    sandbox=SANDBOX
                    src=url
                    title=full.clone()
                    on:load=move |_| post()
                    style=move || format!("width:100%;border:0;height:{}px", tall.get())
                ></iframe>
            }
            .into_any(),
        }}
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::data::Feed;
    fn module() -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("compteur", "1.0.0");
        m.actions.push(
            serde_json::from_value::<Action>(
                json!({"kind":"invoke","name":"incrementer","operation":"inc"}),
            )
            .expect("action"),
        );
        m.actions.push(
            serde_json::from_value::<Action>(
                json!({"kind":"request_secret","name":"cle","secret":"k"}),
            )
            .expect("action"),
        );
        m
    }

    #[test]
    fn the_sandbox_never_carries_the_host_origin() {
        assert_eq!(SANDBOX, "allow-scripts");
        assert!(!SANDBOX.contains("allow-same-origin"));
        // Le mot ne doit figurer, dans le code de montage, nulle part hors des tests et des
        // commentaires : on lit ce fichier-ci avant son module de tests.
        let source = include_str!("html.rs");
        let code = source.split("#[cfg(test)]").next().expect("code");
        for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
            assert!(
                !line.contains("allow-same-origin"),
                "allow-same-origin dans le code de montage : {line}"
            );
        }
    }

    #[test]
    fn height_is_bounded() {
        assert_eq!(height_px(None), DEFAULT_HEIGHT);
        assert_eq!(height_px(Some(0)), 1);
        assert_eq!(height_px(Some(99_999)), 2000);
        assert_eq!(height_px(Some(480)), 480);
    }

    #[test]
    fn a_declared_invoke_intent_is_accepted() {
        let i = intent_from_message(
            &module(),
            &json!({"type":"this:intent","kind":"invoke","action":"incrementer","values":{"n":"1"}}),
        )
        .expect("accepté");
        assert_eq!(i.kind(), "invoke");
        assert_eq!(i.action(), Some("incrementer"));
    }

    #[test]
    fn an_unknown_or_unreachable_intent_is_refused_with_its_reason() {
        let m = module();
        for (msg, why) in [
            (json!({"kind":"invoke","action":"inconnue"}), "non déclarée"),
            (json!({"kind":"rm_rf","action":"incrementer"}), "genre"),
            (json!({"kind":"invoke"}), "sans `action`"),
            (json!({"action":"incrementer"}), "sans `kind`"),
            // une action existante, mais pas du genre annoncé
            (json!({"kind":"delete","action":"incrementer"}), "invoke"),
            // un secret émis sous un autre genre que le sien : refusé
            (json!({"kind":"invoke","action":"cle"}), "`secret`"),
            // `secret` n'est plus un genre inconnu : une page peut le demander, mais il ne
            // sort jamais de CETTE fonction — il lui faut la porte de l'hôte
            (json!({"kind":"secret","action":"cle"}), "porte"),
        ] {
            let err = intent_from_message(&m, &msg).expect_err(&msg.to_string());
            assert!(err.contains(why), "{msg} → {err}");
        }
    }

    #[test]
    fn data_carries_only_the_declared_entities() {
        let snap = Snapshot::new()
            .with("compte", Feed::Ready(vec![json!({"id":"1"})]))
            .with("secret", Feed::Ready(vec![json!({"id":"x"})]));
        let mut only = BTreeSet::new();
        only.insert("compte".to_owned());
        let msg = data_message(&snap, &only, "dark", "fr");
        assert_eq!(msg["type"], "this:data");
        assert_eq!(msg["theme"], "dark");
        assert_eq!(msg["locale"], "fr");
        assert!(msg["rows"].get("compte").is_some());
        assert!(msg["rows"].get("secret").is_none());
    }

    fn taches() -> (ModuleDescriptor, Vec<serde_json::Value>) {
        use this_descriptor::{EntityDesc, FieldDesc, FieldType};
        let mut m = module();
        let mut t = EntityDesc::new("tache");
        t.fields = vec![
            FieldDesc::new("titre", FieldType::String),
            FieldDesc::new("fait", FieldType::Bool),
            FieldDesc::new("note", FieldType::String),
        ];
        m.entities.push(t);
        let rows = vec![
            json!({"id":"1","created_at":"x","titre":"a","fait":false,"note":"n1"}),
            json!({"id":"2","created_at":"x","titre":"b","fait":true,"note":"n2"}),
            json!({"id":"3","created_at":"x","titre":"c","fait":false,"note":"n3"}),
        ];
        (m, rows)
    }

    fn grant(json: serde_json::Value) -> DataGrant {
        serde_json::from_value(json).expect("grant")
    }

    #[test]
    fn a_grant_sends_the_filtered_projected_rows_and_nothing_else() {
        let (m, rows) = taches();
        let snap = Snapshot::new()
            .with("tache", Feed::Ready(rows))
            .with("secret", Feed::Ready(vec![json!({"id":"x"})]));
        let g = grant(json!({"entity":"tache","fields":["titre","fait"],
            "filter":{"cmp":{"field":"fait","op":"eq","value":false}}}));
        let (msg, lines) = grants_message(&snap, &[g], &m, "dark", "fr");
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(
            msg["rows"],
            json!({"tache":[{"titre":"a","fait":false},{"titre":"c","fait":false}]})
        );
        // Ni l'entité non demandée, ni les champs non demandés, ni les champs de base.
        assert!(msg["rows"].get("secret").is_none());
        assert_eq!(msg["theme"], "dark");
    }

    #[test]
    fn a_grant_without_fields_sends_the_declared_fields_never_the_base_ones() {
        let (m, rows) = taches();
        let snap = Snapshot::new().with("tache", Feed::Ready(rows));
        let (msg, _) = grants_message(&snap, &[grant(json!({"entity":"tache"}))], &m, "t", "l");
        let first = &msg["rows"]["tache"][0];
        assert_eq!(first, &json!({"titre":"a","fait":false,"note":"n1"}));
        assert!(first.get("id").is_none() && first.get("created_at").is_none());
    }

    #[test]
    fn an_undeclared_field_is_refused_by_name_before_any_send() {
        let (m, rows) = taches();
        let snap = Snapshot::new().with("tache", Feed::Ready(rows));
        for champ in ["secret", "created_at"] {
            let g = grant(json!({"entity":"tache","fields":["titre", champ]}));
            let (msg, lines) = grants_message(&snap, &[g], &m, "t", "l");
            assert_eq!(msg["rows"], json!({}), "{champ}");
            assert_eq!(lines.len(), 1);
            let code = if champ == "secret" {
                "html_data_field_unknown"
            } else {
                "html_data_field_reserved"
            };
            assert!(lines[0].contains(code), "{lines:?}");
            assert!(lines[0].contains(champ), "{lines:?}");
        }
    }

    #[test]
    fn id_is_granted_when_asked_to_target_a_row_but_created_at_never() {
        let (m, rows) = taches();
        let snap = Snapshot::new().with("tache", Feed::Ready(rows));
        let g = grant(json!({"entity":"tache","fields":["id","titre"]}));
        let (msg, lines) = grants_message(&snap, &[g], &m, "t", "l");
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(msg["rows"]["tache"][1], json!({"id":"2","titre":"b"}));
        assert!(msg["rows"]["tache"][1].get("created_at").is_none());
    }

    #[test]
    fn an_entity_the_module_may_not_read_is_refused_and_logged_with_empty_rows() {
        let (m, _) = taches();
        let snap = Snapshot::new().with("tache", Feed::Denied("perm:tache.list".into()));
        let (msg, lines) = grants_message(&snap, &[grant(json!({"entity":"tache"}))], &m, "t", "l");
        assert_eq!(msg["rows"], json!({}));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("refusée") && lines[0].contains("perm:tache.list"));
        // Une entité hors du module : refusée nommément, même si l'hôte la détient.
        let snap = Snapshot::new().with("autre", Feed::Ready(vec![json!({"x":1})]));
        let (msg, lines) = grants_message(&snap, &[grant(json!({"entity":"autre"}))], &m, "t", "l");
        assert_eq!(msg["rows"], json!({}));
        assert!(lines[0].contains("html_data_entity_unknown"), "{lines:?}");
        // Un échec n'est pas un envoi silencieux non plus ; une lecture en cours attend.
        let snap = Snapshot::new().with("tache", Feed::Failed("réseau".into()));
        let (_, lines) = grants_message(&snap, &[grant(json!({"entity":"tache"}))], &m, "t", "l");
        assert!(lines[0].contains("en échec"));
        let snap = Snapshot::new().with("tache", Feed::Loading);
        let (msg, lines) = grants_message(&snap, &[grant(json!({"entity":"tache"}))], &m, "t", "l");
        assert!(lines.is_empty() && msg["rows"] == json!({}));
    }

    #[test]
    fn a_limit_is_applied_and_one_past_the_max_is_clamped_out_loud() {
        let (m, _) = taches();
        let beaucoup: Vec<_> = (0..1500).map(|i| json!({"titre": i.to_string()})).collect();
        let snap = Snapshot::new().with("tache", Feed::Ready(beaucoup));
        let (msg, lines) = grants_message(
            &snap,
            &[grant(
                json!({"entity":"tache","fields":["titre"],"limit":2}),
            )],
            &m,
            "t",
            "l",
        );
        assert_eq!(msg["rows"]["tache"].as_array().map(Vec::len), Some(2));
        assert!(lines.is_empty());
        let (msg, lines) = grants_message(
            &snap,
            &[grant(
                json!({"entity":"tache","fields":["titre"],"limit":5000}),
            )],
            &m,
            "t",
            "l",
        );
        assert_eq!(msg["rows"]["tache"].as_array().map(Vec::len), Some(1000));
        assert!(lines[0].contains("bornée à 1000"), "{lines:?}");
        let (msg, _) = grants_message(
            &snap,
            &[grant(json!({"entity":"tache","fields":["titre"]}))],
            &m,
            "t",
            "l",
        );
        assert_eq!(msg["rows"]["tache"].as_array().map(Vec::len), Some(200));
    }

    #[test]
    fn content_type_follows_the_extension() {
        assert_eq!(content_type("a/b.svg"), "image/svg+xml");
        assert!(content_type("a/b.html").starts_with("text/html"));
    }

    #[test]
    fn the_served_html_carries_the_host_csp() {
        let served = rewrite_served(
            "ui/index.html",
            "<!doctype html><html><head><title>x</title></head><body><p>hi</p></body></html>"
                .as_bytes()
                .to_vec(),
        )
        .expect("servi");
        let html = String::from_utf8(served).expect("utf8");
        assert!(html.contains("default-src 'none'"), "{html}");
        assert!(html.contains("connect-src 'none'"), "{html}");
        assert!(html.contains("form-action 'none'"), "{html}");
        // Injectée dans le <head>, pas à côté.
        let head_close = html.find("</head>").expect("head");
        assert!(
            html[..head_close].contains("Content-Security-Policy"),
            "{html}"
        );
    }

    #[test]
    fn a_more_permissive_module_csp_is_intersected_away() {
        let served = rewrite_served(
            "ui/index.html",
            concat!(
                "<!doctype html><html><head>",
                "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src *; connect-src *\">",
                "</head><body>x</body></html>",
            )
            .as_bytes()
            .to_vec(),
        )
        .expect("servi");
        let html = String::from_utf8(served).expect("utf8");
        assert!(html.contains("default-src 'none'"), "{html}");
        assert!(html.contains("connect-src 'none'"), "{html}");
        assert!(!html.contains("default-src *"), "{html}");
        assert!(!html.contains("connect-src *"), "{html}");
    }

    #[test]
    fn html_without_head_still_gets_the_csp() {
        let served = rewrite_served(
            "ui/index.html",
            "<html><body><button>x</button></body></html>"
                .as_bytes()
                .to_vec(),
        )
        .expect("servi");
        let html = String::from_utf8(served).expect("utf8");
        assert!(html.contains("default-src 'none'"), "{html}");
        assert!(html.contains("form-action 'none'"), "{html}");
    }

    #[test]
    fn inline_script_and_style_survive() {
        let page = "<!doctype html><html><head><style>body{color:red}</style></head><body><script>parent.postMessage(1)</script></body></html>";
        let served = rewrite_served("ui/index.html", page.as_bytes().to_vec()).expect("servi");
        let html = String::from_utf8(served).expect("utf8");
        assert!(html.contains("<style>body{color:red}</style>"), "{html}");
        assert!(
            html.contains("<script>parent.postMessage(1)</script>"),
            "{html}"
        );
    }

    #[test]
    fn an_svg_is_refused_by_name() {
        let err = rewrite_served(
            "ui/logo.svg",
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
        )
        .expect_err("refusé");
        assert_eq!(err.0, "html_svg_refused");
    }

    #[test]
    fn the_imposed_line_names_module_and_view() {
        let line = csp_imposed_line("compteur", "ecran");
        assert_eq!(
            line,
            "html : compteur/ecran — CSP imposée (default-src 'none')"
        );
    }

    #[test]
    fn the_link_guard_is_injected_before_the_first_module_script() {
        let served = rewrite_served(
            "ui/index.html",
            "<!doctype html><html><head><title>x</title></head><body>             <a href='https://exfil.example/?d=secret'>partir</a>             <script>let module_script = 1;</script></body></html>"
                .as_bytes()
                .to_vec(),
        )
        .expect("servi");
        let html = String::from_utf8(served).expect("utf8");
        // La CSP est toujours là (on ne l'a pas cassée en réécrivant le HTML).
        assert!(html.contains("default-src 'none'"), "{html}");
        assert!(html.contains("connect-src 'none'"), "{html}");
        // La garde est en phase de capture…
        assert!(
            LINK_GUARD.contains("addEventListener('click'"),
            "{LINK_GUARD}"
        );
        assert!(LINK_GUARD.contains("}, true)"), "{LINK_GUARD}");
        // …et précède le premier script du module.
        let guard = html.find(LINK_GUARD).expect("garde");
        let script = html.find("module_script").expect("script du module");
        assert!(
            guard < script,
            "la garde ({guard}) doit précéder le script du module ({script}) : {html}"
        );
    }

    #[test]
    fn internal_anchors_and_blob_links_are_not_blocked() {
        // La liberté de navigation DANS la page est intacte.
        assert!(!blocks_link("#section"));
        assert!(!blocks_link("#"));
        assert!(!blocks_link("blob:http://opaque/abc"));
        assert!(!blocks_link("data:text/html,<p>page interne</p>"));
    }

    #[test]
    fn an_outgoing_link_is_blocked_and_names_its_href() {
        assert!(blocks_link("https://exfil.example/?d=secret"));
        assert!(blocks_link("http://x/"));
        assert!(blocks_link("//exfil.example/?d=secret"));
        assert!(blocks_link("mailto:a@b.c"));
        let line = link_refused_line("https://exfil.example/?d=sec");
        assert_eq!(
            line,
            "html : lien sortant refusé — https://exfil.example/?d=sec"
        );
    }

    #[test]
    fn a_page_without_script_still_gets_the_guard() {
        let served = rewrite_served(
            "ui/index.html",
            "<!doctype html><html><head><title>x</title></head><body><p>hi</p></body></html>"
                .as_bytes()
                .to_vec(),
        )
        .expect("servi");
        let html = String::from_utf8(served).expect("utf8");
        assert!(html.contains("link_refused"), "{html}");
        assert!(html.contains("default-src 'none'"), "{html}");
    }

    // ── La porte : genres sensibles, origine, réponses, débit ─────────────────────────────

    use this_descriptor::views::{AgentOp, DeviceCapability, Grant, HostSurface};

    /// Un module qui déclare une action de CHAQUE genre sensible, plus une ordinaire.
    fn sensible() -> ModuleDescriptor {
        let mut m = module();
        for action in [
            json!({"kind":"agent_tool","name":"demander","op":"send_message","text":"résume"}),
            json!({"kind":"device_capability","name":"micro","capability":"microphone","grant":"allow"}),
            json!({"kind":"host","name":"reglages","surface":"settings"}),
        ] {
            m.actions
                .push(serde_json::from_value::<Action>(action).expect("action"));
        }
        m
    }

    /// Chaque genre sensible : (genre, action déclarée, intention qu'une BRIQUE produit).
    fn sensitive_cases() -> Vec<(&'static str, &'static str, Intent)> {
        vec![
            (
                "secret",
                "cle",
                Intent::Secret {
                    action: "cle".into(),
                    secret: "k".into(),
                },
            ),
            (
                "tool",
                "demander",
                Intent::Tool {
                    action: "demander".into(),
                    op: AgentOp::SendMessage,
                    entity: None,
                    text: Some("résume".into()),
                },
            ),
            (
                "capability",
                "micro",
                Intent::Capability {
                    action: "micro".into(),
                    capability: DeviceCapability::Microphone,
                    grant: Grant::Allow,
                },
            ),
            (
                "host",
                "reglages",
                Intent::Host {
                    action: "reglages".into(),
                    surface: HostSurface::Settings,
                },
            ),
        ]
    }

    fn message(kind: &str, action: &str) -> Value {
        json!({"type":"this:intent","kind":kind,"action":action})
    }

    #[test]
    fn the_kind_list_is_the_single_source_and_splits_ordinary_from_sensitive() {
        assert_eq!(
            kinds_of(KindClass::Ordinary).collect::<Vec<_>>(),
            ["create", "update", "delete", "invoke", "navigate"]
        );
        assert_eq!(
            kinds_of(KindClass::Sensitive).collect::<Vec<_>>(),
            ["secret", "tool", "capability", "host"]
        );
        assert_eq!(kind_class("secret_entry"), None);
        assert_eq!(kind_class("rm_rf"), None);
        // Chaque variante d'`Action` rend un genre de la liste : rien de déclarable n'est
        // hors d'atteinte, et les noms sont bien ceux d'`Intent::kind`.
        let every = [
            json!({"kind":"create","name":"a","entity":"e","fields":[]}),
            json!({"kind":"update","name":"a","entity":"e","fields":[]}),
            json!({"kind":"delete","name":"a","entity":"e"}),
            json!({"kind":"navigate","name":"a","view":"v"}),
            json!({"kind":"request_secret","name":"a","secret":"k"}),
            json!({"kind":"agent_tool","name":"a","op":"stop_run"}),
            json!({"kind":"invoke","name":"a","operation":"o"}),
            json!({"kind":"device_capability","name":"a","capability":"camera","grant":"deny"}),
            json!({"kind":"host","name":"a","surface":"modules"}),
        ];
        let mut seen = BTreeSet::new();
        for action in every {
            let action = serde_json::from_value::<Action>(action).expect("action");
            let kind = render::intent_of(Some(action), "a", None, None, vec![]).kind();
            assert!(kind_class(kind).is_some(), "`{kind}` hors de INTENT_KINDS");
            seen.insert(kind);
        }
        assert_eq!(seen.len(), INTENT_KINDS.len());
    }

    #[test]
    fn without_a_gate_every_sensitive_kind_is_refused_and_nothing_is_runnable() {
        let m = sensible();
        for (kind, action, _) in sensitive_cases() {
            // L'ancien chemin ne rend jamais un genre sensible.
            let err = intent_from_message(&m, &message(kind, action)).expect_err(kind);
            assert!(err.contains("porte"), "{kind} → {err}");
            // Le nouveau le valide, mais sans porte il est refusé et le débit n'est pas touché.
            let demand = page_demand(&m, &message(kind, action)).expect(kind);
            assert!(matches!(demand, Demand::Sensitive(_)), "{kind}");
            let mut rate = RateWindow::default();
            let refusal = admit(demand, false, &mut rate, 0).expect_err(kind);
            assert_eq!(refusal.reason, Reason::NoGate, "{kind}");
            assert_eq!(refusal.reason.code(), "no_gate");
            assert!(rate.admitted.is_empty(), "{kind}");
        }
    }

    #[test]
    fn a_sensitive_demand_is_never_admitted_as_runnable_even_with_a_gate() {
        let m = sensible();
        for (kind, action, _) in sensitive_cases() {
            let demand = page_demand(&m, &message(kind, action)).expect(kind);
            let admission = admit(demand, true, &mut RateWindow::default(), 0).expect(kind);
            assert!(matches!(admission, Admission::Ask(_)), "{kind}");
        }
    }

    #[test]
    fn an_approving_gate_yields_exactly_the_brick_intent_marked_as_page() {
        let m = sensible();
        for (kind, action, brick) in sensitive_cases() {
            let demand = page_demand(&m, &message(kind, action)).expect(kind);
            let Admission::Ask(request) =
                admit(demand, true, &mut RateWindow::default(), 0).expect(kind)
            else {
                panic!("{kind} : une demande sensible doit passer par la porte");
            };
            // La porte sait qui demande quoi.
            assert_eq!(request.module, "compteur");
            assert_eq!(request.kind, kind);
            assert_eq!(request.name, action);
            assert_eq!(request.action.name(), action);
            assert_eq!(request.origin, Origin::Page);
            // Ce que la brique produit pour la même action déclarée, par la même fonction…
            let declared = m.actions.iter().find(|a| a.name() == action).cloned();
            assert_eq!(
                render::intent_of(declared, action, None, None, vec![]),
                brick
            );
            // …est exactement ce que la porte, en disant oui, laisse remettre.
            let emitted = settle(request, GateAnswer::Approved).expect(kind);
            assert_eq!(emitted.intent, brick, "{kind}");
            assert_eq!(emitted.origin, Origin::Page, "{kind}");
        }
    }

    #[test]
    fn a_denying_gate_refuses_with_denied() {
        let m = sensible();
        for (kind, action, _) in sensitive_cases() {
            let demand = page_demand(&m, &message(kind, action)).expect(kind);
            let Admission::Ask(request) =
                admit(demand, true, &mut RateWindow::default(), 0).expect(kind)
            else {
                panic!("{kind}");
            };
            let refusal = settle(request, GateAnswer::Denied).expect_err(kind);
            assert_eq!(refusal.reason, Reason::Denied, "{kind}");
            assert_eq!(refusal.reason.code(), "denied");
        }
    }

    #[test]
    fn an_ordinary_demand_runs_without_a_gate_and_is_marked_as_page() {
        let m = sensible();
        let msg = json!({"kind":"invoke","action":"incrementer","id":"7","values":{"n":"1"}});
        let demand = page_demand(&m, &msg).expect("validé");
        let mut rate = RateWindow::default();
        let Admission::Run(emitted) = admit(demand, false, &mut rate, 0).expect("admis") else {
            panic!("un genre ordinaire ne passe pas par la porte");
        };
        assert_eq!(emitted.origin, Origin::Page);
        // La même intention que l'ancien chemin, qui reste celui des hôtes sans porte.
        assert_eq!(emitted.intent, intent_from_message(&m, &msg).expect("ok"));
        assert_eq!(
            emitted.intent,
            Intent::Invoke {
                action: "incrementer".into(),
                operation: "inc".into(),
                entity: None,
                id: Some("7".into()),
                values: vec![("n".into(), FieldValue::text("1"))],
            }
        );
        // Un genre ordinaire ne consomme pas le débit de la porte.
        assert!(rate.admitted.is_empty());
    }

    #[test]
    fn undeclared_and_wrong_kind_stay_refused_with_or_without_a_gate() {
        let m = sensible();
        for (msg, reason) in [
            // non déclarée, sous un genre ordinaire comme sous un genre sensible
            (message("invoke", "inconnue"), Reason::UndeclaredAction),
            (message("capability", "camera"), Reason::UndeclaredAction),
            (message("host", "inconnue"), Reason::UndeclaredAction),
            // déclarée, mais émise sous un autre genre que le sien — dans les deux sens
            (message("invoke", "micro"), Reason::WrongKind),
            (message("capability", "incrementer"), Reason::WrongKind),
            (message("tool", "cle"), Reason::WrongKind),
            (message("host", "micro"), Reason::WrongKind),
            // genres qu'aucune page n'émet
            (message("secret_entry", "cle"), Reason::UnknownKind),
            (message("request_secret", "cle"), Reason::UnknownKind),
            (message("rm_rf", "incrementer"), Reason::UnknownKind),
            (json!({"kind":"host"}), Reason::Malformed),
            (json!({"action":"micro"}), Reason::Malformed),
        ] {
            let refusal = page_demand(&m, &msg).expect_err(&msg.to_string());
            assert_eq!(refusal.reason, reason, "{msg} → {refusal}");
            // Et l'ancien chemin refuse aussi.
            assert!(intent_from_message(&m, &msg).is_err(), "{msg}");
        }
    }

    #[test]
    fn a_secret_value_never_crosses_the_bridge() {
        let m = sensible();
        let leak = "hunter2-super-secret";
        // Une page qui glisse une valeur dans sa demande de secret : elle n'entre nulle part.
        let msg = json!({"kind":"secret","action":"cle","id":leak,
            "values":{"k":leak,"secret":leak},"request":"r1"});
        let demand = page_demand(&m, &msg).expect("validé");
        assert!(!format!("{demand:?}").contains(leak), "{demand:?}");
        let Admission::Ask(request) =
            admit(demand, true, &mut RateWindow::default(), 0).expect("admis")
        else {
            panic!("secret : par la porte");
        };
        let emitted = settle(request, GateAnswer::Approved).expect("oui");
        assert_eq!(
            emitted.intent,
            Intent::Secret {
                action: "cle".into(),
                secret: "k".into()
            }
        );
        assert!(emitted.intent.reaches_module());
        // Les réponses ne portent que l'identifiant et un code.
        let id = correlation(&msg).expect("id");
        for reply in [
            result_message(&id),
            refused_message(&id, Reason::Denied),
            refused_message(&id, Reason::NoGate),
        ] {
            assert!(!reply.to_string().contains(leak), "{reply}");
        }
        // Et une page ne peut pas émettre la saisie d'un secret, seule porteuse d'une valeur.
        let entry = json!({"kind":"secret_entry","action":"cle","values":{"k":leak}});
        assert_eq!(
            page_demand(&m, &entry).expect_err("refusé").reason,
            Reason::UnknownKind
        );
    }

    #[test]
    fn the_rate_window_refuses_the_one_past_the_max_then_readmits_after_the_window() {
        let mut rate = RateWindow::default();
        let t0 = 1_000_000;
        for i in 0..SENSITIVE_MAX_PER_WINDOW as u64 {
            assert!(rate.admit(t0 + i), "demande {i}");
        }
        // La N+1ᵉ, dans la fenêtre : refusée, et insister ne change rien.
        assert!(!rate.admit(t0 + 10));
        assert!(!rate.admit(t0 + SENSITIVE_WINDOW_MS - 1));
        // La première sort de la fenêtre : UNE place se libère, pas plus.
        assert!(rate.admit(t0 + SENSITIVE_WINDOW_MS));
        assert!(!rate.admit(t0 + SENSITIVE_WINDOW_MS));
        // Bien après : la fenêtre est vide, le plein est de nouveau admis.
        let later = t0 + 10 * SENSITIVE_WINDOW_MS;
        for i in 0..SENSITIVE_MAX_PER_WINDOW as u64 {
            assert!(rate.admit(later + i), "demande {i} après la fenêtre");
        }
        assert!(!rate.admit(later + 100));
        // Une horloge qui recule ne libère rien.
        assert!(!rate.admit(0));
    }

    #[test]
    fn the_gate_is_not_asked_past_the_rate_and_is_asked_again_after_the_window() {
        let m = sensible();
        let mut rate = RateWindow::default();
        let ask = |rate: &mut RateWindow, now: u64| {
            let demand = page_demand(&m, &message("capability", "micro")).expect("validé");
            admit(demand, true, rate, now)
        };
        for i in 0..SENSITIVE_MAX_PER_WINDOW as u64 {
            assert!(matches!(ask(&mut rate, i), Ok(Admission::Ask(_))), "{i}");
        }
        let refusal = ask(&mut rate, 50).expect_err("débit dépassé");
        assert_eq!(refusal.reason, Reason::RateLimited);
        assert_eq!(refusal.reason.code(), "rate_limited");
        // Les genres ordinaires ne sont pas freinés par le débit de la porte.
        let ordinary = page_demand(&m, &message("invoke", "incrementer")).expect("validé");
        assert!(matches!(
            admit(ordinary, true, &mut rate, 50),
            Ok(Admission::Run(_))
        ));
        assert!(matches!(
            ask(&mut rate, SENSITIVE_WINDOW_MS),
            Ok(Admission::Ask(_))
        ));
    }

    #[test]
    fn the_replies_have_their_exact_json_shape() {
        assert_eq!(
            result_message(&json!("r-1")),
            json!({"type":"this:result","id":"r-1","ok":true})
        );
        assert_eq!(
            refused_message(&json!(42), Reason::NoGate),
            json!({"type":"this:refused","id":42,"reason":"no_gate"})
        );
        // Le vocabulaire des raisons : stable, lisible, sans doublon.
        let codes: Vec<_> = Reason::ALL.iter().map(|r| r.code()).collect();
        assert_eq!(
            codes,
            [
                "malformed",
                "unknown_kind",
                "undeclared_action",
                "wrong_kind",
                "no_gate",
                "denied",
                "rate_limited"
            ]
        );
        for reason in Reason::ALL {
            let reply = refused_message(&json!("x"), reason);
            assert_eq!(reply["reason"], reason.code());
            assert_eq!(reply.as_object().map(|o| o.len()), Some(3));
        }
    }

    #[test]
    fn the_correlation_is_the_request_field_never_the_row_id() {
        // `id` est la ligne visée : ce n'est pas une demande de réponse.
        assert_eq!(correlation(&json!({"kind":"delete","id":"7"})), None);
        assert_eq!(
            correlation(&json!({"request":"a1","id":"7"})),
            Some(json!("a1"))
        );
        assert_eq!(correlation(&json!({"request":12})), Some(json!(12)));
        // Ni vide, ni trop long, ni structuré : pas de réponse, la demande reste traitée.
        assert_eq!(correlation(&json!({"request":""})), None);
        let long = "x".repeat(CORRELATION_MAX_LEN + 1);
        assert_eq!(correlation(&json!({"request":long})), None);
        let max = "x".repeat(CORRELATION_MAX_LEN);
        assert_eq!(
            correlation(&json!({"request":max.clone()})),
            Some(json!(max))
        );
        assert_eq!(correlation(&json!({"request":{"a":1}})), None);
        assert_eq!(correlation(&json!({"request":null})), None);
    }

    #[test]
    fn a_host_built_without_a_gate_is_closed() {
        let owner = Owner::new();
        owner.set();
        let host = HtmlHost::new(
            Callback::new(|_: String| -> ReadFuture { Box::pin(async { None }) }),
            Signal::derive(|| "dark".to_owned()),
            Signal::derive(|| "fr".to_owned()),
        );
        assert!(host.gate.is_none() && host.on_page_intent.is_none());
        let gated = host.with_gate(Callback::new(|_: GateRequest| -> GateFuture {
            Box::pin(async { GateAnswer::Denied })
        }));
        assert!(gated.gate.is_some() && gated.on_page_intent.is_none());
    }
}
