//! PowerPoint (`.pptx`, Office Open XML) : une page par diapositive, dans l'ordre
//! de la présentation.
//!
//! Derrière la feature `pptx`, avec les mêmes dépendances que `docx` (`zip`,
//! `quick-xml`) et aucune autre. Un `.pptx` est un ZIP de parties XML : le texte
//! de chaque diapositive (`ppt/slides/slideN.xml`) et les notes de l'orateur qui
//! s'y rattachent (`ppt/notesSlides/notesSlideM.xml`). Le texte vit en DrawingML :
//! des paragraphes `<a:p>` qui contiennent des runs `<a:r>` qui contiennent des
//! nœuds `<a:t>`. Même piège qu'en WordprocessingML (voir `docx.rs`) : aucun
//! espace entre deux paragraphes, donc `</a:p>` → `\n`, `<a:br/>` → `\n`.
//!
//! ## L'ordre est celui de la présentation, pas celui des fichiers
//!
//! `slide2.xml` n'est pas forcément la deuxième diapositive : réordonner les
//! diapositives dans PowerPoint réécrit `<p:sldIdLst>` dans
//! `ppt/presentation.xml` et laisse les noms de parties tels quels. Et l'ordre
//! lexicographique des noms met `slide10` avant `slide2`. L'ordre vient donc de
//! cette liste, résolue par `ppt/_rels/presentation.xml.rels` ; l'ordre
//! **numérique** des noms n'est qu'un repli, annoncé par un avertissement, pour
//! une présentation dont la liste ne se résout pas. De même, les notes d'une
//! diapositive se trouvent par les relations de la diapositive, jamais en
//! appariant des numéros.
//!
//! ## Forme du texte
//!
//! ```text
//! ## Slide 1
//! Revue trimestrielle
//!
//! ### Notes
//! Commencer par le taux d'attrition.
//!
//! ## Slide 2
//! ...
//! ```
//!
//! Le numéro est la position dans la présentation. Chaque diapositive, notes
//! comprises, est une entrée de [`ExtractedText::pages`] : un extrait se rapporte
//! « diapositive 4 ». Les pages tuilent le texte ; une diapositive illisible garde
//! sa page (son seul en-tête) et un avertissement, comme une page de PDF.
//!
//! ## Ce qui est laissé de côté
//!
//! Numéro de diapositive, pied de page et date se répètent sur chaque diapositive
//! et ne disent rien : écartés. Dans une page de notes, seul l'espace réservé
//! `body` est le texte de l'orateur ; la vignette et le numéro ne le sont pas.
//! Images, graphiques et SmartArt n'ont pas de runs de texte ici.
//!
//! ## Plafonds
//!
//! Chaque partie est lue en lecture bornée ([`MAX_PART_BYTES`], quoi qu'en dise
//! l'en-tête de l'archive), la somme des parties lues l'est aussi
//! ([`MAX_TOTAL_BYTES`]), comme le nombre d'entrées ([`MAX_ARCHIVE_ENTRIES`]), de
//! diapositives ([`MAX_SLIDES`]) et le texte produit ([`MAX_TEXT_BYTES`]).
//! Dépasser l'un d'eux rend `Malformed` en nommant la limite : une présentation
//! coupée en silence passerait pour entière.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::{DocumentFormat, ExtractError, ExtractedText, FormatProbe, TextExtractor};
use crate::ByteInterval;

/// La partie principale. Fixée par ECMA-376 : sans elle, ce n'est pas un `.pptx`.
pub(crate) const PRESENTATION_PART: &str = "ppt/presentation.xml";
const PRESENTATION_RELS_PART: &str = "ppt/_rels/presentation.xml.rels";

/// Plafond des entrées de l'archive (une diapositive en coûte deux ou trois avec
/// ses relations et ses notes).
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// Plafond du contenu **décompressé** d'une partie. Même valeur que
/// `docx::MAX_DOCUMENT_XML_BYTES`.
pub const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;

/// Plafond de la somme des parties décompressées et lues.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Plafond des diapositives lues. Même valeur que `pdf::MAX_PAGES`.
pub const MAX_SLIDES: usize = 10_000;

/// Plafond du texte produit. Même valeur que DOCX, PDF et XLSX.
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

const FORMAT: &str = "PPTX";

