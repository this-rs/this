//! Cadrage des messages sur le fil : longueur (4 octets, big-endian) puis les
//! octets de [`crate::Message::to_bytes`].
//!
//! La longueur annoncée est contrôlée **avant** toute allocation du corps : un
//! pair qui annonce 4 Gio ne fait jamais allouer un tampon de cette taille, la
//! lecture échoue immédiatement avec [`CadrageError::TropGros`].

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum CadrageError {
    #[error("longueur annoncée {taille} octets > plafond {max}")]
    TropGros { taille: usize, max: usize },
    #[error("lecture/écriture sur le fil : {0}")]
    Io(#[from] std::io::Error),
}

/// Écrit `octets` préfixé de sa longueur sur 4 octets big-endian.
pub async fn ecrire<S: AsyncWrite + Unpin>(s: &mut S, octets: &[u8]) -> std::io::Result<()> {
    let len = octets.len() as u32;
    s.write_all(&len.to_be_bytes()).await?;
    s.write_all(octets).await?;
    Ok(())
}

/// Lit un message cadré. La longueur est refusée **avant lecture** du corps si
/// elle dépasse `ceiling`.
pub async fn lire<S: AsyncRead + Unpin>(
    s: &mut S,
    ceiling: usize,
) -> Result<Vec<u8>, CadrageError> {
    let mut len_buf = [0u8; 4];
    s.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > ceiling {
        return Err(CadrageError::TropGros {
            taille: len,
            max: ceiling,
        });
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, duplex};

    #[tokio::test]
    async fn une_longueur_au_dela_du_plafond_est_refusee_avant_allocation() {
        // `duplex(64)` : on n'écrit que 4 octets (la longueur), jamais le corps.
        let (mut client, mut serveur) = duplex(64);
        // Annonce u32::MAX octets — aucun tampon de cette taille ne doit être
        // alloué : `lire` rend avant le `vec![0; len]`.
        client.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        let err = lire(&mut serveur, 1024).await.unwrap_err();
        match err {
            CadrageError::TropGros { taille, max } => {
                assert_eq!(taille, u32::MAX as usize);
                assert_eq!(max, 1024);
                assert!(max < taille);
            }
            other => panic!("attendu TropGros, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn un_message_dans_les_limites_est_relu_a_l_identique() {
        let (mut client, mut serveur) = duplex(1024);
        let corps = b"un message du maillage".to_vec();
        ecrire(&mut client, &corps).await.unwrap();
        let relu = lire(&mut serveur, 1024).await.unwrap();
        assert_eq!(relu, corps);
    }
}
