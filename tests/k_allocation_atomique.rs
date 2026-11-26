//! Correction `AutoriteK` — allocation atomique, verrou, domaine.
//!
//! Ce fichier porte les témoins DISCRIMINANTS de la correction. Chacun échoue si
//! la propriété qu'il énonce manque :
//!
//!   1. `collision_concurrente_interleaving_deterministe` — deux PROCESSUS
//!      lisent le même compteur, calculent la même identité, publient. Sur
//!      `a583633f` (lecture à l'ouverture) les deux K sont ÉGAUX : la collision
//!      est reproduite, puis éliminée par l'allocation atomique.
//!   2. `contention_multiprocessus_sans_doublon` — 4 processus × 20 émissions :
//!      aucun K en double, aucun trou inattendu, compteur final exact.
//!   3. `interruption_apres_persistance_ne_reattribue_jamais` — le processus
//!      meurt entre la persistance et la publication : le K est BRÛLÉ, jamais
//!      réattribué, et le verrou n'est pas orphelin.
//!   4. `corruption_refusee_sans_reinitialisation` — état tronqué/nul/disparu :
//!      refus nommé, octets préservés, jamais de retour à 1.
//!   5. `compteur_regresse_refuse_dans_un_processus_vivant` — état restauré sous
//!      les pieds de l'instance : refus, pas de réattribution.
//!   6. `verrou_porte_sur_un_objet_distinct_de_l_etat_renomme` — le `rename` de
//!      l'état change l'inode de l'ÉTAT et pas celui du VERROU.
//!   7. `obrr_v1_string_jamais_reinterprete_en_u128` — un `k` String du format V1
//!      reste en quarantaine : aucune réinterprétation silencieuse en u128.
//!   8. `domaine_canonique_deux_fichiers_deux_autorites` — mesure la NON-unicité
//!      globale : deux domaines émetteurs indépendants émettent les mêmes K.
//!
//! Les scénarios multi-processus traversent une frontière de PROCESSUS réelle
//! (`std::process::Command` rejouant ce binaire avec `--exact`), pas un simple
//! close/open.

#![cfg(feature = "obrain")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use obrain_common::types::NodeId;
use this::autorite_k::{AutoriteK, EtapeEmission};
use this::reconciliation::{
    DecodageEnregistrement, ENREGISTREMENT_MAGIC, ENREGISTREMENT_VERSION,
    ENREGISTREMENT_VERSION_LEGACY, EnregistrementOriginal, EtatEnregistrement, MotifIllisibilite,
    qualifier,
};

const DELAI: Duration = Duration::from_secs(120);

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "kalloc_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

/// L'état durable, lu SANS passer par l'autorité (octet = seule source).
fn etat_durable(chemin: &Path) -> u128 {
    let octets = std::fs::read(chemin).expect("lecture de l'état durable");
    assert_eq!(octets.len(), 16, "état = un u128 little-endian");
    let mut b = [0u8; 16];
    b.copy_from_slice(&octets);
    u128::from_le_bytes(b)
}

// ── Protocole worker (le même binaire, rejoué avec `--exact`) ─────────────────

fn est_worker() -> bool {
    std::env::var("K_ALLOC_MODE").is_ok()
}

