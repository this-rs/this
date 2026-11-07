//! Des octets au texte, sans perdre la provenance.
//!
//! Un trait, un registre, un extracteur par famille. Le découpage et l'alignement
//! ([`crate::chunk`], [`crate::align`]) travaillent sur le *texte* produit ici :
//! tout ce module existe pour leur en livrer un qui soit fidèle à l'original.
//!
//! ## On détecte par le contenu, pas par l'extension
//!
//! Un `rapport.txt` qui commence par `%PDF-` est un PDF. L'extension est un indice
//! venu d'une source non fiable ; les magic bytes sont une preuve. Elle ne sert
//! qu'à départager des familles qui partagent un conteneur : un `.docx` et un
//! `.jar` sont tous deux des ZIP.
//!
//! ## Les pages font partie de la provenance
//!
//! [`ExtractedText::pages`] dit où commence et finit chaque page dans le texte :
//! un alignement à l'octet 12 000 devient « page 7 ». Les formats sans pagination
//! laissent le vecteur vide ; c'est une information, pas un manque.
//!
//! ## Les images n'ont pas de texte
//!
//! [`image`] les reconnaît et nomme leur format (le type de média, lu dans les
//! magic bytes) ; il n'en extrait rien. Ranger les octets de l'image revient à
//! l'application, et l'envoyer au modèle relève du contrat de message
//! (`this-harness`), pas de ce crate : à l'extraction, une image n'a pas de texte,
//! et c'est tout ce qu'on en dit ici.
//!
//! ## Les formats Office partagent un conteneur
//!
//! `.docx`, `.xlsx` et `.pptx` sont tous des ZIP (`PK\x03\x04`). Ce qui les
//! distingue est leur partie principale — `word/document.xml`,
//! `xl/workbook.xml`, `ppt/presentation.xml` — lue dans l'annuaire de l'archive,
//! jamais l'extension. Chaque extracteur revendique une archive par sa partie ;
//! quand aucun n'est compilé pour elle, [`ExtractorRegistry::extract`] la
//! reconnaît quand même (voir `office_kind`) pour dire `FeatureDisabled` avec
//! le bon format, plutôt qu'un « format inconnu ».

#[cfg(feature = "docx")]
pub mod docx;
pub mod image;
#[cfg(feature = "pdf")]
pub mod pdf;
#[cfg(feature = "pptx")]
pub mod pptx;
pub mod text;
#[cfg(feature = "xlsx")]
pub mod xlsx;

use serde::{Deserialize, Serialize};

use crate::ByteInterval;

/// Familles de documents connues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    /// Texte brut, Markdown, code, JSON, CSV, YAML, TOML…
    PlainText,
    /// Word (`.docx`, Office Open XML).
    Docx,
    Pdf,
    /// Excel (`.xlsx`).
    Xlsx,
    /// PowerPoint (`.pptx`).
    Pptx,
    /// Une image : reconnue et typée, sans texte à extraire (voir [`image`]).
    Image,
    /// Tout le reste, gardé comme pièce opaque : aucun extracteur ne l'a lu.
    /// Jamais produit par un extracteur.
    Binary,
}

impl DocumentFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PlainText => "plain_text",
            Self::Docx => "docx",
            Self::Pdf => "pdf",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Image => "image",
            Self::Binary => "binary",
        }
    }

    /// Inverse de [`as_str`](Self::as_str).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "plain_text" => Self::PlainText,
            "docx" => Self::Docx,
            "pdf" => Self::Pdf,
            "xlsx" => Self::Xlsx,
            "pptx" => Self::Pptx,
            "image" => Self::Image,
            "binary" => Self::Binary,
            _ => return None,
        })
    }

    /// Ce format porte-t-il du texte qu'on peut extraire, découper et indexer ?
    /// Faux pour une image (elle n'a pas de texte) et pour une pièce opaque.
    pub fn is_text_bearing(&self) -> bool {
        !matches!(self, Self::Image | Self::Binary)
    }
}