#[derive(Clone, Copy)]
struct Limits {
    max_entries: usize,
    max_part_bytes: u64,
    max_total_bytes: u64,
    max_slides: usize,
    max_text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ARCHIVE_ENTRIES,
            max_part_bytes: MAX_PART_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_slides: MAX_SLIDES,
            max_text_bytes: MAX_TEXT_BYTES,
        }
    }
}

fn malformed(detail: String) -> ExtractError {
    ExtractError::Malformed {
        format: FORMAT,
        detail,
    }
}

/// L'extracteur PowerPoint. Derrière la feature `pptx`.
pub struct PptxExtractor;

impl TextExtractor for PptxExtractor {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Pptx
    }

    /// Même règle que DOCX et XLSX : la preuve est `ppt/presentation.xml` dans
    /// l'annuaire central. L'extension `pptx` ne fait revendiquer que si l'archive
    /// ne porte la partie principale d'aucun autre format Office.
    fn can_handle(&self, probe: &FormatProbe<'_>) -> bool {
        if !probe.starts_with(b"PK\x03\x04") {
            return false;
        }
        let claims_pptx = probe.extension().as_deref() == Some("pptx");
        match zip::ZipArchive::new(Cursor::new(probe.bytes)) {
            Ok(archive) => {
                archive.index_for_name(PRESENTATION_PART).is_some()
                    || (claims_pptx && !super::has_other_office_part(&archive, PRESENTATION_PART))
            }
            Err(_) => claims_pptx,
        }
    }

    fn extract(&self, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
        extract_with(bytes, Limits::default())
    }
}

/// Pourquoi une partie n'a pas été lue. Une borne dépassée condamne le document
/// (c'est une bombe) ; une partie illisible ne condamne que sa diapositive.
enum PartError {
    OverLimit(String),
    Unreadable(String),
}

/// L'archive et le budget de décompression qui lui reste.
struct Package<R> {
    archive: zip::ZipArchive<R>,
    limits: Limits,
    read_total: u64,
}

impl<R: Read + Seek> Package<R> {
    /// Lit une partie en UTF-8, sans jamais décompresser au-delà des plafonds.
    /// `Ok(None)` : la partie est absente.
    fn read_part(&mut self, name: &str) -> Result<Option<String>, PartError> {
        let Some(index) = self.archive.index_for_name(name) else {
            return Ok(None);
        };
        let max = self.limits.max_part_bytes;
        let part = self
            .archive
            .by_index(index)
            .map_err(|e| PartError::Unreadable(format!("{name}: {e}")))?;
        let declared = part.size();
        if declared > max {
            return Err(PartError::OverLimit(format!(
                "{name} declares {declared} bytes, over the {max}-byte limit"
            )));
        }
        // `take` borne la mémoire quoi qu'ait écrit l'en-tête ; l'octet de plus rend
        // le dépassement observable.
        let mut raw = Vec::new();
        part.take(max + 1)
            .read_to_end(&mut raw)
            .map_err(|e| PartError::Unreadable(format!("could not decompress {name}: {e}")))?;
        if raw.len() as u64 > max {
            return Err(PartError::OverLimit(format!(
                "{name} decompresses past the {max}-byte limit (declared {declared} bytes); \
                 refusing to expand it further"
            )));
        }
        self.read_total += raw.len() as u64;
        if self.read_total > self.limits.max_total_bytes {
            return Err(PartError::OverLimit(format!(
                "parts read decompress past the {}-byte total limit; refusing to expand further",
                self.limits.max_total_bytes
            )));
        }
        String::from_utf8(raw).map(Some).map_err(|e| {
            PartError::Unreadable(format!(
                "{name} is not valid UTF-8 at byte {}",
                e.utf8_error().valid_up_to()
            ))
        })
    }

    fn has(&self, name: &str) -> bool {
        self.archive.index_for_name(name).is_some()
    }
}

