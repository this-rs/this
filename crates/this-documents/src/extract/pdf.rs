//! PDF, lu **page par page** pour qu'une citation puisse dire « page 7 ».
//!
//! Derrière la feature `pdf` : `pdf_oxide` est un outillage PDF complet, et un
//! consommateur qui ne lit que du Markdown ne doit pas compiler ses 36
//! dépendances.
//!
//! ## Pourquoi page par page et pas `extract_all_text()`
//!
//! `pdf_oxide` offre un appel unique qui rend tout le texte. Il n'est pas utilisé
//! ici : il rend une seule chaîne, sans la mémoire de l'endroit où chaque page
//! finissait, et cette mémoire est la raison d'être de ce module.
//! [`ExtractedText::pages`] doit **paver** le texte pour que
//! [`ExtractedText::page_of`] transforme un offset rendu par l'aligneur en un
//! numéro de page qu'un humain peut ouvrir. Extraire page par page et mesurer le
//! texte au fur et à mesure qu'il est ajouté est la seule façon honnête de
//! construire ces tranches ; tout le reste serait reconstituer des frontières de
//! page à partir d'une chaîne qui ne les a plus.
//!
//! ## Aucune page n'est jetée
//!
//! Une page dont le texte est illisible occupe quand même un intervalle — vide —
//! plus un avertissement. La sauter renuméroterait toutes les pages suivantes, et
//! une citation qui pointe sur la mauvaise page est pire qu'une citation qui
//! pointe sur une page qu'on sait illisible.
//!
//! ## Un scan est une erreur, pas un succès vide
//!
//! Voir [`PdfExtractor::extract`] : un PDF sans couche texte nulle part rend
//! [`ExtractError::Malformed`] et non un [`ExtractedText`] vide, parce qu'en aval
//! rien ne distingue une extraction vide d'un document réellement blanc — les
//! deux se découpent en rien et s'alignent sur rien, silencieusement.
//!
//! ## Le parseur est une surface hostile
//!
//! Il tourne sur des octets envoyés par un tiers et fait de l'arithmétique
//! d'index sur des offsets qu'il n'a pas choisis. Toutes ses fonctions rendent
//! `Result` et il est bien testé, mais une panique ici emporterait la tâche qui
//! traite l'envoi au lieu de faire échouer un document. [`catch_unwind`] ramène
//! le pire cas au même `Malformed` que n'importe quel autre échec de lecture.
//!
//! ## Plafonds, jamais de troncature muette
//!
//! - pages lues : [`MAX_PAGES`] (10 000) ;
//! - texte produit : [`MAX_TEXT_BYTES`] (16 Mio) ;
//! - avertissements gardés : [`MAX_WARNINGS`] (32, ligne de résumé comprise).
//!
//! Dépasser un plafond rend `Malformed` en nommant la limite, et non un document
//! tronqué avec un avertissement : un texte coupé à 16 Mio passerait pour entier
//! dans l'index, ce qu'aucun vrai document n'approche (mille pages ≈ 2,5 Mio de
//! prose). Ce qui les dépasse est une bombe, pas un rapport.
//!
//! ## Chiffrement
//!
//! Un PDF chiffré que le mot de passe utilisateur vide n'ouvre pas est refusé. La
//! version par défaut de `pdf_oxide` embarque `legacy-crypto` (RC4/MD5) ; ici elle
//! est désactivée pour ne pas tirer `md-5`, donc un PDF d'avant AES-2005 tombe
//! dans le même refus, et le message le dit. Le cas à éviter n'est pas l'erreur,
//! c'est le **succès** : sans ce contrôle l'extraction rend des pages blanches et
//! l'appelant stocke un document vide.

use std::panic::{AssertUnwindSafe, catch_unwind};

use pdf_oxide::PdfDocument;

use super::{DocumentFormat, ExtractError, ExtractedText, FormatProbe, TextExtractor};
use crate::ByteInterval;

/// Inséré entre deux pages pour que le texte de la page N ne coule pas dans celui
/// de la page N+1.
///
/// Une ligne vide plutôt qu'un saut de page (`\x0c`) : le découpeur en aval traite
/// une ligne vide comme une frontière de paragraphe, donc une fin de page devient
/// un endroit naturel pour couper. Un `\x0c` serait un caractère invisible au
/// milieu d'un morceau.
const PAGE_SEPARATOR: &str = "\n\n";

