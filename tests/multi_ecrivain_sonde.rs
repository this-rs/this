//! Sonde multi-écrivain d'obrain (feature `obrain`) — met en défaut l'écriture
//! partagée multi-processus pour remonter des issues à obrain-db/obrain.
//!
//! Ce test NE FAIT PAS partie de la suite ordinaire : il est `#[ignore]` et se
//! lance explicitement :
//!
//!     cargo +1.99.1 test -p this-rs --features obrain \
//!         --test multi_ecrivain_sonde -- --ignored --nocapture
//!
//! Il se relance LUI-MÊME en sous-processus (rôle choisi par `SONDE_ROLE`) pour
//! produire de VRAIS écrivains concurrents — pas des threads : les atomiques du
//! substrat sont PAR PROCESSUS. Il appelle obrain directement (`ObrainDB`),
//! au-dessus de this-rs : le bail d'écriture de `src/storage/writer_lease.rs`
//! n'entre pas en jeu, c'est l'intention.
//!
//! Scénarios mesurés (détails dans `docs/obrain-sonde-multi-ecrivain.md`) :
//!   A — entités distinctes (MP-3), un écrivain tué -9 en plein vol ;
//!   B — même entité (MP-3b), verrou d'entité + clé martelée en commun ;
//!   C — import en lot contre écrivain unitaire sur la même entité (#386) ;
//!   D — entité créée par un pair APRÈS notre ouverture (#387) ;
//!   E — arène des blobs, deux écrivains vivants (MP-3c, limite connue).
//!   F — ARÊTE créée par un pair après notre ouverture : le pendant de D côté
//!       zone des arêtes (ce que #398 annonce symétrique mais non mesuré).
//!
//! Chaque exécution est bornée (drapeaux de départ, délais d'attente, garde de
//! profondeur) et nettoie ses répertoires temporaires (`tempfile::tempdir`).

#![cfg(feature = "obrain")]

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_engine::{GraphStore, ObrainDB};

const ROLE: &str = "SONDE_ROLE";
const DIR: &str = "SONDE_DIR";
const TEST: &str = "sonde_multi_ecrivain";

/// Nœuds par écrivain du scénario A.
const N_A: u64 = 200;
/// Clés distinctes par écrivain du scénario B.
const K_B: u64 = 120;
/// Écritures de la clé partagée (scénario B).
const M_B: u64 = 150;
/// Clés par variante du scénario C.
const K_C: u64 = 120;
/// Longueur d'une valeur longue : > 256 octets → arène des blobs (`dat`).
const TAILLE_LONGUE: usize = 600;

