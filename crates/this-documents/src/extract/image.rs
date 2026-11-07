//! Reconnaissance d'image par magic bytes.
//!
//! Une image n'a pas de texte : il n'y a donc **pas** d'extracteur ici. Une image
//! part telle quelle au modèle, avec son type de média ; ce module sert à savoir
//! *que* c'en est une, et laquelle, sans se fier à l'extension.

/// Type de média d'une image reconnue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaType {
    Png,
    Jpeg,
    Gif,
    Webp,
    Heic,
}

impl MediaType {
    /// Le type MIME, tel qu'on le met dans un en-tête ou un bloc de contenu.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Heic => "image/heic",
        }
    }

    /// Reconnaît une image à ses premiers octets.
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            return Some(Self::Jpeg);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        // WebP : conteneur RIFF « RIFF <taille> WEBP ».
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some(Self::Webp);
        }
        // HEIC : boîte ISO-BMFF « <taille> ftyp <marque> » avec une marque HEVC.
        // Les marques génériques (mif1, msf1) sont refusées : AVIF les emploie aussi.
        if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
            let brand = &bytes[8..12];
            if [&b"heic"[..], b"heix", b"hevc", b"hevx", b"heim", b"heis"].contains(&brand) {
                return Some(Self::Heic);
            }
        }
        None
    }
}

/// Le type de média de `bytes` si c'est une image reconnue.
pub fn sniff_image(bytes: &[u8]) -> Option<MediaType> {
    MediaType::from_magic(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // PNG et JPEG sont les deux cas courants ; la signature complète du PNG est exigée.
    #[test]
    fn sniff_recognises_png_and_jpeg() {
        assert_eq!(
            sniff_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            Some(MediaType::Png)
        );
        assert_eq!(
            sniff_image(&[0xff, 0xd8, 0xff, 0xe0, 0, 0x10]),
            Some(MediaType::Jpeg)
        );
        assert_eq!(MediaType::Png.as_str(), "image/png");
        assert_eq!(MediaType::Jpeg.as_str(), "image/jpeg");
    }

    #[test]
    fn sniff_recognises_gif_webp_and_heic() {
        assert_eq!(sniff_image(b"GIF89a\x01\0"), Some(MediaType::Gif));
        assert_eq!(
            sniff_image(b"RIFF\x24\0\0\0WEBPVP8 "),
            Some(MediaType::Webp)
        );
        assert_eq!(
            sniff_image(b"\0\0\0\x18ftypheic\0\0\0\0"),
            Some(MediaType::Heic)
        );
    }

    // Un texte, même nommé image.png, n'est pas une image : seuls les octets comptent.
    #[test]
    fn sniff_refuses_plain_text() {
        assert_eq!(sniff_image(b"just some text, not a picture"), None);
        assert_eq!(sniff_image(b""), None);
    }

    // RIFF seul est aussi WAV/AVI ; ftyp seul est aussi MP4/AVIF. Ne pas les prendre.
    #[test]
    fn sniff_does_not_claim_other_containers() {
        assert_eq!(sniff_image(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_image(b"\0\0\0\x18ftypmp42\0\0\0\0"), None);
        assert_eq!(sniff_image(b"\0\0\0\x18ftypmif1\0\0\0\0"), None);
    }
}
