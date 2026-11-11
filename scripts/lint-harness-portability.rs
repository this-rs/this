//! Liste noire de portabilité de `this-harness` (wasm32 : page, mobile, serveur).
//!
//! Usage : `rustc -O scripts/lint-harness-portability.rs -o /tmp/lint && /tmp/lint [dossier-src]`
//! (défaut : `crates/this-harness/src`). Sortie 1, en nommant fichier:ligne et
//! symbole, si une API de plateforme apparaît dans du code compilé pour wasm32.
//!
//! Ce que le lint regarde : tout chemin (`use`, `extern crate`, ET chemin qualifié
//! en ligne comme `std::thread::spawn(..)`), car le harnais qualifie presque tout
//! en ligne. Ce qu'il ignore, volontairement :
//!   - les commentaires et les chaînes ;
//!   - l'item qui suit `#[cfg(not(target_arch = "wasm32"))]` (le code natif a
//!     le droit d'utiliser la plateforme : c'est le port) ;
//!   - l'item qui suit `#[cfg(test)]` (les tests ne sont pas compilés en livraison).
//!
//! Autorisé aussi : `std::thread::panicking` (existe en wasm32 ; le reste de `std::thread` non).
//!
//! Autorisés (disponibles en wasm via le port `Clock`) : `tokio::time`,
//! `std::time::{Duration, Instant}`. Pas de dépendance : rustc seul.
#![allow(clippy::needless_range_loop)]
use std::{env, fs, path::Path, process};

const BANNED: &[&str] = &[
    "std::process",
    "std::fs",
    "std::net",
    "std::thread",
    "tokio::process",
    "tokio::fs",
    "tokio::net",
    "reqwest",
    "rusqlite",
    "std::time::SystemTime",
];

