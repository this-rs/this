//! Retrouver un extrait dans sa source, avec un verdict de qualité.
//!
//! Une citation qu'on ne sait pas localiser n'est pas un résultat. [`locate`]
//! tente trois stratégies, de la moins chère à la plus tolérante : exacte, puis
//! normalisée (blancs repliés), puis floue (recouvrement de mots garanti par une
//! couverture et une densité minimales). Si rien ne tient, il rend `None` —
//! jamais un faux positif.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ByteInterval;

/// Qualité de la correspondance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentStatus {
    /// Identique à l'octet près.
    Exact,
    /// Identique une fois les suites de blancs repliées (reflow, réindentation).
    Normalized,
    /// Assez de mots retrouvés, assez serrés, pour être le même passage réécrit.
    Fuzzy,
}

/// Où un extrait a été trouvé, et à quel point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub interval: ByteInterval,
    pub status: AlignmentStatus,
    /// Part des mots de l'extrait retrouvés (1.0 si exact).
    pub coverage: f32,
    /// Mots retrouvés / mots couverts par la correspondance. La garde qui compte :
    /// sans elle, trois mots courants éparpillés sur deux pages « correspondent ».
    pub density: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignConfig {
    /// Retomber sur le recouvrement de mots quand exact et normalisé échouent.
    pub enable_fuzzy: bool,
    /// Part minimale des mots de l'extrait à retrouver.
    pub min_coverage: f32,
    /// Rapport minimal mots retrouvés / mots couverts.
    pub min_density: f32,
    /// Étirement maximal de la correspondance, en multiple du nombre de mots de
    /// l'extrait : « une réécriture, pas une section entière ».
    pub max_span_ratio: usize,
}

impl Default for AlignConfig {
    fn default() -> Self {
        Self {
            enable_fuzzy: true,
            min_coverage: 0.66,
            min_density: 0.5,
            max_span_ratio: 3,
        }
    }
}

/// Cherche `needle` dans `source`, stratégie la moins chère d'abord.
pub fn locate(source: &str, needle: &str, config: &AlignConfig) -> Option<Alignment> {
    if needle.trim().is_empty() || source.is_empty() {
        return None;
    }
    if let Some(start) = source.find(needle) {
        return Some(Alignment {
            interval: ByteInterval::new(start, start + needle.len()),
            status: AlignmentStatus::Exact,
            coverage: 1.0,
            density: 1.0,
        });
    }
    if let Some(interval) = locate_normalized(source, needle) {
        return Some(Alignment {
            interval,
            status: AlignmentStatus::Normalized,
            coverage: 1.0,
            density: 1.0,
        });
    }
    if config.enable_fuzzy {
        return locate_fuzzy(source, needle, config);
    }
    None
}

/// Replie les blancs en un espace, en gardant une table vers les offsets d'origine.
/// `map[i]` = offset d'origine du caractère occupant l'octet normalisé `i` ;
/// `map[normalized.len()]` = longueur d'origine.
fn normalize_with_map(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut map: Vec<usize> = Vec::with_capacity(s.len() + 1);
    let mut prev_ws = false;

    for (idx, ch) in s.char_indices() {
        if ch.is_whitespace() {
            if prev_ws {
                continue;
            }
            prev_ws = true;
            out.push(' ');
            map.push(idx);
        } else {
            prev_ws = false;
            let before = out.len();
            out.push(ch);
            for _ in before..out.len() {
                map.push(idx);
            }
        }
    }
    map.push(s.len());
    (out, map)
}

fn locate_normalized(source: &str, needle: &str) -> Option<ByteInterval> {
    let (norm_source, map) = normalize_with_map(source);
    let (norm_needle_raw, _) = normalize_with_map(needle);
    let norm_needle = norm_needle_raw.trim();
    if norm_needle.is_empty() {
        return None;
    }
    let at = norm_source.find(norm_needle)?;
    let end = at + norm_needle.len();
    Some(ByteInterval::new(map[at], map[end]))
}

struct Token {
    lower: String,
    interval: ByteInterval,
}

