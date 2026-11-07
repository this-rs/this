//! La projection générique, en données, d'un document et de ses morceaux.
//!
//! Ce crate **produit des données**, l'application les écrit : aucun appel à un
//! store ici. La forme suit la convention maison — label de nœud en minuscules,
//! arête en MAJUSCULES, propriétés texte — pour qu'un backend de graphe quelconque
//! puisse les ranger sans connaître ce crate.

use sha2::{Digest, Sha256};

use crate::ByteInterval;
use crate::chunk::TextChunk;
use crate::extract::DocumentFormat;

/// Label des nœuds document.
pub const DOCUMENT_LABEL: &str = "document";
/// Label des nœuds morceau.
pub const CHUNK_LABEL: &str = "chunk";
/// Arête du propriétaire vers un document.
pub const HAS_DOCUMENT: &str = "HAS_DOCUMENT";
/// Arête d'un document vers ses morceaux.
pub const HAS_CHUNK: &str = "HAS_CHUNK";

/// Un enregistrement lu en propriétés texte est invalide.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    #[error("missing property `{0}`")]
    Missing(&'static str),
    #[error("property `{key}` is not valid: {detail}")]
    Invalid { key: &'static str, detail: String },
}

/// SHA-256 des octets d'origine, en hexadécimal minuscule : l'identité du contenu.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

type Props = [(String, String)];

fn get<'a>(props: &'a Props, key: &'static str) -> Result<&'a str, RecordError> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .ok_or(RecordError::Missing(key))
}

fn get_opt<'a>(props: &'a Props, key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn parse_num<T: std::str::FromStr>(props: &Props, key: &'static str) -> Result<T, RecordError>
where
    T::Err: std::fmt::Display,
{
    get(props, key)?
        .parse::<T>()
        .map_err(|e| RecordError::Invalid {
            key,
            detail: e.to_string(),
        })
}

/// Un document ingéré.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DocumentRecord {
    /// Fourni par l'appelant (ce crate n'a ni horloge ni hasard).
    pub id: String,
    /// Nom donné à l'envoi. Affichage seulement : ce n'est pas l'identité.
    pub filename: String,
    pub format: DocumentFormat,
    pub mime_type: Option<String>,
    /// Voir [`sha256_hex`] : l'identité du contenu, calculée sur les octets d'origine.
    pub sha256: String,
    pub size_bytes: u64,
    /// 0 pour un format sans pagination.
    pub page_count: usize,
    pub chunk_count: usize,
    /// Un extracteur a-t-il lu ce fichier ? Faux pour une image ou une pièce opaque.
    pub extracted: bool,
    pub warnings: Vec<String>,
}

impl DocumentRecord {
    pub fn to_props(&self) -> Vec<(String, String)> {
        let mut p = vec![
            ("id".to_string(), self.id.clone()),
            ("filename".to_string(), self.filename.clone()),
            ("format".to_string(), self.format.as_str().to_string()),
            ("sha256".to_string(), self.sha256.clone()),
            ("size_bytes".to_string(), self.size_bytes.to_string()),
            ("page_count".to_string(), self.page_count.to_string()),
            ("chunk_count".to_string(), self.chunk_count.to_string()),
            ("extracted".to_string(), self.extracted.to_string()),
        ];
        if let Some(m) = &self.mime_type {
            p.push(("mime_type".to_string(), m.clone()));
        }
        // Un avertissement par propriété : pas de séparateur à échapper, et un
        // avertissement vide reste distinct de « aucun avertissement ».
        for (i, w) in self.warnings.iter().enumerate() {
            p.push((format!("warning.{i}"), w.clone()));
        }
        p
    }

