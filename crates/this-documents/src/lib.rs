//! Le cœur pur d'une pipeline de documents : reconnaître un format, en tirer du
//! texte, le découper en morceaux qui pavent la source, retrouver un extrait
//! dans cette source, et produire des enregistrements prêts à être écrits.
//!
//! ## Pourquoi la provenance est la partie difficile
//!
//! Découper un texte est facile. Ce qui est difficile, c'est de pouvoir dire,
//! plus tard, *d'où* vient une phrase : quel octet, quelle page, et si elle y
//! figure encore. Un extrait qu'on ne sait pas localiser n'est pas un
//! résultat : c'est une affirmation sans appui, et c'est exactement ce que fait
//! un modèle qui cite de mémoire. D'où trois règles qui traversent le crate :
//!
//! - les morceaux **pavent** la source (contigus, sans recouvrement, leur
//!   concaténation redonne la source) ; toute provenance en découle ;
//! - [`locate`] rend `None` plutôt qu'un faux positif : « introuvable » est la
//!   réponse la plus utile qu'il puisse donner ;
//! - [`ByteInterval::slice`] rend `None` plutôt que de paniquer : un intervalle
//!   stocké il y a des mois peut être relu contre un texte qui a changé.
//!
//! ## Pourquoi ce crate est pur
//!
//! Aucun I/O, aucun async, aucun graphe, aucune horloge. Il consomme des octets
//! et rend des données ; l'application, elle, les écrit. C'est ce qui lui permet
//! de compiler pour `wasm32-unknown-unknown` comme pour la cible native, donc de
//! servir la même logique à un serveur et à une page, et de se tester sans rien
//! démarrer.

pub mod align;
pub mod chunk;
pub mod extract;
pub mod record;

pub use align::{AlignConfig, Alignment, AlignmentStatus, locate};
pub use chunk::{ChunkConfig, DEFAULT_MAX_CHUNK_BYTES, TextChunk, chunk_text};
pub use extract::image::{MediaType, sniff_image};
pub use extract::{
    DocumentFormat, ExtractError, ExtractedText, ExtractorRegistry, FormatProbe, TextExtractor,
};
pub use record::{
    CHUNK_LABEL, ChunkRecord, DOCUMENT_LABEL, DocumentRecord, HAS_CHUNK, HAS_DOCUMENT, RecordError,
    chunk_to_record, sha256_hex,
};

use serde::{Deserialize, Serialize};

/// Un intervalle d'octets semi-ouvert `[start, end)` dans un texte source.
///
/// Des octets, pas des caractères : c'est ce que prend `&source[..]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteInterval {
    pub start: usize,
    pub end: usize,
}

impl ByteInterval {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "interval start must not exceed end");
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Découpe la source que cet intervalle désigne.
    ///
    /// Rend `None` — jamais de panique — si l'intervalle dépasse le texte, est
    /// inversé, ou tombe au milieu d'un caractère.
    pub fn slice<'a>(&self, source: &'a str) -> Option<&'a str> {
        if self.start > self.end
            || self.end > source.len()
            || !source.is_char_boundary(self.start)
            || !source.is_char_boundary(self.end)
        {
            return None;
        }
        source.get(self.start..self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Le cas nominal : sans lui, plus rien de ce qui repose sur `slice` n'a de sens.
    #[test]
    fn slice_returns_the_referenced_text() {
        assert_eq!(ByteInterval::new(2, 5).slice("abcdefg"), Some("cde"));
    }

    // Un intervalle stocké, relu contre un texte devenu plus court, doit dégrader en
    // « irrésolu », pas faire tomber l'appelant.
    #[test]
    fn slice_is_none_out_of_bounds_instead_of_panicking() {
        assert_eq!(ByteInterval::new(0, 99).slice("abc"), None);
        assert_eq!(ByteInterval { start: 3, end: 1 }.slice("abcdef"), None);
    }

    // « é » fait 2 octets : couper à l'octet 1 le casserait. `None`, pas de panique.
    #[test]
    fn slice_is_none_inside_a_multibyte_character() {
        let s = "é!";
        assert_eq!(ByteInterval::new(0, 1).slice(s), None);
        assert_eq!(ByteInterval::new(1, 2).slice(s), None);
        assert_eq!(ByteInterval::new(0, 2).slice(s), Some("é"));
    }

    #[test]
    fn len_and_is_empty_agree() {
        assert_eq!(ByteInterval::new(4, 4).len(), 0);
        assert!(ByteInterval::new(4, 4).is_empty());
        assert!(!ByteInterval::new(4, 5).is_empty());
    }
}