fn role() -> Option<String> {
    std::env::var(ROLE).ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

fn attendre_fichier(p: &Path, quoi: &str) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(60),
            "attente de {quoi} ({}) : délai dépassé",
            p.display()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

const DELAI_LIGNE: Duration = Duration::from_secs(120);

/// Un sous-processus relancé depuis ce même binaire de test, avec son stdout
/// canalisé pour la synchronisation par lignes.
struct Enfant {
    process: Child,
    lignes: std::sync::mpsc::Receiver<String>,
    _lecteur: std::thread::JoinHandle<()>,
}

impl Enfant {
    fn lancer(role: &str, dir: &Path, extras: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--ignored", "--exact", TEST, "--nocapture"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        cmd.env(ROLE, role).env(DIR, dir);
        for (k, v) in extras {
            cmd.env(k, v);
        }
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        let (tx, rx) = std::sync::mpsc::channel();
        let lecteur = std::thread::spawn(move || {
            for ligne in std::io::BufReader::new(sortie)
                .lines()
                .map_while(Result::ok)
            {
                if tx.send(ligne).is_err() {
                    break;
                }
            }
        });
        Self {
            process,
            lignes: rx,
            _lecteur: lecteur,
        }
    }

    fn attendre_ligne(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.recv_timeout(DELAI_LIGNE) {
                Ok(l) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    panic!("enfant muet depuis {DELAI_LIGNE:?} en attendant « {prefixe} »")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("enfant terminé sans « {prefixe} »")
                }
            }
        }
    }

    /// Attend la fin propre, avec une borne : un enfant bloqué sur un verrou
    /// doit échouer, pas figer la sonde.
    fn terminer(mut self) {
        let echeance = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(statut) = self.process.try_wait().expect("try_wait") {
                assert!(statut.success(), "l'enfant a échoué : {statut:?}");
                return;
            }
            if Instant::now() > echeance {
                let _ = self.process.kill();
                let _ = self.process.wait();
                panic!("l'enfant n'a pas terminé à temps (tué)");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// SIGKILL — « crash sans flush » : aucun `Drop`, aucun msync, aucun
    /// checkpoint. Le noyau rend le `flock` du répertoire au passage.
    fn tuer(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

// ---------------------------------------------------------------------------
// Valeurs de mesure
// ---------------------------------------------------------------------------

fn longue(proprietaire: &str, i: u64) -> String {
    format!("{proprietaire}:{i}:{}", "x".repeat(TAILLE_LONGUE))
}

fn vecteur(proprietaire: &str, i: u64) -> Vec<f32> {
    let mut v = vec![0.0f32; 16];
    v[0] = if proprietaire == "A" { 1.0 } else { 2.0 };
    v[1] = i as f32;
    v
}

fn verifier_vecteur(v: &[f32], proprietaire: &str, i: u64) -> bool {
    v == vecteur(proprietaire, i).as_slice()
}

/// Décode `{proprietaire}:{i}:…` ; `None` si la valeur n'est pas un préfixe
/// décodable (valeur d'un autre nœud tronquée, corrompue, etc.).
fn decode_longue(s: &str) -> Option<(String, u64)> {
    let (o, reste) = s.split_once(':')?;
    let (idx, _) = reste.split_once(':')?;
    Some((o.to_string(), idx.parse().ok()?))
}

/// Parse la ligne « IDS 1,2,3 » d'un enfant.
fn ids_depuis(ligne: &str) -> Vec<u64> {
    ligne
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("id"))
        .collect()
}

fn poser_noeud(db: &ObrainDB, proprietaire: &str, i: u64) -> NodeId {
    let id = db.create_node(&["Sonde"]);
    db.set_node_property(id, "_proprietaire", Value::String(proprietaire.into()));
    db.set_node_property(id, "_index", Value::Int64(i as i64));
    db.set_node_property(id, "_long", Value::String(longue(proprietaire, i).into()));
    db.set_node_property(id, "_vec", Value::Vector(vecteur(proprietaire, i).into()));
    id
}

/// Crée une base neuve, y pose un nœud (dimensionne les zones ET initialise la
/// colonne blob / la colonne de vecteurs), puis ferme.
fn creer_base(dir: &Path) {
    let db = ObrainDB::open(dir).expect("open (créateur exclusif)");
    let id = db.create_node(&["Sonde"]);
    db.set_node_property(id, "_long", Value::String(longue("init", 0).into()));
    db.set_node_property(id, "_vec", Value::Vector(vec![0.0f32; 16].into()));
    db.close().expect("close (créateur)");
}

// ---------------------------------------------------------------------------
// Rôles enfants
// ---------------------------------------------------------------------------

/// A — pose N nœuds, ANONCE leurs ids, puis (si `propre`) ferme, sinon continue
/// SANS fin pour être tué -9 en plein vol. L'annonce des ids donne au parent la
/// vérité terrain (quel id appartient à quel écrivain), indispensable pour
/// distinguer « page de propriétés écrasée » (MP-3) d'« arène de blobs écrasée ».
fn role_a(dir: &Path, proprietaire: &str, propre: bool) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (A)");
    let n: u64 = env_obligatoire("SONDE_N").parse().expect("SONDE_N");
    let mut ids = Vec::with_capacity(n as usize);
    for i in 0..n {
        ids.push(poser_noeud(&db, proprietaire, i).0);
    }
    let liste = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    annoncer(&format!("IDS {liste}"));
    if propre {
        db.close().expect("close (A propre)");
        return;
    }
    // Continue de poser des nœuds (jamais flushés, jamais annoncés) : le parent
    // tue ce processus au milieu de cette boucle.
    let mut i = n;
    loop {
        poser_noeud(&db, proprietaire, i);
        i += 1;
        std::thread::sleep(Duration::from_micros(50));
    }
}

/// B — deux écrivains sur la MÊME entité (verrou d'entité MP-3b) : clés
/// distinctes + une clé martelée en commun.
fn role_b(dir: &Path, proprietaire: &str) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (B)");
    let noeud: u64 = env_obligatoire("SONDE_NOEUD").parse().expect("SONDE_NOEUD");
    let k: u64 = env_obligatoire("SONDE_K").parse().expect("SONDE_K");
    let m: u64 = env_obligatoire("SONDE_M").parse().expect("SONDE_M");
    let id = NodeId(noeud);
    for i in 0..k {
        let cle = format!("{proprietaire}{i}");
        db.set_node_property(
            id,
            &cle,
            Value::String(format!("{proprietaire}-{i}").into()),
        );
    }
    for j in 0..m {
        db.set_node_property(
            id,
            "partagee",
            Value::String(format!("{proprietaire}-{j}").into()),
        );
    }
    db.close().expect("close (B)");
    annoncer("FINI");
}

