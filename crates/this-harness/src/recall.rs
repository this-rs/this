//! # Le rappel — une mémoire qui cite, au lieu d'une paraphrase
//!
//! Un résumé ([`Compactor`](crate::Compactor)) est une paraphrase invérifiable,
//! qui vieillit et se compose en résumé de résumé. Ici, on extrait ce que les
//! tours ont **littéralement** touché ([`anchors`]), puis l'hôte dit l'état
//! **actuel** de ces entités dans son graphe ([`Recall`]) et [`rappel`] en fait
//! un message synthétique qui cite les ids.
//!
//! **Règle : un rappel n'est pas une preuve — chaque fait est daté et attribué,
//! l'extrait de la conversation qui citait l'id est rendu à côté du fait du
//! graphe, et le harnais n'arbitre jamais en silence : en cas d'écart, les deux
//! apparaissent.**
//!
//! ## Ce que les ancres savent (et ne savent pas)
//! - Ids : `#kind:uuid` (kind = `[a-z_]+`, uuid canonique 8-4-4-4-12 hexa) ; une
//!   entité désignée par son *nom* n'est pas retrouvée, ni un id mal formé.
//! - Chemins : heuristique — un mot (séparé par blancs, guillemets, virgules,
//!   parenthèses) fait de `[A-Za-z0-9_./~-]`, d'extension connue
//!   ([`EXTENSIONS`]), sans `://` ni `//` initial. Cherchés dans les arguments
//!   et résultats d'outils seulement ; un chemin sans extension connue, ou avec
//!   espaces, est manqué.
//! - Outils : nom et nombre d'appels. Commandes : champ `command`/`cmd` des
//!   arguments, tronqué à [`COMMAND_MAX`] caractères.
//! - Rien n'est déduit : pas de sens, pas de succès/échec des appels.

use crate::{Message, ToolCall};
use async_trait::async_trait;
use serde_json::Value;

/// Extensions reconnues pour un chemin de fichier.
pub const EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "mjs", "py", "go", "c",
    "h", "cpp", "hpp", "java", "kt", "swift", "sh", "html", "css", "scss", "sql", "lock", "txt",
    "mmd", "proto", "wasm", "svelte", "vue",
];
/// Longueur maximale (caractères) d'une commande retenue.
pub const COMMAND_MAX: usize = 200;
/// Nombre maximal d'entités interrogées et rendues.
pub const MAX_ENTITIES: usize = 40;
/// Nombre maximal de chemins / commandes rendus.
pub const MAX_LISTED: usize = 20;
/// Taille maximale (caractères) du message produit.
pub const MAX_CHARS: usize = 8000;
const FACT_MAX: usize = 240;
const EXCERPT_MAX: usize = 120;

/// Un id `#kind:uuid` cité, avec le premier message qui le cite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Citation {
    /// Forme complète, `#kind:uuid`.
    pub id: String,
    pub kind: String,
    /// Indice du message dans la suite analysée.
    pub message: usize,
    /// La ligne qui le citait, tronquée : ce que la conversation en disait.
    pub excerpt: String,
}

/// Ce qui est littéralement présent dans une suite de messages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Anchors {
    /// Dédupliqués, dans l'ordre d'apparition.
    pub cited: Vec<Citation>,
    /// Chemins vus dans des arguments/résultats d'outils : (chemin, message).
    pub files: Vec<(String, usize)>,
    /// Outils appelés et leur nombre, dans l'ordre de première apparition.
    pub tools: Vec<(String, usize)>,
    /// Commandes lancées (tronquées) : (commande, message).
    pub commands: Vec<(String, usize)>,
}

impl Anchors {
    /// Rien à rappeler : ni id, ni chemin.
    pub fn is_empty(&self) -> bool {
        self.cited.is_empty() && self.files.is_empty()
    }
}

/// Un fait ramené du graphe : l'état actuel d'une entité, daté et attribué.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    /// L'id demandé (`#kind:uuid`).
    pub id: String,
    pub kind: String,
    /// Un fait court.
    pub fact: String,
    /// Quand ce fait a été vrai/lu (texte libre de l'hôte, ex. ISO 8601).
    pub as_of: String,
    /// Qui l'affirme (ex. « obrain »).
    pub source: String,
}

/// Le port que l'hôte implémente : l'état actuel de ces entités.
///
/// Rendre **uniquement** les ids trouvés ; un id absent de la réponse est
/// signalé « introuvable » par [`rappel`].
#[async_trait(?Send)]
pub trait Recall {
    async fn recall(&self, ids: &[String]) -> Vec<Fact>;
}

/// Extrait les ancres d'une suite de messages. Pur.
pub fn anchors(messages: &[Message]) -> Anchors {
    let mut out = Anchors::default();
    for (index, message) in messages.iter().enumerate() {
        match message {
            Message::Assistant { content, calls } => {
                ids(content, index, &mut out);
                for call in calls {
                    call_anchors(call, index, &mut out);
                }
            }
            Message::Tool { content, .. } => {
                ids(content, index, &mut out);
                paths(content, index, &mut out);
            }
            other => ids(other.text(), index, &mut out),
        }
    }
    out
}

fn call_anchors(call: &ToolCall, index: usize, out: &mut Anchors) {
    match out.tools.iter_mut().find(|(n, _)| *n == call.name) {
        Some((_, n)) => *n += 1,
        None => out.tools.push((call.name.clone(), 1)),
    }
    let mut strings = Vec::new();
    collect_strings(&call.arguments, &mut strings);
    for s in &strings {
        ids(s, index, out);
        paths(s, index, out);
    }
    if let Value::Object(map) = &call.arguments {
        for key in ["command", "cmd"] {
            if let Some(Value::String(c)) = map.get(key) {
                out.commands.push((truncate(c.trim(), COMMAND_MAX), index));
            }
        }
    }
}

