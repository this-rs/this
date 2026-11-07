//! Word (`.docx`, Office Open XML) : l'archive ZIP, puis le texte de `word/document.xml`.
//!
//! Un `.docx` est un ZIP de parties XML. Le corps du texte vit dans une seule,
//! `word/document.xml`, sous forme de paragraphes (`<w:p>`) qui contiennent des
//! runs (`<w:r>`) qui contiennent des nœuds de texte (`<w:t>`). Styles, polices,
//! numérotation, images incorporées : présentation ou binaire, rien qu'un lecteur
//! citerait.
//!
//! ## On ouvre le conteneur avant de revendiquer le fichier
//!
//! `PK\x03\x04` est le magic de toute la famille ZIP : `.docx`, `.xlsx`, `.jar`,
//! `.epub`, une archive renommée. L'extension ne tranche pas non plus (elle vient
//! de celui qui envoie). La seule preuve qu'un blob est un document Word, c'est que
//! son annuaire central contienne `word/document.xml` ; [`DocxExtractor::can_handle`]
//! le lit, sans rien décompresser. Une archive ZIP qui n'est pas du Word n'est donc
//! **pas** revendiquée : elle retombe en `UnsupportedFormat`, pas en `Malformed`.
//! Seul cas d'exception : l'extension dit `docx` et le magic ZIP est là. L'envoyeur
//! a affirmé que c'est du Word et la forme est compatible ; si l'archive est
//! cassée ou sans corps, « document DOCX malformé » est la réponse exacte.
//!
//! ## Les séparateurs valent autant que le texte
//!
//! WordprocessingML n'écrit aucun espace entre deux paragraphes : leurs `<w:t>`
//! se colleraient en une seule ligne, plausible et inutilisable, car le découpeur
//! ([`crate::chunk`]) ne trouverait ni fin de phrase ni saut de paragraphe. Donc :
//! `</w:p>` → `\n`, `<w:br/>` → `\n`, `<w:tab/>` → `\t`.
//!
//! ## Tableaux
//!
//! Une cellule se termine par une tabulation et une rangée par un saut de ligne
//! (la dernière tabulation de la rangée est remplacée par ce saut). Les
//! paragraphes d'une même cellule sont joints par une espace : un saut de ligne
//! au milieu d'une cellule casserait la règle « une ligne = une rangée ». Un
//! tableau imbriqué est aplati dans sa cellule hôte.
//!
//! ## Pages : des sections, pas des pages imprimées
//!
//! La pagination d'un `.docx` n'est pas dans le fichier : les sauts de page sont
//! calculés par le moteur de rendu (police, papier, imprimante). Annoncer « page
//! 7 » serait une provenance fausse avec aplomb. Ce qui est stable, c'est la
//! **section** (`<w:sectPr>`), la vraie unité structurelle du document : on
//! en fait une entrée de [`ExtractedText::pages`]. Sans aucun `sectPr`, une seule
//! page couvre tout le texte. Les sections tuilent le texte : aucun octet n'est
//! dans deux d'entre elles.
//!
//! ## Plafonds
//!
//! - XML décompressé : [`MAX_DOCUMENT_XML_BYTES`] (64 Mio) ;
//! - texte produit : [`MAX_TEXT_BYTES`] (16 Mio).
//!
//! Dépasser l'un ou l'autre rend `Malformed` en disant la limite, et non un texte
//! tronqué avec un avertissement : un document coupé silencieusement à 16 Mio
//! passerait pour entier dans l'index, alors qu'aucun vrai document Word n'en
//! approche (mille pages ≈ 2,5 Mio de prose) ; ce qui les dépasse est une bombe.
//!
//! ## Ce qui n'est pas lu est dit
//!
//! Images, objets incorporés (OLE), codes de champ et texte supprimé (suivi des
//! modifications), en-têtes, pieds de page, notes et commentaires : chacun produit
//! un avertissement. Jamais un silence.

use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::events::Event;

use super::{DocumentFormat, ExtractError, ExtractedText, FormatProbe, TextExtractor};
use crate::ByteInterval;

/// La partie qui porte le corps. Fixée par ECMA-376 : sans elle, ce n'est pas du Word.
const DOCUMENT_PART: &str = "word/document.xml";