/// Ce que l'extraction a produit, et ce qui s'est mal passé en chemin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedText {
    pub text: String,
    pub format: DocumentFormat,
    /// Tranches d'octets des pages (ou sections) dans `text`, dans l'ordre. Vide
    /// pour un format sans pagination.
    pub pages: Vec<ByteInterval>,
    /// Problèmes non fatals : page illisible, objet incorporé ignoré… Remontés
    /// plutôt qu'avalés : une extraction qui a silencieusement perdu la moitié
    /// d'un document empoisonnerait tout ce qui est bâti dessus.
    pub warnings: Vec<String>,
}

impl ExtractedText {
    /// Numéro de page (à partir de 1) qui contient `offset`, si le format est paginé.
    pub fn page_of(&self, offset: usize) -> Option<usize> {
        self.pages
            .iter()
            .position(|p| offset >= p.start && offset < p.end)
            .map(|i| i + 1)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// Rien ne reconnaît ce contenu.
    #[error("no extractor for this content (sniffed: {sniffed})")]
    UnsupportedFormat { sniffed: String },
    /// Le contenu est reconnu, mais le support de ce format n'est pas dans cette
    /// construction. Différent de `UnsupportedFormat` : ici la capacité manque, le
    /// fichier n'est pas en cause.
    #[error(
        "{format} support is not compiled in: no `{feature}` extractor is registered in this build"
    )]
    FeatureDisabled {
        format: &'static str,
        feature: &'static str,
    },
    #[error("input is empty")]
    Empty,
    #[error("malformed {format} document: {detail}")]
    Malformed {
        format: &'static str,
        detail: String,
    },
}

/// Indices sur la nature d'un blob.
#[derive(Debug, Clone)]
pub struct FormatProbe<'a> {
    pub bytes: &'a [u8],
    /// Nom donné par celui qui envoie : un indice, jamais une décision.
    pub filename: Option<&'a str>,
}

impl<'a> FormatProbe<'a> {
    pub fn new(bytes: &'a [u8], filename: Option<&'a str>) -> Self {
        Self { bytes, filename }
    }

    /// Extension en minuscules, si le nom en porte une.
    pub fn extension(&self) -> Option<String> {
        self.filename
            .and_then(|f| f.rsplit_once('.'))
            .map(|(_, ext)| ext.to_ascii_lowercase())
    }

    pub fn starts_with(&self, magic: &[u8]) -> bool {
        self.bytes.starts_with(magic)
    }

    /// Description courte et sûre de ce qui a été reniflé, pour les messages d'erreur.
    pub fn describe(&self) -> String {
        let head: String = self
            .bytes
            .iter()
            .take(8)
            .map(|b| {
                if b.is_ascii_graphic() {
                    (*b as char).to_string()
                } else {
                    format!("\\x{b:02x}")
                }
            })
            .collect();
        match self.extension() {
            Some(ext) => format!("{head} (.{ext})"),
            None => head,
        }
    }
}

/// Un extracteur pour une famille de documents.
pub trait TextExtractor: Send + Sync {
    fn format(&self) -> DocumentFormat;

    /// Reconnaît-il ce contenu ? Décider d'abord sur `probe.bytes` ; l'extension ne
    /// sert qu'à départager des formats qui partagent un conteneur.
    fn can_handle(&self, probe: &FormatProbe<'_>) -> bool;

    fn extract(&self, bytes: &[u8]) -> Result<ExtractedText, ExtractError>;
}

/// Ensemble ordonné d'extracteurs ; le premier qui reconnaît gagne.
///
/// L'ordre compte : les formats conteneurs avant le texte, qui accepte tout ce qui
/// est de l'UTF-8 valide et avalerait sinon un DOCX comme du bruit.
pub struct ExtractorRegistry {
    extractors: Vec<Box<dyn TextExtractor>>,
}

impl ExtractorRegistry {
    /// Registre vide — préférer [`with_builtins`](Self::with_builtins).
    pub fn new() -> Self {
        Self {
            extractors: Vec::new(),
        }
    }

