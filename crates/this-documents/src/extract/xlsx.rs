//! Excel (`.xlsx`, Office Open XML) : une page par feuille, dans l'ordre du classeur.
//!
//! Derrière la feature `xlsx`. Contrairement à `.docx`, un tableur ne vaut pas
//! d'être lu à la main : la valeur d'une cellule est éclatée entre le XML de la
//! feuille et la table des chaînes partagées, et nombres, booléens, dates et
//! erreurs sont typés. [`calamine`] s'en charge ; ce module décide seulement de
//! ce qu'on **émet**, et de ce qu'on accepte de décompresser.
//!
//! ## Forme du texte
//!
//! Une section par feuille, dans l'ordre du classeur (celui des onglets, que
//! l'utilisateur voit — pas celui des parties du ZIP) : un en-tête
//! `# Sheet: <nom>`, puis une ligne par rangée utilisée, cellules séparées par
//! des tabulations.
//!
//! ```text
//! # Sheet: Budget
//! Poste\tT1\tT2
//! Licences\t1200\t1300
//! ```
//!
//! - une cellule vide (ou chaîne vide) n'émet rien : la ligne s'arrête à la
//!   dernière cellule qui porte une valeur, sans tabulations de queue ;
//! - une cellule vide **entre** deux valeurs garde sa tabulation, pour que la
//!   valeur reste sous son en-tête de colonne ;
//! - une rangée entièrement vide n'émet pas de ligne ;
//! - une tabulation ou un saut de ligne *dans* une cellule devient une espace :
//!   sinon il décalerait les colonnes ou couperait la rangée.
//!
//! Chaque feuille est une entrée de [`ExtractedText::pages`] : un extrait trouvé
//! dans un tableur se rapporte « feuille 2 » comme un PDF « page 2 ». Les pages
//! tuilent le texte, et une feuille vide garde sa page (son seul en-tête), plus
//! un avertissement : la sauter renumérote les suivantes.
//!
//! ## Ce qui est borné, et comment
//!
//! calamine lit les parties avec `zip` sans aucune borne de décompression, et la
//! taille déclarée dans l'archive est écrite par celui qui l'a fabriquée. Avant
//! de lui confier les octets, `check_archive` **décompresse réellement** chaque
//! entrée vers un puits, en lecture bornée : au plus [`MAX_ARCHIVE_ENTRIES`]
//! entrées, [`MAX_PART_BYTES`] par entrée et [`MAX_TOTAL_BYTES`] au total. Une
//! bombe est coupée à la borne, sans rien allouer ; ce que calamine lira ensuite
//! est exactement ce qui vient d'être mesuré (même archive, même crate `zip`).
//!
//! La feuille est lue en **flux** de cellules
//! ([`calamine::Xlsx::worksheet_cells_reader`]) et non par `worksheet_range`, qui
//! alloue une grille dense de la taille du rectangle englobant : une valeur
//! égarée en `XFD1048576` en demanderait dix-sept milliards. Le flux est borné à
//! [`MAX_CELLS`] cellules inspectées (vides comprises : un million de cellules
//! stylées sans valeur coûtent aussi du temps), le texte à [`MAX_TEXT_BYTES`].
//!
//! Dépasser l'une de ces bornes rend `Malformed` en nommant la limite, jamais un
//! classeur tronqué : une moitié de tableur passerait pour entière dans l'index.
//! Seule exception, de présentation et non de volume : les colonnes au-delà de
//! [`MAX_COLUMNS`] sont ignorées avec un avertissement (une note égarée en
//! colonne 16 000 coûterait 16 000 tabulations par rangée).
//!
//! ## Le parseur est une surface hostile
//!
//! Comme pour le PDF : calamine fait de l'arithmétique sur des références de
//! cellule qu'il n'a pas choisies. [`catch_unwind`] ramène une panique au même
//! `Malformed` que tout autre échec de lecture.

use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};

use calamine::{Data, Reader, Xlsx};

use super::{DocumentFormat, ExtractError, ExtractedText, FormatProbe, TextExtractor};
use crate::ByteInterval;

/// La partie du classeur. Fixée par ECMA-376 : sans elle, ce n'est pas un `.xlsx`.
pub(crate) const WORKBOOK_PART: &str = "xl/workbook.xml";

