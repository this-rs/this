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
//! - page → hôte : `{type:"this:intent", kind, action, id?, values?}`, validé par
//!   [`intent_from_message`] : le genre est l'un de create/update/delete/invoke/navigate,
//!   l'action est DÉCLARÉE par le module et de ce genre. Le reste est refusé : l'appelant
//!   journalise et n'exécute rien ;
//! - page → hôte : `{type:"this:height", px}` règle la hauteur (bornée à [`MAX_HEIGHT`]).

use std::collections::BTreeSet;

use leptos::prelude::*;
use serde_json::{json, Map, Value};
use this_descriptor::engine::query::matches;
use this_descriptor::views::{html_resolve, DataGrant, Node, HTML_DATA_MAX_LIMIT};
use this_descriptor::ModuleDescriptor;

use crate::views::data::Snapshot;
use crate::views::intent::{FieldValue, Intent, Values};
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

/// Ce que l'hôte fournit (par le contexte Leptos) pour monter une page : lire un fichier du
/// VFS, dire le thème et la langue. Sans lui, le nœud `html` dit pourquoi il est vide.
#[derive(Clone, Copy)]
pub struct HtmlHost {
    /// Lit les octets d'un fichier du VFS (chemin complet `/modules/<nom>/<version>/…`).
    /// Asynchrone : le VFS d'une app l'est.
    pub read: Callback<String, ReadFuture>,
    /// Le thème courant, tel que l'hôte le nomme (`light`, `dark`).
    pub theme: Signal<String>,
    /// La langue courante (`fr`, `en`).
    pub locale: Signal<String>,
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
                if let Some(bad) = f.iter().find(|x| !d.contains(x)) {
                    lines.push(format!(
                        "html data : html_data_field_unknown — `{bad}` n'est pas un champ déclaré de `{e}`"
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

/// Les genres d'intention qu'une page a le droit d'émettre.
pub const INTENT_KINDS: [&str; 5] = ["create", "update", "delete", "invoke", "navigate"];

/// Valide un message page → hôte et en tire l'intention, ou dit pourquoi non.
///
/// Rien n'est exécuté ici : `Ok` rend une [`Intent`] que l'hôte traite comme n'importe
/// quelle autre ; `Err` est une ligne de journal. Les secrets, capacités d'appareil, surfaces
/// de l'hôte et outils d'agent ne sont pas atteignables : ce ne sont pas des genres admis.
pub fn intent_from_message(module: &ModuleDescriptor, message: &Value) -> Result<Intent, String> {
    let kind = message
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("intention sans `kind`")?;
    if !INTENT_KINDS.contains(&kind) {
        return Err(format!("genre d'intention inconnu `{kind}`"));
    }
    let name = message
        .get("action")
        .and_then(Value::as_str)
        .ok_or("intention sans `action`")?;
    let declared = module
        .actions
        .iter()
        .find(|a| a.name() == name)
        .cloned()
        .ok_or_else(|| format!("action `{name}` non déclarée par le module"))?;
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
    let intent = render::intent_of(Some(declared), name, None, id, values);
    if intent.kind() != kind {
        return Err(format!(
            "`{name}` est une action `{}`, la page l'a émise comme `{kind}`",
            intent.kind()
        ));
    }
    Ok(intent)
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
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

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
    let post_again = post.clone();
    Effect::new(move |_| {
        ctx.data.track();
        host.theme.track();
        host.locale.track();
        post_again();
    });

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
                        match ctx.module.try_with_value(|m| intent_from_message(m, &msg)) {
                            Some(Ok(intent)) => ctx.on_intent.run(intent),
                            Some(Err(why)) => render::log_refusal(&refusal_line(&why)),
                            None => {}
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
    use this_descriptor::views::Action;

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
            // un secret n'est pas atteignable par une page
            (json!({"kind":"invoke","action":"cle"}), "`secret`"),
            (json!({"kind":"secret","action":"cle"}), "genre"),
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
        for champ in ["secret", "id"] {
            let g = grant(json!({"entity":"tache","fields":["titre", champ]}));
            let (msg, lines) = grants_message(&snap, &[g], &m, "t", "l");
            assert_eq!(msg["rows"], json!({}), "{champ}");
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains("html_data_field_unknown"), "{lines:?}");
            assert!(lines[0].contains(champ), "{lines:?}");
        }
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
}