fn tokenize(s: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut push = |st: usize, end: usize| {
        out.push(Token {
            lower: s[st..end].to_lowercase(),
            interval: ByteInterval::new(st, end),
        });
    };
    for (idx, ch) in s.char_indices() {
        let is_word = ch.is_alphanumeric() || ch == '_';
        match (is_word, start) {
            (true, None) => start = Some(idx),
            (false, Some(st)) => {
                push(st, idx);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(st) = start {
        push(st, s.len());
    }
    out
}

fn locate_fuzzy(source: &str, needle: &str, config: &AlignConfig) -> Option<Alignment> {
    let needle_tokens = tokenize(needle);
    let source_tokens = tokenize(source);
    if needle_tokens.is_empty() || source_tokens.is_empty() {
        return None;
    }

    let mut wanted: HashMap<&str, usize> = HashMap::new();
    for t in &needle_tokens {
        *wanted.entry(t.lower.as_str()).or_insert(0) += 1;
    }
    let needle_len = needle_tokens.len();
    let max_span = needle_len
        .saturating_mul(config.max_span_ratio)
        .max(needle_len);

    let mut best: Option<Alignment> = None;

    for start in 0..source_tokens.len() {
        let mut remaining = wanted.clone();
        let mut matched = 0usize;
        let limit = (start + max_span).min(source_tokens.len());

        for end in start..limit {
            if let Some(count) = remaining.get_mut(source_tokens[end].lower.as_str())
                && *count > 0
            {
                *count -= 1;
                matched += 1;
            }
            let span_len = end - start + 1;
            let coverage = matched as f32 / needle_len as f32;
            let density = matched as f32 / span_len as f32;
            if coverage < config.min_coverage || density < config.min_density {
                continue;
            }
            let candidate = Alignment {
                interval: ByteInterval::new(
                    source_tokens[start].interval.start,
                    source_tokens[end].interval.end,
                ),
                status: AlignmentStatus::Fuzzy,
                coverage,
                density,
            };
            // Couverture, puis densité, puis le passage le plus court.
            let better = match &best {
                None => true,
                Some(b) => {
                    (
                        candidate.coverage,
                        candidate.density,
                        usize::MAX - candidate.interval.len(),
                    ) > (b.coverage, b.density, usize::MAX - b.interval.len())
                }
            };
            if better {
                best = Some(candidate);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "The quick brown fox jumps over the lazy dog.\n\
                       Pack my box with five dozen liquor jugs.\n\
                       How vexingly quick daft zebras jump!";

    // Le cas le moins cher doit gagner et pointer la bonne tranche.
    #[test]
    fn an_exact_quote_is_located_exactly() {
        let a = locate(SRC, "five dozen liquor jugs", &AlignConfig::default()).unwrap();
        assert_eq!(a.status, AlignmentStatus::Exact);
        assert_eq!(a.coverage, 1.0);
        assert_eq!(a.interval.slice(SRC).unwrap(), "five dozen liquor jugs");
    }

    // La citation a été écrite avant que le fichier soit rewrappé : la phrase tient
    // toujours, seul le formatage a bougé. L'intervalle doit viser le texte d'origine.
    #[test]
    fn a_reflowed_quote_is_located_as_normalized() {
        let a = locate(
            SRC,
            "five    dozen\n  liquor   jugs",
            &AlignConfig::default(),
        )
        .unwrap();
        assert_eq!(a.status, AlignmentStatus::Normalized);
        assert_eq!(a.interval.slice(SRC).unwrap(), "five dozen liquor jugs");
    }

    // Une phrase reformulée (un mot changé) : ni exacte ni un problème de blancs,
    // mais clairement le même passage.
    #[test]
    fn a_reworded_sentence_is_located_as_fuzzy_in_the_right_region() {
        let a = locate(SRC, "quick brown cat jumps over", &AlignConfig::default()).unwrap();
        assert_eq!(a.status, AlignmentStatus::Fuzzy);
        assert!(a.coverage >= 0.66);
        assert!(
            a.interval
                .slice(SRC)
                .unwrap()
                .contains("quick brown fox jumps over")
        );
    }

    // La réponse qui compte le plus : une citation inventée ne se localise pas.
    #[test]
    fn an_invented_quote_is_not_located() {
        assert!(
            locate(
                SRC,
                "entirely unrelated wording about turtles",
                &AlignConfig::default()
            )
            .is_none()
        );
    }

    // Sans garde de densité, des mots courants dispersés « correspondent » : c'est le
    // mode de défaillance que la densité existe pour empêcher.
    #[test]
    fn the_density_guard_rejects_scattered_words() {
        let permissive = AlignConfig {
            min_density: 0.0,
            ..AlignConfig::default()
        };
        let loose = locate(SRC, "the my how", &permissive).expect("sans garde, ça correspond");
        assert!(loose.density < 0.5);
        assert!(locate(SRC, "the my how", &AlignConfig::default()).is_none());
    }

    #[test]
    fn fuzzy_can_be_switched_off() {
        let strict = AlignConfig {
            enable_fuzzy: false,
            ..AlignConfig::default()
        };
        assert!(locate(SRC, "quick brown cat jumps over", &strict).is_none());
        assert!(locate(SRC, "lazy dog", &strict).is_some());
    }

    #[test]
    fn empty_inputs_never_locate() {
        let cfg = AlignConfig::default();
        assert!(locate(SRC, "", &cfg).is_none());
        assert!(locate(SRC, " \n ", &cfg).is_none());
        assert!(locate("", "x", &cfg).is_none());
    }

    // Les offsets rendus sont des octets : sur du texte accentué/CJK ils doivent
    // rester des frontières de caractère, sinon `slice` rendrait `None`.
    #[test]
    fn intervals_on_multibyte_sources_fall_on_char_boundaries() {
        let src = "Voilà une phrase accentuée. 日本語のテキスト. Et la fin.";
        let cfg = AlignConfig::default();
        for needle in ["phrase accentuée", "日本語のテキスト", "Et la fin"] {
            let a = locate(src, needle, &cfg).unwrap();
            assert_eq!(a.interval.slice(src), Some(needle));
        }
        // Flou sur du multi-octets : l'intervalle doit aussi se résoudre.
        let a = locate(src, "phrase très accentuée", &cfg).unwrap();
        assert!(a.interval.slice(src).is_some());
    }

    // La table de normalisation est le point délicat de la stratégie 2.
    #[test]
    fn normalization_maps_back_to_original_offsets() {
        let src = "alpha   beta\n\n\tgamma";
        let (norm, map) = normalize_with_map(src);
        assert_eq!(norm, "alpha beta gamma");
        assert_eq!(map.len(), norm.len() + 1);
        assert_eq!(map[map.len() - 1], src.len());
        assert!(map.iter().all(|&o| src.is_char_boundary(o)));
    }
}