/// Plafond des entrées de l'archive. Un classeur réel en compte quelques dizaines
/// (une par feuille, plus styles, thème, chaînes, relations) ; dix mille couvrent
/// un classeur de milliers de feuilles, et bornent le temps du contrôle.
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// Plafond du contenu **décompressé** d'une entrée. Même valeur que
/// `docx::MAX_DOCUMENT_XML_BYTES` : une feuille de cent mille rangées tient
/// dans quelques dizaines de Mio de SpreadsheetML.
pub const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;

/// Plafond du contenu décompressé de toute l'archive.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Plafond des cellules inspectées, sur tout le classeur.
pub const MAX_CELLS: u64 = 10_000_000;

/// Plafond du texte produit. Même valeur que DOCX et PDF : une seule définition
/// de « document trop gros » dans le crate.
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Colonne la plus large émise (la 1024ᵉ, `AMJ`). Au-delà, une valeur est bien
/// plus souvent un artefact de mise en forme qu'une donnée.
pub const MAX_COLUMNS: u32 = 1024;

const FORMAT: &str = "XLSX";

/// Les plafonds, en paramètre pour que les tests les abaissent et voient la
/// borne se déclencher pour de vrai, sur un vrai classeur.
#[derive(Clone, Copy)]
struct Limits {
    max_entries: usize,
    max_part_bytes: u64,
    max_total_bytes: u64,
    max_cells: u64,
    max_text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ARCHIVE_ENTRIES,
            max_part_bytes: MAX_PART_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_cells: MAX_CELLS,
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

/// L'extracteur Excel. Derrière la feature `xlsx`.
pub struct XlsxExtractor;

impl TextExtractor for XlsxExtractor {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Xlsx
    }

    /// Même règle que DOCX : la preuve est `xl/workbook.xml` dans l'annuaire
    /// central, lu sans rien décompresser. L'extension `xlsx` ne fait revendiquer
    /// que si l'archive ne porte la partie principale d'**aucun** autre format
    /// Office : un `.docx` renommé `.xlsx` reste du Word.
    fn can_handle(&self, probe: &FormatProbe<'_>) -> bool {
        if !probe.starts_with(b"PK\x03\x04") {
            return false;
        }
        let claims_xlsx = probe.extension().as_deref() == Some("xlsx");
        match zip::ZipArchive::new(Cursor::new(probe.bytes)) {
            Ok(archive) => {
                archive.index_for_name(WORKBOOK_PART).is_some()
                    || (claims_xlsx && !super::has_other_office_part(&archive, WORKBOOK_PART))
            }
            Err(_) => claims_xlsx,
        }
    }

    fn extract(&self, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
        extract_with(bytes, Limits::default())
    }
}

fn extract_with(bytes: &[u8], limits: Limits) -> Result<ExtractedText, ExtractError> {
    check_archive(bytes, &limits)?;
    // `AssertUnwindSafe` : tout ce que la fermeture mute y naît et y meurt (voir
    // la même justification dans `pdf.rs`) ; seul le message traverse.
    catch_unwind(AssertUnwindSafe(|| read_workbook(bytes, &limits))).unwrap_or_else(|payload| {
        Err(malformed(format!(
            "spreadsheet parser panicked: {}",
            super::panic_message(&payload)
        )))
    })
}