    pub fn from_props(props: &[(String, String)]) -> Result<Self, RecordError> {
        let format_s = get(props, "format")?;
        let format = DocumentFormat::parse(format_s).ok_or_else(|| RecordError::Invalid {
            key: "format",
            detail: format!("unknown format `{format_s}`"),
        })?;
        let mut warnings = Vec::new();
        while let Some(w) = get_opt(props, &format!("warning.{}", warnings.len())) {
            warnings.push(w.to_string());
        }
        Ok(Self {
            id: get(props, "id")?.to_string(),
            filename: get(props, "filename")?.to_string(),
            format,
            mime_type: get_opt(props, "mime_type").map(str::to_string),
            sha256: get(props, "sha256")?.to_string(),
            size_bytes: parse_num(props, "size_bytes")?,
            page_count: parse_num(props, "page_count")?,
            chunk_count: parse_num(props, "chunk_count")?,
            extracted: parse_num(props, "extracted")?,
            warnings,
        })
    }
}

/// Un morceau d'un document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChunkRecord {
    pub id: String,
    pub document_id: String,
    /// Position dans le document, à partir de 0.
    pub ordinal: usize,
    pub text: String,
    /// Octets dans le *texte extrait*, semi-ouvert `[start, end)`.
    pub start_byte: usize,
    pub end_byte: usize,
    /// Page (à partir de 1), si le format est paginé.
    pub page: Option<usize>,
}

impl ChunkRecord {
    pub fn interval(&self) -> ByteInterval {
        ByteInterval::new(self.start_byte, self.end_byte)
    }

    pub fn to_props(&self) -> Vec<(String, String)> {
        let mut p = vec![
            ("id".to_string(), self.id.clone()),
            ("document_id".to_string(), self.document_id.clone()),
            ("ordinal".to_string(), self.ordinal.to_string()),
            ("text".to_string(), self.text.clone()),
            ("start_byte".to_string(), self.start_byte.to_string()),
            ("end_byte".to_string(), self.end_byte.to_string()),
        ];
        if let Some(page) = self.page {
            p.push(("page".to_string(), page.to_string()));
        }
        p
    }

    pub fn from_props(props: &[(String, String)]) -> Result<Self, RecordError> {
        let page = match get_opt(props, "page") {
            None => None,
            Some(s) => Some(s.parse::<usize>().map_err(|e| RecordError::Invalid {
                key: "page",
                detail: e.to_string(),
            })?),
        };
        let start_byte: usize = parse_num(props, "start_byte")?;
        let end_byte: usize = parse_num(props, "end_byte")?;
        if start_byte > end_byte {
            return Err(RecordError::Invalid {
                key: "end_byte",
                detail: format!("end {end_byte} is before start {start_byte}"),
            });
        }
        Ok(Self {
            id: get(props, "id")?.to_string(),
            document_id: get(props, "document_id")?.to_string(),
            ordinal: parse_num(props, "ordinal")?,
            text: get(props, "text")?.to_string(),
            start_byte,
            end_byte,
            page,
        })
    }
}

