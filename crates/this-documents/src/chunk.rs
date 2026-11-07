//! Découpage par phrases qui pave exactement la source.
//!
//! Trois cas :
//! - **C** — plusieurs phrases entières tiennent dans le budget : on les groupe ;
//! - **A** — une phrase dépasse le budget : on la coupe au dernier `\n`, sinon au
//!   dernier espace ;
//! - **B** — un seul mot indivisible dépasse le budget : il *devient* le morceau,
//!   et déborde plutôt que d'être coupé.
//!
//! Le budget est une cible, pas une contrainte : couper un mot détruirait ce pour
//! quoi le morceau existe — pouvoir être retrouvé dans la source.
//!
//! ## L'invariant
//!
//! Les morceaux **pavent** la source : contigus, sans recouvrement, concaténation
//! égale à l'entrée. Toutes les coupes se font sur de l'ASCII (`.`, `!`, `?`,
//! `\n`, espace), donc toujours sur une frontière UTF-8.

use serde::{Deserialize, Serialize};

use crate::ByteInterval;

/// Budget par défaut, en octets (≈ la fenêtre d'un modèle d'embedding courant).
pub const DEFAULT_MAX_CHUNK_BYTES: usize = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkConfig {
    /// Taille cible maximale d'un morceau, en octets. Une cible : un mot plus long
    /// est émis entier (cas B) ; voir [`TextChunk::is_oversized`].
    pub max_bytes: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_CHUNK_BYTES,
        }
    }
}

/// Un morceau de la source, avec son origine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChunk {
    /// Exactement `&source[interval]`.
    pub text: String,
    pub interval: ByteInterval,
}

impl TextChunk {
    /// Ce morceau a-t-il dû dépasser le budget (cas B) ?
    pub fn is_oversized(&self, config: &ChunkConfig) -> bool {
        self.interval.len() > config.max_bytes
    }
}

/// Découpe `source` en morceaux qui la pavent. Une source vide ou blanche ne donne
/// aucun morceau : rien à ancrer.
pub fn chunk_text(source: &str, config: &ChunkConfig) -> Vec<TextChunk> {
    if source.trim().is_empty() {
        return Vec::new();
    }
    // Budget nul : on le traite comme « un morceau par unité indivisible ».
    let max = config.max_bytes.max(1);

    let mut out: Vec<TextChunk> = Vec::new();
    let mut pending: Option<ByteInterval> = None;

    for sentence in sentence_spans(source) {
        if sentence.len() > max {
            if let Some(p) = pending.take() {
                out.push(materialize(source, p));
            }
            for piece in split_oversized(source, sentence, max) {
                out.push(materialize(source, piece));
            }
            continue;
        }
        match pending {
            Some(p) if sentence.end - p.start <= max => {
                pending = Some(ByteInterval::new(p.start, sentence.end));
            }
            Some(p) => {
                out.push(materialize(source, p));
                pending = Some(sentence);
            }
            None => pending = Some(sentence),
        }
    }
    if let Some(p) = pending {
        out.push(materialize(source, p));
    }
    out
}

fn materialize(source: &str, interval: ByteInterval) -> TextChunk {
    TextChunk {
        text: interval
            .slice(source)
            .expect("les intervalles produits ici tombent toujours sur une frontière de caractère")
            .to_string(),
        interval,
    }
}