/// C — import en lot contre écrivain unitaire, même entité (#386). Le départ
/// est synchronisé par un drapeau pour forcer le recouvrement.
fn role_c(dir: &Path, variante: &str) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (C)");
    let noeud: u64 = env_obligatoire("SONDE_NOEUD").parse().expect("SONDE_NOEUD");
    let k: u64 = env_obligatoire("SONDE_K").parse().expect("SONDE_K");
    let go = PathBuf::from(env_obligatoire("SONDE_GO"));
    annoncer("PRET");
    attendre_fichier(&go, "le départ du scénario C");
    match variante {
        "lot" => {
            let cles: Vec<String> = (0..k).map(|i| format!("L{i}")).collect();
            let vals: Vec<Value> = (0..k)
                .map(|i| Value::String(format!("L-{i}").into()))
                .collect();
            let props: Vec<(&str, Value)> = cles
                .iter()
                .zip(vals)
                .map(|(c, v)| (c.as_str(), v))
                .collect();
            db.bulk_import_props(noeud as u32, &[props])
                .expect("bulk_import_props (C)");
        }
        "unitaire" => {
            for i in 0..k {
                let cle = format!("U{i}");
                db.set_node_property(NodeId(noeud), &cle, Value::String(format!("U-{i}").into()));
            }
        }
        autre => panic!("variante C inconnue : {autre}"),
    }
    db.close().expect("close (C)");
    annoncer("FINI");
}

/// D — un pair crée une entité APRÈS notre ouverture (#387). « nous » ouvre
/// d'abord, attend l'id du nœud créé par le pair, mesure s'il le VOIT, puis
/// écrit dessus.
fn role_d(dir: &Path, variante: &str) {
    match variante {
        "nous" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (D nous)");
            let id_fichier = PathBuf::from(env_obligatoire("SONDE_ID"));
            annoncer("PRET");
            attendre_fichier(&id_fichier, "l'id du nœud créé par le pair");
            let noeud: u64 = std::fs::read_to_string(&id_fichier)
                .expect("lire l'id")
                .trim()
                .parse()
                .expect("id valide");
            let vu = db.get_node(NodeId(noeud)).is_some();
            db.set_node_property(
                NodeId(noeud),
                "cle_du_pair",
                Value::String("ecrite_par_nous".into()),
            );
            let lu = db
                .substrate_handle()
                .and_then(|s| s.get_node_property(NodeId(noeud), &PropertyKey::new("cle_du_pair")))
                .is_some();
            annoncer(&format!("VU {} LU {}", vu as u8, lu as u8));
            db.close().expect("close (D nous)");
        }
        "pair" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (D pair)");
            let id = db.create_node(&["Sonde"]);
            let id_fichier = PathBuf::from(env_obligatoire("SONDE_ID"));
            std::fs::write(&id_fichier, id.0.to_string()).expect("écrire l'id du nœud");
            db.close().expect("close (D pair)");
            annoncer("FINI");
        }
        autre => panic!("variante D inconnue : {autre}"),
    }
}