/// Plafond du XML **décompressé** de `word/document.xml`.
///
/// La taille déclarée dans l'archive est écrite par celui qui l'a fabriquée : une
/// bombe ment dessus. Seul un arrêt de lecture protège, c'est ce que borne ce
/// plafond. Dimensionné sur le pire document réel : mille pages ≈ 2,5 Mio de
/// prose, et le balisage WordprocessingML coûte 10 à 20 fois le texte, soit
/// 25–50 Mio. 64 Mio laissent de la marge au-dessus, et une bombe (quelques
/// centaines de Ko qui se déplient en gigaoctets) est coupée après 64 Mio lus.
pub const MAX_DOCUMENT_XML_BYTES: u64 = 64 * 1024 * 1024;

/// Plafond du texte produit. Le XML borné ne borne pas le texte de façon utile
/// (un million de `<w:p/>` donne un million de sauts de ligne pour quelques Mio),
/// et c'est le texte que découpent, alignent et stockent les étages suivants.
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// L'extracteur Word. Derrière la feature `docx`.
pub struct DocxExtractor;

fn malformed(detail: String) -> ExtractError {
    ExtractError::Malformed {
        format: "DOCX",
        detail,
    }
}

impl TextExtractor for DocxExtractor {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Docx
    }

    fn can_handle(&self, probe: &FormatProbe<'_>) -> bool {
        // Garde bon marché d'abord : le registre sonde chaque extracteur sur
        // chaque envoi, et ce qui suit parse un annuaire ZIP.
        if !probe.starts_with(b"PK\x03\x04") {
            return false;
        }
        let claims_docx = probe.extension().as_deref() == Some("docx");
        match zip::ZipArchive::new(Cursor::new(probe.bytes)) {
            Ok(archive) => archive.index_for_name(DOCUMENT_PART).is_some() || claims_docx,
            // Annuaire illisible : sans l'extension, on ne sait pas que c'est du
            // Word, donc on décline (le registre dira ce qu'il a reniflé). Avec
            // l'extension `docx`, on revendique pour dire « malformé ».
            Err(_) => claims_docx,
        }
    }

    fn extract(&self, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| malformed(format!("not a readable ZIP archive: {e}")))?;

        let mut warnings = Vec::new();
        note_unread_parts(&archive, &mut warnings);
        let xml = read_document_part(&mut archive)?;
        let (text, pages) = extract_body(&xml, &mut warnings)?;

        if text.is_empty() {
            // Pas une erreur (un document peut être vide ou ne contenir que des
            // images), mais une chaîne vide sans un mot ressemblerait à une réussite.
            warnings.push(format!("{DOCUMENT_PART} contained no text"));
        }

        Ok(ExtractedText {
            text,
            format: DocumentFormat::Docx,
            pages,
            warnings,
        })
    }
}

/// Signale les parties de texte que cet extracteur ne lit pas.
fn note_unread_parts<R: Read + std::io::Seek>(
    archive: &zip::ZipArchive<R>,
    warnings: &mut Vec<String>,
) {
    // zip 9 rend un `Result` par nom (un nom non décodable est une erreur) : un nom
    // illisible n'est pas une partie de texte connue, on l'écarte.
    let mut unread: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|n| n.into_owned())
        .filter(|n| {
            n.starts_with("word/header")
                || n.starts_with("word/footer")
                || matches!(
                    n.as_str(),
                    "word/footnotes.xml" | "word/endnotes.xml" | "word/comments.xml"
                )
        })
        .collect();
    if !unread.is_empty() {
        unread.sort_unstable();
        warnings.push(format!(
            "parts not extracted (only the body is read): {}",
            unread.join(", ")
        ));
    }
}

/// Lit `word/document.xml` sans jamais décompresser plus que [`MAX_DOCUMENT_XML_BYTES`].
fn read_document_part<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<String, ExtractError> {
    let mut part = archive
        .by_name(DOCUMENT_PART)
        .map_err(|e| malformed(format!("missing {DOCUMENT_PART}: {e}")))?;

    // La taille déclarée n'est qu'une prétention : elle sert à échouer vite quand
    // l'archive avoue être énorme. La vraie garde est la lecture bornée ci-dessous.
    let declared = part.size();
    if declared > MAX_DOCUMENT_XML_BYTES {
        return Err(malformed(format!(
            "{DOCUMENT_PART} declares {declared} bytes, over the {MAX_DOCUMENT_XML_BYTES}-byte limit"
        )));
    }

    // `take` borne la *mémoire*, quoi qu'ait écrit l'en-tête. L'octet de plus rend
    // le dépassement observable : s'il arrive, il en restait.
    let mut raw = Vec::new();
    part.by_ref()
        .take(MAX_DOCUMENT_XML_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| malformed(format!("could not decompress {DOCUMENT_PART}: {e}")))?;

    if raw.len() as u64 > MAX_DOCUMENT_XML_BYTES {
        return Err(malformed(format!(
            "{DOCUMENT_PART} decompresses past the {MAX_DOCUMENT_XML_BYTES}-byte limit \
             (declared {declared} bytes); refusing to expand it further"
        )));
    }

    String::from_utf8(raw).map_err(|e| {
        malformed(format!(
            "{DOCUMENT_PART} is not valid UTF-8 at byte {}",
            e.utf8_error().valid_up_to()
        ))
    })
}