/// Phrases de la source, qui la pavent entièrement. Détection volontairement
/// naïve : `.`, `!`, `?` suivis d'un blanc, plus la ligne vide. Une mauvaise
/// frontière coûte une coupe un peu étrange, jamais la justesse : le pavage tient
/// quoi qu'il arrive.
fn sentence_spans(source: &str) -> Vec<ByteInterval> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        let is_terminator = matches!(b, b'.' | b'!' | b'?');
        let is_break = b == b'\n' && bytes.get(i + 1) == Some(&b'\n');

        if is_terminator || is_break {
            let mut end = i + 1;
            while end < bytes.len() && bytes[end].is_ascii_whitespace() {
                end += 1;
            }
            // « 3.14 », « fichier.rs » : un point collé n'est pas une fin de phrase.
            if is_break || end > i + 1 || end == bytes.len() {
                spans.push(ByteInterval::new(start, end));
                start = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    if start < bytes.len() {
        spans.push(ByteInterval::new(start, bytes.len()));
    }
    spans
}

/// Pave un intervalle trop long : dernier `\n` de la fenêtre, sinon dernier blanc,
/// sinon le mot entier (cas B).
fn split_oversized(source: &str, span: ByteInterval, max: usize) -> Vec<ByteInterval> {
    let mut out = Vec::new();
    let mut cursor = span.start;

    while cursor < span.end {
        if span.end - cursor <= max {
            out.push(ByteInterval::new(cursor, span.end));
            break;
        }
        let window = &source.as_bytes()[cursor..cursor + max];
        // `\n` et l'espace sont de l'ASCII : la coupe est une frontière de caractère.
        let cut = window
            .iter()
            .rposition(|&b| b == b'\n')
            .or_else(|| window.iter().rposition(|&b| b.is_ascii_whitespace()))
            .map(|rel| cursor + rel + 1);

        match cut {
            Some(end) if end > cursor => {
                out.push(ByteInterval::new(cursor, end));
                cursor = end;
            }
            _ => {
                let end = next_break_after(source, cursor, span.end);
                out.push(ByteInterval::new(cursor, end));
                cursor = end;
            }
        }
    }
    out
}

/// Fin du mot indivisible qui commence en `from` (blancs suivants absorbés).
fn next_break_after(source: &str, from: usize, limit: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = from;
    while i < limit && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < limit && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i > from { i } else { limit }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La propriété sur laquelle tout repose.
    fn assert_tiles(source: &str, chunks: &[TextChunk]) {
        let mut expected = 0usize;
        for c in chunks {
            assert_eq!(
                c.interval.start, expected,
                "trou ou recouvrement avant {:?}",
                c.text
            );
            assert_eq!(c.text, c.interval.slice(source).unwrap());
            expected = c.interval.end;
        }
        if !chunks.is_empty() {
            assert_eq!(expected, source.len());
            let rebuilt: String = chunks.iter().map(|c| c.text.as_str()).collect();
            assert_eq!(rebuilt, source);
        }
    }

    // Une source sans contenu ne donne aucun morceau : un morceau vide ne
    // correspondrait jamais à rien.
    #[test]
    fn empty_and_blank_sources_yield_no_chunks() {
        let cfg = ChunkConfig::default();
        assert!(chunk_text("", &cfg).is_empty());
        assert!(chunk_text("  \n\t ", &cfg).is_empty());
    }

    // Le défaut documenté (1500) ne doit pas dériver sans qu'un test le voie.
    #[test]
    fn default_budget_is_1500_bytes() {
        assert_eq!(ChunkConfig::default().max_bytes, 1500);
        assert_eq!(DEFAULT_MAX_CHUNK_BYTES, 1500);
    }

    // Texte multi-paragraphes : les morceaux doivent redonner la source à l'octet près.
    #[test]
    fn chunks_tile_a_multi_paragraph_text() {
        let src = "# Titre\n\nUn paragraphe qui s'étend un moment. Il a plusieurs phrases! \
                   Certaines sont courtes. D'autres divaguent bien plus longtemps que les \
                   autres, sans raison.\n\n- un item\n- un autre\n\nLigne finale.";
        for max in [1usize, 2, 5, 13, 40, 100, 10_000] {
            assert_tiles(src, &chunk_text(src, &ChunkConfig { max_bytes: max }));
        }
    }

    // Les coupes sont sur de l'ASCII ; ce test prouve qu'aucune ne tombe dans un
    // caractère multi-octets (accents, CJK, emoji).
    #[test]
    fn chunks_tile_accented_utf8_text_without_splitting_a_character() {
        let src =
            "Voilà une phrase accentuée. 日本語のテキストもあります。Et un emoji 🎯 ici. Fin éàü.";
        for max in [3usize, 8, 16, 32, 64, 1000] {
            assert_tiles(src, &chunk_text(src, &ChunkConfig { max_bytes: max }));
        }
    }

    // Cas C : des phrases entières se groupent tant que ça tient.
    #[test]
    fn whole_sentences_are_packed_up_to_the_budget() {
        let src = "Roses are red. Violets are blue. Flowers are nice. And so are you.";
        let chunks = chunk_text(src, &ChunkConfig { max_bytes: 60 });
        assert_tiles(src, &chunks);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].text.trim(), "And so are you.");
    }

    // Cas A : une phrase trop longue se coupe d'abord sur les fins de ligne.
    #[test]
    fn a_long_sentence_is_cut_on_newlines_first() {
        let src = "No man is an island,\nEntire of itself,\nEvery man is a piece of the continent,\nA part of the main.";
        let chunks = chunk_text(src, &ChunkConfig { max_bytes: 40 });
        assert_tiles(src, &chunks);
        assert!(chunks.len() >= 3);
        for c in &chunks {
            assert!(c.text.ends_with('\n') || c.interval.end == src.len());
        }
    }

    // Cas B : couper un mot le perdrait. Il est émis entier, et signalé surdimensionné.
    #[test]
    fn an_oversized_token_is_emitted_whole_and_flagged() {
        let src = "This is antidisestablishmentarianism.";
        let cfg = ChunkConfig { max_bytes: 20 };
        let chunks = chunk_text(src, &cfg);
        assert_tiles(src, &chunks);
        let long: Vec<_> = chunks
            .iter()
            .filter(|c| c.text.contains("antidisestablishmentarianism"))
            .collect();
        assert_eq!(
            long.len(),
            1,
            "le mot ne doit pas être réparti sur deux morceaux"
        );
        assert!(long[0].is_oversized(&cfg));
        assert!(!chunks[0].is_oversized(&cfg));
    }

    // Un point dans « 3.14 » ou « manager.rs » n'est pas une fin de phrase.
    #[test]
    fn a_dot_inside_a_token_is_not_a_sentence_end() {
        let src = "Edit src/chat/manager.rs at line 3.14 then rerun. Done.";
        let chunks = chunk_text(src, &ChunkConfig { max_bytes: 1000 });
        assert_tiles(src, &chunks);
        assert_eq!(chunks.len(), 1);
    }

    // Un budget nul bouclerait à l'infini s'il était traité comme une contrainte dure.
    #[test]
    fn a_zero_budget_terminates_and_still_tiles() {
        let src = "Short one. And another one here.";
        let chunks = chunk_text(src, &ChunkConfig { max_bytes: 0 });
        assert_tiles(src, &chunks);
        assert!(!chunks.is_empty());
    }
}