/// Remplace commentaires, chaînes et caractères par des espaces (garde les `\n`).
fn blank(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let (mut i, n) = (0, b.len());
    let sp = |c: char| if c == '\n' { '\n' } else { ' ' };
    while i < n {
        if b[i] == '/' && i + 1 < n && b[i + 1] == '/' {
            while i < n && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if b[i] == '/' && i + 1 < n && b[i + 1] == '*' {
            let mut depth = 0;
            while i < n {
                if b[i] == '/' && i + 1 < n && b[i + 1] == '*' {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && i + 1 < n && b[i + 1] == '/' {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(sp(b[i]));
                    i += 1;
                }
            }
        } else if b[i] == 'r' && i + 1 < n && (b[i + 1] == '"' || b[i + 1] == '#') {
            // chaîne brute r#"…"#
            let mut j = i + 1;
            let mut hashes = 0;
            while j < n && b[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < n && b[j] == '"' {
                j += 1;
                loop {
                    if j >= n {
                        break;
                    }
                    if b[j] == '"' && (0..hashes).all(|k| j + 1 + k < n && b[j + 1 + k] == '#') {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                for k in i..j.min(n) {
                    out.push(sp(b[k]));
                }
                i = j;
            } else {
                out.push(b[i]);
                i += 1;
            }
        } else if b[i] == '"' {
            out.push(' ');
            i += 1;
            while i < n && b[i] != '"' {
                if b[i] == '\\' && i + 1 < n {
                    out.push(sp(b[i]));
                    i += 1;
                }
                out.push(sp(b[i]));
                i += 1;
            }
            out.push(' ');
            i += 1;
        } else if b[i] == '\'' && i + 2 < n && (b[i + 2] == '\'' || (b[i + 1] == '\\')) {
            // littéral de caractère ('x' ou '\n'), pas une durée de vie
            let mut j = i + 1;
            if b[j] == '\\' {
                j += 1;
            }
            j += 1;
            while j < n && b[j] != '\'' {
                j += 1;
            }
            for k in i..=j.min(n - 1) {
                out.push(sp(b[k]));
            }
            i = j + 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// Lignes (0-based) à ignorer : l'item qui suit un `#[cfg(...)]` natif ou test.
fn skipped(orig: &[&str], lines: &[&str]) -> Vec<bool> {
    let mut skip = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        let t = orig[i].trim();
        let native = t.starts_with("#[cfg(") && {
            let c: String = t.chars().filter(|c| !c.is_whitespace()).collect();
            c == "#[cfg(test)]" || c.contains("not(target_arch=\"wasm32\")")
        };
        if !native {
            i += 1;
            continue;
        }
        let (mut depth, mut seen, mut j) = (0i32, false, i);
        while j < lines.len() {
            skip[j] = true;
            let mut end = false;
            for ch in lines[j].chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        seen = true;
                    }
                    '}' => depth -= 1,
                    ';' if depth == 0 && j > i => end = true,
                    _ => {}
                }
            }
            // l'item est fini : accolade refermée, ou `;` hors accolade (use, const…)
            let is_attr = lines[j].trim().starts_with("#[");
            if !is_attr && ((seen && depth <= 0) || end) {
                break;
            }
            j += 1;
        }
        i = j + 1;
    }
    skip
}

/// Développe `a::{b, c::d}` en chemins à plat (un seul niveau d'imbrication utile).
fn expand(path: &str) -> Vec<String> {
    let p: String = path.chars().filter(|c| !c.is_whitespace()).collect();
    if let (Some(o), Some(c)) = (p.find('{'), p.rfind('}')) {
        let (head, inner) = (&p[..o], &p[o + 1..c]);
        let (mut parts, mut depth, mut cur) = (vec![], 0, String::new());
        for ch in inner.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(ch);
        }
        parts.push(cur);
        parts
            .iter()
            .flat_map(|part| expand(&format!("{head}{part}")))
            .collect()
    } else {
        vec![p]
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

const ALLOWED: &[&str] = &["std::thread::panicking"];

/// `path` est-il le symbole interdit, ou un de ses sous-chemins ?
fn hit(path: &str) -> Option<&'static str> {
    if ALLOWED.contains(&path) {
        return None;
    }
    BANNED
        .iter()
        .find(|b| path == **b || path.strip_prefix(**b).is_some_and(|r| r.starts_with("::")))
        .copied()
}

fn scan(file: &Path, out: &mut Vec<String>) {
    let src = fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("lecture de {}: {e}", file.display());
        process::exit(2)
    });
    let clean = blank(&src);
    let lines: Vec<&str> = clean.lines().collect();
    let orig: Vec<&str> = src.lines().collect();
    let skip = skipped(&orig, &lines);
    let mut idx = 0;
    while idx < lines.len() {
        if skip[idx] {
            idx += 1;
            continue;
        }
        let line = lines[idx];
        let t = line.trim_start();
        // `use …;` / `extern crate …;` : on joint jusqu'au `;`, puis on développe les groupes.
        let is_use =
            t.starts_with("use ") || t.starts_with("pub use ") || t.starts_with("pub(crate) use ");
        if is_use {
            let start = idx;
            let mut stmt = String::new();
            while idx < lines.len() {
                stmt.push_str(lines[idx]);
                stmt.push(' ');
                if lines[idx].contains(';') {
                    break;
                }
                idx += 1;
            }
            let body = stmt
                .split_once("use ")
                .map_or("", |x| x.1)
                .trim_end_matches([';', ' ']);
            for p in expand(body) {
                let p = p.trim_start_matches("::");
                if let Some(b) = hit(p) {
                    out.push(format!("{}:{}: `{b}` (use {p})", file.display(), start + 1));
                }
            }
            idx += 1;
            continue;
        }
        if t.starts_with("extern crate ") {
            let name = t
                .strip_prefix("extern crate ")
                .unwrap_or("")
                .trim_end_matches(';')
                .trim();
            if let Some(b) = hit(name) {
                out.push(format!(
                    "{}:{}: `{b}` (extern crate)",
                    file.display(),
                    idx + 1
                ));
            }
        }
        // chemins qualifiés en ligne : `std::thread::spawn`, `reqwest::Client`…
        let chars: Vec<(usize, char)> = line.char_indices().collect();
        let mut k = 0;
        while k < chars.len() {
            if is_ident(chars[k].1)
                && (k == 0 || (!is_ident(chars[k - 1].1) && chars[k - 1].1 != ':'))
            {
                let st = chars[k].0;
                let mut e = k;
                while e < chars.len() && (is_ident(chars[e].1) || chars[e].1 == ':') {
                    e += 1;
                }
                let end = chars.get(e).map_or(line.len(), |c| c.0);
                if let Some(b) = hit(line[st..end].trim_end_matches(':')) {
                    out.push(format!(
                        "{}:{}: `{b}` ({})",
                        file.display(),
                        idx + 1,
                        &line[st..end]
                    ));
                }
                k = e;
            } else {
                k += 1;
            }
        }
        idx += 1;
    }
}

fn walk(dir: &Path, out: &mut Vec<String>) {
    let mut es: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| {
            eprintln!("dossier {}: {e}", dir.display());
            process::exit(2)
        })
        .flatten()
        .map(|e| e.path())
        .collect();
    es.sort();
    for p in es {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            scan(&p, out);
        }
    }
}

fn main() {
    let dir = env::args()
        .nth(1)
        .unwrap_or_else(|| "crates/this-harness/src".into());
    let mut out = vec![];
    walk(Path::new(&dir), &mut out);
    if out.is_empty() {
        println!("portabilité wasm32 : aucune API de plateforme interdite dans {dir}");
    } else {
        for l in &out {
            eprintln!("INTERDIT {l}");
        }
        eprintln!(
            "\n{} violation(s) : cette API n'existe pas en wasm32. Passe par un port, ou garde-la derrière #[cfg(not(target_arch = \"wasm32\"))].",
            out.len()
        );
        process::exit(1);
    }
}