/// Plafond du nombre de pages lues.
///
/// Un PDF peut déclarer un `/Count` arbitraire ; le croire sans borne, c'est
/// laisser un fichier de 3 Kio demander une boucle de plusieurs millions de tours.
/// 10 000 pages est déjà au-delà de tout document lisible par un humain.
pub const MAX_PAGES: usize = 10_000;

/// Plafond du texte produit.
///
/// Même valeur que l'extracteur DOCX : une limite différente par format ferait
/// deux définitions de « document trop gros » dans un crate qui n'en veut qu'une.
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Plafond des avertissements conservés, **ligne de résumé comprise**.
///
/// `pdf_oxide` émet un diagnostic par police non décodée, flux cassé ou objet
/// reconstruit : un fichier de 500 pages mal produit en génère des milliers. Les
/// premiers nomment le problème, les suivants rendent la liste illisible — et
/// cette structure est sérialisée puis stockée.
const MAX_WARNINGS: usize = 32;

/// Plafond des diagnostics du parseur recopiés tels quels.
///
/// Plus serré que [`MAX_WARNINGS`] parce que ces lignes viennent d'un tiers : ce
/// sont les nôtres qui doivent survivre en priorité à la coupe.
const MAX_LIBRARY_WARNINGS: usize = 16;

/// Les deux plafonds de volume, en paramètre pour que les tests puissent les
/// abaisser et voir la limite se déclencher pour de vrai, sur un vrai PDF.
struct Limits {
    max_pages: usize,
    max_text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pages: MAX_PAGES,
            max_text_bytes: MAX_TEXT_BYTES,
        }
    }
}

pub struct PdfExtractor;

impl TextExtractor for PdfExtractor {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Pdf
    }

    /// Les magic bytes seulement.
    ///
    /// `%PDF-` est l'en-tête que la norme impose (ISO 32000-1 §7.5.2). L'extension
    /// n'est délibérément pas consultée : un `.pdf` sur un ZIP est un mensonge
    /// qu'on répéterait, et un PDF nommé `rapport.txt` reste un PDF.
    fn can_handle(&self, probe: &FormatProbe<'_>) -> bool {
        probe.starts_with(b"%PDF-")
    }

    /// Extrait le texte et les frontières de pages.
    ///
    /// # Erreurs
    ///
    /// [`ExtractError::Malformed`] quand le fichier ne se lit pas, quand il est
    /// protégé par un mot de passe, ou quand il ne porte **aucune** couche texte.
    /// Ce dernier cas est un choix, pas un oubli :
    ///
    /// - un scan **sans** texte extractible est une erreur : le détail dit combien
    ///   de pages n'ont pas de couche texte, pour qu'un appelant qui a un chemin
    ///   OCR y route le document au lieu de deviner. Rendre `Ok` avec une chaîne
    ///   vide mettrait un document vide dans le découpeur, et toutes les
    ///   vérifications en aval passeraient sur rien ;
    /// - un document dont **certaines** pages sont des images rend `Ok`, avec un
    ///   avertissement nommant chaque page concernée. Le texte qui existe vaut
    ///   d'être gardé, les tranches de pages restent justes, et l'avertissement dit
    ///   exactement ce qui manque.
    fn extract(&self, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
        // Pourquoi `AssertUnwindSafe` : la garantie qu'il désactive est celle qui
        // dit « rien d'observable n'est resté dans un état incohérent après la
        // panique ». Ici c'est vrai par construction — `PdfDocument` naît et meurt
        // dans la fermeture, tout ce qui est muté (`text`, `pages`, `warnings`)
        // aussi, et `PdfExtractor` est une structure vide (`&self` n'est pas
        // touché). Rien ne traverse la frontière du `catch_unwind` : après une
        // panique, le seul objet récupéré est le message, et la valeur de retour
        // est `Malformed`. Le marquer `UnwindSafe` à la main serait le même
        // mensonge avec plus de code ; l'assertion est donc portée ici, avec sa
        // raison, et pas dans une implémentation générique.
        catch_unwind(AssertUnwindSafe(|| {
            extract_within(bytes, Limits::default())
        }))
        .unwrap_or_else(|payload| {
            Err(ExtractError::Malformed {
                format: "PDF",
                detail: format!("PDF parser panicked: {}", panic_message(&payload)),
            })
        })
    }
}