fn extract_with(bytes: &[u8], limits: Limits) -> Result<ExtractedText, ExtractError> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| malformed(format!("not a readable ZIP archive: {e}")))?;
    if archive.len() > limits.max_entries {
        return Err(malformed(format!(
            "archive has {} entries, over the {}-entry limit",
            archive.len(),
            limits.max_entries
        )));
    }
    let mut pkg = Package {
        archive,
        limits,
        read_total: 0,
    };

    let mut warnings = Vec::new();
    let slide_parts = ordered_slide_parts(&mut pkg, &mut warnings)?;
    if slide_parts.len() > limits.max_slides {
        return Err(malformed(format!(
            "presentation lists {} slides, over the {}-slide limit",
            slide_parts.len(),
            limits.max_slides
        )));
    }
    if slide_parts.is_empty() {
        warnings.push("the presentation contains no slides".to_string());
    }

    let mut text = String::new();
    let mut pages = Vec::with_capacity(slide_parts.len());
    let mut any_text = false;

    for (i, slide_part) in slide_parts.iter().enumerate() {
        let n = i + 1;
        let start = text.len();
        if i > 0 {
            // La ligne vide qui sépare deux diapositives appartient à la suivante :
            // les pages restent contiguës et la dernière ne finit pas sur du vide.
            text.push('\n');
        }
        text.push_str(&format!("## Slide {n}\n"));

        match pkg.read_part(slide_part) {
            Ok(Some(xml)) => match shape_text(&xml, Mode::Slide, slide_part, &mut warnings) {
                Ok(body) if !body.is_empty() => {
                    any_text = true;
                    text.push_str(&body);
                    text.push('\n');
                }
                Ok(_) => {}
                Err(detail) => warnings.push(format!("slide {n} skipped: {detail}")),
            },
            Ok(None) => warnings.push(format!("slide {n}: {slide_part} is listed but missing")),
            Err(PartError::OverLimit(detail)) => return Err(malformed(detail)),
            Err(PartError::Unreadable(detail)) => {
                warnings.push(format!("slide {n} skipped: {detail}"))
            }
        }

        match notes_part_for(&mut pkg, slide_part) {
            Ok(Some(notes_part)) => match pkg.read_part(&notes_part) {
                Ok(Some(xml)) => match shape_text(&xml, Mode::Notes, &notes_part, &mut warnings) {
                    Ok(notes) if !notes.is_empty() => {
                        any_text = true;
                        text.push_str("\n### Notes\n");
                        text.push_str(&notes);
                        text.push('\n');
                    }
                    Ok(_) => {}
                    Err(detail) => warnings.push(format!("notes of slide {n} skipped: {detail}")),
                },
                Ok(None) => warnings.push(format!(
                    "slide {n}: notes part {notes_part} is referenced but missing"
                )),
                Err(PartError::OverLimit(detail)) => return Err(malformed(detail)),
                Err(PartError::Unreadable(detail)) => {
                    warnings.push(format!("notes of slide {n} skipped: {detail}"))
                }
            },
            Ok(None) => {}
            Err(detail) => return Err(malformed(detail)),
        }

        pages.push(ByteInterval::new(start, text.len()));
        if text.len() > limits.max_text_bytes {
            return Err(malformed(format!(
                "extracted text passed the {}-byte limit on slide {n} of {}; \
                 refusing to produce a truncated presentation",
                limits.max_text_bytes,
                slide_parts.len()
            )));
        }
    }

    if !any_text && !slide_parts.is_empty() {
        warnings.push("the presentation contained no text".to_string());
    }

    Ok(ExtractedText {
        text,
        format: DocumentFormat::Pptx,
        pages,
        warnings,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Slide,
    Notes,
}

/// Un `<Relationship>` d'une partie `.rels`.
struct Relationship {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

fn parse_relationships(xml: &str) -> Vec<Relationship> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "Relationship" => {
                if let (Some(id), Some(target)) = (attr(&e, "Id"), attr(&e, "Target")) {
                    out.push(Relationship {
                        id,
                        kind: attr(&e, "Type").unwrap_or_default(),
                        target,
                        external: attr(&e, "TargetMode")
                            .is_some_and(|m| m.eq_ignore_ascii_case("External")),
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// Résout la cible d'une relation contre le dossier de la partie qui la porte, en
/// repliant les `..` : le résultat est un nom d'entrée de l'archive.
fn resolve_target(base_dir: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Les parties des diapositives, dans l'ordre de la présentation.
fn ordered_slide_parts<R: Read + Seek>(
    pkg: &mut Package<R>,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, ExtractError> {
    let presentation = match pkg.read_part(PRESENTATION_PART) {
        Ok(Some(xml)) => xml,
        Ok(None) => return Err(malformed(format!("missing {PRESENTATION_PART}"))),
        Err(PartError::OverLimit(d) | PartError::Unreadable(d)) => return Err(malformed(d)),
    };

    // Les rId de <p:sldIdLst>, dans l'ordre de la liste.
    let mut rids = Vec::new();
    let mut reader = Reader::from_str(&presentation);
    let mut saw_root = false;
    let mut list_broken = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "presentation" => saw_root = true,
                "sldId" => {
                    // `id` (numérique) et `r:id` (référence de relation) partagent un
                    // nom local ; la référence est celle qui porte un préfixe.
                    let reference = e.attributes().flatten().find_map(|a| {
                        a.key
                            .as_ref()
                            .ends_with(":id")
                            .then(|| {
                                a.normalized_value(XmlVersion::Implicit1_0)
                                    .ok()
                                    .map(|v| v.into_owned())
                            })
                            .flatten()
                    });
                    if let Some(rid) = reference {
                        rids.push(rid);
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                warnings.push(format!(
                    "{PRESENTATION_PART} is malformed ({err}); slide order falls back to part names"
                ));
                rids.clear();
                list_broken = true;
                break;
            }
        }
    }
    if !saw_root {
        return Err(malformed(format!(
            "{PRESENTATION_PART} has no <p:presentation> root element"
        )));
    }

    let rels: HashMap<String, String> = match pkg.read_part(PRESENTATION_RELS_PART) {
        Ok(Some(xml)) => parse_relationships(&xml)
            .into_iter()
            .filter(|r| !r.external)
            .map(|r| (r.id, resolve_target("ppt", &r.target)))
            .collect(),
        Ok(None) => HashMap::new(),
        Err(PartError::OverLimit(d)) => return Err(malformed(d)),
        Err(PartError::Unreadable(d)) => {
            warnings.push(format!("{PRESENTATION_RELS_PART} unreadable: {d}"));
            HashMap::new()
        }
    };

    let mut ordered = Vec::with_capacity(rids.len());
    let mut unresolved = 0usize;
    for rid in &rids {
        match rels.get(rid) {
            Some(name) if pkg.has(name) => ordered.push(name.clone()),
            _ => unresolved += 1,
        }
    }
    if !ordered.is_empty() {
        if unresolved > 0 {
            warnings.push(format!(
                "{unresolved} entr(y/ies) of the slide list point to no slide part and were skipped"
            ));
        }
        return Ok(ordered);
    }

    // Repli : chaque `ppt/slides/slideN.xml`, dans l'ordre NUMÉRIQUE (2 avant 10).
    let mut numbered: Vec<(u64, String)> = pkg
        .archive
        .file_names()
        .flatten()
        .filter_map(|n| {
            let num = n
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse::<u64>()
                .ok()?;
            Some((num, n.to_string()))
        })
        .collect();
    numbered.sort();
    if !numbered.is_empty() && !list_broken {
        warnings.push(
            "the slide list in ppt/presentation.xml could not be resolved; slides are in \
             part-name (numeric) order, which may not be the presentation order"
                .to_string(),
        );
    }
    Ok(numbered.into_iter().map(|(_, n)| n).collect())
}

/// La partie de notes rattachée à `slide_part`, par les relations de la diapositive.
/// `Err` seulement pour une borne dépassée ; une relation illisible = pas de notes.
fn notes_part_for<R: Read + Seek>(
    pkg: &mut Package<R>,
    slide_part: &str,
) -> Result<Option<String>, String> {
    let Some((dir, file)) = slide_part.rsplit_once('/') else {
        return Ok(None);
    };
    let rels_name = format!("{dir}/_rels/{file}.rels");
    let xml = match pkg.read_part(&rels_name) {
        Ok(Some(xml)) => xml,
        Ok(None) | Err(PartError::Unreadable(_)) => return Ok(None),
        Err(PartError::OverLimit(d)) => return Err(d),
    };
    Ok(parse_relationships(&xml)
        .into_iter()
        .find(|r| !r.external && r.kind.ends_with("/notesSlide"))
        .map(|r| resolve_target(dir, &r.target)))
}

/// Le texte d'une diapositive ou d'une page de notes, un paragraphe par ligne.
///
/// `Err` quand la partie n'est pas du XML dès le départ ; une coupure en cours de
/// route garde ce qui a été lu et le dit. `part` ne sert qu'aux messages.
fn shape_text(
    xml: &str,
    mode: Mode,
    part: &str,
    warnings: &mut Vec<String>,
) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    // Les espaces significatives sont aux bords des runs : rogner collerait les
    // mots d'un run au suivant.
    reader.config_mut().trim_text(false);

    let mut text = String::new();
    let mut in_text_node = false;
    // Le texte d'un <a:fld type="slidenum"> est le glyphe « ‹#› » de l'espace réservé.
    let mut skip_field = false;
    // La forme en cours : où elle commence dans `text`, et quel espace réservé c'est.
    let mut shape_start: Option<usize> = None;
    let mut placeholder: Option<String> = None;
    let mut depth = 0usize;
    let mut saw_root = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                match e.local_name().as_ref() {
                    "sld" | "notes" => saw_root = true,
                    "sp" if shape_start.is_none() => {
                        shape_start = Some(text.len());
                        placeholder = None;
                    }
                    "ph" => placeholder = Some(attr(&e, "type").unwrap_or_else(|| "obj".into())),
                    "fld" => {
                        skip_field =
                            attr(&e, "type").is_some_and(|t| t.eq_ignore_ascii_case("slidenum"));
                    }
                    "t" if !skip_field => in_text_node = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "sld" | "notes" => saw_root = true,
                "ph" => placeholder = Some(attr(&e, "type").unwrap_or_else(|| "obj".into())),
                "br" => text.push('\n'),
                _ => {}
            },
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                match e.local_name().as_ref() {
                    "t" => in_text_node = false,
                    "fld" => skip_field = false,
                    "p" => text.push('\n'),
                    // Cellule de tableau : tabulation ; rangée : saut de ligne. Le saut
                    // que le paragraphe de la cellule a déjà émis est remplacé.
                    "tc" => {
                        trim_trailing(&mut text, '\n');
                        text.push('\t');
                    }
                    "tr" => {
                        trim_trailing(&mut text, '\t');
                        text.push('\n');
                    }
                    "sp" => {
                        if let Some(start) = shape_start.take() {
                            let keep = match (mode, placeholder.as_deref()) {
                                (Mode::Slide, Some("sldNum" | "ftr" | "dt")) => false,
                                (Mode::Slide, _) => true,
                                (Mode::Notes, Some("body")) => true,
                                (Mode::Notes, _) => false,
                            };
                            if !keep {
                                text.truncate(start);
                            }
                        }
                        placeholder = None;
                    }
                    _ => {}
                }
            }
            // quick-xml 0.42 livre le texte déjà validé en `str`.
            Ok(Event::Text(e)) if in_text_node => text.push_str(&e),
            // quick-xml ne résout pas les entités : voir `docx.rs`.
            Ok(Event::GeneralRef(e)) if in_text_node => {
                let name: &str = &e;
                match quick_xml::escape::unescape(&format!("&{name};")) {
                    Ok(resolved) => text.push_str(&resolved),
                    Err(_) => warnings.push(format!(
                        "{part}: dropped an entity reference this parser cannot resolve: &{name};"
                    )),
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                if text.is_empty() {
                    return Err(format!("{part} is not well-formed XML: {err}"));
                }
                warnings.push(format!(
                    "{part} is malformed at byte {}; kept the {} bytes read before it: {err}",
                    reader.buffer_position(),
                    text.len()
                ));
                depth = 0;
                break;
            }
        }
    }
    if !saw_root && text.is_empty() {
        return Err(format!("{part} has no <p:sld> or <p:notes> root element"));
    }
    if depth > 0 {
        warnings.push(format!(
            "{part} ends with {depth} element(s) unclosed: the part is truncated \
             and the text stops where it was cut"
        ));
    }

    text.truncate(text.trim_end_matches(['\n', '\t']).len());
    Ok(text)
}

fn trim_trailing(text: &mut String, ch: char) {
    let len = text.trim_end_matches(ch).len();
    text.truncate(len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::ExtractorRegistry;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_of(entries: &[(String, String)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            w.start_file(name.as_str(), SimpleFileOptions::default())
                .unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

    fn paragraphs(texts: &[&str]) -> String {
        texts
            .iter()
            .map(|t| format!("<a:p><a:r><a:t>{t}</a:t></a:r></a:p>"))
            .collect()
    }

    /// Une forme portant `body` ; `ph` est un type d'espace réservé optionnel.
    fn shape(ph: Option<&str>, body: &str) -> String {
        let ph = ph.map_or(String::new(), |t| {
            format!(r#"<p:nvPr><p:ph type="{t}"/></p:nvPr>"#)
        });
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="s"/><p:cNvSpPr/>{ph}</p:nvSpPr><p:txBody>{body}</p:txBody></p:sp>"#
        )
    }

    fn slide_xml(shapes: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><p:sld {NS}><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#
        )
    }

    fn notes_xml(shapes: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><p:notes {NS}><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:notes>"#
        )
    }

    fn simple(t: &str) -> String {
        slide_xml(&shape(None, &paragraphs(&[t])))
    }

    /// `slides` : `(numéro de partie, xml, notes optionnelles)`, dans l'ordre où ils
    /// doivent apparaître. Les parties sont écrites dans l'archive dans l'ordre
    /// **inverse**, pour que rien ne dépende de l'ordre des entrées.
    fn deck(slides: &[(u32, String, Option<String>)]) -> Vec<u8> {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut ids = String::new();
        let mut pres_rels = String::new();
        for (i, (n, slide, notes)) in slides.iter().enumerate() {
            ids.push_str(&format!(
                r#"<p:sldId id="{}" r:id="rId{}"/>"#,
                256 + i,
                100 + i
            ));
            pres_rels.push_str(&format!(
                r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{n}.xml"/>"#,
                100 + i
            ));
            entries.push((format!("ppt/slides/slide{n}.xml"), slide.clone()));
            if let Some(notes) = notes {
                // Numérotées autrement que la diapositive, exprès : le lien doit
                // venir des relations, pas d'un appariement de chiffres.
                let m = n + 40;
                entries.push((format!("ppt/notesSlides/notesSlide{m}.xml"), notes.clone()));
                entries.push((
                    format!("ppt/slides/_rels/slide{n}.xml.rels"),
                    format!(
                        r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide{m}.xml"/></Relationships>"#
                    ),
                ));
            }
        }
        entries.reverse();
        entries.push((
            PRESENTATION_PART.into(),
            format!(
                r#"<?xml version="1.0"?><p:presentation {NS}><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"#
            ),
        ));
        entries.push((
            PRESENTATION_RELS_PART.into(),
            format!(r#"<Relationships xmlns="{REL_NS}">{pres_rels}</Relationships>"#),
        ));
        zip_of(&entries)
    }

    fn assert_tiles(out: &ExtractedText) {
        let mut at = 0;
        for p in &out.pages {
            assert_eq!(p.start, at, "pages must be contiguous: {:?}", out.pages);
            at = p.end;
        }
        assert_eq!(at, out.text.len(), "pages must cover the whole text");
    }

    #[test]
    fn slides_become_numbered_sections_of_paragraphs() {
        let bytes = deck(&[
            (
                1,
                slide_xml(&shape(Some("title"), &paragraphs(&["Revue trimestrielle"]))),
                None,
            ),
            (
                2,
                slide_xml(&shape(
                    None,
                    &paragraphs(&["Revenus en hausse", "Attrition en baisse"]),
                )),
                None,
            ),
        ]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.format, DocumentFormat::Pptx);
        assert_eq!(
            out.text,
            "## Slide 1\nRevue trimestrielle\n\n## Slide 2\nRevenus en hausse\nAttrition en baisse\n"
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(out.pages.len(), 2);
        assert_tiles(&out);
        assert_eq!(out.page_of(out.text.find("Revenus").unwrap()), Some(2));
    }

    // Le piège : slide10.xml est la première diapositive, slide2.xml la deuxième,
    // slide1.xml la troisième. Ni l'ordre lexicographique (1, 10, 2) ni l'ordre
    // numérique (1, 2, 10) des noms ne donne le bon résultat.
    #[test]
    fn slide_order_follows_the_presentation_not_the_part_names() {
        let bytes = deck(&[
            (10, simple("premier"), None),
            (2, simple("deuxième"), None),
            (1, simple("troisième"), None),
        ]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(
            out.text,
            "## Slide 1\npremier\n\n## Slide 2\ndeuxième\n\n## Slide 3\ntroisième\n"
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    // Sans liste résoluble, le repli est numérique (2 avant 10), et il est annoncé.
    #[test]
    fn unresolvable_slide_list_falls_back_to_numeric_part_order_with_a_warning() {
        let entries = vec![
            (
                PRESENTATION_PART.to_string(),
                format!(
                    r#"<p:presentation {NS}><p:sldIdLst><p:sldId id="256" r:id="rIdX"/></p:sldIdLst></p:presentation>"#
                ),
            ),
            ("ppt/slides/slide10.xml".into(), simple("dix")),
            ("ppt/slides/slide2.xml".into(), simple("deux")),
        ];
        let out = PptxExtractor.extract(&zip_of(&entries)).unwrap();
        assert_eq!(out.text, "## Slide 1\ndeux\n\n## Slide 2\ndix\n");
        assert!(
            out.warnings.iter().any(|w| w.contains("numeric")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn speaker_notes_follow_their_slide_marked_as_notes() {
        let notes = notes_xml(&format!(
            "{}{}{}",
            shape(Some("sldImg"), ""),
            shape(Some("body"), &paragraphs(&["Commencer par l'attrition"])),
            shape(Some("sldNum"), &paragraphs(&["7"])),
        ));
        let bytes = deck(&[
            (1, simple("Titre"), Some(notes)),
            (2, simple("Suite"), None),
        ]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(
            out.text,
            "## Slide 1\nTitre\n\n### Notes\nCommencer par l'attrition\n\n## Slide 2\nSuite\n"
        );
        // Les notes appartiennent à la page de leur diapositive.
        assert_eq!(out.page_of(out.text.find("Commencer").unwrap()), Some(1));
        assert_tiles(&out);
    }

    #[test]
    fn footers_dates_and_slide_numbers_are_dropped() {
        let shapes = format!(
            "{}{}{}{}",
            shape(None, &paragraphs(&["Vrai contenu"])),
            shape(Some("ftr"), &paragraphs(&["Confidentiel"])),
            shape(Some("dt"), &paragraphs(&["2026-01-01"])),
            shape(Some("sldNum"), &paragraphs(&["3"])),
        );
        let body = r#"<a:p><a:r><a:t>Page </a:t></a:r><a:fld id="{1}" type="slidenum"><a:t>‹#›</a:t></a:fld></a:p>"#;
        let bytes = deck(&[(1, slide_xml(&(shapes + &shape(None, body))), None)]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "## Slide 1\nVrai contenu\nPage \n");
    }

    #[test]
    fn tables_breaks_entities_and_unicode() {
        let cell = |t: &str| format!("<a:tc><a:txBody>{}</a:txBody></a:tc>", paragraphs(&[t]));
        let row = |a: &str, b: &str| format!("<a:tr>{}{}</a:tr>", cell(a), cell(b));
        let table = format!(
            "<p:graphicFrame><a:graphic><a:graphicData><a:tbl>{}{}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>",
            row("Poste", "Coût"),
            row("Licences", "1200"),
        );
        let body =
            "<a:p><a:r><a:t>R&amp;D</a:t></a:r><a:br/><a:r><a:t>café 日本語</a:t></a:r></a:p>";
        let bytes = deck(&[(1, slide_xml(&(table + &shape(None, body))), None)]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(
            out.text,
            "## Slide 1\nPoste\tCoût\nLicences\t1200\nR&D\ncafé 日本語\n"
        );
    }

    #[test]
    fn an_empty_slide_keeps_its_page_and_an_empty_deck_says_so() {
        let bytes = deck(&[(1, slide_xml(""), None), (2, simple("x"), None)]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "## Slide 1\n\n## Slide 2\nx\n");
        assert_tiles(&out);
        assert_eq!(out.page_of(out.text.find('x').unwrap()), Some(2));

        let blank = PptxExtractor
            .extract(&deck(&[(1, slide_xml(""), None)]))
            .unwrap();
        assert!(
            blank.warnings.iter().any(|w| w.contains("no text")),
            "{:?}",
            blank.warnings
        );
    }

    #[test]
    fn a_broken_slide_keeps_its_page_and_the_rest_is_read() {
        let bytes = deck(&[
            (1, "not xml <<<".into(), None),
            (2, simple("lisible"), None),
        ]);
        let out = PptxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "## Slide 1\n\n## Slide 2\nlisible\n");
        assert!(
            out.warnings.iter().any(|w| w.contains("slide 1 skipped")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn a_zip_bomb_slide_is_refused_with_the_limit_named() {
        let mut xml = slide_xml("");
        xml.push_str(&" ".repeat(MAX_PART_BYTES as usize + 1));
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        w.start_file(PRESENTATION_PART, opts).unwrap();
        w.write_all(
            format!(r#"<p:presentation {NS}><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#)
                .as_bytes(),
        )
        .unwrap();
        w.start_file(PRESENTATION_RELS_PART, opts).unwrap();
        w.write_all(format!(r#"<Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="x/slide" Target="slides/slide1.xml"/></Relationships>"#).as_bytes()).unwrap();
        w.start_file("ppt/slides/slide1.xml", opts).unwrap();
        w.write_all(xml.as_bytes()).unwrap();
        let bomb = w.finish().unwrap().into_inner();
        match PptxExtractor.extract(&bomb) {
            Err(ExtractError::Malformed {
                format: "PPTX",
                detail,
            }) => {
                assert!(detail.contains("67108864"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lowered_limits_are_enforced_as_errors_not_truncation() {
        let bytes = deck(&[
            (1, simple("a"), None),
            (2, simple("b"), None),
            (3, simple("c"), None),
        ]);
        let cases = [
            (
                Limits {
                    max_slides: 2,
                    ..Limits::default()
                },
                "2-slide limit",
            ),
            (
                Limits {
                    max_entries: 2,
                    ..Limits::default()
                },
                "2-entry limit",
            ),
            (
                Limits {
                    max_text_bytes: 20,
                    ..Limits::default()
                },
                "20-byte limit",
            ),
            // Le total ne compte que des octets réellement décompressés.
            (
                Limits {
                    max_total_bytes: 600,
                    ..Limits::default()
                },
                "total limit",
            ),
            (
                Limits {
                    max_part_bytes: 50,
                    ..Limits::default()
                },
                "50-byte limit",
            ),
        ];
        for (limits, needle) in cases {
            match extract_with(&bytes, limits) {
                Err(ExtractError::Malformed {
                    format: "PPTX",
                    detail,
                }) => {
                    assert!(detail.contains(needle), "{needle}: {detail}")
                }
                other => panic!("{needle}: {other:?}"),
            }
        }
    }

    #[test]
    fn claims_a_deck_by_content_and_refuses_other_zips() {
        let bytes = deck(&[(1, simple("a"), None)]);
        assert!(PptxExtractor.can_handle(&FormatProbe::new(&bytes, Some("deck.bin"))));
        assert!(PptxExtractor.can_handle(&FormatProbe::new(&bytes, None)));
        let jar = zip_of(&[(
            "META-INF/MANIFEST.MF".into(),
            "Manifest-Version: 1.0".into(),
        )]);
        assert!(!PptxExtractor.can_handle(&FormatProbe::new(&jar, Some("app.jar"))));
        let xlsx = zip_of(&[("xl/workbook.xml".into(), "<workbook/>".into())]);
        assert!(!PptxExtractor.can_handle(&FormatProbe::new(&xlsx, Some("a.pptx"))));
        assert!(!PptxExtractor.can_handle(&FormatProbe::new(b"plain", Some("a.pptx"))));
    }

    #[test]
    fn broken_presentations_are_malformed_never_a_panic() {
        let full = deck(&[(1, simple("a"), None)]);
        let cases: Vec<Vec<u8>> = vec![
            zip_of(&[(PRESENTATION_PART.into(), "not xml".into())]),
            zip_of(&[("ppt/slides/slide1.xml".into(), simple("orphan"))]),
            b"PK\x03\x04garbage".to_vec(),
            full[..full.len() / 2].to_vec(),
            full[..full.len() - 1].to_vec(),
        ];
        for (i, bytes) in cases.iter().enumerate() {
            match PptxExtractor.extract(bytes) {
                Err(ExtractError::Malformed { format: "PPTX", .. }) => {}
                Ok(out) => assert!(!out.warnings.is_empty(), "case {i}: {out:?}"),
                Err(other) => panic!("case {i}: {other:?}"),
            }
        }
    }

    #[test]
    fn resolves_relationship_targets_against_the_owning_directory() {
        assert_eq!(
            resolve_target("ppt/slides", "../notesSlides/n.xml"),
            "ppt/notesSlides/n.xml"
        );
        assert_eq!(resolve_target("ppt", "slides/s.xml"), "ppt/slides/s.xml");
        assert_eq!(
            resolve_target("ppt", "/ppt/slides/s.xml"),
            "ppt/slides/s.xml"
        );
        assert_eq!(resolve_target("ppt", "../../../x.xml"), "x.xml");
    }

    #[test]
    fn the_registry_routes_a_deck_here_whatever_its_name() {
        let reg = ExtractorRegistry::with_builtins();
        assert!(reg.supported_formats().contains(&DocumentFormat::Pptx));
        let bytes = deck(&[(1, simple("routé"), None)]);
        for name in [Some("a.bin"), Some("a.docx"), Some("a.xlsx"), None] {
            let out = reg.extract(&bytes, name).unwrap();
            assert_eq!(out.format, DocumentFormat::Pptx, "{name:?}");
            assert!(out.text.contains("routé"));
        }
    }
}