fn env(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant"))
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

/// Barrière inter-processus : le worker annonce `pret`, puis attend `go`.
fn barriere(d: &Path, role: &str) {
    std::fs::write(d.join(format!("{role}.pret")), b"").expect("marqueur pret");
    if !attendre(&d.join("go"), DELAI) {
        panic!("barrière jamais levée pour {role}");
    }
}

fn executer_worker() -> bool {
    if !est_worker() {
        return false;
    }
    let d = PathBuf::from(env("K_ALLOC_DIR"));
    let role = std::env::var("K_ALLOC_ROLE").unwrap_or_else(|_| "w".to_string());
    let chemin = d.join("autorite.k");
    match env("K_ALLOC_MODE").as_str() {
        // Le même interleaving que le défaut : ouverture AVANT toute émission.
        "emis_barriere" => {
            let mut ak = AutoriteK::ouvrir(chemin).expect("ouverture");
            barriere(&d, &role);
            let k = ak.emettre().expect("émission");
            std::fs::write(d.join(format!("{role}.k")), k.to_string()).expect("écriture k");
        }
        "emis_serie" => {
            let n: usize = env("K_ALLOC_N").parse().expect("K_ALLOC_N");
            let mut ak = AutoriteK::ouvrir(chemin).expect("ouverture");
            let mut sortie = String::new();
            for _ in 0..n {
                sortie.push_str(&format!("{}\n", ak.emettre().expect("émission")));
            }
            std::fs::write(d.join(format!("{role}.ks")), sortie).expect("écriture ks");
        }
        // Meurt ENTRE la persistance et la publication (aucun destructeur exécuté).
        "brule" => {
            let mut ak = AutoriteK::ouvrir(chemin).expect("ouverture");
            let mut publie: Option<u128> = None;
            let d2 = d.clone();
            let _ = ak.emettre_observe(&mut |e| {
                if let EtapeEmission::SuivantPersiste(suivant) = e {
                    std::fs::write(d2.join("brule.k"), (suivant - 1).to_string())
                        .expect("trace du K brûlé");
                    // `exit` ne déroule AUCUN destructeur : le descripteur du
                    // verrou n'est pas fermé par nous, c'est le noyau qui le fait.
                    std::process::exit(9);
                }
                if let EtapeEmission::Publie(k) = e {
                    publie = Some(k);
                }
            });
            panic!("le worker aurait dû mourir avant publication (publié={publie:?})");
        }
        "ouvre" => {
            let resultat = match AutoriteK::ouvrir(chemin) {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("err:{}:{e}", e.kind()),
            };
            std::fs::write(d.join(format!("{role}.resultat")), resultat).expect("résultat");
        }
        autre => panic!("mode worker inconnu : {autre}"),
    }
    true
}

fn commande(exe: &Path, test: &str, d: &Path, mode: &str, role: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--exact", test])
        .env("K_ALLOC_MODE", mode)
        .env("K_ALLOC_DIR", d)
        .env("K_ALLOC_ROLE", role);
    cmd
}

fn spawner_sync(exe: &Path, test: &str, d: &Path, mode: &str, role: &str) {
    let statut = commande(exe, test, d, mode, role)
        .status()
        .expect("spawn du worker");
    assert!(
        statut.success(),
        "worker {mode}/{role} a échoué : {statut:?}"
    );
}

// ── 1. Collision concurrente : interleaving DÉTERMINISTE ──────────────────────
//
// Les deux processus ouvrent l'autorité (donc lisent l'état) AVANT la barrière,
// puis émettent après. C'est exactement la fenêtre du défaut : lecture, calcul,
// publication. Ici la barrière rend l'entrelacement REJOUABLE, pas probabiliste.
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

    // Les deux ont LU le compteur avant que quiconque n'écrive : interleaving forcé.
    assert!(attendre(&d.join("a.pret"), DELAI), "a n'a pas ouvert");
    assert!(attendre(&d.join("b.pret"), DELAI), "b n'a pas ouvert");
    std::fs::write(d.join("go"), b"").expect("levée de la barrière");

    assert!(a.wait().expect("attente a").success(), "worker a");
    assert!(b.wait().expect("attente b").success(), "worker b");

    let ka: u128 = std::fs::read_to_string(d.join("a.k"))
        .expect("K de a")
        .trim()
        .parse()
        .expect("K u128");
    let kb: u128 = std::fs::read_to_string(d.join("b.k"))
        .expect("K de b")
        .trim()
        .parse()
        .expect("K u128");
    let durable = etat_durable(&d.join("autorite.k"));
    println!("MESURÉ : K(a)={ka}  K(b)={kb}  état durable={durable}");

    assert_ne!(ka, kb, "deux processus ne peuvent pas publier le même K");
    let mut vus = [ka, kb];
    vus.sort();
    assert_eq!(
        vus,
        [1, 2],
        "les deux K émis sont exactement les deux premiers"
    );
    assert_eq!(
        durable, 3,
        "l'état durable compte les DEUX émissions (sans écrasement mutuel)"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 2. Contention : 4 processus × 20 émissions ───────────────────────────────
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
        .env("K_ALLOC_N", n.to_string())
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
    println!("MESURÉ : {} K émis ({} attendus)", tous.len(), total);
    assert_eq!(tous.len(), total, "aucune émission perdue");

    let mut tries = tous.clone();
    tries.sort_unstable();
    tries.dedup();
    assert_eq!(tries.len(), total, "aucun K en double entre processus");
    assert_eq!(tries.first(), Some(&1), "premier K = 1");
    assert_eq!(tries.last(), Some(&(total as u128)), "dernier K = {total}");
    assert_eq!(
        etat_durable(&d.join("autorite.k")),
        total as u128 + 1,
        "compteur durable = émissions + 1 (aucun trou, aucun écrasement)"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 3. Interruption entre persistance et publication ─────────────────────────
#[test]
fn interruption_apres_persistance_ne_reattribue_jamais() {
    if executer_worker() {
        return;
    }
    let d = dir("interruption");
    let exe = std::env::current_exe().expect("exe de test");
    let chemin = d.join("autorite.k");

    let statut = commande(
        &exe,
        "interruption_apres_persistance_ne_reattribue_jamais",
        &d,
        "brule",
        "x",
    )
    .status()
    .expect("spawn brule");
    assert_eq!(
        statut.code(),
        Some(9),
        "le worker est mort entre persistance et publication"
    );

    let brule: u128 = std::fs::read_to_string(d.join("brule.k"))
        .expect("K brûlé tracé")
        .trim()
        .parse()
        .expect("K u128");
    let durable = etat_durable(&chemin);
    println!("MESURÉ : K brûlé={brule}  état durable={durable}");

    assert_eq!(
        durable,
        brule + 1,
        "la persistance a eu lieu AVANT la publication : l'état couvre déjà le K brûlé"
    );

    // Le verrou n'est pas orphelin : le noyau l'a libéré avec le processus.
    let mut apres = AutoriteK::ouvrir(chemin.clone()).expect("réouverture");
    assert_eq!(
        apres.prochain_durable().expect("état durable sous verrou"),
        brule + 1,
        "le compteur n'a pas reculé : le verrou est réacquérable"
    );
    let k = apres.emettre().expect("émission après l'interruption");
    assert_eq!(
        k,
        brule + 1,
        "un K brûlé n'est JAMAIS réattribué (saut accepté)"
    );
    assert_ne!(k, brule, "aucune réutilisation");

    let _ = std::fs::remove_dir_all(&d);
}

// ── 4. Corruption : refus nommé, octets préservés, pas de retour à 1 ─────────
#[test]
fn corruption_refusee_sans_reinitialisation() {
    if executer_worker() {
        return;
    }
    let d = dir("corruption");
    let exe = std::env::current_exe().expect("exe de test");
    let chemin = d.join("autorite.k");

    // 4a — état TRONQUÉ : refus dans un AUTRE processus, octets intacts.
    std::fs::write(&chemin, b"tronque").expect("corruption");
    spawner_sync(
        &exe,
        "corruption_refusee_sans_reinitialisation",
        &d,
        "ouvre",
        "x",
    );
    let resultat = std::fs::read_to_string(d.join("x.resultat")).expect("résultat");
    assert!(
        resultat.starts_with("err:"),
        "l'ouverture doit REFUSER, pas repartir à 1 : {resultat}"
    );
    assert!(
        resultat.contains("invalid data"),
        "refus nommé (ErrorKind::InvalidData) : {resultat}"
    );
    assert_eq!(
        std::fs::read(&chemin).expect("octets"),
        b"tronque",
        "les octets corrompus sont PRÉSERVÉS (jamais réécrits)"
    );

    // 4b — compteur NUL : refusé (0 = « jamais émise », donc fabriqué).
    std::fs::write(&chemin, 0u128.to_le_bytes()).expect("compteur nul");
    let err = AutoriteK::ouvrir(chemin.clone()).expect_err("refus attendu");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    // 4c — corruption APPARUE après ouverture : l'émission refuse sous verrou.
    let mut ak = AutoriteK::ouvrir(d.join("sain.k")).expect("ouverture");
    assert_eq!(ak.emettre().expect("émission"), 1);
    std::fs::write(d.join("sain.k"), b"court").expect("corruption tardive");
    let err = ak.emettre().expect_err("émission refusée");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(
        std::fs::read(d.join("sain.k")).expect("octets"),
        b"court",
        "aucune réécriture après refus"
    );

    // 4d — état DISPARU sous verrou : refusé, jamais recréé à 1.
    std::fs::remove_file(d.join("sain.k")).expect("suppression");
    let err = ak.emettre().expect_err("état disparu refusé");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        !d.join("sain.k").exists(),
        "aucune réinitialisation silencieuse : le fichier n'est pas recréé"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 5. Régression du compteur dans un processus vivant ───────────────────────
#[test]
fn compteur_regresse_refuse_dans_un_processus_vivant() {
    let d = dir("regression");
    let chemin = d.join("autorite.k");
    let mut ak = AutoriteK::ouvrir(chemin.clone()).expect("ouverture");
    assert_eq!(ak.emettre().expect("émission"), 1);

    // Restauration historique du fichier : le compteur revient à 1.
    std::fs::write(&chemin, 1u128.to_le_bytes()).expect("restauration");

    let err = ak.emettre().expect_err("réattribution refusée");
    assert!(
        err.to_string().contains("régressé"),
        "refus nommé d'une régression : {err}"
    );
    assert_eq!(
        etat_durable(&chemin),
        1,
        "l'état n'est pas réécrit par un refus"
    );

    // LIMITE NOMMÉE : une instance FRAÎCHE sur un fichier restauré ne peut pas
    // savoir qu'elle recule — le domaine de détection est l'instance vivante.
    let fraiche = AutoriteK::ouvrir(chemin).expect("ouverture fraîche");
    assert_eq!(
        fraiche.prochain_durable().expect("état"),
        1,
        "limite mesurée : la restauration n'est pas détectable sans témoin externe"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 6. Le verrou ne porte pas sur l'objet renommé ────────────────────────────
#[cfg(unix)]
#[test]
fn verrou_porte_sur_un_objet_distinct_de_l_etat_renomme() {
    use std::os::unix::fs::MetadataExt;

    let d = dir("verrou");
    let chemin = d.join("autorite.k");
    let verrou = d.join("autorite.k.verrou");
    let mut ak = AutoriteK::ouvrir(chemin.clone()).expect("ouverture");
    let _ = ak.emettre().expect("émission 1");

    let ino_etat_1 = std::fs::metadata(&chemin).expect("état").ino();
    let ino_verrou_1 = std::fs::metadata(&verrou).expect("verrou").ino();
    let _ = ak.emettre().expect("émission 2");
    let ino_etat_2 = std::fs::metadata(&chemin).expect("état").ino();
    let ino_verrou_2 = std::fs::metadata(&verrou).expect("verrou").ino();

    assert_ne!(
        ino_etat_1, ino_etat_2,
        "l'écriture durable REMPLACE l'état (rename)"
    );
    assert_eq!(
        ino_verrou_1, ino_verrou_2,
        "le verrou porte sur le MÊME objet d'une émission à l'autre"
    );
    assert_ne!(
        ino_etat_1, ino_verrou_1,
        "l'état et le verrou sont deux objets distincts"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 7. OBRR : un `k` String du format V1 reste en quarantaine ────────────────
fn v1_avec_k(k: &str) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(&ENREGISTREMENT_MAGIC);
    o.extend_from_slice(&ENREGISTREMENT_VERSION_LEGACY.to_le_bytes());
    o.extend_from_slice(&(k.len() as u32).to_le_bytes());
    o.extend_from_slice(k.as_bytes());
    o.extend_from_slice(&7u64.to_le_bytes()); // backend
    o.extend_from_slice(&1u64.to_le_bytes()); // generation
    o.extend_from_slice(&1u64.to_le_bytes()); // cible
    o.extend_from_slice(&0u32.to_le_bytes()); // n_effets
    o.extend_from_slice(&0u64.to_le_bytes()); // ref_len
    o
}

#[test]
fn obrr_v1_string_jamais_reinterprete_en_u128() {
    // Un V1 dont le `k` est la CHAÎNE « 7 » : elle ne doit jamais devenir K = 7.
    let octets = v1_avec_k("7");
    match qualifier(&octets, "test") {
        EtatEnregistrement::QuarantaineLegacy(legacy) => {
            assert_eq!(
                legacy.vue.as_ref().map(|v| v.k.as_str()),
                Some("7"),
                "la vue V1 garde la chaîne telle quelle"
            );
            assert!(!legacy.octets.is_empty(), "les octets sont conservés");
        }
        autre => panic!("un V1 doit être mis en QUARANTAINE, pas interprété : {autre:?}"),
    }

    // Le codec COURANT refuse ces octets : aucune réinterprétation silencieuse.
    assert!(matches!(
        EnregistrementOriginal::decoder(&octets),
        Err(DecodageEnregistrement::VersionInconnue(1))
    ));

    // Contrôle POSITIF : un V2 portant K = 7 est, lui, courant — même valeur
    // logique, deux encodages, et le legacy n'est jamais promu.
    let v2 = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 7,
        backend: 7,
        generation: 1,
        decision_id: 1,
        autorite_consommateur: 1,
        cible: NodeId(1),
        effets_attendus: vec![],
        reference_bytes: vec![],
    };
    match qualifier(&v2.encoder(), "test") {
        EtatEnregistrement::Courant(enr) => assert_eq!(enr.k, 7, "K u128 du format courant"),
        autre => panic!("un V2 admissible doit être courant : {autre:?}"),
    }

    // Et un V1 « 7 » n'est jamais promu par la qualification : l'obligation est
    // conservée, l'identité n'est pas inventée.
    let etat = qualifier(&octets, "test");
    assert!(!etat.est_courant(), "un V1 n'est jamais courant");
    assert_eq!(
        etat.octets_preserves(),
        octets.as_slice(),
        "les octets du V1 sont préservés pour l'obligation"
    );

    // Le refus nommé existe pour les corruptions V2 (contrôle négatif).
    let mut corrompu = v2.encoder();
    corrompu[4] = 99;
    assert!(matches!(
        qualifier(&corrompu, "test").refus(),
        Some(MotifIllisibilite::VersionInconnue(99))
    ));

    // ANCIEN record SUR DISQUE : relu, qualifié, mis en quarantaine, octets
    // préservés — la qualification est reproductible dans un autre processus
    // (l'octet est la seule source, aucun état RAM n'intervient).
    let d = dir("obrr_v1");
    let chemin = d.join("record.obrr");
    std::fs::write(&chemin, &octets).expect("record V1 sur disque");
    match this::reconciliation::relire_qualifie(&chemin).expect("relecture") {
        EtatEnregistrement::QuarantaineLegacy(legacy) => {
            assert_eq!(legacy.provenance.longueur, octets.len());
            assert_eq!(legacy.octets, octets, "octets du V1 conservés VERBATIM");
        }
        autre => panic!("un V1 sur disque n'est jamais courant : {autre:?}"),
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ── 8. Domaine canonique : deux fichiers = deux autorités (non-unicité globale) ─
#[test]
fn domaine_canonique_deux_fichiers_deux_autorites() {
    let d = dir("domaine");
    let mut a = AutoriteK::ouvrir(d.join("a.k")).expect("autorité A");
    let mut b = AutoriteK::ouvrir(d.join("b.k")).expect("autorité B");
    let ka = a.emettre().expect("K de A");
    let kb = b.emettre().expect("K de B");
    println!("MESURÉ : K(A)={ka} K(B)={kb} sur deux domaines distincts");
    assert_eq!(
        ka, kb,
        "NON-unicité mesurée : deux fichiers d'état sont deux autorités ; K n'est \
         unique que DANS son domaine (l'unicité globale n'est pas établie ici)"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ── 9. Vue cachée vs état réel : la lecture sous verrou, pas à l'ouverture ───
#[test]
fn vue_perimee_vs_etat_durable_relu_sous_verrou() {
    let d = dir("vue");
    let chemin = d.join("autorite.k");
    let mut a = AutoriteK::ouvrir(chemin.clone()).expect("A");
    let b = AutoriteK::ouvrir(chemin).expect("B");
    assert_eq!(a.emettre().expect("émission A"), 1);
    assert_eq!(
        b.prochain(),
        1,
        "la vue d'ouverture est PÉRIMÉE (c'est elle qui produisait la collision)"
    );
    assert_eq!(
        b.prochain_durable().expect("état relu sous verrou"),
        2,
        "l'état réel, relu sous verrou, a suivi l'émission de A"
    );
    let _ = std::fs::remove_dir_all(&d);
}
