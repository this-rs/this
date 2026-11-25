//! Sonde multi-écrivain d'obrain (feature `obrain`) — met en défaut l'écriture
//! partagée multi-processus pour remonter des issues à obrain-db/obrain.
//!
//! Ce test NE FAIT PAS partie de la suite ordinaire : il est `#[ignore]` et se
//! lance explicitement :
//!
//!     cargo +1.99.1 test -p this-rs --features obrain \
//!         --test multi_writer_probe -- --ignored --nocapture
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
const TEST: &str = "multi_writer_probe";

/// Nœuds par écrivain du scénario A.
const N_A: u64 = 200;
/// Clés distinctes par écrivain du scénario B.
const K_B: u64 = 120;
/// Écritures de la clé partagée (scénario B).
const M_B: u64 = 150;
/// Clés par variant du scénario C.
const K_C: u64 = 120;
/// Longueur d'une valeur longue : > 256 octets → arène des blobs (`dat`).
const LONG_VALUE_SIZE: usize = 600;

fn role() -> Option<String> {
    std::env::var(ROLE).ok()
}

fn required_env(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn announce(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

fn wait_for_file(p: &Path, quoi: &str) {
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

const LINE_TIMEOUT: Duration = Duration::from_secs(120);

/// Un sous-processus relancé depuis ce même binaire de test, avec son stdout
/// canalisé pour la synchronisation par lignes.
struct Writer {
    process: Child,
    lignes: std::sync::mpsc::Receiver<String>,
    _lecteur: std::thread::JoinHandle<()>,
}

impl Writer {
    fn spawn(role: &str, dir: &Path, extras: &[(&str, String)]) -> Self {
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
        let output = process.stdout.take().expect("stdout de l'enfant");
        let (tx, rx) = std::sync::mpsc::channel();
        let lecteur = std::thread::spawn(move || {
            for ligne in std::io::BufReader::new(output)
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

    fn wait_for_line(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.recv_timeout(LINE_TIMEOUT) {
                Ok(l) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    panic!("enfant muet depuis {LINE_TIMEOUT:?} en attendant « {prefixe} »")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("enfant terminé sans « {prefixe} »")
                }
            }
        }
    }

    /// Attend la fin propre, avec une borne : un enfant bloqué sur un verrou
    /// doit échouer, pas figer la sonde.
    fn finish(mut self) {
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

fn long_value(proprietaire: &str, i: u64) -> String {
    format!("{proprietaire}:{i}:{}", "x".repeat(LONG_VALUE_SIZE))
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
fn decode_long(s: &str) -> Option<(String, u64)> {
    let (o, reste) = s.split_once(':')?;
    let (idx, _) = reste.split_once(':')?;
    Some((o.to_string(), idx.parse().ok()?))
}

/// Parse la ligne « IDS 1,2,3 » d'un enfant.
fn ids_from(ligne: &str) -> Vec<u64> {
    ligne
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("id"))
        .collect()
}

fn put_node(db: &ObrainDB, proprietaire: &str, i: u64) -> NodeId {
    let id = db.create_node(&["Sonde"]);
    db.set_node_property(id, "_proprietaire", Value::String(proprietaire.into()));
    db.set_node_property(id, "_index", Value::Int64(i as i64));
    db.set_node_property(
        id,
        "_long",
        Value::String(long_value(proprietaire, i).into()),
    );
    db.set_node_property(id, "_vec", Value::Vector(vecteur(proprietaire, i).into()));
    id
}

/// Crée une base neuve, y pose un nœud (dimensionne les zones ET initialise la
/// colonne blob / la colonne de vecteurs), puis ferme.
fn create_base(dir: &Path) {
    let db = ObrainDB::open(dir).expect("open (créateur exclusif)");
    let id = db.create_node(&["Sonde"]);
    db.set_node_property(id, "_long", Value::String(long_value("init", 0).into()));
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
    let n: u64 = required_env("SONDE_N").parse().expect("SONDE_N");
    let mut ids = Vec::with_capacity(n as usize);
    for i in 0..n {
        ids.push(put_node(&db, proprietaire, i).0);
    }
    let liste = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    announce(&format!("IDS {liste}"));
    if propre {
        db.close().expect("close (A propre)");
        return;
    }
    // Continue de poser des nœuds (jamais flushés, jamais annoncés) : le parent
    // tue ce processus au milieu de cette boucle.
    let mut i = n;
    loop {
        put_node(&db, proprietaire, i);
        i += 1;
        std::thread::sleep(Duration::from_micros(50));
    }
}

/// B — deux écrivains sur la MÊME entité (verrou d'entité MP-3b) : clés
/// distinctes + une clé martelée en commun.
fn role_b(dir: &Path, proprietaire: &str) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (B)");
    let node: u64 = required_env("SONDE_NODE").parse().expect("SONDE_NODE");
    let k: u64 = required_env("SONDE_K").parse().expect("SONDE_K");
    let m: u64 = required_env("SONDE_M").parse().expect("SONDE_M");
    let id = NodeId(node);
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
    announce("FINI");
}

/// C — import en lot contre écrivain unitaire, même entité (#386). Le départ
/// est synchronisé par un flag pour forcer le recouvrement.
fn role_c(dir: &Path, variant: &str) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (C)");
    let node: u64 = required_env("SONDE_NODE").parse().expect("SONDE_NODE");
    let k: u64 = required_env("SONDE_K").parse().expect("SONDE_K");
    let go = PathBuf::from(required_env("SONDE_GO"));
    announce("PRET");
    wait_for_file(&go, "le départ du scénario C");
    match variant {
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
            db.bulk_import_props(node as u32, &[props])
                .expect("bulk_import_props (C)");
        }
        "unitaire" => {
            for i in 0..k {
                let cle = format!("U{i}");
                db.set_node_property(NodeId(node), &cle, Value::String(format!("U-{i}").into()));
            }
        }
        autre => panic!("variant C inconnue : {autre}"),
    }
    db.close().expect("close (C)");
    announce("FINI");
}

/// D — un pair crée une entité APRÈS notre ouverture (#387). « nous » ouvre
/// d'abord, attend l'id du nœud créé par le pair, mesure s'il le VOIT, puis
/// écrit dessus.
fn role_d(dir: &Path, variant: &str) {
    match variant {
        "nous" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (D nous)");
            let id_file = PathBuf::from(required_env("SONDE_ID"));
            announce("PRET");
            wait_for_file(&id_file, "l'id du nœud créé par le pair");
            let node: u64 = std::fs::read_to_string(&id_file)
                .expect("lire l'id")
                .trim()
                .parse()
                .expect("id valide");
            let vu = db.get_node(NodeId(node)).is_some();
            db.set_node_property(
                NodeId(node),
                "cle_du_pair",
                Value::String("ecrite_par_nous".into()),
            );
            let lu = db
                .substrate_handle()
                .and_then(|s| s.get_node_property(NodeId(node), &PropertyKey::new("cle_du_pair")))
                .is_some();
            announce(&format!("VU {} LU {}", vu as u8, lu as u8));
            db.close().expect("close (D nous)");
        }
        "pair" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (D pair)");
            let id = db.create_node(&["Sonde"]);
            let id_file = PathBuf::from(required_env("SONDE_ID"));
            std::fs::write(&id_file, id.0.to_string()).expect("écrire l'id du nœud");
            db.close().expect("close (D pair)");
            announce("FINI");
        }
        autre => panic!("variant D inconnue : {autre}"),
    }
}

/// E — arène des blobs : deux écrivains VIVANTS écrivent des valeurs longues.
/// « e_b » écrit une valeur courte-routée-blob (300 octets) et ferme ; « e_lit »
/// rouvre et déverse les valeurs longues dans un file.
fn role_e(dir: &Path, variant: &str) {
    match variant {
        "e_b" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (E B)");
            let n: usize = required_env("SONDE_SIZE").parse().expect("SONDE_SIZE");
            let c: char = required_env("SONDE_CHAR").parse().expect("SONDE_CHAR");
            let id = db.create_node(&["Sonde"]);
            db.set_node_property(id, "corps", Value::String(c.to_string().repeat(n).into()));
            announce(&format!("ID {}", id.0));
            db.close().expect("close (E B)");
        }
        "e_lit" => {
            // Relecture par Cypher, comme `blob_arene_deux_processus.rs` : la
            // requête hydrate les propriétés du nœud (dont la colonne blob).
            let db = ObrainDB::open_shared_write(dir).expect("relecture (E)");
            let resultat = db
                .execute("MATCH (n:Sonde) RETURN id(n), n.corps")
                .expect("requête (E)");
            let mut output = String::new();
            for ligne in &resultat.rows {
                let id = match ligne.first() {
                    Some(Value::Int64(i)) => i.to_string(),
                    autre => format!("<{autre:?}>"),
                };
                let corps = match ligne.get(1) {
                    Some(Value::String(s)) => s.to_string(),
                    autre => format!("<{autre:?}>"),
                };
                output.push_str(&format!("{id}\t{corps}\n"));
            }
            db.close().expect("close relecture (E)");
            std::fs::write(dir.parent().expect("parent").join("lecture-e.txt"), output)
                .expect("écrire lecture-e");
            announce("FINI");
        }
        autre => panic!("variant E inconnue : {autre}"),
    }
}