/// Fait d'un morceau du découpeur un enregistrement persistable.
///
/// `page` vient de [`ExtractedText::page_of`](crate::ExtractedText::page_of) sur le
/// début du morceau (`None` pour un format sans pagination). L'identifiant est
/// **déterministe** — `"<document_id>:<ordinal>"` — : réécrire le même document
/// retombe sur les mêmes nœuds au lieu d'en créer de nouveaux.
pub fn chunk_to_record(
    chunk: &TextChunk,
    ordinal: usize,
    page: Option<usize>,
    document_id: &str,
) -> ChunkRecord {
    ChunkRecord {
        id: format!("{document_id}:{ordinal}"),
        document_id: document_id.to_string(),
        ordinal,
        text: chunk.text.clone(),
        start_byte: chunk.interval.start,
        end_byte: chunk.interval.end,
        page,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> DocumentRecord {
        DocumentRecord {
            id: "d1".into(),
            filename: "rapport final = v2.pdf".into(),
            format: DocumentFormat::Pdf,
            mime_type: Some("application/pdf".into()),
            sha256: sha256_hex(b"abc"),
            size_bytes: 3,
            page_count: 7,
            chunk_count: 12,
            extracted: true,
            warnings: vec![
                "page 3 illisible".into(),
                "".into(),
                "ligne\nsuivante".into(),
            ],
        }
    }

    // La maison écrit des propriétés texte : un aller-retour qui perdrait un champ
    // (avertissement vide, saut de ligne, mime absent) corromprait le graphe en silence.
    #[test]
    fn document_record_survives_a_props_round_trip() {
        let d = doc();
        assert_eq!(DocumentRecord::from_props(&d.to_props()).unwrap(), d);

        let bare = DocumentRecord {
            mime_type: None,
            warnings: vec![],
            format: DocumentFormat::Image,
            extracted: false,
            ..doc()
        };
        assert_eq!(DocumentRecord::from_props(&bare.to_props()).unwrap(), bare);
    }

    // Texte accentué, multilignes, avec des `=` et des guillemets : rien ne doit être
    // échappé ni tronqué, et la page absente doit rester absente.
    #[test]
    fn chunk_record_survives_a_props_round_trip() {
        let c = ChunkRecord {
            id: "d1:4".into(),
            document_id: "d1".into(),
            ordinal: 4,
            text: "Voilà \"un\" texte=accentué\n日本語 🎯".into(),
            start_byte: 10,
            end_byte: 60,
            page: Some(2),
        };
        assert_eq!(ChunkRecord::from_props(&c.to_props()).unwrap(), c);
        let unpaged = ChunkRecord { page: None, ..c };
        assert_eq!(
            ChunkRecord::from_props(&unpaged.to_props()).unwrap(),
            unpaged
        );
    }

    // Une propriété manquante ou illisible est une erreur nommée, pas une valeur par défaut.
    #[test]
    fn from_props_reports_missing_and_invalid_properties() {
        let mut p = doc().to_props();
        p.retain(|(k, _)| k != "sha256");
        assert_eq!(
            DocumentRecord::from_props(&p),
            Err(RecordError::Missing("sha256"))
        );

        let mut p = doc().to_props();
        for (k, v) in p.iter_mut() {
            if k == "format" {
                *v = "docm".into();
            }
        }
        assert!(matches!(
            DocumentRecord::from_props(&p),
            Err(RecordError::Invalid { key: "format", .. })
        ));

        let mut p = ChunkRecord::from_props(&[]).unwrap_err();
        assert_eq!(p, RecordError::Missing("start_byte"));
        p = ChunkRecord::from_props(&[
            ("id".into(), "x".into()),
            ("document_id".into(), "d".into()),
            ("ordinal".into(), "0".into()),
            ("text".into(), "t".into()),
            ("start_byte".into(), "9".into()),
            ("end_byte".into(), "3".into()),
        ])
        .unwrap_err();
        assert!(matches!(
            p,
            RecordError::Invalid {
                key: "end_byte",
                ..
            }
        ));
    }

    // L'identifiant est déterministe et l'intervalle dérivé redonne le morceau.
    #[test]
    fn chunk_to_record_keeps_provenance_and_a_deterministic_id() {
        let src = "Un. Deux. Trois.";
        let chunk = TextChunk {
            text: "Deux. ".into(),
            interval: ByteInterval::new(4, 10),
        };
        let r = chunk_to_record(&chunk, 1, Some(3), "doc-9");
        assert_eq!(r.id, "doc-9:1");
        assert_eq!(r.document_id, "doc-9");
        assert_eq!(r.page, Some(3));
        assert_eq!(r.interval().slice(src), Some("Deux. "));
        assert_eq!(r, chunk_to_record(&chunk, 1, Some(3), "doc-9"));
    }

    // Vecteur connu (sha256("abc")) : l'identité du contenu doit être la bonne.
    #[test]
    fn sha256_hex_matches_the_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn labels_follow_the_house_convention() {
        assert_eq!(DOCUMENT_LABEL, "document");
        assert_eq!(CHUNK_LABEL, "chunk");
        assert_eq!(HAS_DOCUMENT, "HAS_DOCUMENT");
        assert_eq!(HAS_CHUNK, "HAS_CHUNK");
    }
}
