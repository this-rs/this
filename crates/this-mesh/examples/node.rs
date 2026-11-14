//! Nœud de maillage headless, pour tester l'appairage entre deux machines sans
//! Tauri ni trousseau.
//!
//! ```text
//! node listen <ip:port> <fichier-cle> [--oui]
//! node pair   <ip:port> <fichier-cle> [--oui]
//! ```
//!
//! `--oui` confirme le SAS automatiquement (mode test) ; sinon on attend `y` sur stdin.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::Path;

use ed25519_dalek::{SigningKey, VerifyingKey};
use this_mesh::transport::{self, AppairageEnCours, Identite};
use this_mesh::{DeviceId, Limits, Peers};
use this_vfs::layout::AnchorKeyRole;

fn id_maillage(cle: &VerifyingKey) -> DeviceId {
    let mut id = String::from("device-");
    for o in &cle.to_bytes()[..4] {
        id.push_str(&format!("{o:02x}"));
    }
    DeviceId::parse(&id).expect("device- + 8 hex")
}

/// Lit la clé du fichier, ou en tire une de `/dev/urandom` et l'écrit.
fn charger_cle(chemin: &Path) -> SigningKey {
    let mut graine = [0u8; 32];
    if chemin.exists() {
        let octets = std::fs::read(chemin).expect("lecture du fichier de clé");
        assert_eq!(octets.len(), 32, "fichier de clé : 32 octets attendus");
        graine.copy_from_slice(&octets);
    } else {
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut graine))
            .expect("lecture de /dev/urandom");
        std::fs::write(chemin, graine).expect("écriture du fichier de clé");
    }
    SigningKey::from_bytes(&graine)
}

fn confirmer(oui: bool) -> bool {
    if oui {
        return true;
    }
    print!("confirmer le code ? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut ligne = String::new();
    std::io::stdin().read_line(&mut ligne).is_ok() && ligne.trim().eq_ignore_ascii_case("y")
}

async fn traiter(demande: AppairageEnCours, oui: bool) -> bool {
    let pair = demande.pair().clone();
    println!("SAS {} pair {}", demande.code(), pair);
    let accepte = confirmer(oui);
    match demande.decider(accepte).await {
        Ok(_) if accepte => {
            println!("ADMIS {pair}");
            true
        }
        Ok(_) => {
            println!("REFUS {pair} refusé localement");
            false
        }
        Err(e) => {
            println!("REFUS {pair} {e}");
            false
        }
    }
}

fn usage() -> ! {
    eprintln!("usage : node listen|pair <ip:port> <fichier-cle> [--oui]");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        usage();
    }
    let mode = args[0].as_str();
    let addr: SocketAddr = args[1].parse().unwrap_or_else(|_| usage());
    let chemin = Path::new(&args[2]);
    let oui = args.iter().skip(3).any(|a| a == "--oui");

    let cle = charger_cle(chemin);
    let id = id_maillage(&cle.verifying_key());
    let identite = Identite::de_cle(&cle, &id).expect("identité TLS");
    println!("MOI {id} keyfile {}", chemin.display());

    match mode {
        "listen" => {
            let (mut flux, mut demandes) = transport::ecouter_avec_appairage(
                addr,
                &identite,
                Peers::new(),
                Limits::default(),
                AnchorKeyRole::Shared,
                transport::DELAI_APPAIRAGE,
            )
            .await
            .expect("écoute");
            println!("ECOUTE {}", flux.local_addr());
            loop {
                tokio::select! {
                    Some(demande) = demandes.recv() => { traiter(demande, oui).await; }
                    Some((de, _verdict)) = flux.prochain() => { println!("MSG de {de}"); }
                    else => break,
                }
            }
        }
        "pair" => {
            let demande = transport::appairer_vers(
                addr,
                &identite,
                AnchorKeyRole::Shared,
                transport::DELAI_APPAIRAGE,
            )
            .await;
            match demande {
                Ok(d) => {
                    if !traiter(d, oui).await {
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    println!("REFUS {addr} {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}