/// F — une ARÊTE créée par un pair APRÈS notre ouverture : le pendant du
/// scénario D côté zone des **arêtes**. #398 (correctif de #387) annonce ce côté
/// « symétrique mais aucun test inter-processus de bout en bout » — c'est
/// exactement ce que cette variant mesure.
fn role_f(dir: &Path, variant: &str) {
    match variant {
        "nous" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (F nous)");
            let file = PathBuf::from(required_env("SONDE_ID"));
            announce("PRET");
            wait_for_file(&file, "l'id de l'arête créée par le pair");
            let edge: u64 = std::fs::read_to_string(&file)
                .expect("lire l'id d'arête")
                .trim()
                .parse()
                .expect("id d'arête valide");
            let vu = db.get_edge(EdgeId(edge)).is_some();
            db.set_edge_property(
                EdgeId(edge),
                "cle_du_pair",
                Value::String("ecrite_par_nous".into()),
            );
            let lu = db
                .substrate_handle()
                .and_then(|s| s.get_edge_property(EdgeId(edge), &PropertyKey::new("cle_du_pair")))
                .is_some();
            announce(&format!("VU {} LU {}", vu as u8, lu as u8));
            db.close().expect("close (F nous)");
        }
        "pair" => {
            let db = ObrainDB::open_shared_write(dir).expect("open_shared_write (F pair)");
            let a = db.create_node(&["Sonde"]);
            let b = db.create_node(&["Sonde"]);
            let edge = db.create_edge(a, b, "Sonde");
            let file = PathBuf::from(required_env("SONDE_ID"));
            std::fs::write(&file, edge.0.to_string()).expect("écrire l'id d'arête");
            db.close().expect("close (F pair)");
            announce("FINI");
        }
        autre => panic!("variant F inconnue : {autre}"),
    }
}