/// Décompresse chaque entrée vers un puits, en lecture bornée. Voir les docs du
/// module : c'est la seule garde réelle contre une bombe, calamine n'en a pas.
fn check_archive(bytes: &[u8], limits: &Limits) -> Result<(), ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| malformed(format!("not a readable ZIP archive: {e}")))?;
    let entries = archive.len();
    if entries > limits.max_entries {
        return Err(malformed(format!(
            "archive has {entries} entries, over the {}-entry limit",
            limits.max_entries
        )));
    }
    if archive.index_for_name(WORKBOOK_PART).is_none() {
        return Err(malformed(format!("missing {WORKBOOK_PART}")));
    }
    let mut total: u64 = 0;
    for index in 0..entries {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| malformed(format!("archive entry #{index} is unreadable: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        // Un nom non décodable n'empêche pas de mesurer l'entrée : il ne sert qu'aux messages.
        let name = entry
            .name()
            .map(|n| n.into_owned())
            .unwrap_or_else(|_| format!("archive entry #{index}"));
        // La taille déclarée sert à échouer vite quand l'archive avoue être énorme ;
        // la lecture bornée ci-dessous est la vraie garde.
        let declared = entry.size();
        if declared > limits.max_part_bytes {
            return Err(malformed(format!(
                "{name} declares {declared} bytes, over the {}-byte limit",
                limits.max_part_bytes
            )));
        }
        // L'octet de plus rend le dépassement observable.
        let read = std::io::copy(
            &mut entry.by_ref().take(limits.max_part_bytes + 1),
            &mut std::io::sink(),
        )
        .map_err(|e| malformed(format!("could not decompress {name}: {e}")))?;
        if read > limits.max_part_bytes {
            return Err(malformed(format!(
                "{name} decompresses past the {}-byte limit (declared {declared} bytes); \
                 refusing to expand it further",
                limits.max_part_bytes
            )));
        }
        total += read;
        if total > limits.max_total_bytes {
            return Err(malformed(format!(
                "archive decompresses past the {}-byte total limit; refusing to expand it further",
                limits.max_total_bytes
            )));
        }
    }
    Ok(())
}

fn read_workbook(bytes: &[u8], limits: &Limits) -> Result<ExtractedText, ExtractError> {
    let mut workbook: Xlsx<_> = Xlsx::new(Cursor::new(bytes))
        .map_err(|e| malformed(format!("not a readable workbook: {e}")))?;

    let mut warnings = Vec::new();
    let mut text = String::new();
    let mut pages = Vec::new();
    let mut cells_seen: u64 = 0;
    let mut any_value = false;

    // `sheet_names` suit `<sheets>` de `xl/workbook.xml` : l'ordre des onglets.
    for name in workbook.sheet_names() {
        let start = text.len();
        text.push_str("# Sheet: ");
        push_flat(&mut text, &name);
        text.push('\n');

        let mut reader = match workbook.worksheet_cells_reader(&name) {
            Ok(r) => r,
            Err(e) => {
                // La feuille garde sa page : la sauter renumérote les suivantes.
                warnings.push(format!("sheet {name:?} could not be read: {e}"));
                pages.push(ByteInterval::new(start, text.len()));
                continue;
            }
        };

        let mut current_row: Option<u32> = None;
        let mut last_col: u32 = 0;
        let mut columns_clipped = false;
        let mut sheet_has_value = false;

        loop {
            let cell = match reader.next_cell() {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    warnings.push(format!(
                        "sheet {name:?} stopped early, keeping the rows read before it: {e}"
                    ));
                    break;
                }
            };
            cells_seen += 1;
            if cells_seen > limits.max_cells {
                return Err(malformed(format!(
                    "workbook has more than {} cells (limit reached in sheet {name:?}); \
                     refusing to produce a truncated workbook",
                    limits.max_cells
                )));
            }

            let (row, col) = cell.get_position();
            let value = Data::from(cell.get_value().clone());
            let rendered = value.to_string();
            // Vide ou chaîne vide : rien à émettre, et surtout pas de tabulation de
            // queue. Le rattrapage des colonnes se fait au prochain vrai contenu.
            if rendered.is_empty() {
                continue;
            }
            if col >= MAX_COLUMNS {
                columns_clipped = true;
                continue;
            }

            if current_row != Some(row) {
                if current_row.is_some() {
                    text.push('\n');
                }
                current_row = Some(row);
                // Les colonnes vides avant la première valeur de la rangée.
                push_tabs(&mut text, col);
            } else {
                push_tabs(&mut text, col.saturating_sub(last_col).max(1));
            }
            push_flat(&mut text, &rendered);
            last_col = col;
            sheet_has_value = true;

            if text.len() > limits.max_text_bytes {
                return Err(malformed(format!(
                    "extracted text passed the {}-byte limit in sheet {name:?}; \
                     refusing to produce a truncated workbook",
                    limits.max_text_bytes
                )));
            }
        }

        if current_row.is_some() {
            text.push('\n');
        }
        if columns_clipped {
            warnings.push(format!(
                "sheet {name:?}: cells beyond column {MAX_COLUMNS} were ignored"
            ));
        }
        if !sheet_has_value {
            warnings.push(format!("sheet {name:?} is empty"));
        }
        any_value |= sheet_has_value;
        pages.push(ByteInterval::new(start, text.len()));
    }

    if pages.is_empty() {
        warnings.push(format!("{WORKBOOK_PART} lists no sheets"));
    } else if !any_value {
        warnings.push("the workbook contained no cell values".to_string());
    }

    Ok(ExtractedText {
        text,
        format: DocumentFormat::Xlsx,
        pages,
        warnings,
    })
}