    /// Les extracteurs de cette construction, dans l'ordre de sondage : XLSX,
    /// PPTX et DOCX, PDF, chacun si sa feature est active, puis le texte brut.
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        // Les formats conteneurs d'abord, avant le fourre-tout. XLSX et PPTX avant
        // DOCX : l'extracteur Word revendique toute archive nommée `.docx`, même un
        // classeur renommé ; les deux autres ne revendiquent par leur extension
        // qu'une archive sans partie Office d'un autre format. Sondés en premier,
        // ils prennent leur contenu quel que soit le nom, et laissent le Word à DOCX.
        #[cfg(feature = "xlsx")]
        reg.register(Box::new(xlsx::XlsxExtractor));
        #[cfg(feature = "pptx")]
        reg.register(Box::new(pptx::PptxExtractor));
        #[cfg(feature = "docx")]
        reg.register(Box::new(docx::DocxExtractor));
        // Un PDF est binaire : le fourre-tout refuserait déjà `%PDF-`, mais l'ordre
        // compte pour le cas où il l'oublierait — et pour `supported_formats`, qui
        // rapporte ce que ce registre sait vraiment lire.
        #[cfg(feature = "pdf")]
        reg.register(Box::new(pdf::PdfExtractor));
        reg.register(Box::new(text::PlainTextExtractor));
        reg
    }

    pub fn register(&mut self, extractor: Box<dyn TextExtractor>) {
        self.extractors.push(extractor);
    }

    /// Les formats que ce registre sait réellement lire. Rapporté, pas supposé :
    /// « non supporté » et « non compilé » sont deux réponses différentes.
    pub fn supported_formats(&self) -> Vec<DocumentFormat> {
        self.extractors.iter().map(|e| e.format()).collect()
    }

    fn has(&self, format: DocumentFormat) -> bool {
        self.extractors.iter().any(|e| e.format() == format)
    }

    pub fn extract(
        &self,
        bytes: &[u8],
        filename: Option<&str>,
    ) -> Result<ExtractedText, ExtractError> {
        if bytes.is_empty() {
            return Err(ExtractError::Empty);
        }
        let probe = FormatProbe::new(bytes, filename);

        // Les conteneurs d'abord : l'ordre d'enregistrement est celui du sondage, et
        // `with_builtins` met le texte en dernier.
        for extractor in &self.extractors {
            if extractor.can_handle(&probe) {
                return extractor.extract(bytes);
            }
        }

        // Dire « capacité absente » plutôt que « fichier cassé » quand c'est le cas :
        // le format est reconnu, mais aucun extracteur de ce format n'est enregistré.
        if probe.starts_with(b"%PDF-") && !self.has(DocumentFormat::Pdf) {
            return Err(ExtractError::FeatureDisabled {
                format: "PDF",
                feature: "pdf",
            });
        }
        if probe.starts_with(b"PK\x03\x04") {
            // Le contenu d'abord (la partie principale nommée dans l'archive),
            // l'extension seulement quand l'archive ne dit rien.
            let format = office_kind(bytes).or_else(|| match probe.extension().as_deref() {
                Some("docx") => Some(DocumentFormat::Docx),
                Some("xlsx") => Some(DocumentFormat::Xlsx),
                Some("pptx") => Some(DocumentFormat::Pptx),
                _ => None,
            });
            let Some(format) = format else {
                return Err(ExtractError::UnsupportedFormat {
                    sniffed: probe.describe(),
                });
            };
            let (name, feature) = match format {
                DocumentFormat::Xlsx => ("XLSX", "xlsx"),
                DocumentFormat::Pptx => ("PPTX", "pptx"),
                _ => ("DOCX", "docx"),
            };
            if !self.has(format) {
                return Err(ExtractError::FeatureDisabled {
                    format: name,
                    feature,
                });
            }
        }
        if let Some(media) = image::sniff_image(bytes) {
            return Err(ExtractError::UnsupportedFormat {
                sniffed: format!("{} (image: no text to extract)", media.as_str()),
            });
        }

        Err(ExtractError::UnsupportedFormat {
            sniffed: probe.describe(),
        })
    }
}

