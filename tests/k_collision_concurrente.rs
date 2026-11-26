//! Témoin DISCRIMINANT de la collision concurrente de K.
//!
//! Ce fichier n'utilise QUE l'API publique commune à la base `a583633f` et à la
//! version corrigée (`ouvrir` / `emettre`) : il compile donc sur les DEUX arbres,
//! et c'est là sa valeur — le même code de témoin doit échouer sur l'un et
//! passer sur l'autre.
//!
//! Interleaving DÉTERMINISTE : les deux processus ouvrent l'autorité (donc
//! lisent le compteur) AVANT la barrière, puis émettent après. Aucun hasard,
//! aucun `sleep` de course : l'ordre est imposé par des fichiers de rendez-vous.
//!
//! Sur la base, `emettre` publie la valeur lue à l'OUVERTURE : les deux
//! processus rendent le MÊME K et l'état durable est écrit deux fois à la même
//! valeur. C'est la collision, reproduite à volonté.

// Ce témoin n'utilise que `this::autorite_k`, donc il tourne SANS la feature
// `obrain` (aucun type `obrain` n'intervient dans l'allocation de K).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use this::autorite_k::AutoriteK;

const DELAI: Duration = Duration::from_secs(120);

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "kcollision_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

fn etat_durable(chemin: &Path) -> u128 {
    let octets = std::fs::read(chemin).expect("lecture de l'état durable");
    assert_eq!(octets.len(), 16, "état = un u128 little-endian");
    let mut b = [0u8; 16];
    b.copy_from_slice(&octets);
    u128::from_le_bytes(b)
}

fn attendre(chemin: &Path, delai: Duration) -> bool {
    let debut = Instant::now();
    while debut.elapsed() < delai {
        if chemin.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn env(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant"))
}

fn est_worker() -> bool {
    std::env::var("K_COLL_MODE").is_ok()
}

fn commande(exe: &Path, test: &str, d: &Path, mode: &str, role: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--exact", test])
        .env("K_COLL_MODE", mode)
        .env("K_COLL_DIR", d)
        .env("K_COLL_ROLE", role);
    cmd
}

fn executer_worker() -> bool {
    if !est_worker() {
        return false;
    }
    let d = PathBuf::from(env("K_COLL_DIR"));
    let role = std::env::var("K_COLL_ROLE").unwrap_or_else(|_| "w".to_string());
    let chemin = d.join("autorite.k");
    match env("K_COLL_MODE").as_str() {
        "emis_barriere" => {
            // Lecture de l'état AVANT la barrière : c'est la fenêtre du défaut.
            let mut ak = AutoriteK::ouvrir(chemin).expect("ouverture");
            std::fs::write(d.join(format!("{role}.pret")), b"").expect("marqueur pret");
            if !attendre(&d.join("go"), DELAI) {
                panic!("barrière jamais levée pour {role}");
            }
            let k = ak.emettre().expect("émission");
            std::fs::write(d.join(format!("{role}.k")), k.to_string()).expect("écriture k");
        }
        "emis_serie" => {
            let n: usize = env("K_COLL_N").parse().expect("K_COLL_N");
            let mut ak = AutoriteK::ouvrir(chemin).expect("ouverture");
            let mut sortie = String::new();
            for _ in 0..n {
                sortie.push_str(&format!("{}\n", ak.emettre().expect("émission")));
            }
            std::fs::write(d.join(format!("{role}.ks")), sortie).expect("écriture ks");
        }
        autre => panic!("mode worker inconnu : {autre}"),
    }
    true
}

#[test]
fn collision_concurrente_interleaving_deterministe() {
    if executer_worker() {
        return;
    }
    let d = dir("collision");
    let exe = std::env::current_exe().expect("exe de test");

    let mut a = commande(
        &exe,
        "collision_concurrente_interleaving_deterministe",
        &d,
        "emis_barriere",
        "a",
    )
    .spawn()
    .expect("spawn a");
    let mut b = commande(
        &exe,
        "collision_concurrente_interleaving_deterministe",
        &d,
        "emis_barriere",
        "b",
    )
    .spawn()
    .expect("spawn b");

    assert!(attendre(&d.join("a.pret"), DELAI), "a n'a pas ouvert");
    assert!(attendre(&d.join("b.pret"), DELAI), "b n'a pas ouvert");
    std::fs::write(d.join("go"), b"").expect("levée de la barrière");

    assert!(a.wait().expect("attente a").success(), "worker a");
    assert!(b.wait().expect("attente b").success(), "worker b");

    let ka: u128 = std::fs::read_to_string(d.join("a.k"))
        .expect("K de a")
        .trim()
        .parse()
        .expect("u128");
    let kb: u128 = std::fs::read_to_string(d.join("b.k"))
        .expect("K de b")
        .trim()
        .parse()
        .expect("u128");
    let durable = etat_durable(&d.join("autorite.k"));
    println!("MESURÉ : K(a)={ka}  K(b)={kb}  état durable={durable}");

    let mut vus = [ka, kb];
    vus.sort();
    assert_eq!(
        vus,
        [1, 2],
        "COLLISION : les deux processus ont publié le même K ({ka} et {kb}) ; \
         l'état durable vaut {durable} au lieu de 3"
    );
    assert_eq!(durable, 3, "l'état durable doit compter les DEUX émissions");

    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn contention_multiprocessus_sans_doublon() {
    if executer_worker() {
        return;
    }
    let d = dir("contention");
    let exe = std::env::current_exe().expect("exe de test");
    let roles = ["a", "b", "c", "d"];
    let n = 20usize;

    let mut enfants = Vec::new();
    for role in roles {
        let enfant = commande(
            &exe,
            "contention_multiprocessus_sans_doublon",
            &d,
            "emis_serie",
            role,
        )
        .env("K_COLL_N", n.to_string())
        .spawn()
        .expect("spawn");
        enfants.push(enfant);
    }
    for (i, enfant) in enfants.iter_mut().enumerate() {
        let statut = enfant.wait().expect("attente");
        assert!(
            statut.success(),
            "worker {} a échoué : {statut:?}",
            roles[i]
        );
    }

    let mut tous = Vec::new();
    for role in roles {
        let contenu = std::fs::read_to_string(d.join(format!("{role}.ks"))).expect("liste des K");
        for ligne in contenu.lines() {
            tous.push(ligne.trim().parse::<u128>().expect("K u128"));
        }
    }
    let total = roles.len() * n;
    let mut tries = tous.clone();
    tries.sort_unstable();
    tries.dedup();
    println!(
        "MESURÉ : {} K émis, {} distincts ({} attendus)",
        tous.len(),
        tries.len(),
        total
    );
    assert_eq!(tous.len(), total, "aucune émission perdue");
    assert_eq!(
        tries.len(),
        total,
        "DOUBLONS : {} K seulement sont distincts sur {} émissions",
        tries.len(),
        total
    );
    assert_eq!(etat_durable(&d.join("autorite.k")), total as u128 + 1);

    let _ = std::fs::remove_dir_all(&d);
}