fn too_much_text() -> ExtractError {
    malformed(format!(
        "extracted text exceeds the {MAX_TEXT_BYTES}-byte limit; refusing to produce a truncated document"
    ))
}

/// Parcourt le XML en flux : texte, séparateurs, tableaux, sections.
///
/// Un passage en flux plutôt qu'un DOM : rien n'a besoin d'un ancêtre, seulement
/// de savoir où l'on est (dans un `<w:t>`, dans une cellule, dans un `<w:pPr>`).
fn extract_body(
    xml: &str,
    warnings: &mut Vec<String>,
) -> Result<(String, Vec<ByteInterval>), ExtractError> {
    let mut reader = Reader::from_str(xml);
    // `xml:space="preserve"` marque des runs dont les espaces de bord sont du
    // texte : rogner collerait les mots d'un run au suivant, sans que l'alignement
    // puisse jamais le détecter.
    reader.config_mut().trim_text(false);

    let mut text = String::new();
    let mut in_text_node = false;
    // `<w:tab/>` hors d'un run déclare un *arrêt* de tabulation dans `<w:pPr>`
    // (métadonnée de mise en page), pas un caractère : seul le run compte.
    let mut in_run = false;
    let mut in_ppr = false;
    // Début, dans `text`, de chaque cellule ouverte (pile : tableaux imbriqués).
    let mut cell_starts: Vec<usize> = Vec::new();
    // Un `sectPr` dans un `pPr` clôt la section à la fin de CE paragraphe.
    let mut section_ends_with_paragraph = false;
    let mut section_ends: Vec<usize> = Vec::new();
    // quick-xml rend `Eof` sans broncher sur un fichier tronqué : seule une
    // profondeur non nulle trahit la coupure.
    let mut depth = 0usize;
    let mut saw_document_root = false;
    let (mut images, mut objects, mut hidden_runs) = (0usize, 0usize, 0usize);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                match e.local_name().as_ref() {
                    "document" => saw_document_root = true,
                    "r" => in_run = true,
                    "t" => in_text_node = true,
                    "pPr" => in_ppr = true,
                    "sectPr" if in_ppr => section_ends_with_paragraph = true,
                    "tc" => cell_starts.push(text.len()),
                    "drawing" | "pict" => images += 1,
                    "object" => objects += 1,
                    "instrText" | "delText" => hidden_runs += 1,
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                match e.local_name().as_ref() {
                    "r" => in_run = false,
                    "t" => in_text_node = false,
                    "pPr" => in_ppr = false,
                    "p" => {
                        // Dans une cellule : une espace, pour garder « une
                        // rangée = une ligne ». Ailleurs : le saut de paragraphe.
                        text.push(if cell_starts.is_empty() { '\n' } else { ' ' });
                        if std::mem::take(&mut section_ends_with_paragraph) {
                            section_ends.push(text.len());
                        }
                    }
                    "tc" => {
                        let start = cell_starts.pop().unwrap_or(0).min(text.len());
                        // Les paragraphes de la cellule ont laissé une espace de fin.
                        let kept = text[start..].trim_end_matches(' ').len();
                        text.truncate(start + kept);
                        text.push('\t');
                    }
                    "tr" => {
                        // La dernière cellule n'a pas à finir par une tabulation :
                        // la rangée finit, c'est un saut de ligne.
                        if text.ends_with('\t') {
                            text.pop();
                        }
                        text.push('\n');
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "document" => saw_document_root = true,
                // Un `<w:sectPr/>` vide dans un `pPr` clôt aussi la section.
                "sectPr" if in_ppr => section_ends_with_paragraph = true,
                "tab" if in_run => text.push('\t'),
                // `<w:br/>` : saut mou dans un paragraphe ; `<w:cr/>` : l'ancien
                // retour chariot, même effet.
                "br" | "cr" if in_run => text.push('\n'),
                "drawing" | "pict" => images += 1,
                "object" => objects += 1,
                _ => {}
            },
            // quick-xml 0.42 livre le texte déjà validé en `str`.
            Ok(Event::Text(e)) if in_text_node => text.push_str(&e),
            // quick-xml ne résout pas les entités : `R&amp;D` arrive en trois
            // événements. L'ignorer effacerait le `&` de chaque raison sociale.
            Ok(Event::GeneralRef(e)) if in_text_node => {
                let name: &str = &e;
                match quick_xml::escape::unescape(&format!("&{name};")) {
                    Ok(resolved) => text.push_str(&resolved),
                    Err(_) => warnings.push(format!(
                        "dropped an entity reference this parser cannot resolve: &{name};"
                    )),
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                // XML cassé en cours de route : garder ce qui a été lu et dire où
                // ça s'arrête. Une erreur jetterait un document lisible à 99 % ;
                // rendre le préfixe en silence le ferait passer pour entier.
                if text.is_empty() {
                    return Err(malformed(format!(
                        "{DOCUMENT_PART} is not well-formed XML: {err}"
                    )));
                }
                warnings.push(format!(
                    "{DOCUMENT_PART} is malformed at byte {}; kept the {} bytes read before it: {err}",
                    reader.buffer_position(),
                    text.len()
                ));
                depth = 0;
                break;
            }
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(too_much_text());
        }
    }

    // Un membre nommé `word/document.xml` qui n'est pas du WordprocessingML est
    // possible (n'importe qui fabrique l'archive) : pas de « succès vide ».
    if !saw_document_root {
        return Err(malformed(format!(
            "{DOCUMENT_PART} has no <w:document> root element"
        )));
    }
    if depth > 0 {
        warnings.push(format!(
            "{DOCUMENT_PART} ends with {depth} element(s) unclosed: the part is truncated \
             and the text stops where it was cut"
        ));
    }
    if images > 0 {
        warnings.push(format!(
            "{images} image/drawing(s) have no text and were skipped"
        ));
    }
    if objects > 0 {
        warnings.push(format!("{objects} embedded object(s) (OLE) were not read"));
    }
    if hidden_runs > 0 {
        warnings.push(format!(
            "skipped {hidden_runs} field-code or deleted-text run(s), which are not visible document text"
        ));
    }

    // Chaque paragraphe finit par un saut de ligne ; les vides de queue n'ont aucun
    // contenu et deviendraient des chunks vides plus loin.
    text.truncate(text.trim_end_matches(['\n', ' ', '\t']).len());

    Ok((text.clone(), tile_sections(&section_ends, text.len())))
}

/// Transforme les fins de section en intervalles qui tuilent `[0, len)`.
///
/// Les fins sont clampées à `len` (le rognage de queue a pu raccourcir le texte) et
/// les sections vides sont écartées. Sans fin de section : une page unique.
fn tile_sections(ends: &[usize], len: usize) -> Vec<ByteInterval> {
    let mut pages = Vec::new();
    let mut start = 0;
    for &end in ends.iter().map(|e| e.min(&len)) {
        if end > start {
            pages.push(ByteInterval::new(start, end));
            start = end;
        }
    }
    if start < len {
        pages.push(ByteInterval::new(start, len));
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::ExtractorRegistry;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Archive ZIP en mémoire à partir de couples (nom, contenu). Pas de .docx
    /// binaire versionné : on y verrait mal ce qu'il contient, et la moitié de ce
    /// qu'on teste est le traitement d'archives qu'aucun Word ne produirait.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(content).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn docx_of(body: &str) -> Vec<u8> {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
        );
        zip_of(&[(DOCUMENT_PART, xml.as_bytes())])
    }

    fn p(t: &str) -> String {
        format!("<w:p><w:r><w:t>{t}</w:t></w:r></w:p>")
    }

    fn cell(t: &str) -> String {
        format!("<w:tc>{}</w:tc>", p(t))
    }

    #[test]
    fn docx_extracts_paragraphs_in_order() {
        let out = DocxExtractor
            .extract(&docx_of(
                &(p("Premier.") + &p("Second.") + &p("Troisième.")),
            ))
            .unwrap();
        assert_eq!(out.text, "Premier.\nSecond.\nTroisième.");
        assert_eq!(out.format, DocumentFormat::Docx);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        // Aucune section déclarée : une seule page couvre tout.
        assert_eq!(out.pages, vec![ByteInterval::new(0, out.text.len())]);
    }

    #[test]
    fn docx_table_cells_are_tab_separated_and_rows_are_lines() {
        let table = format!(
            "<w:tbl><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>",
            cell("A1"),
            cell("B1"),
            cell("A2"),
            cell("B2")
        );
        let out = DocxExtractor
            .extract(&docx_of(&(p("Avant") + &table + &p("Après"))))
            .unwrap();
        assert_eq!(out.text, "Avant\nA1\tB1\nA2\tB2\nAprès");
    }

    #[test]
    fn docx_sections_become_pages_that_tile_the_text() {
        let first = "<w:p><w:pPr><w:sectPr/></w:pPr><w:r><w:t>Un</w:t></w:r></w:p>";
        let out = DocxExtractor
            .extract(&docx_of(&(first.to_string() + &p("Deux") + "<w:sectPr/>")))
            .unwrap();
        assert_eq!(out.text, "Un\nDeux");
        assert_eq!(
            out.pages,
            vec![ByteInterval::new(0, 3), ByteInterval::new(3, 7)]
        );
        assert_eq!(out.page_of(4), Some(2));
    }

    #[test]
    fn docx_entities_tabs_and_skipped_content_are_handled_and_reported() {
        let body = "<w:p><w:r><w:t>R&amp;D</w:t><w:tab/><w:t>x</w:t></w:r>\
                    <w:r><w:drawing/></w:r></w:p>";
        let out = DocxExtractor.extract(&docx_of(body)).unwrap();
        assert_eq!(out.text, "R&D\tx");
        assert!(out.warnings.iter().any(|w| w.contains("image")));
    }

    #[test]
    fn docx_zip_without_a_word_body_is_not_claimed() {
        let jar = zip_of(&[("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n")]);
        assert!(!DocxExtractor.can_handle(&FormatProbe::new(&jar, Some("app.jar"))));
        // Un .xlsx n'est pas du Word non plus, quoi que dise son nom.
        let xlsx = zip_of(&[("xl/workbook.xml", b"<workbook/>")]);
        assert!(!DocxExtractor.can_handle(&FormatProbe::new(&xlsx, Some("a.xlsx"))));
        // Dans le registre : « format non supporté », jamais « DOCX malformé ».
        let err = ExtractorRegistry::with_builtins()
            .extract(&jar, Some("app.jar"))
            .unwrap_err();
        assert!(
            matches!(err, ExtractError::UnsupportedFormat { .. }),
            "{err:?}"
        );
        // Un vrai document Word est revendiqué, même sans extension.
        let real = docx_of(&p("x"));
        assert!(DocxExtractor.can_handle(&FormatProbe::new(&real, None)));
    }

    #[test]
    fn docx_truncated_archive_is_malformed_without_panicking() {
        let full = docx_of(&p("Contenu"));
        for cut in [4, full.len() / 2, full.len() - 1] {
            let cutoff = &full[..cut];
            // Avec l'extension docx, l'extracteur revendique et dit « malformé ».
            let reg = ExtractorRegistry::with_builtins();
            match reg.extract(cutoff, Some("cassé.docx")) {
                Err(ExtractError::Malformed { format: "DOCX", .. }) => {}
                other => panic!("coupe à {cut} : attendu Malformed, reçu {other:?}"),
            }
        }
    }

    #[test]
    fn docx_xml_over_the_limit_is_refused_with_the_limit_named() {
        // Une bombe honnête sur sa taille : 65 Mio d'espaces compressent en ~65 Ko.
        let mut xml = String::from(r#"<w:document xmlns:w="x"><w:body>"#);
        xml.push_str(&" ".repeat(MAX_DOCUMENT_XML_BYTES as usize + 1));
        xml.push_str("</w:body></w:document>");
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(
            DOCUMENT_PART,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        w.write_all(xml.as_bytes()).unwrap();
        let bomb = w.finish().unwrap().into_inner();
        match DocxExtractor.extract(&bomb) {
            Err(ExtractError::Malformed { detail, .. }) => {
                assert!(detail.contains("67108864"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }
}