fn extract_within(bytes: &[u8], limits: Limits) -> Result<ExtractedText, ExtractError> {
    let doc = PdfDocument::from_bytes(bytes.to_vec()).map_err(|e| malformed(&e))?;

    // Contrôlé avant toute lecture. `pdf_oxide` a déjà essayé le mot de passe
    // utilisateur vide en ouvrant le fichier : arriver ici non authentifié veut
    // dire qu'un vrai mot de passe est requis — ou que le fichier est chiffré en
    // RC4/MD5, que ce build ne fournit pas (voir les docs du module). Dans les
    // deux cas l'extraction rendrait tranquillement des pages blanches ; on le dit.
    if doc.is_encrypted() && !doc.is_authenticated() {
        return Err(ExtractError::Malformed {
            format: "PDF",
            detail: "document is encrypted and the empty user password does not open it \
                     (a password is required, or it uses legacy RC4 encryption, which this \
                     build does not support)"
                .to_string(),
        });
    }

    let page_count = doc.page_count().map_err(|e| malformed(&e))?;
    if page_count == 0 {
        return Err(ExtractError::Malformed {
            format: "PDF",
            detail: "document declares no pages".to_string(),
        });
    }
    if page_count > limits.max_pages {
        return Err(ExtractError::Malformed {
            format: "PDF",
            detail: format!(
                "document declares {page_count} pages, over the {}-page limit; refusing to \
                 read it (a page count is attacker-controlled and a 3 KiB PDF can claim \
                 millions)",
                limits.max_pages
            ),
        });
    }

    let mut text = String::new();
    let mut pages: Vec<ByteInterval> = Vec::with_capacity(page_count);
    let mut warnings = CappedWarnings::new(MAX_WARNINGS);
    let mut pages_without_text = 0usize;

    for index in 0..page_count {
        let start = text.len();

        // `has_text_layer` est conservateur — il répond `true` quand il ne peut
        // pas inspecter la page — donc il distingue « scan en images » de
        // « l'extraction n'a rien rendu », deux problèmes différents pour
        // l'appelant même si les deux donnent une page vide.
        let image_only = doc.has_text_layer(index).map(|has| !has).unwrap_or(false);

        let page_text = match doc.extract_text(index) {
            Ok(t) => t,
            Err(e) => {
                // La page garde son intervalle ; seul son contenu est perdu. Voir
                // les docs du module sur la renumérotation.
                warnings.push(format!("page {}: text extraction failed: {e}", index + 1));
                String::new()
            }
        };

        if page_text.trim().is_empty() {
            pages_without_text += 1;
            if image_only {
                warnings.push(format!(
                    "page {}: no text layer (image-only page — OCR would be required)",
                    index + 1
                ));
            } else {
                // Le texte qui existe est gardé : une page sans texte par accident
                // (contenu vide, police absente) n'est pas une raison de jeter le
                // document, mais elle n'est pas silencieuse non plus.
                warnings.push(format!(
                    "page {}: no text was extracted from this page",
                    index + 1
                ));
            }
        }

        text.push_str(&page_text);
        // Le plafond est vérifié après chaque page, pas à la fin : c'est ce qui
        // permet de s'arrêter avant d'avoir tout mis en mémoire, et de nommer la
        // page où la limite a été franchie.
        if text.len() > limits.max_text_bytes {
            return Err(ExtractError::Malformed {
                format: "PDF",
                detail: format!(
                    "text passed the {}-byte limit on page {} ({page_count} page(s) declared); \
                     refusing to produce a truncated document",
                    limits.max_text_bytes,
                    index + 1
                ),
            });
        }
        // Le séparateur appartient à la page qui le précède, donc les intervalles
        // pavent `text` bout à bout sans qu'aucun offset ne tombe hors de toutes
        // les pages.
        if index + 1 < page_count {
            text.push_str(PAGE_SEPARATOR);
        }
        pages.push(ByteInterval::new(start, text.len()));
    }

    if pages_without_text == page_count {
        return Err(ExtractError::Malformed {
            format: "PDF",
            detail: format!(
                "no extractable text on any of the {page_count} page(s) — the document is \
                 probably a scan and needs OCR, which this extractor does not perform"
            ),
        });
    }

    // Diagnostics que le parseur a enregistrés en se rattrapant : polices cassées,
    // flux endommagés, objets reconstruits depuis une xref déduite. Ils disent que
    // le texte ci-dessus est un meilleur effort, ce qui est exactement le rôle de
    // `warnings`. La surface structurée est préférée à `take_warnings()` parce
    // qu'elle porte le numéro de page, et un diagnostic sans page ne s'exploite pas
    // plus qu'un offset sans page.
    let library_warnings = doc.take_structured_warnings();
    let dropped = library_warnings.len().saturating_sub(MAX_LIBRARY_WARNINGS);
    for warning in library_warnings.into_iter().take(MAX_LIBRARY_WARNINGS) {
        let line = match warning.page {
            Some(p) => format!(
                "page {}: {}: {}",
                p + 1,
                warning.category.as_str(),
                warning.message
            ),
            None => format!(
                "document: {}: {}",
                warning.category.as_str(),
                warning.message
            ),
        };
        warnings.push(line);
    }
    if dropped > 0 {
        warnings.push(format!("... and {dropped} further parser diagnostics"));
    }

    Ok(ExtractedText {
        text,
        format: DocumentFormat::Pdf,
        pages,
        warnings: warnings.finish(),
    })
}