impl Default for ExtractorRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Les parties principales des formats Office, et le format qu'elles prouvent.
const OFFICE_PARTS: [(&[u8], DocumentFormat); 3] = [
    (b"word/document.xml", DocumentFormat::Docx),
    (b"xl/workbook.xml", DocumentFormat::Xlsx),
    (b"ppt/presentation.xml", DocumentFormat::Pptx),
];

/// Quel format Office une archive ZIP porte, d'après les **noms** de ses entrées.
///
/// Ne sert qu'à choisir le diagnostic quand aucun extracteur n'a revendiqué
/// l'archive — typiquement parce que la feature du format n'est pas compilée,
/// donc sans le crate `zip` sous la main. Ce n'est pas un lecteur ZIP : rien n'est
/// décompressé ni alloué. On parcourt les en-têtes (locaux `PK\x03\x04` et
/// centraux `PK\x01\x02`), on lit la longueur du nom à son offset fixe, et l'on
/// compare le nom, avec des bornes vérifiées : une archive hostile ou tronquée
/// rend `None`, jamais une panique. Les en-têtes locaux comptent aussi, pour
/// qu'une archive dont l'annuaire central est coupé soit encore reconnue.
///
/// En cas d'ambiguïté (une archive qui porterait deux parties principales), le
/// premier de [`OFFICE_PARTS`] l'emporte : un hasard de fabrication, pas un
/// format réel.
pub(crate) fn office_kind(bytes: &[u8]) -> Option<DocumentFormat> {
    // (signature, offset de la longueur du nom, offset du nom)
    const HEADERS: [(&[u8; 4], usize, usize); 2] =
        [(b"PK\x03\x04", 26, 30), (b"PK\x01\x02", 28, 46)];
    let mut found: Option<usize> = None;
    let mut at = 0;
    while let Some(rel) = bytes.get(at..)?.windows(2).position(|w| w == b"PK") {
        let start = at + rel;
        at = start + 2;
        for (sig, len_at, name_at) in HEADERS {
            if bytes.get(start..start + 4) != Some(&sig[..]) {
                continue;
            }
            let Some(len) = bytes
                .get(start + len_at..start + len_at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            else {
                continue;
            };
            let Some(name) = bytes.get(start + name_at..start + name_at + len) else {
                continue;
            };
            if let Some(i) = OFFICE_PARTS.iter().position(|(part, _)| *part == name) {
                found = Some(found.map_or(i, |f| f.min(i)));
            }
        }
    }
    found.map(|i| OFFICE_PARTS[i].1)
}

/// L'archive porte-t-elle la partie principale d'un **autre** format Office que
/// celui de `own_part` ? Un extracteur qui revendique par l'extension doit d'abord
/// s'en assurer : un `.docx` renommé `.xlsx` reste du Word.
#[cfg(any(feature = "xlsx", feature = "pptx"))]
pub(crate) fn has_other_office_part<R: std::io::Read + std::io::Seek>(
    archive: &zip::ZipArchive<R>,
    own_part: &str,
) -> bool {
    OFFICE_PARTS.iter().any(|(part, _)| {
        let part = std::str::from_utf8(part).unwrap_or_default();
        part != own_part && archive.index_for_name(part).is_some()
    })
}

/// Meilleur effort sur la charge d'une panique, `&str` ou `String` en pratique.
#[cfg(feature = "xlsx")]
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Une entrée vide ne doit atteindre aucun extracteur : elle n'a pas de format.
    #[test]
    fn empty_input_is_rejected_before_any_extractor_runs() {
        let reg = ExtractorRegistry::with_builtins();
        assert!(matches!(reg.extract(b"", None), Err(ExtractError::Empty)));
        // Même un registre vide : `Empty` prime sur « rien ne reconnaît ».
        assert!(matches!(
            ExtractorRegistry::new().extract(b"", None),
            Err(ExtractError::Empty)
        ));
    }

    #[test]
    fn plain_text_round_trips_through_the_registry() {
        let out = ExtractorRegistry::with_builtins()
            .extract(b"# Title\n\nSome prose.", Some("notes.md"))
            .unwrap();
        assert_eq!(out.format, DocumentFormat::PlainText);
        assert_eq!(out.text, "# Title\n\nSome prose.");
    }

    // Un PDF nommé .txt ne doit pas être pris pour du texte. Sans extracteur PDF
    // compilé, la bonne réponse est « capacité absente » (PDF), jamais du bruit en
    // texte ni « format inconnu ».
    #[cfg(not(feature = "pdf"))]
    #[test]
    fn a_pdf_named_txt_is_not_taken_for_text() {
        let reg = ExtractorRegistry::with_builtins();
        let err = reg
            .extract(
                b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\nbinary garbage",
                Some("report.txt"),
            )
            .unwrap_err();
        match err {
            ExtractError::FeatureDisabled { format, .. } => assert_eq!(format, "PDF"),
            other => panic!("le contenu doit l'emporter sur l'extension : {other}"),
        }
    }

    // Avec la feature `pdf`, la même entrée ne dit plus « non compilé » : elle est
    // revendiquée par l'extracteur PDF, qui la trouve illisible. C'est la même
    // règle que pour DOCX juste en dessous — le contenu l'emporte sur l'extension,
    // et le diagnostic porte sur le fichier, plus sur la construction.
    #[cfg(feature = "pdf")]
    #[test]
    fn a_pdf_named_txt_is_claimed_by_the_pdf_extractor_once_compiled_in() {
        let reg = ExtractorRegistry::with_builtins();
        assert!(reg.supported_formats().contains(&DocumentFormat::Pdf));
        let err = reg
            .extract(
                b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\nbinary garbage",
                Some("report.txt"),
            )
            .unwrap_err();
        match err {
            ExtractError::Malformed { format, detail } => {
                assert_eq!(format, "PDF");
                assert!(!detail.is_empty(), "detail must say something usable");
            }
            other => panic!("le contenu doit l'emporter sur l'extension : {other}"),
        }
    }

    // « Pas compilé » et « pas supporté » sont deux réponses : un DOCX sans extracteur
    // dit la première ; un ZIP quelconque dit la seconde.
    // Sans la feature `docx` seulement : avec elle, un « a.docx » est revendiqué
    // par l'extracteur Word (et rendu `Malformed`), le test ne décrit plus ce cas.
    #[cfg(not(feature = "docx"))]
    #[test]
    fn not_compiled_in_is_distinguished_from_unsupported() {
        let reg = ExtractorRegistry::with_builtins();
        let zip = b"PK\x03\x04rest";
        let disabled = reg.extract(zip, Some("a.docx")).unwrap_err();
        assert!(matches!(
            disabled,
            ExtractError::FeatureDisabled { format: "DOCX", .. }
        ));
        assert!(disabled.to_string().contains("not compiled in"));

        let unsupported = reg.extract(zip, Some("a.jar")).unwrap_err();
        assert!(matches!(
            unsupported,
            ExtractError::UnsupportedFormat { .. }
        ));
        assert!(!unsupported.to_string().contains("not compiled in"));
    }

    // Avec la feature : le registre déclare DOCX, ne dit plus « non compilé », et
    // un ZIP quelconque reste « non supporté ».
    #[cfg(feature = "docx")]
    #[test]
    fn docx_feature_registers_before_plain_text_and_keeps_unsupported_zip() {
        let reg = ExtractorRegistry::with_builtins();
        // Ce qui compte est un ordre RELATIF : un conteneur se sonde avant le texte, sinon
        // un .docx serait lu comme du texte. Épingler la liste entière ferait échouer ce
        // test dès qu'une autre feature (`pdf`) ajoute son extracteur au même registre —
        // on défendrait alors la composition du build au lieu de la propriété.
        let formats = reg.supported_formats();
        let docx = formats
            .iter()
            .position(|f| *f == DocumentFormat::Docx)
            .expect("docx est enregistré quand la feature est active");
        let texte = formats
            .iter()
            .position(|f| *f == DocumentFormat::PlainText)
            .expect("le texte est toujours enregistré");
        assert!(
            docx < texte,
            "docx doit être sondé avant le texte, reçu {formats:?}"
        );
        let jar = reg.extract(b"PK\x03\x04rest", Some("a.jar")).unwrap_err();
        assert!(matches!(jar, ExtractError::UnsupportedFormat { .. }));
        let docx = reg.extract(b"PK\x03\x04rest", Some("a.docx")).unwrap_err();
        assert!(matches!(
            docx,
            ExtractError::Malformed { format: "DOCX", .. }
        ));
    }

    // Un blob illisible doit dire ce qu'on a reniflé, pour qu'on puisse le diagnostiquer.
    #[test]
    fn an_unknown_blob_reports_what_was_sniffed() {
        let err = ExtractorRegistry::with_builtins()
            .extract(&[0xff, 0xfe, 0x00, 0x01, 0x02], Some("mystery.bin"))
            .unwrap_err();
        match err {
            ExtractError::UnsupportedFormat { sniffed } => {
                assert!(sniffed.contains("\\xff"), "{sniffed:?}");
                assert!(sniffed.contains(".bin"));
            }
            other => panic!("attendu UnsupportedFormat, reçu {other:?}"),
        }
    }

    // Une image est reconnue mais n'a pas de texte : l'erreur le dit, au lieu de
    // la faire passer pour du texte.
    #[test]
    fn an_image_is_not_text_and_the_error_says_so() {
        let err = ExtractorRegistry::with_builtins()
            .extract(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", Some("a.png"))
            .unwrap_err();
        match err {
            ExtractError::UnsupportedFormat { sniffed } => assert!(sniffed.contains("image/png")),
            other => panic!("{other:?}"),
        }
    }

    // Un extracteur enregistré qui reconnaît le contenu l'emporte, et le registre
    // ne ment plus sur « non compilé ».
    #[test]
    fn a_registered_container_extractor_is_probed_before_text() {
        struct FakePdf;
        impl TextExtractor for FakePdf {
            fn format(&self) -> DocumentFormat {
                DocumentFormat::Pdf
            }
            fn can_handle(&self, p: &FormatProbe<'_>) -> bool {
                p.starts_with(b"%PDF-")
            }
            fn extract(&self, _: &[u8]) -> Result<ExtractedText, ExtractError> {
                Ok(ExtractedText {
                    text: "x".into(),
                    format: DocumentFormat::Pdf,
                    pages: vec![],
                    warnings: vec![],
                })
            }
        }
        let mut reg = ExtractorRegistry::new();
        reg.register(Box::new(FakePdf));
        reg.register(Box::new(text::PlainTextExtractor));
        assert_eq!(
            reg.extract(b"%PDF-1.7 ascii only", None).unwrap().format,
            DocumentFormat::Pdf
        );
        assert_eq!(
            reg.supported_formats(),
            vec![DocumentFormat::Pdf, DocumentFormat::PlainText]
        );
    }

    // Les pages sont numérotées à partir de 1 et bornées : la fin du texte n'est
    // dans aucune page.
    #[test]
    fn page_of_is_one_based_and_bounded() {
        let e = ExtractedText {
            text: "aaaabbbbcccc".into(),
            format: DocumentFormat::Pdf,
            pages: vec![
                ByteInterval::new(0, 4),
                ByteInterval::new(4, 8),
                ByteInterval::new(8, 12),
            ],
            warnings: vec![],
        };
        assert_eq!(e.page_of(0), Some(1));
        assert_eq!(e.page_of(7), Some(2));
        assert_eq!(e.page_of(11), Some(3));
        assert_eq!(e.page_of(12), None);
    }

    #[test]
    fn unpaginated_formats_have_no_page() {
        let e = ExtractedText {
            text: "hello".into(),
            format: DocumentFormat::PlainText,
            pages: vec![],
            warnings: vec![],
        };
        assert_eq!(e.page_of(0), None);
    }

    // Le format est sérialisé en snake_case et `as_str` doit coller, sinon le
    // graphe et le fil diraient deux choses différentes.
    #[test]
    fn format_names_are_snake_case_and_round_trip() {
        for f in [
            DocumentFormat::PlainText,
            DocumentFormat::Docx,
            DocumentFormat::Pdf,
            DocumentFormat::Xlsx,
            DocumentFormat::Pptx,
            DocumentFormat::Image,
            DocumentFormat::Binary,
        ] {
            assert_eq!(DocumentFormat::parse(f.as_str()), Some(f));
        }
        assert_eq!(DocumentFormat::PlainText.as_str(), "plain_text");
        assert_eq!(DocumentFormat::parse("nope"), None);
    }

    #[test]
    fn only_images_and_binaries_carry_no_text() {
        assert!(DocumentFormat::Pdf.is_text_bearing());
        assert!(DocumentFormat::PlainText.is_text_bearing());
        assert!(!DocumentFormat::Image.is_text_bearing());
        assert!(!DocumentFormat::Binary.is_text_bearing());
    }

    /// Archive ZIP en mémoire (dev-dependency `zip`, présente dans tous les builds de test).
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(content).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    // Le format Office se lit dans les noms d'entrées, quel que soit le nom du fichier.
    #[test]
    fn office_kind_reads_the_main_part_from_entry_names() {
        let docx = zip_of(&[("word/document.xml", b"<w:document/>")]);
        // La partie principale en premier : la coupe à mi-archive ci-dessous garde
        // son en-tête local et perd l'annuaire central.
        let xlsx = zip_of(&[
            ("xl/workbook.xml", b"<w/>"),
            ("[Content_Types].xml", b"<x/>"),
        ]);
        let pptx = zip_of(&[("ppt/presentation.xml", b"<p/>")]);
        let jar = zip_of(&[("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n")]);
        assert_eq!(office_kind(&docx), Some(DocumentFormat::Docx));
        assert_eq!(office_kind(&xlsx), Some(DocumentFormat::Xlsx));
        assert_eq!(office_kind(&pptx), Some(DocumentFormat::Pptx));
        assert_eq!(office_kind(&jar), None);
        // Un nom de partie qui n'apparaît que dans le CONTENU d'une entrée ne compte pas.
        let tricky = zip_of(&[("readme.txt", b"see xl/workbook.xml")]);
        assert_eq!(office_kind(&tricky), None);
        // Annuaire central coupé : les en-têtes locaux suffisent encore.
        assert_eq!(
            office_kind(&xlsx[..xlsx.len() / 2]),
            Some(DocumentFormat::Xlsx)
        );
    }

    // Octets hostiles : longueurs de nom mensongères, signatures en fin de tampon,
    // archive tronquée à chaque octet. Jamais de panique.
    #[test]
    fn office_kind_never_panics_on_hostile_bytes() {
        let full = zip_of(&[("ppt/presentation.xml", b"<p/>")]);
        for cut in 0..full.len() {
            let _ = office_kind(&full[..cut]);
        }
        for bytes in [
            &b"PK"[..],
            b"PK\x03\x04",
            b"PK\x01\x02\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
            b"PK\x03\x04\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\xff\xffxl/",
        ] {
            assert_eq!(office_kind(bytes), None);
        }
    }

    // Une archive ZIP qui n'est ni xlsx ni pptx ni docx : « non supporté », jamais
    // une panique ni « malformé », quel que soit le jeu de features.
    #[test]
    fn a_zip_that_is_no_office_format_is_unsupported_in_every_build() {
        let reg = ExtractorRegistry::with_builtins();
        let jar = zip_of(&[("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n")]);
        for name in [Some("app.jar"), Some("archive.zip"), None] {
            let err = reg.extract(&jar, name).unwrap_err();
            assert!(
                matches!(err, ExtractError::UnsupportedFormat { .. }),
                "{name:?}: {err:?}"
            );
        }
        // Et un ZIP illisible sans extension Office non plus.
        let err = reg
            .extract(b"PK\x03\x04\xff\xff garbage", None)
            .unwrap_err();
        assert!(
            matches!(err, ExtractError::UnsupportedFormat { .. }),
            "{err:?}"
        );
    }

    // Sans la feature `xlsx` : un classeur reconnu par son CONTENU répond « pas
    // compilé » avec le bon format, même nommé autrement ou pas nommé du tout.
    #[cfg(not(feature = "xlsx"))]
    #[test]
    fn a_workbook_without_the_xlsx_feature_is_feature_disabled() {
        let reg = ExtractorRegistry::with_builtins();
        assert!(!reg.supported_formats().contains(&DocumentFormat::Xlsx));
        let xlsx = zip_of(&[("xl/workbook.xml", b"<workbook/>")]);
        for name in [Some("a.xlsx"), Some("a.bin"), None] {
            match reg.extract(&xlsx, name) {
                Err(ExtractError::FeatureDisabled { format, feature }) => {
                    assert_eq!((format, feature), ("XLSX", "xlsx"), "{name:?}")
                }
                other => panic!("{name:?}: {other:?}"),
            }
        }
        // Le dernier recours reste l'extension, quand l'archive ne dit rien.
        assert!(matches!(
            reg.extract(b"PK\x03\x04rest", Some("a.xlsx")),
            Err(ExtractError::FeatureDisabled { format: "XLSX", .. })
        ));
    }

    #[cfg(not(feature = "pptx"))]
    #[test]
    fn a_presentation_without_the_pptx_feature_is_feature_disabled() {
        let reg = ExtractorRegistry::with_builtins();
        assert!(!reg.supported_formats().contains(&DocumentFormat::Pptx));
        let pptx = zip_of(&[("ppt/presentation.xml", b"<p:presentation/>")]);
        for name in [Some("a.pptx"), Some("a.bin"), None] {
            match reg.extract(&pptx, name) {
                Err(ExtractError::FeatureDisabled { format, feature }) => {
                    assert_eq!((format, feature), ("PPTX", "pptx"), "{name:?}")
                }
                other => panic!("{name:?}: {other:?}"),
            }
        }
        assert!(matches!(
            reg.extract(b"PK\x03\x04rest", Some("a.pptx")),
            Err(ExtractError::FeatureDisabled { format: "PPTX", .. })
        ));
    }

    // Avec la feature : un ZIP illisible nommé .xlsx est revendiqué et dit
    // « malformé » ; le registre déclare XLSX avant le texte.
    #[cfg(feature = "xlsx")]
    #[test]
    fn xlsx_feature_claims_a_broken_archive_named_xlsx() {
        let reg = ExtractorRegistry::with_builtins();
        let f = reg.supported_formats();
        let pos = |x| f.iter().position(|g| *g == x).unwrap();
        assert!(pos(DocumentFormat::Xlsx) < pos(DocumentFormat::PlainText));
        assert!(matches!(
            reg.extract(b"PK\x03\x04rest", Some("a.xlsx")),
            Err(ExtractError::Malformed { format: "XLSX", .. })
        ));
    }

    #[cfg(feature = "pptx")]
    #[test]
    fn pptx_feature_claims_a_broken_archive_named_pptx() {
        let reg = ExtractorRegistry::with_builtins();
        let f = reg.supported_formats();
        let pos = |x| f.iter().position(|g| *g == x).unwrap();
        assert!(pos(DocumentFormat::Pptx) < pos(DocumentFormat::PlainText));
        assert!(matches!(
            reg.extract(b"PK\x03\x04rest", Some("a.pptx")),
            Err(ExtractError::Malformed { format: "PPTX", .. })
        ));
    }

    // Toutes features : chaque format Office va à son extracteur par son contenu,
    // même sous l'extension d'un autre — Word compris, qui revendique par extension.
    #[cfg(all(feature = "docx", feature = "xlsx", feature = "pptx"))]
    #[test]
    fn office_content_wins_over_a_misleading_extension() {
        let reg = ExtractorRegistry::with_builtins();
        let docx = zip_of(&[(
            "word/document.xml",
            br#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>mot</w:t></w:r></w:p></w:body></w:document>"#,
        )]);
        for name in ["a.xlsx", "a.pptx", "a.docx"] {
            let out = reg.extract(&docx, Some(name)).unwrap();
            assert_eq!(out.format, DocumentFormat::Docx, "{name}");
        }
    }
}
