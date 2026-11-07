//! Les formats dont les octets *sont* le texte : Markdown, code, JSON, CSV, YAML…
//!
//! Le fourre-tout du registre, donc celui qui doit être **strict sur ce qu'il
//! accepte** : sondé en dernier, tout ce qu'il prend est ce qu'aucun extracteur
//! spécifique n'a reconnu. Accepter de l'UTF-8 invalide ferait d'un envoi corrompu
//! un document de caractères de remplacement qui se découpe et s'aligne très bien
//! et ne veut rien dire.

use super::image::sniff_image;
use super::{DocumentFormat, ExtractError, ExtractedText, FormatProbe, TextExtractor};

/// Magics de conteneurs binaires que cet extracteur ne doit jamais revendiquer,
/// même si l'extension le dit.
const BINARY_MAGICS: &[&[u8]] = &[
    b"%PDF-",            // PDF
    b"PK\x03\x04",       // famille ZIP : docx, xlsx, jar, odt
    b"\x7fELF",          // exécutable ELF
    b"\x89PNG",          // PNG
    b"GIF8",             // GIF
    b"\xff\xd8\xff",     // JPEG
    b"\x00\x61\x73\x6d", // WebAssembly
    b"\x1f\x8b",         // gzip
    b"BZh",              // bzip2
    b"\xfd7zXZ",         // xz
];

pub struct PlainTextExtractor;

impl TextExtractor for PlainTextExtractor {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::PlainText
    }

    fn can_handle(&self, probe: &FormatProbe<'_>) -> bool {
        if BINARY_MAGICS.iter().any(|m| probe.starts_with(m)) {
            return false;
        }
        // WebP et HEIC commencent par des octets qui peuvent être de l'UTF-8 valide.
        if sniff_image(probe.bytes).is_some() {
            return false;
        }
        // BOM UTF-16 : décodable en principe, non décodé ici ; l'accepter donnerait
        // du bruit plutôt qu'une erreur honnête.
        if probe.starts_with(&[0xff, 0xfe]) || probe.starts_with(&[0xfe, 0xff]) {
            return false;
        }
        std::str::from_utf8(probe.bytes).is_ok()
    }

    fn extract(&self, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
        let text = std::str::from_utf8(bytes).map_err(|e| ExtractError::Malformed {
            format: "plain text",
            detail: format!("not valid UTF-8 at byte {}", e.valid_up_to()),
        })?;
        // Un BOM UTF-8 est une métadonnée : laissé en place, il devient un premier
        // caractère invisible qui casse l'alignement exact de toute citation à l'octet 0.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text).to_string();

        Ok(ExtractedText {
            text,
            format: DocumentFormat::PlainText,
            pages: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe<'a>(bytes: &'a [u8], name: Option<&'a str>) -> FormatProbe<'a> {
        FormatProbe::new(bytes, name)
    }

    // Le texte (accents, CJK, emoji) doit ressortir à l'octet près, sinon les
    // intervalles calculés dessus ne désignent plus rien.
    #[test]
    fn utf8_text_is_preserved_byte_for_byte() {
        let src = "# Titre\n\nUne phrase accentuée, du 日本語, et un emoji 🎯.\n";
        let out = PlainTextExtractor.extract(src.as_bytes()).unwrap();
        assert_eq!(out.text, src);
        assert!(out.pages.is_empty());
    }

    #[test]
    fn a_utf8_bom_is_stripped_so_offset_zero_is_content() {
        let out = PlainTextExtractor
            .extract("\u{feff}hello".as_bytes())
            .unwrap();
        assert_eq!(out.text, "hello");
    }

    // Chaque magic binaire doit être refusé malgré une extension .txt qui ment.
    #[test]
    fn binary_magics_are_refused_despite_a_txt_extension() {
        for magic in BINARY_MAGICS {
            let mut bytes = magic.to_vec();
            bytes.extend_from_slice(b"trailing content");
            assert!(
                !PlainTextExtractor.can_handle(&probe(&bytes, Some("notes.txt"))),
                "doit refuser {magic:?}"
            );
        }
    }

    // WebP/HEIC ne sont pas dans les magics ci-dessus et sont de l'UTF-8 valide
    // (NUL, ASCII) : seul le sniff d'image les écarte.
    #[test]
    fn webp_and_heic_are_not_mistaken_for_text() {
        assert!(!PlainTextExtractor.can_handle(&probe(b"RIFF\x24\0\0\0WEBPVP8 ", Some("a.txt"))));
        assert!(
            !PlainTextExtractor.can_handle(&probe(b"\0\0\0\x18ftypheic\0\0\0\0", Some("a.txt")))
        );
    }

    #[test]
    fn invalid_utf8_is_refused_rather_than_mangled() {
        let bytes = [0x48, 0x65, 0xff, 0xfe, 0x6c];
        assert!(!PlainTextExtractor.can_handle(&probe(&bytes, None)));
        assert!(matches!(
            PlainTextExtractor.extract(&bytes),
            Err(ExtractError::Malformed { .. })
        ));
    }

    #[test]
    fn utf16_boms_are_refused_rather_than_pretended() {
        assert!(!PlainTextExtractor.can_handle(&probe(&[0xff, 0xfe, 0x68, 0x00], Some("a.txt"))));
        assert!(!PlainTextExtractor.can_handle(&probe(&[0xfe, 0xff, 0x00, 0x68], Some("a.txt"))));
    }

    #[test]
    fn the_whole_text_family_is_accepted_whatever_the_extension() {
        for (name, body) in [
            ("a.md", "# h\n\ntext"),
            ("a.json", r#"{"k": 1}"#),
            ("a.csv", "a,b\n1,2"),
            ("a.rs", "fn main() {}"),
            ("sans_extension", "still text"),
        ] {
            assert!(
                PlainTextExtractor.can_handle(&probe(body.as_bytes(), Some(name))),
                "{name}"
            );
        }
    }
}