fn malformed(error: &pdf_oxide::Error) -> ExtractError {
    ExtractError::Malformed {
        format: "PDF",
        // Le `Display` de `pdf_oxide` porte l'offset en octets et la raison, ce qui
        // est ce qui rend exploitable un rapport d'envoi corrompu.
        detail: error.to_string(),
    }
}

/// Meilleur effort sur la charge d'une panique, qui est `&str` ou `String` en
/// pratique.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown payload".to_string()
    }
}

/// Liste d'avertissements à capacité bornée.
///
/// Le dernier emplacement est réservé à la ligne de résumé : peu importe le nombre
/// de problèmes rencontrés, celle qui dit « et N autres » trouve toujours sa place.
struct CappedWarnings {
    kept: Vec<String>,
    cap: usize,
    suppressed: usize,
}

impl CappedWarnings {
    fn new(cap: usize) -> Self {
        Self {
            kept: Vec::new(),
            cap,
            suppressed: 0,
        }
    }

    fn push(&mut self, message: String) {
        // `cap - 1` : voir la docs de la structure. `saturating_sub` couvre le cas
        // dégénéré d'un plafond à 0 ou 1.
        if self.kept.len() < self.cap.saturating_sub(1) {
            self.kept.push(message);
        } else {
            self.suppressed += 1;
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.suppressed > 0 {
            self.kept
                .push(format!("... and {} further warnings", self.suppressed));
        }
        self.kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Échappe ce qui a un sens dans une chaîne littérale PDF (`(...)`).
    ///
    /// Les textes de test sont volontairement ASCII : `/Length` compte des octets
    /// et non des caractères, donc un accent rendrait le flux incohérent avec sa
    /// propre longueur.
    fn escape_pdf_string(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '(' | ')' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        out
    }

    /// Construit un PDF syntaxiquement valide en mémoire.
    ///
    /// Écrit à la main plutôt qu'en fixture binaire pour que les tests disent ce
    /// dont ils dépendent : `Some(texte)` est une page avec une police et un `Tj`,
    /// `None` est une page sans aucun `/Resources` — ce à quoi ressemble un scan en
    /// images pour un extracteur de texte.
    ///
    /// `encrypted` ajoute un dictionnaire `/Encrypt` (AES-256, V5/R6) référencé
    /// depuis le trailer **avant** que les offsets ne soient calculés : le fichier
    /// reste structurellement valide, donc un refus vient bien du chiffrement et
    /// pas d'une xref cassée.
    fn build_pdf(pages: &[Option<&str>], encrypted: bool) -> Vec<u8> {
        let n = pages.len();
        // Objets : 1 catalogue, 2 arbre de pages, 3 police, la page i en 4 + 2i et
        // son flux de contenu en 5 + 2i. Contigus, ce que la table xref exige.
        let kids: String = (0..n).map(|i| format!("{} 0 R ", 4 + 2 * i)).collect();
        let mut objects: Vec<(usize, Vec<u8>)> = vec![
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (
                2,
                format!("<< /Type /Pages /Kids [ {kids}] /Count {n} >>").into_bytes(),
            ),
            (
                3,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            ),
        ];

        for (i, page) in pages.iter().enumerate() {
            let page_id = 4 + 2 * i;
            let content_id = page_id + 1;
            let (resources, stream) = match page {
                Some(body) => (
                    "/Resources << /Font << /F1 3 0 R >> >>".to_string(),
                    format!(
                        "BT /F1 24 Tf 72 700 Td ({}) Tj ET\n",
                        escape_pdf_string(body)
                    ),
                ),
                None => (String::new(), "0 0 0 rg 72 72 200 200 re f\n".to_string()),
            };
            objects.push((
                page_id,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {resources} \
                     /Contents {content_id} 0 R >>"
                )
                .into_bytes(),
            ));
            let mut content = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
            content.extend_from_slice(stream.as_bytes());
            content.extend_from_slice(b"endstream");
            objects.push((content_id, content));
        }

        // L'objet de chiffrement vient après les pages : les identifiants des pages
        // ne bougent donc pas quand `encrypted` change, et la même fixture sert aux
        // deux cas.
        let encrypt_id = 4 + 2 * n;
        if encrypted {
            objects.push((
                encrypt_id,
                b"<< /Filter /Standard /V 5 /R 6 /Length 256 \
                  /O <0011223344556677889900112233445566778899001122334455667788990011> \
                  /U <9988776655443322110099887766554433221100998877665544332211009988> \
                  /P -3904 >>"
                    .to_vec(),
            ));
        }

        // Le commentaire binaire de la ligne 2 est ce que la norme recommande pour
        // qu'un agent de transfert traite le fichier comme binaire ; il fait aussi
        // ressembler la fixture à la sortie d'un vrai producteur.
        let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let max_id = objects.iter().map(|(id, _)| *id).max().unwrap_or(0);
        let mut offsets = vec![0usize; max_id + 1];
        for (id, body) in &objects {
            offsets[*id] = out.len();
            out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }

        let xref_offset = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", max_id + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        // L'entrée 0 est la tête de liste libre écrite ci-dessus ; les objets
        // 1..=max_id suivent dans l'ordre des identifiants, ce qui fait de la
        // contiguïté des identifiants une exigence.
        for offset in offsets.iter().skip(1) {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let extra = if encrypted {
            format!(
                " /Encrypt {encrypt_id} 0 R /ID [<01020304050607080910111213141516> \
                     <01020304050607080910111213141516>]"
            )
        } else {
            String::new()
        };
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R{extra} >>\nstartxref\n{xref_offset}\n%%EOF\n",
                max_id + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn claims_pdfs_by_magic_and_ignores_the_extension() {
        let pdf = build_pdf(&[Some("hello")], false);
        assert!(PdfExtractor.can_handle(&FormatProbe::new(&pdf, Some("report.txt"))));
        assert!(PdfExtractor.can_handle(&FormatProbe::new(&pdf, None)));
        // Un `.pdf` sur autre chose qu'un PDF ne doit pas être revendiqué :
        // l'extracteur de texte est un meilleur foyer qu'une erreur de parse.
        assert!(!PdfExtractor.can_handle(&FormatProbe::new(b"Not a PDF", Some("invoice.pdf"))));
        assert!(!PdfExtractor.can_handle(&FormatProbe::new(b"PK\x03\x04", Some("a.pdf"))));
    }

    #[test]
    fn extracts_text_from_a_single_page() {
        let pdf = build_pdf(&[Some("Hello from page one")], false);
        let out = PdfExtractor.extract(&pdf).unwrap();
        assert_eq!(out.format, DocumentFormat::Pdf);
        assert!(
            out.text.contains("Hello from page one"),
            "text was {:?}",
            out.text
        );
        assert_eq!(out.pages.len(), 1);
        assert_eq!(out.pages[0], ByteInterval::new(0, out.text.len()));
    }

    #[test]
    fn page_spans_tile_the_text_and_resolve_to_real_page_numbers() {
        // Le but de tout le module : un offset doit revenir en numéro de page.
        let pdf = build_pdf(
            &[Some("Alpha one"), Some("Beta two"), Some("Gamma three")],
            false,
        );
        let out = PdfExtractor.extract(&pdf).unwrap();

        assert_eq!(out.pages.len(), 3, "one interval per page");

        // Pavage : ni trou ni recouvrement, le premier part de 0, le dernier finit
        // à la fin du texte, et la concaténation des tranches redonne le texte.
        assert_eq!(out.pages[0].start, 0);
        assert_eq!(out.pages.last().unwrap().end, out.text.len());
        for pair in out.pages.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "pages must be contiguous");
        }
        let rejoined: String = out
            .pages
            .iter()
            .map(|p| p.slice(&out.text).expect("every interval must slice"))
            .collect();
        assert_eq!(
            rejoined, out.text,
            "the slices must rebuild the text exactly"
        );

        // Et tout offset du document tombe dans une page — ce que `page_of`
        // rendant `None` casserait pour l'aligneur.
        for offset in 0..out.text.len() {
            assert!(
                out.page_of(offset).is_some(),
                "offset {offset} fell outside every page"
            );
        }

        // Le marqueur de chaque page doit se trouver dans l'intervalle de sa page.
        for (i, marker) in ["Alpha", "Beta", "Gamma"].iter().enumerate() {
            let at = out
                .text
                .find(marker)
                .unwrap_or_else(|| panic!("{marker} missing from {:?}", out.text));
            assert_eq!(
                out.page_of(at),
                Some(i + 1),
                "{marker} should be reported as page {}",
                i + 1
            );
            let slice = out.pages[i].slice(&out.text).expect("interval must slice");
            assert!(slice.contains(marker), "page {} was {slice:?}", i + 1);
        }
    }

    #[test]
    fn a_page_without_a_text_layer_is_reported_but_does_not_renumber_the_others() {
        // La page 2 est une image. La page 3 doit rester la page 3.
        let pdf = build_pdf(&[Some("First page"), None, Some("Third page")], false);
        let out = PdfExtractor.extract(&pdf).unwrap();

        assert_eq!(out.pages.len(), 3, "the blank page keeps its interval");
        let third = out
            .text
            .find("Third page")
            .expect("third page text missing");
        assert_eq!(out.page_of(third), Some(3), "the blank page must not shift");

        assert!(
            out.warnings.iter().any(|w| w.starts_with("page 2:")),
            "the image-only page must be named in the warnings: {:?}",
            out.warnings
        );
        // La page vide est vide, pas décalée : son intervalle fait exactement le
        // séparateur.
        assert_eq!(out.pages[1].len(), PAGE_SEPARATOR.len());
    }

    #[test]
    fn a_fully_scanned_document_errors_rather_than_returning_an_empty_document() {
        // Choix documenté : un `ExtractedText` vide est indistinguable en aval d'un
        // document réellement blanc, il ne doit donc pas être la valeur de retour
        // d'un scan.
        let pdf = build_pdf(&[None, None], false);
        match PdfExtractor.extract(&pdf) {
            Err(ExtractError::Malformed { format, detail }) => {
                assert_eq!(format, "PDF");
                assert!(
                    detail.contains("scan") && detail.contains("OCR"),
                    "detail must point at OCR, was {detail:?}"
                );
            }
            other => panic!("expected Malformed for a text-free scan, got {other:?}"),
        }
    }

    #[test]
    fn a_truncated_pdf_is_malformed_without_panicking() {
        let pdf = build_pdf(&[Some("Some text")], false);
        // On coupe la table xref et le trailer : un envoi tronqué plausible.
        let truncated = &pdf[..pdf.len() / 2];
        match PdfExtractor.extract(truncated) {
            Err(ExtractError::Malformed { format, detail }) => {
                assert_eq!(format, "PDF");
                assert!(!detail.is_empty(), "detail must say something usable");
            }
            // Se rattraper sur un fichier tronqué est légitime — `pdf_oxide`
            // reconstruit la xref — mais seulement si une page en est revenue.
            Ok(out) => assert!(!out.pages.is_empty()),
            Err(other) => panic!("expected Malformed or recovery, got {other}"),
        }
    }

    #[test]
    fn garbage_behind_a_pdf_header_does_not_panic() {
        // Des octets qui passent `can_handle` puis n'ont aucun sens. Ce qui est
        // prouvé ici est l'absence de panique ; l'un ou l'autre résultat du parse
        // est acceptable.
        let cases: Vec<Vec<u8>> = vec![
            b"%PDF-1.7\n".to_vec(),
            b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\nbinary garbage\n".to_vec(),
            {
                let mut v = b"%PDF-1.5\n".to_vec();
                v.extend((0u8..=255).cycle().take(4096));
                v
            },
            b"%PDF-1.4\nxref\n0 99999999\ntrailer\n<< /Size 99999999 /Root 1 0 R >>\nstartxref\n999999999\n%%EOF".to_vec(),
        ];
        for bytes in cases {
            assert!(PdfExtractor.can_handle(&FormatProbe::new(&bytes, None)));
            match PdfExtractor.extract(&bytes) {
                Err(ExtractError::Malformed { format, .. }) => assert_eq!(format, "PDF"),
                Err(other) => panic!("expected Malformed, got {other}"),
                Ok(out) => assert!(
                    !out.text.trim().is_empty(),
                    "a successful parse must carry text"
                ),
            }
        }
    }

    #[test]
    fn an_encrypted_pdf_is_refused_instead_of_yielding_blank_pages() {
        // Un handler de sécurité standard que le mot de passe vide n'ouvre pas. Le
        // mode de défaillance contre lequel on se garde n'est pas l'erreur mais le
        // *succès* : sans ce contrôle, l'extraction rend des pages blanches et
        // l'appelant stocke un document vide.
        let pdf = build_pdf(&[Some("Secret text")], true);
        match PdfExtractor.extract(&pdf) {
            Err(ExtractError::Malformed { format, detail }) => {
                assert_eq!(format, "PDF");
                assert!(!detail.is_empty(), "detail was empty");
                assert!(
                    detail.to_lowercase().contains("encrypt")
                        || detail.to_lowercase().contains("password"),
                    "the detail must name encryption, was {detail:?}"
                );
            }
            other => panic!("expected Malformed for an encrypted PDF, got {other:?}"),
        }
    }

    #[test]
    fn the_page_cap_errors_instead_of_reading_forever() {
        // Le plafond réel est de 10 000 pages ; il est abaissé ici pour prouver que
        // le chemin de code se déclenche, sur un vrai PDF, et pas seulement que la
        // constante existe.
        let pdf = build_pdf(&[Some("one"), Some("two"), Some("three")], false);
        let err = extract_within(
            &pdf,
            Limits {
                max_pages: 2,
                ..Limits::default()
            },
        )
        .unwrap_err();
        match err {
            ExtractError::Malformed { format, detail } => {
                assert_eq!(format, "PDF");
                assert!(
                    detail.contains("3 pages") && detail.contains("2-page limit"),
                    "the detail must name both the count and the limit, was {detail:?}"
                );
            }
            other => panic!("expected Malformed over the page cap, got {other}"),
        }
    }

    #[test]
    fn the_text_cap_errors_instead_of_truncating_silently() {
        // Une troncature muette ferait passer un document coupé pour entier : le
        // plafond doit être une erreur qui nomme la limite.
        let pdf = build_pdf(&[Some("Alpha one"), Some("Beta two")], false);
        let err = extract_within(
            &pdf,
            Limits {
                max_text_bytes: 8,
                ..Limits::default()
            },
        )
        .unwrap_err();
        match err {
            ExtractError::Malformed { format, detail } => {
                assert_eq!(format, "PDF");
                assert!(
                    detail.contains("8-byte limit") && detail.contains("page 1"),
                    "the detail must name the limit and the page, was {detail:?}"
                );
                assert!(
                    detail.contains("truncated"),
                    "the detail must say it refused to truncate, was {detail:?}"
                );
            }
            other => panic!("expected Malformed over the text cap, got {other}"),
        }
    }

    #[test]
    fn the_warning_list_is_capped_and_says_how_many_were_dropped() {
        let mut warnings = CappedWarnings::new(4);
        for i in 1..=10 {
            warnings.push(format!("w{i}"));
        }
        let out = warnings.finish();
        // 4 de plafond : 3 lignes nommées + la ligne de résumé, qui a toujours sa
        // place même quand tout déborde.
        assert_eq!(out.len(), 4, "{out:?}");
        assert_eq!(out[..3], ["w1", "w2", "w3"]);
        assert_eq!(out[3], "... and 7 further warnings");

        // Rien à résumer : pas de ligne de résumé vide.
        let mut quiet = CappedWarnings::new(4);
        quiet.push("only".to_string());
        assert_eq!(quiet.finish(), ["only"]);
    }

    #[test]
    fn the_registry_routes_a_pdf_here_even_when_the_filename_lies() {
        use super::super::ExtractorRegistry;
        let pdf = build_pdf(&[Some("Routed correctly")], false);
        let out = ExtractorRegistry::with_builtins()
            .extract(&pdf, Some("notes.txt"))
            .expect("registry must hand a %PDF- blob to this extractor");
        assert_eq!(out.format, DocumentFormat::Pdf);
    }
}