/// E — arène des blobs : deux écrivains VIVANTS écrivent des valeurs longues.
/// « e_b » écrit une valeur courte-routée-blob (300 octets) et ferme ; « e_lit »
/// rouvre et déverse les valeurs longues dans un fichier.
fn role_e(dir: &Path, variante: &str) {
    match variante {
        "e_b" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (E B)");
            let n: usize = env_obligatoire("SONDE_TAILLE")
                .parse()
                .expect("SONDE_TAILLE");
            let c: char = env_obligatoire("SONDE_CAR").parse().expect("SONDE_CAR");
            let id = db.create_node(&["Sonde"]);
            db.set_node_property(id, "corps", Value::String(c.to_string().repeat(n).into()));
            annoncer(&format!("ID {}", id.0));
            db.close().expect("close (E B)");
        }
        "e_lit" => {
            // Relecture par Cypher, comme `blob_arene_deux_processus.rs` : la
            // requête hydrate les propriétés du nœud (dont la colonne blob).
            let db = ObrainDB::open_shared_write(dir).expect("relecture (E)");
            let resultat = db
                .execute("MATCH (n:Sonde) RETURN id(n), n.corps")
                .expect("requête (E)");
            let mut sortie = String::new();
            for ligne in &resultat.rows {
                let id = match ligne.first() {
                    Some(Value::Int64(i)) => i.to_string(),
                    autre => format!("<{autre:?}>"),
                };
                let corps = match ligne.get(1) {
                    Some(Value::String(s)) => s.to_string(),
                    autre => format!("<{autre:?}>"),
                };
                sortie.push_str(&format!("{id}\t{corps}\n"));
            }
            db.close().expect("close relecture (E)");
            std::fs::write(dir.parent().expect("parent").join("lecture-e.txt"), sortie)
                .expect("écrire lecture-e");
            annoncer("FINI");
        }
        autre => panic!("variante E inconnue : {autre}"),
    }
}

/// F — une ARÊTE créée par un pair APRÈS notre ouverture : le pendant du
/// scénario D côté zone des **arêtes**. #398 (correctif de #387) annonce ce côté
/// « symétrique mais aucun test inter-processus de bout en bout » — c'est
/// exactement ce que cette variante mesure.
fn role_f(dir: &Path, variante: &str) {
    match variante {
        "nous" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (F nous)");
            let fichier = PathBuf::from(env_obligatoire("SONDE_ID"));
            annoncer("PRET");
            attendre_fichier(&fichier, "l'id de l'arête créée par le pair");
            let arete: u64 = std::fs::read_to_string(&fichier)
                .expect("lire l'id d'arête")
                .trim()
                .parse()
                .expect("id d'arête valide");
            let vu = db.get_edge(EdgeId(arete)).is_some();
            db.set_edge_property(
                EdgeId(arete),
                "cle_du_pair",
                Value::String("ecrite_par_nous".into()),
            );
            let lu = db
                .substrate_handle()
                .and_then(|s| s.get_edge_property(EdgeId(arete), &PropertyKey::new("cle_du_pair")))
                .is_some();
            annoncer(&format!("VU {} LU {}", vu as u8, lu as u8));
            db.close().expect("close (F nous)");
        }
        "pair" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (F pair)");
            let a = db.create_node(&["Sonde"]);
            let b = db.create_node(&["Sonde"]);
            let arete = db.create_edge(a, b, "Sonde");
            let fichier = PathBuf::from(env_obligatoire("SONDE_ID"));
            std::fs::write(&fichier, arete.0.to_string()).expect("écrire l'id d'arête");
            db.close().expect("close (F pair)");
            annoncer("FINI");
        }
        autre => panic!("variante F inconnue : {autre}"),
    }
}

