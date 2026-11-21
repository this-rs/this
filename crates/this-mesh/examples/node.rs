//! Nœud de maillage headless, pour tester l'appairage entre deux machines sans
//! Tauri ni trousseau.
//!
//! ```text
//! node listen <ip:port> <fichier-cle> [--oui]
//! node pair   <ip:port> <fichier-cle> [--oui] [--send]
//! ```
//!
//! `--oui` confirme le SAS automatiquement (mode test) ; sinon on attend `y` sur stdin.
//! `--send` (côté `pair`) envoie, une fois le pair admis, un vrai message signé à
//! travers sa porte : c'est la preuve que l'appairage ouvre bien sur du trafic.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{SigningKey, VerifyingKey};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use serde_json::json;
use this_mesh::transport::{self, AppairageEnCours, Epingle, Identite};
use this_mesh::{DeviceId, Limits, Message, MessageType, Peers, Raison, Verdict};
use this_vfs::layout::AnchorKeyRole;
use uuid::Uuid;

/// Nombre d'essais d'envoi (voir [`envoyer_message`]) et pause entre deux.
const ESSAIS_ENVOI: u32 = 6;
const PAUSE_ENVOI: Duration = Duration::from_millis(200);

/// Un identifiant d'enveloppe neuf (l'anti-rejeu de la porte est indexé dessus) :
/// nanosecondes dans la moitié haute, compteur de processus dans la moitié basse.
/// `Uuid::from_u128` évite de dépendre de la feature `v4` du crate `uuid`.
fn nouvel_id() -> Uuid {
    static COMPTEUR: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    let n = COMPTEUR.fetch_add(1, Ordering::Relaxed);
    Uuid::from_u128((nanos << 64) | u128::from(n))
}

/// L'`horloge` de l'enveloppe : millisecondes depuis l'epoch.
fn horloge_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Le verdict en clair, pour le journal du test.
fn verdict_nom(v: &Verdict) -> String {
    match v {
        Verdict::Admis(m) => format!("admis (id {}, type {})", m.id, m.kind.wire()),
        Verdict::DejaVu(id) => format!("déjà vu ({id})"),
        Verdict::Ignore(t) => format!("ignoré (type inconnu {t})"),
        Verdict::Refuse(r) => format!("refusé ({r})"),
    }
}

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