fn collect_strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Cherche `#kind:uuid` dans un texte.
fn ids(text: &str, index: usize, out: &mut Anchors) {
    for (start, _) in text.match_indices('#') {
        let rest = &text[start + 1..];
        let kind_len = rest
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || *b == b'_')
            .count();
        if kind_len == 0 || rest.as_bytes().get(kind_len) != Some(&b':') {
            continue;
        }
        let Some(uuid) = rest.get(kind_len + 1..kind_len + 37) else {
            continue;
        };
        // Ni plus long (hexa/tiret qui suit), ni coupé au milieu.
        let after = rest.as_bytes().get(kind_len + 37);
        if !is_uuid(uuid) || after.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-') {
            continue;
        }
        let id = format!("#{}:{}", &rest[..kind_len], uuid);
        if out.cited.iter().any(|c| c.id == id) {
            continue;
        }
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        out.cited.push(Citation {
            id,
            kind: rest[..kind_len].to_string(),
            message: index,
            excerpt: truncate(text[line_start..line_end].trim(), EXCERPT_MAX),
        });
    }
}

/// Cherche ce qui ressemble à un chemin de fichier dans un texte.
fn paths(text: &str, index: usize, out: &mut Anchors) {
    let sep = |c: char| c.is_whitespace() || "\"'`,;()[]{}<>=:".contains(c) && c != ':';
    for raw in text.split(sep) {
        let tok = raw.trim_end_matches(['.', ',', ':', ';', '!', '?']);
        if tok.is_empty()
            || tok.contains("://")
            || tok.starts_with("//")
            || !tok
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_./~-".contains(c))
        {
            continue;
        }
        let name = tok.rsplit('/').next().unwrap_or(tok);
        let Some((stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        if stem.is_empty() || !EXTENSIONS.contains(&ext) {
            continue;
        }
        if !out.files.iter().any(|(p, _)| p == tok) {
            out.files.push((tok.to_string(), index));
        }
    }
}

/// Construit le message de rappel. `None` quand il n'y a ni id ni chemin :
/// pas de message creux.
///
/// Le message est `synthetic` (une consigne du harnais, jamais épinglée comme
/// la question de l'utilisateur), cite les ids, dit les introuvables, et dit
/// ce qu'il a coupé.
pub async fn rappel(anchors: &Anchors, recall: &dyn Recall) -> Option<Message> {
    if anchors.is_empty() {
        return None;
    }
    let shown = &anchors.cited[..anchors.cited.len().min(MAX_ENTITIES)];
    let facts = if shown.is_empty() {
        Vec::new()
    } else {
        let asked: Vec<String> = shown.iter().map(|c| c.id.clone()).collect();
        recall.recall(&asked).await
    };

    let mut body = String::from(
        "[Rappel du harnais — état ACTUEL du graphe pour ce que ces tours ont touché ; ce n'est pas un résumé]\n\
         Règle : un rappel n'est pas une preuve. Si un fait du graphe contredit ce que la conversation disait, \
         présente les deux (fait daté et attribué + extrait de la conversation) ; n'arbitre pas en silence.\n",
    );
    let mut cut = false;
    if !shown.is_empty() {
        body.push_str(&format!("\nEntités citées ({}) :\n", anchors.cited.len()));
    }
    for (n, c) in shown.iter().enumerate() {
        let line = match facts.iter().find(|f| f.id == c.id) {
            Some(f) => format!(
                "- {} ({}) — graphe : {} [source : {}, au : {}]\n    conversation (message {}) : « {} »\n",
                c.id,
                f.kind,
                truncate(&one_line(&f.fact), FACT_MAX),
                one_line(&f.source),
                one_line(&f.as_of),
                c.message,
                c.excerpt
            ),
            None => format!(
                "- {} ({}) — INTROUVABLE dans le graphe (cité au message {} : « {} ») : id inventé, supprimé, ou hors de portée — à ne pas tenir pour acquis.\n",
                c.id, c.kind, c.message, c.excerpt
            ),
        };
        if body.chars().count() + line.chars().count() > MAX_CHARS {
            body.push_str(&format!(
                "[coupé : {} entité(s) citée(s) non rendue(s) (borne de taille)]\n",
                anchors.cited.len() - n
            ));
            cut = true;
            break;
        }
        body.push_str(&line);
    }
    if !cut && anchors.cited.len() > shown.len() {
        body.push_str(&format!(
            "[coupé : {} entité(s) citée(s) non interrogée(s) (borne de {MAX_ENTITIES})]\n",
            anchors.cited.len() - shown.len()
        ));
    }
    list(
        &mut body,
        "Fichiers vus",
        anchors.files.iter().map(|(p, _)| p.clone()),
    );
    list(
        &mut body,
        "Outils appelés",
        anchors.tools.iter().map(|(n, k)| format!("{n}×{k}")),
    );
    list(
        &mut body,
        "Commandes lancées",
        anchors.commands.iter().map(|(c, _)| c.clone()),
    );
    Some(Message::hint(body))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn list(body: &mut String, title: &str, items: impl ExactSizeIterator<Item = String>) {
    let total = items.len();
    if total == 0 {
        return;
    }
    let shown: Vec<String> = items.take(MAX_LISTED).collect();
    body.push_str(&format!("\n{title} ({total}) : {}", shown.join(" ; ")));
    if total > MAX_LISTED {
        body.push_str(&format!(" [coupé : {} de plus]", total - MAX_LISTED));
    }
    body.push('\n');
}