fn play_role(r: &str) {
    let dir = PathBuf::from(required_env(DIR));
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
    create_base(&dir);

    let mut a = Writer::spawn("a_a", &dir, &[("SONDE_N", N_A.to_string())]);
    let mut b = Writer::spawn("a_b", &dir, &[("SONDE_N", N_A.to_string())]);
    let ids_a = ids_from(&a.wait_for_line("IDS "));
    let ids_b = ids_from(&b.wait_for_line("IDS "));
    assert_eq!(ids_a.len(), N_A as usize, "A doit announce ses {N_A} ids");
    assert_eq!(ids_b.len(), N_A as usize, "B doit announce ses {N_A} ids");
    b.finish();
    // Tue A pendant qu'il continue d'écrire (boucle sans fin, jamais flushé).
    a.tuer();

    let db = ObrainDB::open(&dir).expect("réouverture exclusive (A)");
    let sub = db.substrate_handle().expect("substrate_handle (A)");

    // (propriétaire, ids) → compteurs.
    struct Mesure {
        nodes_manquants: i64,
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
            nodes_manquants: 0,
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
                m.nodes_manquants += 1;
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
                Some(Value::String(s)) => match decode_long(&s) {
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
        manquants += m.nodes_manquants;
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
                m.nodes_manquants, m.courtes_fausses, m.longues_mauvaises, m.vecteurs_mauvais
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
    let node = {
        let db = ObrainDB::open(&dir).expect("open (B)");
        let id = db.create_node(&["Sonde"]);
        db.close().expect("close (B)");
        id.0
    };
    let mut a = Writer::spawn(
        "b_a",
        &dir,
        &[
            ("SONDE_NODE", node.to_string()),
            ("SONDE_K", K_B.to_string()),
            ("SONDE_M", M_B.to_string()),
        ],
    );
    let mut b = Writer::spawn(
        "b_b",
        &dir,
        &[
            ("SONDE_NODE", node.to_string()),
            ("SONDE_K", K_B.to_string()),
            ("SONDE_M", M_B.to_string()),
        ],
    );
    a.wait_for_line("FINI");
    b.wait_for_line("FINI");
    a.finish();
    b.finish();

    let db = ObrainDB::open(&dir).expect("réouverture exclusive (B)");
    let sub = db.substrate_handle().expect("substrate_handle (B)");
    let id = NodeId(node);
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
    let node = {
        let db = ObrainDB::open(&dir).expect("open (C)");
        let id = db.create_node(&["Sonde"]);
        db.set_node_property(id, "base", Value::String("presente".into()));
        db.close().expect("close (C)");
        id.0
    };
    let go = td.path().join("go-c");
    let mut lot = Writer::spawn(
        "c_lot",
        &dir,
        &[
            ("SONDE_NODE", node.to_string()),
            ("SONDE_K", K_C.to_string()),
            ("SONDE_GO", go.display().to_string()),
        ],
    );
    let mut unitaire = Writer::spawn(
        "c_unitaire",
        &dir,
        &[
            ("SONDE_NODE", node.to_string()),
            ("SONDE_K", K_C.to_string()),
            ("SONDE_GO", go.display().to_string()),
        ],
    );
    lot.wait_for_line("PRET");
    unitaire.wait_for_line("PRET");
    std::fs::write(&go, b"go").expect("poser le flag (C)");
    lot.wait_for_line("FINI");
    unitaire.wait_for_line("FINI");
    lot.finish();
    unitaire.finish();

    let db = ObrainDB::open(&dir).expect("réouverture exclusive (C)");
    let sub = db.substrate_handle().expect("substrate_handle (C)");
    let id = NodeId(node);
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
    create_base(&dir);
    let id_file = td.path().join("id-d.txt");

    let mut nous = Writer::spawn(
        "d_nous",
        &dir,
        &[("SONDE_ID", id_file.display().to_string())],
    );
    nous.wait_for_line("PRET");
    let mut pair = Writer::spawn(
        "d_pair",
        &dir,
        &[("SONDE_ID", id_file.display().to_string())],
    );
    pair.wait_for_line("FINI");
    pair.finish();
    let vu_lu = nous.wait_for_line("VU ");
    nous.finish();

    let node: u64 = std::fs::read_to_string(&id_file)
        .expect("id-d")
        .trim()
        .parse()
        .expect("id");
    let db = ObrainDB::open(&dir).expect("réouverture exclusive (D)");
    let sub = db.substrate_handle().expect("substrate_handle (D)");
    let cle = sub.get_node_property(NodeId(node), &PropertyKey::new("cle_du_pair"));
    db.close().expect("close (D)");
    println!(
        "[D] pair vu par l'écrivain antérieur + écriture relue par lui : {vu_lu} | \
         après réouverture exclusive, 'cle_du_pair'={cle:?}"
    );
}

fn scenario_f() {
    let td = tempfile::tempdir().expect("tempdir F");
    let dir = td.path().join("store-f");
    create_base(&dir);
    let file = td.path().join("id-f.txt");

    let mut nous = Writer::spawn("f_nous", &dir, &[("SONDE_ID", file.display().to_string())]);
    nous.wait_for_line("PRET");
    let mut pair = Writer::spawn("f_pair", &dir, &[("SONDE_ID", file.display().to_string())]);
    pair.wait_for_line("FINI");
    pair.finish();
    let vu_lu = nous.wait_for_line("VU ");
    nous.finish();

    let edge: u64 = std::fs::read_to_string(&file)
        .expect("id-f")
        .trim()
        .parse()
        .expect("id d'arête");
    let db = ObrainDB::open(&dir).expect("réouverture exclusive (F)");
    let sub = db.substrate_handle().expect("substrate_handle (F)");
    let cle = sub.get_edge_property(EdgeId(edge), &PropertyKey::new("cle_du_pair"));
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
    let mut b = Writer::spawn(
        "e_b",
        &dir,
        &[
            ("SONDE_SIZE", "300".to_string()),
            ("SONDE_CHAR", "b".to_string()),
        ],
    );
    let id_b: u64 = b.wait_for_line("ID ").parse().expect("id de B");
    b.finish();
    // A écrit à son `arena_end` périmé, par-dessus les octets de B.
    let id_a = {
        let id = a.create_node(&["Sonde"]);
        a.set_node_property(id, "corps", Value::String("a".repeat(400).into()));
        id.0
    };
    a.close().expect("close A (E)");

    let mut lit = Writer::spawn("e_lit", &dir, &[]);
    lit.wait_for_line("FINI");
    lit.finish();
    let read = std::fs::read_to_string(td.path().join("lecture-e.txt")).expect("lecture-e");
    let mut corps_a: Option<String> = None;
    let mut corps_b: Option<String> = None;
    for l in read.lines() {
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
fn multi_writer_probe() {
    if let Some(r) = role() {
        play_role(&r);
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