async fn traiter(
    demande: AppairageEnCours,
    oui: bool,
    peers: &Peers,
    envoi: Option<(SocketAddr, &Identite, &SigningKey)>,
) -> bool {
    let pair = demande.pair().clone();
    // La clé du pair, capturée AVANT `decider` (qui consomme l'appairage) : c'est
    // elle qu'on épingle pour joindre sa porte, et elle ne vient que du certificat
    // TLS présenté pendant l'appairage — jamais d'un message.
    let vk_pair = demande.cle_pair();
    println!("SAS {} pair {}", demande.code(), pair);
    let accepte = confirmer(oui);
    match demande.decider(accepte).await {
        Ok(admission) if accepte => {
            // L'`Admission` ne s'écrit pas toute seule : sans cette inscription, la
            // porte du nœud refuserait le premier message du pair (`CleInconnue`).
            match admission.inscrire_verifie(peers) {
                Ok(()) => println!("INSCRIT {pair} registre {} pair(s)", peers.len()),
                Err(e) => println!("REFUS {pair} inscription : {e}"),
            }
            println!("ADMIS {pair}");
            if let Some((addr, identite, cle)) = envoi {
                envoyer_message(addr, identite, cle, &pair, &vk_pair).await;
            }
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

/// Envoie un vrai message signé au pair qui vient d'être admis, sur une **nouvelle
/// connexion** épinglée sur sa clé (`Epingle::cle`) — c'est le chemin qu'emprunte
/// tout trafic d'application, pas la connexion d'appairage.
///
/// Le pair n'inscrit l'admission qu'après sa propre décision : le premier envoi
/// peut donc le trouver encore inconnu (`CleInconnue`). On réessaie, borné.
async fn envoyer_message(
    addr: SocketAddr,
    identite: &Identite,
    cle: &SigningKey,
    pair: &DeviceId,
    vk_pair: &VerifyingKey,
) {
    let epingle = Epingle::cle(vk_pair);
    let message = Message::signed(
        cle,
        nouvel_id(),
        identite.id().clone(),
        pair.clone(),
        // Type qu'une porte admet sans autre règle qu'une clé active : la
        // proposition d'agent ne demande ni droit ni espace préalable.
        MessageType::AgentPropose,
        horloge_ms(),
        json!({"objet": "bonjour", "texte": "premier message apres appairage"}),
    )
    .expect("charge canonique : entiers et chaînes seulement");

    for essai in 1..=ESSAIS_ENVOI {
        let verdict = match transport::envoyer(addr, identite, &epingle, &message).await {
            Ok(v) => v,
            Err(e) => {
                println!("ENVOI erreur {e}");
                return;
            }
        };
        let inconnu_encore = matches!(verdict, Verdict::Refuse(Raison::CleInconnue(_)));
        let nom = verdict_nom(&verdict);
        if essai == 1 {
            println!("ENVOI {nom}");
        } else {
            println!("ENVOI essai {essai} {nom}");
        }
        if !inconnu_encore {
            return;
        }
        tokio::time::sleep(PAUSE_ENVOI).await;
    }
}

/// L'IP LAN primaire : celle que le noyau choisirait pour sortir (aucun paquet
/// n'est émis, `connect` sur UDP ne fait que choisir la route).
fn ip_lan() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// Annonce `_atelier._tcp.local.` en mDNS ; le démon doit rester vivant.
fn annoncer(id: &DeviceId, ecoute: SocketAddr) -> Option<ServiceDaemon> {
    let ip = ip_lan().or_else(|| (!ecoute.ip().is_unspecified()).then(|| ecoute.ip()))?;
    let hote = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|h| h.trim().trim_end_matches(".local").to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "atelier-node".into());
    let instance = format!("Atelier-{id}");
    let props = [("id", id.to_string())];
    let info = ServiceInfo::new(
        "_atelier._tcp.local.",
        &instance,
        &format!("{hote}.local."),
        ip,
        ecoute.port(),
        &props[..],
    )
    .ok()?;
    let demon = ServiceDaemon::new().ok()?;
    demon.register(info).ok()?;
    println!("ANNONCE _atelier._tcp {instance} {ip}:{}", ecoute.port());
    Some(demon)
}

fn usage() -> ! {
    eprintln!("usage : node listen|pair <ip:port> <fichier-cle> [--oui] [--send]");
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
    let envoi_demande = args.iter().skip(3).any(|a| a == "--send");

    let cle = charger_cle(chemin);
    let id = id_maillage(&cle.verifying_key());
    let identite = Identite::de_cle(&cle, &id).expect("identité TLS");
    println!("MOI {id} keyfile {}", chemin.display());

    match mode {
        "listen" => {
            let peers = Peers::new();
            let (mut flux, mut demandes) = transport::ecouter_avec_appairage(
                addr,
                &identite,
                // Le même handle que la porte : ce que `traiter` inscrit est vu du
                // prochain message, sans rouvrir le listener.
                peers.clone(),
                Limits::default(),
                AnchorKeyRole::Shared,
                transport::DELAI_APPAIRAGE,
            )
            .await
            .expect("écoute");
            println!("ECOUTE {}", flux.local_addr());
            // Vivant jusqu'à la fin de la boucle : le lâcher retire l'annonce.
            let _annonce = annoncer(&id, flux.local_addr());
            loop {
                tokio::select! {
                    Some(demande) = demandes.recv() => { traiter(demande, oui, &peers, None).await; }
                    Some((de, _verdict)) = flux.prochain() => { println!("MSG de {de}"); }
                    else => break,
                }
            }
        }
        "pair" => {
            let peers = Peers::new();
            let demande = transport::appairer_vers(
                addr,
                &identite,
                AnchorKeyRole::Shared,
                transport::DELAI_APPAIRAGE,
            )
            .await;
            match demande {
                Ok(d) => {
                    let envoi = envoi_demande.then_some((addr, &identite, &cle));
                    if !traiter(d, oui, &peers, envoi).await {
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