fn push_tabs(text: &mut String, n: u32) {
    // `n` est borné par MAX_COLUMNS (les colonnes au-delà sont écartées avant).
    text.extend(std::iter::repeat_n('\t', n as usize));
}

/// Ajoute `value` en aplatissant ce qui casserait la grille : chaque suite de
/// tabulations ou de sauts de ligne devient une espace.
fn push_flat(out: &mut String, value: &str) {
    let mut in_break = false;
    for c in value.chars() {
        if matches!(c, '\t' | '\n' | '\r') {
            if !in_break {
                out.push(' ');
            }
            in_break = true;
        } else {
            in_break = false;
            out.push(c);
        }
    }
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

    /// Une cellule : nombre si préfixée `n:`, sinon chaîne en ligne.
    fn cell_xml(reference: &str, value: &str) -> String {
        match value.strip_prefix("n:") {
            Some(n) => format!(r#"<c r="{reference}"><v>{n}</v></c>"#),
            None => format!(
                r#"<c r="{reference}" t="inlineStr"><is><t xml:space="preserve">{value}</t></is></c>"#
            ),
        }
    }

    /// Une rangée : son numéro et ses cellules `(colonne, valeur)`.
    type Row<'a> = (u32, Vec<(&'a str, &'a str)>);

    /// Un classeur structurellement réel. `sheets` est dans l'ordre des onglets ;
    /// `part` est le numéro de la partie `sheetN.xml`, volontairement découplé de
    /// cet ordre.
    fn workbook(sheets: &[(&str, u32, Vec<Row<'_>>)]) -> Vec<u8> {
        let mut entries: Vec<(String, String)> = vec![
            (
                "[Content_Types].xml".into(),
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#.into(),
            ),
            (
                "_rels/.rels".into(),
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into(),
            ),
        ];
        let mut sheet_tags = String::new();
        let mut rels = String::new();
        for (i, (name, part, rows)) in sheets.iter().enumerate() {
            let n = i + 1;
            sheet_tags.push_str(&format!(
                r#"<sheet name="{name}" sheetId="{n}" r:id="rId{n}"/>"#
            ));
            rels.push_str(&format!(
                r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{part}.xml"/>"#
            ));
            let mut data = String::new();
            for (row_no, cells) in rows {
                data.push_str(&format!(r#"<row r="{row_no}">"#));
                for (col, value) in cells {
                    data.push_str(&cell_xml(&format!("{col}{row_no}"), value));
                }
                data.push_str("</row>");
            }
            entries.push((
                format!("xl/worksheets/sheet{part}.xml"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#
                ),
            ));
        }
        entries.push((
            WORKBOOK_PART.into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{sheet_tags}</sheets></workbook>"#
            ),
        ));
        entries.push((
            "xl/_rels/workbook.xml.rels".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
            ),
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
    fn sheets_are_tab_separated_rows_under_a_heading() {
        let bytes = workbook(&[
            (
                "Budget",
                1,
                vec![
                    (1, vec![("A", "Poste"), ("B", "T1"), ("C", "T2")]),
                    (2, vec![("A", "Licences"), ("B", "n:1200"), ("C", "n:1300")]),
                ],
            ),
            ("Notes", 2, vec![(1, vec![("A", "Validé par la DAF")])]),
        ]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.format, DocumentFormat::Xlsx);
        assert_eq!(
            out.text,
            "# Sheet: Budget\nPoste\tT1\tT2\nLicences\t1200\t1300\n# Sheet: Notes\nValidé par la DAF\n"
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(out.pages.len(), 2);
        assert_tiles(&out);
        assert_eq!(out.page_of(out.text.find("Validé").unwrap()), Some(2));
    }

    // L'ordre est celui des onglets, pas celui des noms de parties : l'onglet
    // « Premier » vit dans sheet10.xml, « Second » dans sheet2.xml.
    #[test]
    fn sheet_order_is_workbook_order_not_part_names() {
        let bytes = workbook(&[
            ("Premier", 10, vec![(1, vec![("A", "un")])]),
            ("Second", 2, vec![(1, vec![("A", "deux")])]),
        ]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "# Sheet: Premier\nun\n# Sheet: Second\ndeux\n");
    }

    #[test]
    fn empty_cells_keep_columns_aligned_and_trailing_ones_vanish() {
        let bytes = workbook(&[(
            "S",
            1,
            vec![
                // B1 et C1 vides au milieu ; D1 vide en fin de ligne (chaîne vide).
                (1, vec![("A", "x"), ("D", "y"), ("E", "")]),
                (3, vec![("B", "z")]),
                // Rangée dont toutes les cellules sont vides : aucune ligne.
                (4, vec![("A", ""), ("B", "")]),
            ],
        )]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "# Sheet: S\nx\t\t\ty\n\tz\n");
    }

    #[test]
    fn tabs_and_newlines_inside_a_cell_do_not_break_the_grid() {
        let bytes = workbook(&[(
            "S",
            1,
            vec![(1, vec![("A", "ligne1&#10;ligne2&#9;tab"), ("B", "suite")])],
        )]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "# Sheet: S\nligne1 ligne2 tab\tsuite\n");
    }

    #[test]
    fn an_empty_sheet_keeps_its_page_and_is_reported() {
        let bytes = workbook(&[
            ("Vide", 1, vec![]),
            ("Pleine", 2, vec![(1, vec![("A", "x")])]),
        ]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "# Sheet: Vide\n# Sheet: Pleine\nx\n");
        assert_eq!(out.pages.len(), 2, "the empty sheet keeps its page");
        assert_tiles(&out);
        assert_eq!(out.page_of(out.text.find('x').unwrap()), Some(2));
        assert!(
            out.warnings.iter().any(|w| w.contains("\"Vide\" is empty")),
            "{:?}",
            out.warnings
        );
        // Un classeur dont toutes les feuilles sont vides le dit aussi.
        let blank = XlsxExtractor
            .extract(&workbook(&[("Vide", 1, vec![])]))
            .unwrap();
        assert!(
            blank.warnings.iter().any(|w| w.contains("no cell values")),
            "{:?}",
            blank.warnings
        );
    }

    #[test]
    fn a_stray_far_cell_does_not_allocate_a_dense_grid() {
        // ~17 milliards de cellules via `worksheet_range`.
        let bytes = workbook(&[(
            "S",
            1,
            vec![(1, vec![("A", "haut")]), (1048576, vec![("A", "bas")])],
        )]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "# Sheet: S\nhaut\nbas\n");
    }

    #[test]
    fn cells_beyond_the_column_limit_are_dropped_with_a_warning() {
        let bytes = workbook(&[("S", 1, vec![(1, vec![("A", "a"), ("XFD", "loin")])])]);
        let out = XlsxExtractor.extract(&bytes).unwrap();
        assert_eq!(out.text, "# Sheet: S\na\n");
        assert!(out.warnings.iter().any(|w| w.contains("beyond column")));
    }

    #[test]
    fn too_many_cells_is_refused_with_the_limit_named() {
        let rows: Vec<Row<'_>> = (1..=20).map(|r| (r, vec![("A", "v")])).collect();
        let bytes = workbook(&[("S", 1, rows)]);
        let limits = Limits {
            max_cells: 10,
            ..Limits::default()
        };
        match extract_with(&bytes, limits) {
            Err(ExtractError::Malformed {
                format: "XLSX",
                detail,
            }) => {
                assert!(detail.contains("more than 10 cells"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn too_much_text_is_refused_not_truncated() {
        let big = "x".repeat(100);
        let rows: Vec<Row<'_>> = (1..=50).map(|r| (r, vec![("A", big.as_str())])).collect();
        let bytes = workbook(&[("S", 1, rows)]);
        let limits = Limits {
            max_text_bytes: 1000,
            ..Limits::default()
        };
        match extract_with(&bytes, limits) {
            Err(ExtractError::Malformed { detail, .. }) => {
                assert!(detail.contains("1000-byte limit"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }

    // Une bombe honnête sur sa taille (65 Mio d'espaces → ~65 Ko compressés) est
    // arrêtée par sa déclaration. Le plafond total, lui, ne compte que des octets
    // réellement décompressés : l'abaisser exerce la lecture bornée elle-même,
    // celle qui arrête une bombe qui ment sur sa taille.
    #[test]
    fn a_zip_bomb_is_refused_before_calamine_sees_it() {
        let mut bytes = workbook(&[("S", 1, vec![(1, vec![("A", "a")])])]);
        // Ajoute une entrée énorme à un classeur valide.
        let mut w = zip::ZipWriter::new_append(Cursor::new(&mut bytes)).unwrap();
        w.start_file(
            "xl/sharedStrings.xml",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        let chunk = vec![b' '; 1 << 20];
        for _ in 0..=(MAX_PART_BYTES >> 20) {
            w.write_all(&chunk).unwrap();
        }
        w.finish().unwrap();
        match XlsxExtractor.extract(&bytes) {
            Err(ExtractError::Malformed {
                format: "XLSX",
                detail,
            }) => {
                assert!(detail.contains("67108864"), "{detail}")
            }
            other => panic!("{other:?}"),
        }

        // Même chose sous des plafonds abaissés, sur le total cette fois.
        let small = workbook(&[("S", 1, vec![(1, vec![("A", "a")])])]);
        let limits = Limits {
            max_total_bytes: 100,
            ..Limits::default()
        };
        match extract_with(&small, limits) {
            Err(ExtractError::Malformed { detail, .. }) => {
                assert!(detail.contains("total limit"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn too_many_entries_is_refused() {
        let bytes = workbook(&[("S", 1, vec![(1, vec![("A", "a")])])]);
        let limits = Limits {
            max_entries: 3,
            ..Limits::default()
        };
        match extract_with(&bytes, limits) {
            Err(ExtractError::Malformed { detail, .. }) => {
                assert!(detail.contains("3-entry limit"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn claims_a_workbook_by_content_whatever_its_name() {
        let bytes = workbook(&[("S", 1, vec![(1, vec![("A", "a")])])]);
        assert!(XlsxExtractor.can_handle(&FormatProbe::new(&bytes, Some("data.bin"))));
        assert!(XlsxExtractor.can_handle(&FormatProbe::new(&bytes, None)));
    }

    #[test]
    fn does_not_claim_other_zips_or_other_office_formats() {
        let jar = zip_of(&[(
            "META-INF/MANIFEST.MF".into(),
            "Manifest-Version: 1.0".into(),
        )]);
        assert!(!XlsxExtractor.can_handle(&FormatProbe::new(&jar, Some("app.jar"))));
        // Un Word renommé .xlsx reste du Word.
        let docx = zip_of(&[("word/document.xml".into(), "<w:document/>".into())]);
        assert!(!XlsxExtractor.can_handle(&FormatProbe::new(&docx, Some("a.xlsx"))));
        assert!(!XlsxExtractor.can_handle(&FormatProbe::new(b"hello", Some("a.xlsx"))));
    }

    // Archives hostiles ou cassées : jamais de panique, toujours `Malformed`.
    #[test]
    fn broken_workbooks_are_malformed_never_a_panic() {
        let cases: Vec<Vec<u8>> = vec![
            zip_of(&[(WORKBOOK_PART.into(), "not xml at all".into())]),
            zip_of(&[(
                WORKBOOK_PART.into(),
                "<workbook><sheets><sheet name=".into(),
            )]),
            b"PK\x03\x04garbage".to_vec(),
            {
                let full = workbook(&[("S", 1, vec![(1, vec![("A", "a")])])]);
                full[..full.len() / 2].to_vec()
            },
        ];
        for (i, bytes) in cases.iter().enumerate() {
            match XlsxExtractor.extract(bytes) {
                Err(ExtractError::Malformed { format: "XLSX", .. }) => {}
                // Un classeur à moitié lisible peut rendre Ok avec des avertissements ;
                // ce qui compte est de ne pas paniquer.
                Ok(out) => assert!(!out.warnings.is_empty(), "case {i}: {out:?}"),
                Err(other) => panic!("case {i}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_registry_routes_a_workbook_here_whatever_its_name() {
        let reg = ExtractorRegistry::with_builtins();
        assert!(reg.supported_formats().contains(&DocumentFormat::Xlsx));
        let bytes = workbook(&[("S", 1, vec![(1, vec![("A", "routé")])])]);
        for name in [Some("a.bin"), Some("a.docx"), None] {
            let out = reg.extract(&bytes, name).unwrap();
            assert_eq!(out.format, DocumentFormat::Xlsx, "{name:?}");
            assert!(out.text.contains("routé"));
        }
    }
}