fn jouer_role(r: &str) {
    let dir = PathBuf::from(env_obligatoire(DIR));
    match r {
        "a_a" => role_a(&dir, "A", false),
        "a_b" => role_a(&dir, "B", true),
        "b_a" => role_b(&dir, "A"),
        "b_b" => role_b(&dir, "B"),
        "c_lot" => role_c(&dir, "lot"),
        "c_unitaire" => role_c(&dir, "unitaire"),
        "d_nous" => role_d(&dir, "nous"),
        "d_pair" => role_d(&dir, "pair"),
        "e_b" => role_e(&dir, "e_b"),
        "e_lit" => role_e(&dir, "e_lit"),
        "f_nous" => role_f(&dir, "nous"),
        "f_pair" => role_f(&dir, "pair"),
        autre => panic!("rôle inconnu : {autre}"),
    }
}

// ---------------------------------------------------------------------------
// Scénarios (parent)
// ---------------------------------------------------------------------------

fn scenario_a() {
    let td = tempfile::tempdir().expect("tempdir A");
    let dir = td.path().join("store-a");
    creer_base(&dir);

    let mut a = Enfant::lancer("a_a", &dir, &[("SONDE_N", N_A.to_string())]);
    let mut b = Enfant::lancer("a_b", &dir, &[("SONDE_N", N_A.to_string())]);
    let ids_a = ids_depuis(&a.attendre_ligne("IDS "));
    let ids_b = ids_depuis(&b.attendre_ligne("IDS "));
    assert_eq!(ids_a.len(), N_A as usize, "A doit annoncer ses {N_A} ids");
    assert_eq!(ids_b.len(), N_A as usize, "B doit annoncer ses {N_A} ids");
    b.terminer();
    // Tue A pendant qu'il continue d'écrire (boucle sans fin, jamais flushé).
    a.tuer();

    let db = ObrainDB::open(&dir).expect("réouverture exclusive (A)");
    let sub = db.substrate_handle().expect("substrate_handle (A)");

    // (propriétaire, ids) → compteurs.
    struct Mesure {
        noeuds_manquants: i64,
        courtes_fausses: i64,
        longues_mauvaises: i64,
        longues_manquantes: i64,
        longues_corrompues: i64,
        vecteurs_mauvais: i64,
        vecteurs_manquants: i64,
    }
    let mut res: Vec<(&str, Mesure)> = Vec::new();
    for (proprietaire, ids) in [("A", &ids_a), ("B", &ids_b)] {
        let mut m = Mesure {
            noeuds_manquants: 0,
            courtes_fausses: 0,
            longues_mauvaises: 0,
            longues_manquantes: 0,
            longues_corrompues: 0,
            vecteurs_mauvais: 0,
            vecteurs_manquants: 0,
        };
        for (pos, id) in ids.iter().enumerate() {
            let nid = NodeId(*id);
            let prop = sub.get_node_property(nid, &PropertyKey::new("_proprietaire"));
            let idx = sub.get_node_property(nid, &PropertyKey::new("_index"));
            // Nœud absent après la réouverture (record non durable) : tout est
            // compté « manquant », on ne peut rien lire dessus.
            if prop.is_none() || idx.is_none() {
                m.noeuds_manquants += 1;
                continue;
            }
            let attendu_courte = Value::String(proprietaire.into());
            if prop != Some(attendu_courte.clone()) {
                m.courtes_fausses += 1;
            }
            if idx != Some(Value::Int64(pos as i64)) {
                m.courtes_fausses += 1;
            }

            match sub.get_node_property(nid, &PropertyKey::new("_long")) {
                Some(Value::String(s)) => match decode_longue(&s) {
                    Some((o, i)) if o == proprietaire && i == pos as u64 => {}
                    Some((o, i)) => {
                        m.longues_mauvaises += 1;
                        if m.longues_mauvaises <= 3 {
                            eprintln!(
                                "  [A] valeur longue d'un autre nœud : {proprietaire}:{pos} relit {o}:{i}"
                            );
                        }
                    }
                    None => m.longues_corrompues += 1,
                },
                None => m.longues_manquantes += 1,
                _ => m.longues_corrompues += 1,
            }

            match sub.get_node_property(nid, &PropertyKey::new("_vec")) {
                Some(Value::Vector(v)) => {
                    if !verifier_vecteur(&v, proprietaire, pos as u64) {
                        m.vecteurs_mauvais += 1;
                    }
                }
                None => m.vecteurs_manquants += 1,
                _ => m.vecteurs_mauvais += 1,
            }
        }
        res.push((proprietaire, m));
    }
    db.close().expect("close (A)");

    let mut manquants = 0i64;
    let mut courtes_fausses = 0i64;
    let mut longues_mauvaises = 0i64;
    let mut longues_manquantes = 0i64;
    let mut longues_corrompues = 0i64;
    let mut vecteurs_mauvais = 0i64;
    let mut vecteurs_manquants = 0i64;
    for (_, m) in &res {
        manquants += m.noeuds_manquants;
        courtes_fausses += m.courtes_fausses;
        longues_mauvaises += m.longues_mauvaises;
        longues_manquantes += m.longues_manquantes;
        longues_corrompues += m.longues_corrompues;
        vecteurs_mauvais += m.vecteurs_mauvais;
        vecteurs_manquants += m.vecteurs_manquants;
    }
    let detail = res
        .iter()
        .map(|(p, m)| {
            format!(
                "{p}:manquants={} courtes_fausses={} longues_mauvaises={} vecteurs_mauvais={}",
                m.noeuds_manquants, m.courtes_fausses, m.longues_mauvaises, m.vecteurs_mauvais
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    println!("[A] nœuds annoncés={} | {detail}", 2 * N_A);
    println!(
        "[A] TOTAL manquants={manquants} courtes_fausses(MP-3)={courtes_fausses} \
         longues_mauvaises(arène)={longues_mauvaises} longues_manquantes={longues_manquantes} \
         longues_corrompues={longues_corrompues} vecteurs_mauvais={vecteurs_mauvais} \
         vecteurs_manquants={vecteurs_manquants}"
    );
}

fn scenario_b() {
    let td = tempfile::tempdir().expect("tempdir B");
    let dir = td.path().join("store-b");
    let noeud = {
        let db = ObrainDB::open(&dir).expect("open (B)");
        let id = db.create_node(&["Sonde"]);
        db.close().expect("close (B)");
        id.0
    };
    let mut a = Enfant::lancer(
        "b_a",
        &dir,
        &[
            ("SONDE_NOEUD", noeud.to_string()),
            ("SONDE_K", K_B.to_string()),
            ("SONDE_M", M_B.to_string()),
        ],
    );
    let mut b = Enfant::lancer(
        "b_b",
        &dir,
        &[
            ("SONDE_NOEUD", noeud.to_string()),
            ("SONDE_K", K_B.to_string()),
            ("SONDE_M", M_B.to_string()),
        ],
    );
    a.attendre_ligne("FINI");
    b.attendre_ligne("FINI");
    a.terminer();
    b.terminer();

    let db = ObrainDB::open(&dir).expect("réouverture exclusive (B)");
    let sub = db.substrate_handle().expect("substrate_handle (B)");
    let id = NodeId(noeud);
    let mut pertes = 0i64;
    for proprietaire in ["A", "B"] {
        for i in 0..K_B {
            let cle = format!("{proprietaire}{i}");
            let attendu = Value::String(format!("{proprietaire}-{i}").into());
            let lu = sub.get_node_property(id, &PropertyKey::new(cle.as_str()));
            if lu.as_ref() != Some(&attendu) {
                pertes += 1;
                eprintln!("  [B] clé {cle} : {lu:?}");
            }
        }
    }
    let partagee = sub.get_node_property(id, &PropertyKey::new("partagee"));
    let partagee_bien_formee = matches!(
        &partagee,
        Some(Value::String(s)) if s.starts_with("A-") || s.starts_with("B-")
    );
    db.close().expect("close (B)");
    println!(
        "[B] clés distinctes perdues={pertes}/{} | clé partagée finale={partagee:?} \
         bien_formée={partagee_bien_formee}",
        2 * K_B
    );
}

fn scenario_c() {
    let td = tempfile::tempdir().expect("tempdir C");
    let dir = td.path().join("store-c");
    let noeud = {
        let db = ObrainDB::open(&dir).expect("open (C)");
        let id = db.create_node(&["Sonde"]);
        db.set_node_property(id, "base", Value::String("presente".into()));
        db.close().expect("close (C)");
        id.0
    };
    let go = td.path().join("go-c");
    let mut lot = Enfant::lancer(
        "c_lot",
        &dir,
        &[
            ("SONDE_NOEUD", noeud.to_string()),
            ("SONDE_K", K_C.to_string()),
            ("SONDE_GO", go.display().to_string()),
        ],
    );
    let mut unitaire = Enfant::lancer(
        "c_unitaire",
        &dir,
        &[
            ("SONDE_NOEUD", noeud.to_string()),
            ("SONDE_K", K_C.to_string()),
            ("SONDE_GO", go.display().to_string()),
        ],
    );
    lot.attendre_ligne("PRET");
    unitaire.attendre_ligne("PRET");
    std::fs::write(&go, b"go").expect("poser le drapeau (C)");
    lot.attendre_ligne("FINI");
    unitaire.attendre_ligne("FINI");
    lot.terminer();
    unitaire.terminer();

    let db = ObrainDB::open(&dir).expect("réouverture exclusive (C)");
    let sub = db.substrate_handle().expect("substrate_handle (C)");
    let id = NodeId(noeud);
    let mut pertes = 0i64;
    for i in 0..K_C {
        let cle_l = format!("L{i}");
        let attendu_l = Value::String(format!("L-{i}").into());
        if sub
            .get_node_property(id, &PropertyKey::new(cle_l.as_str()))
            .as_ref()
            != Some(&attendu_l)
        {
            pertes += 1;
        }
        let cle_u = format!("U{i}");
        let attendu_u = Value::String(format!("U-{i}").into());
        if sub
            .get_node_property(id, &PropertyKey::new(cle_u.as_str()))
            .as_ref()
            != Some(&attendu_u)
        {
            pertes += 1;
        }
    }
    let base = sub.get_node_property(id, &PropertyKey::new("base"));
    db.close().expect("close (C)");
    println!(
        "[C] clés perdues={pertes}/{} | propriété préexistante 'base' relue={base:?}",
        2 * K_C
    );
}

fn scenario_d() {
    let td = tempfile::tempdir().expect("tempdir D");
    let dir = td.path().join("store-d");
    creer_base(&dir);
    let id_fichier = td.path().join("id-d.txt");

    let mut nous = Enfant::lancer(
        "d_nous",
        &dir,
        &[("SONDE_ID", id_fichier.display().to_string())],
    );
    nous.attendre_ligne("PRET");
    let mut pair = Enfant::lancer(
        "d_pair",
        &dir,
        &[("SONDE_ID", id_fichier.display().to_string())],
    );
    pair.attendre_ligne("FINI");
    pair.terminer();
    let vu_lu = nous.attendre_ligne("VU ");
    nous.terminer();

    let noeud: u64 = std::fs::read_to_string(&id_fichier)
        .expect("id-d")
        .trim()
        .parse()
        .expect("id");
    let db = ObrainDB::open(&dir).expect("réouverture exclusive (D)");
    let sub = db.substrate_handle().expect("substrate_handle (D)");
    let cle = sub.get_node_property(NodeId(noeud), &PropertyKey::new("cle_du_pair"));
    db.close().expect("close (D)");
    println!(
        "[D] pair vu par l'écrivain antérieur + écriture relue par lui : {vu_lu} | \
         après réouverture exclusive, 'cle_du_pair'={cle:?}"
    );
}

fn scenario_f() {
    let td = tempfile::tempdir().expect("tempdir F");
    let dir = td.path().join("store-f");
    creer_base(&dir);
    let fichier = td.path().join("id-f.txt");

    let mut nous = Enfant::lancer(
        "f_nous",
        &dir,
        &[("SONDE_ID", fichier.display().to_string())],
    );
    nous.attendre_ligne("PRET");
    let mut pair = Enfant::lancer(
        "f_pair",
        &dir,
        &[("SONDE_ID", fichier.display().to_string())],
    );
    pair.attendre_ligne("FINI");
    pair.terminer();
    let vu_lu = nous.attendre_ligne("VU ");
    nous.terminer();

    let arete: u64 = std::fs::read_to_string(&fichier)
        .expect("id-f")
        .trim()
        .parse()
        .expect("id d'arête");
    let db = ObrainDB::open(&dir).expect("réouverture exclusive (F)");
    let sub = db.substrate_handle().expect("substrate_handle (F)");
    let cle = sub.get_edge_property(EdgeId(arete), &PropertyKey::new("cle_du_pair"));
    db.close().expect("close (F)");
    println!(
        "[F] arête du pair vue par l'écrivain antérieur + écriture relue par lui : {vu_lu} | \
         après réouverture exclusive, 'cle_du_pair'={cle:?}"
    );
}

fn scenario_e() {
    let td = tempfile::tempdir().expect("tempdir E");
    let dir = td.path().join("store-e");
    // Base de départ : une seule valeur longue, pour que la colonne blob et son
    // spécifiant existent déjà à l'ouverture des deux écrivains — reproduction
    // fidèle de `blob_arene_deux_processus.rs` (pas de vecteur ici).
    {
        let db = ObrainDB::open(&dir).expect("open (E base)");
        let id = db.create_node(&["Sonde"]);
        db.set_node_property(id, "corps", Value::String("z".repeat(300).into()));
        db.close().expect("close (E base)");
    }

    // A ouvre la base AVANT le pair : son writer de blobs cache `arena_end`.
    let a = ObrainDB::open_shared_write(&dir).expect("A open_shared_write (E)");
    let mut b = Enfant::lancer(
        "e_b",
        &dir,
        &[
            ("SONDE_TAILLE", "300".to_string()),
            ("SONDE_CAR", "b".to_string()),
        ],
    );
    let id_b: u64 = b.attendre_ligne("ID ").parse().expect("id de B");
    b.terminer();
    // A écrit à son `arena_end` périmé, par-dessus les octets de B.
    let id_a = {
        let id = a.create_node(&["Sonde"]);
        a.set_node_property(id, "corps", Value::String("a".repeat(400).into()));
        id.0
    };
    a.close().expect("close A (E)");

    let mut lit = Enfant::lancer("e_lit", &dir, &[]);
    lit.attendre_ligne("FINI");
    lit.terminer();
    let lecture = std::fs::read_to_string(td.path().join("lecture-e.txt")).expect("lecture-e");
    let mut corps_a: Option<String> = None;
    let mut corps_b: Option<String> = None;
    for l in lecture.lines() {
        let mut champs = l.split('\t');
        let i = champs.next().unwrap_or("");
        let v = champs.next().unwrap_or("");
        if i == id_a.to_string() {
            corps_a = Some(v.to_string());
        }
        if i == id_b.to_string() {
            corps_b = Some(v.to_string());
        }
    }
    let a_intacte = corps_a.as_deref() == Some("a".repeat(400).as_str());
    let b_corrompue = corps_b.as_deref() != Some("b".repeat(300).as_str());
    println!(
        "[E] A intacte={a_intacte} | B corrompue={b_corrompue} \
         (B relue={:?})",
        corps_b.as_deref().map(|s| &s[..s.len().min(10)])
    );
}

#[test]
#[ignore = "sonde multi-processus : lancer avec --ignored"]
fn sonde_multi_ecrivain() {
    if let Some(r) = role() {
        jouer_role(&r);
        return;
    }
    println!("=== SONDE MULTI-ÉCRIVAIN OBRAIN ===");
    let t0 = Instant::now();
    scenario_a();
    scenario_b();
    scenario_c();
    scenario_d();
    scenario_e();
    scenario_f();
    println!("=== FIN ({} s) ===", t0.elapsed().as_secs_f32());
}
