//! Durabilité du record OBRR — écriture atomique MESURÉE, interruption RÉELLE,
//! reconstruction interprocessus, et versionnement V1/V2 croisé.
//!
//! # Ce que ce fichier mesure, et ce qu'il ne mesure pas
//!
//! Mesuré ici :
//!   1. l'ORDRE des opérations d'écriture (temporaire → octets → `fsync` fichier
//!      → `rename` → `fsync` répertoire), par observation directe ;
//!   2. l'ATOMICITÉ sous interruption RÉELLE (processus tué par SIGKILL), avec un
//!      TÉMOIN POSITIF : un écrivain non atomique, tué de la même façon, laisse
//!      bien des records tronqués. Sans ce témoin, un « 0 troncature » ne
//!      prouverait rien du tout (il pourrait signifier que l'instrument ne voit
//!      rien) ;
//!   3. la DIFFÉRENCE entre une interruption réelle et une troncature
//!      artificielle : la troncature artificielle écrit à la main l'artefact d'un
//!      écrivain NON atomique, et ne mesure donc que le décodeur ;
//!   4. la reconstruction dans un PROCESSUS INDÉPENDANT (spawn réel) ;
//!   5. l'OBLIGATION d'une opération indéterminée, préservée après une mort
//!      abrupte (SIGABRT) : le record survit, la résolution ne conclut jamais à
//!      une absence définitive ;
//!   6. le refus d'un record V1 NE LE DÉTRUIT PAS : quarantaine explicite, octets
//!      et provenance préservés, aucune autorité fabriquée ;
//!   7. la lecture croisée V1↔V2 dans les deux sens ;
//!   8. les corruptions SYNTAXIQUEMENT VALIDES (K vide, génération 0, autorité 0,
//!      bombe d'allocation) : ce que le codec laisse passer, la qualification le
//!      nomme.
//!
//! Non mesuré ici, et pourquoi :
//!   - la survie à une coupure d'ALIMENTATION : `fsync` en est la condition
//!     nécessaire, la preuve exigerait une panne matérielle ;
//!   - l'authenticité : le format n'est pas signé ; une réécriture complète et
//!     cohérente est indétectable. Ce fichier ne prétend jamais le contraire.

#![cfg(feature = "obrain")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, Precondition, ReferenceTentative,
    SubstrateStore,
};
use this::reconciliation::{
    DecodageEnregistrement, ENREGISTREMENT_VERSION, EnregistrementOriginal, EtapeEcriture,
    EtatEnregistrement, MotifIllisibilite, ResolutionLegacy, VerdictReconciliation, decoder_v1,
    ecrire_durable, ecrire_durable_observe, empreinte_sha256, qualifier, reconcilier,
    relire_qualifie, temporaires_orphelins,
};
use this_harness::attribution::{
    Attribution, Definition, Etat, Observation, Reception, Record, Resultat, RésolutionBackend,
};

// ───────────────────────────────────────────────────────────────────────────────
// Outillage
// ───────────────────────────────────────────────────────────────────────────────

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "durab_obrr_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

fn def(identite: &str) -> Definition {
    Definition {
        identite: Some(identite.to_string()),
        prevue: Some(("a".to_string(), "m1".to_string())),
        servie: Some(("a".to_string(), "m1".to_string())),
    }
}

fn obs() -> Observation {
    Observation {
        fournisseur: "a".to_string(),
        modele: "m1".to_string(),
        resultat: Resultat::Succes,
    }
}

/// Les modes des processus enfants, distingués par variable d'environnement —
/// jamais par un chemin fixe.
fn mode() -> Option<String> {
    std::env::var("OBRR_MODE").ok()
}

fn est(m: &str) -> bool {
    mode().as_deref() == Some(m)
}

fn exe() -> PathBuf {
    std::env::current_exe().expect("chemin de l'exécutable de test")
}

/// Lance CE binaire de test sur un seul cas, avec l'environnement du mode.
fn lancer(test: &str, m: &str, d: &Path) -> std::process::Child {
    Command::new(exe())
        .args(["--exact", test])
        .env("OBRR_MODE", m)
        .env("OBRR_DIR", d)
        .spawn()
        .expect("spawn du processus enfant")
}

/// Le cycle COMPLET du consommateur, avec de VRAIES valeurs de réservation :
/// l'autorité, la génération et la décision sortent de `Attribution`, jamais
/// d'une constante. Un record dont `autorite_consommateur` vaut 0 ne serait pas
/// un record : ce serait une autorité fabriquée par le test.
struct Cycle {
    enr: EnregistrementOriginal,
    /// Issue de publication observée (publiée ou non).
    publie: bool,
}

fn cycle_ecrivain(d: &Path, chemin_record: &Path, k: u128, decision_id: u128) -> Cycle {
    let store = SubstrateStore::create(d.join("kb")).expect("création du store");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store
        .capturer_precondition(id, &["a"])
        .expect("capture précondition");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let reference = prep.reference_tentative();

    // Le cycle RÉEL : record → observation → réservation.
    let mut attr = Attribution::new();
    attr.record(decision_id, def("k1"));
    attr.recevoir(decision_id, obs());
    let tentative = attr.reserver(decision_id).expect("réservation réelle");

    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k,
        domaine: "durabilite-test".to_string(),
        ecrivain: reference.auteur(),
        generation: tentative.generation(),
        decision_id: tentative.decision_id(),
        autorite_consommateur: tentative.autorite(),
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: reference.to_bytes(),
    };
    assert!(
        enr.verifier_liaison(&tentative),
        "la liaison au descripteur RÉEL est vérifiée avant d'écrire"
    );

    // La chaîne d'autorité est CONSTITUÉE AVANT publication.
    ecrire_durable(chemin_record, &enr.encoder().unwrap()).expect("écriture durable du record");
    let publie = store
        .publier_si_precondition_intacte(prep)
        .expect("publication")
        == IssuePublication::PublieIntegrale;
    store.close().expect("fermeture");
    assert!(publie, "publication intégrale");
    assert!(
        attr.resoudre(decision_id, tentative, RésolutionBackend::EffetConfirme),
        "le cycle RAM se termine sur la même réservation"
    );
    Cycle { enr, publie }
}

/// Les octets d'un record V1 — le codec HISTORIQUE (7790f0b2), sans
/// `decision_id` ni `autorite_consommateur`. Construit ici pour que la lecture
/// croisée porte sur le format réellement écrit, pas sur une vue de l'esprit.
fn octets_v1(
    k: &str,
    backend: u64,
    generation: u64,
    cible: NodeId,
    effets: &[(&str, i64)],
    reference_bytes: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"OBRR");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(k.len() as u32).to_le_bytes());
    out.extend_from_slice(k.as_bytes());
    out.extend_from_slice(&backend.to_le_bytes());
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&cible.0.to_le_bytes());
    out.extend_from_slice(&(effets.len() as u32).to_le_bytes());
    for (cle, val) in effets {
        out.extend_from_slice(&(cle.len() as u32).to_le_bytes());
        out.extend_from_slice(cle.as_bytes());
        out.extend_from_slice(&val.to_le_bytes());
    }
    out.extend_from_slice(&(reference_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(reference_bytes);
    out
}

fn record_v2(k: u128, generation: u64, autorite: u64) -> EnregistrementOriginal {
    EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k,
        domaine: "durabilite-test".to_string(),
        ecrivain: 7,
        generation,
        decision_id: 3,
        autorite_consommateur: autorite,
        cible: NodeId(99),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: vec![0xAB, 0xCD, 0xEF],
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 1. L'ORDRE de l'écriture durable est OBSERVÉ, pas supposé
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn ecriture_durable_ordre_observe() {
    let d = dir("ordre");
    let chemin = d.join("record.obrr");
    let octets = record_v2(1, 3, 1).encoder().unwrap();

    let mut etapes: Vec<EtapeEcriture> = Vec::new();
    ecrire_durable_observe(&chemin, &octets, &mut |e| etapes.push(e)).expect("écriture durable");

    assert_eq!(
        etapes,
        vec![
            EtapeEcriture::TemporaireCree,
            EtapeEcriture::OctetsEcrits,
            // `fsync` du FICHIER avant le rename : sans lui, le nom peut être
            // publié avant les octets qu'il désigne.
            EtapeEcriture::FichierSynchronise,
            EtapeEcriture::Renomme,
            // `fsync` du RÉPERTOIRE après le rename : sans lui, le rename
            // lui-même peut être perdu.
            EtapeEcriture::RepertoireSynchronise,
        ],
        "les cinq étapes, dans cet ordre — mesuré, pas affirmé"
    );

    // Le contenu publié est EXACTEMENT celui demandé, et aucun temporaire ne
    // subsiste après une écriture qui a réussi.
    let relu = std::fs::read(&chemin).expect("lecture");
    assert_eq!(empreinte_sha256(&relu), empreinte_sha256(&octets));
    assert!(temporaires_orphelins(&chemin).expect("balayage").is_empty());

    // Deuxième écriture : même ordre, et l'ancien contenu est REMPLACÉ (le rename
    // remplace, il n'ajoute pas).
    let octets2 = record_v2(2, 4, 1).encoder().unwrap();
    ecrire_durable(&chemin, &octets2).expect("remplacement");
    let relu2 = std::fs::read(&chemin).expect("lecture");
    assert_eq!(empreinte_sha256(&relu2), empreinte_sha256(&octets2));
    assert_ne!(empreinte_sha256(&relu), empreinte_sha256(&relu2));

    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. Interruption RÉELLE (SIGKILL) vs troncature ARTIFICIELLE
// ───────────────────────────────────────────────────────────────────────────────

/// Écrivain enfant en boucle. Deux modes, pour que la mesure ait un TÉMOIN :
/// - `atomique` : `ecrire_durable` (temporaire + fsync + rename + fsync) ;
/// - `direct` : écriture fragmentée DIRECTEMENT dans le record final, sans
///   temporaire ni rename — l'implémentation naïve, celle dont la troncature
///   artificielle reproduit l'artefact.
///
/// Dans les deux cas le record est ADMISSIBLE (autorité et génération réelles).
#[test]
fn enfant_ecrivain_en_boucle() {
    let m = match mode() {
        Some(m) if m == "atomique" || m == "direct" => m,
        _ => return,
    };
    let d = PathBuf::from(std::env::var("OBRR_DIR").expect("OBRR_DIR"));
    let chemin = d.join("record.obrr");
    let decision_id = 11u128;

    let mut attr = Attribution::new();
    attr.record(decision_id, def("k-boucle"));
    attr.recevoir(decision_id, obs());

    loop {
        let t = attr.reserver(decision_id).expect("réservation");
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 1,
            domaine: "durabilite-test".to_string(),
            ecrivain: 7,
            generation: t.generation(),
            decision_id: t.decision_id(),
            autorite_consommateur: t.autorite(),
            cible: NodeId(5),
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            // Assez gros pour qu'une écriture ne soit pas instantanée : c'est la
            // fenêtre dans laquelle le kill doit tomber.
            reference_bytes: vec![t.generation() as u8; 4 * 1024 * 1024],
        };
        let octets = enr.encoder().unwrap();
        if m == "atomique" {
            ecrire_durable(&chemin, &octets).expect("écriture durable");
        } else {
            // Écriture NON atomique : 256 fragments de 64 Kio directement dans le
            // fichier final. Un kill au milieu laisse un record tronqué.
            let mut f = std::fs::File::create(&chemin).expect("création directe");
            for fragment in octets.chunks(64 * 1024) {
                f.write_all(fragment).expect("fragment");
                std::thread::yield_now();
            }
            // Pas de `fsync` du tout : même pas la peine de simuler la durabilité.
        }
        attr.echec_certain(decision_id, t);
    }
}

#[derive(Debug, Default)]
struct Bilan {
    absents: usize,
    courants: usize,
    illisibles: usize,
    orphelins: usize,
    /// Première raison d'illisibilité observée (diagnostic).
    raisons: Vec<String>,
}

fn mesurer_crash(m: &str, essais: usize) -> Bilan {
    let mut bilan = Bilan::default();
    let mut graine = 0x2545_F491_4F6C_DD1Du64;
    for i in 0..essais {
        let d = dir(&format!("crash_{m}_{i}"));
        let chemin = d.join("record.obrr");
        let mut enfant = lancer("enfant_ecrivain_en_boucle", m, &d);

        // Délai pseudo-aléatoire déterministe : le kill doit tomber à des moments
        // DIFFÉRENTS de la boucle, pas toujours au même endroit.
        graine = graine
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let attente = 4 + (graine >> 33) % 30; // 4..=33 ms
        std::thread::sleep(std::time::Duration::from_millis(attente));

        // SIGKILL : aucune chance de terminaison propre, aucun `Drop`, aucun
        // vidage de tampon par le langage. C'est une interruption RÉELLE.
        enfant.kill().expect("kill du processus écrivain");
        let statut = enfant.wait().expect("attente du processus");
        assert!(!statut.success(), "l'écrivain a bien été tué");

        match relire_qualifie(&chemin) {
            Err(_) => bilan.absents += 1, // tué avant le premier rename
            Ok(EtatEnregistrement::Courant(_)) => bilan.courants += 1,
            Ok(EtatEnregistrement::QuarantaineLegacy(_)) => bilan.illisibles += 1,
            Ok(EtatEnregistrement::QuarantaineIllisible(r)) => {
                bilan.illisibles += 1;
                bilan.raisons.push(format!("{:?}", r.motif));
            }
        }
        let orphelins = temporaires_orphelins(&chemin).expect("balayage");
        if !orphelins.is_empty() {
            bilan.orphelins += 1;
            // Un temporaire n'est JAMAIS promu : on ne le lit pas, on ne le
            // renomme pas, on le constate. Il est retiré ici par le test lui-même.
            for o in &orphelins {
                let _ = std::fs::remove_file(o);
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
    bilan
}

#[test]
fn interruption_reelle_vs_troncature_artificielle() {
    const ESSAIS: usize = 14;

    // (a) TÉMOIN POSITIF — un écrivain NON atomique, tué de la même manière,
    //     laisse des records tronqués. Sans ce témoin, le résultat (b) serait
    //     ininterprétable : on ne saurait pas si l'instrument voit quoi que ce soit.
    let temoin = mesurer_crash("direct", ESSAIS);

    // (b) L'écrivain atomique, tué de la même manière.
    let atomique = mesurer_crash("atomique", ESSAIS);

    println!("témoin (direct)   : {temoin:?}");
    println!("atomique          : {atomique:?}");

    assert!(
        temoin.illisibles > 0,
        "TÉMOIN POSITIF : un écrivain non atomique tué doit laisser des records tronqués ; \
         sans cela la mesure ne prouve rien (bilan : {temoin:?})"
    );
    assert_eq!(
        atomique.illisibles, 0,
        "un record publié par `ecrire_durable` n'est JAMAIS tronqué, même tué : \
         on lit l'ancien ou le nouveau, jamais un entre-deux (bilan : {atomique:?})"
    );
    assert_eq!(
        atomique.absents + atomique.courants,
        ESSAIS,
        "chaque état final est soit absent, soit courant"
    );

    // ── La différence, nommée précisément ───────────────────────────────────
    // (1) La TRONCATURE ARTIFICIELLE (écrire `&octets[..moitie]` à la main dans
    //     le chemin final) reproduit l'artefact de l'écrivain DIRECT — pas celui
    //     de l'écrivain atomique. Elle mesure donc le DÉCODEUR, et c'est tout.
    let d = dir("artificielle");
    let chemin = d.join("record.obrr");
    let octets = record_v2(1, 1, 1).encoder().unwrap();
    std::fs::write(&chemin, &octets[..octets.len() / 2]).expect("troncature artificielle");
    assert!(
        matches!(
            qualifier(&std::fs::read(&chemin).expect("lecture"), "artificielle"),
            EtatEnregistrement::QuarantaineIllisible(..)
        ),
        "octets tronqués → refus NOMMÉ, jamais une reconstruction silencieuse"
    );
    // La provenance est conservée avec le refus : les octets refusés ne sont pas
    // jetés, ils sont qualifiés.
    let etat = relire_qualifie(&chemin).expect("relecture");
    assert_eq!(etat.octets_preserves().len(), octets.len() / 2);
    assert!(matches!(etat.refus(), Some(MotifIllisibilite::Malforme(_))));
    let _ = std::fs::remove_dir_all(&d);

    // (2) L'INTERRUPTION RÉELLE d'un écrivain atomique ne produit jamais cet
    //     artefact (mesure (b)) : elle produit un temporaire orphelin — constaté
    //     dans les bilans — qui n'est ni lu ni promu.
    assert!(
        atomique.orphelins > 0,
        "une interruption réelle laisse bien un temporaire orphelin à au moins un essai \
         (bilan : {atomique:?}) — c'est son empreinte, pas une troncature"
    );
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. Reconstruction dans un PROCESSUS INDÉPENDANT
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn enfant_ecrivain_cycle() {
    if !est("cycle") {
        return;
    }
    let d = PathBuf::from(std::env::var("OBRR_DIR").expect("OBRR_DIR"));
    let chemin = d.join("record.obrr");
    let cycle = cycle_ecrivain(&d, &chemin, 1, 5);
    assert!(cycle.publie);
    // Le processus meurt ici : c'est un AUTRE processus OS qui a écrit.
}

#[test]
fn reconstruction_interprocessus_independant() {
    let d = dir("interprocessus");
    let chemin = d.join("record.obrr");
    let mut enfant = lancer("enfant_ecrivain_cycle", "cycle", &d);
    let statut = enfant.wait().expect("attente de l'écrivain");
    assert!(statut.success(), "le processus écrivain a réussi");

    // Ce processus (≠ écrivain) qualifie, résout et réconcilie.
    let etat = relire_qualifie(&chemin).expect("relecture du record");
    let EtatEnregistrement::Courant(enr) = etat else {
        panic!("record courant attendu après reconstruction, obtenu : {etat:?}");
    };
    assert_eq!(enr.k, 1);

    let store = SubstrateStore::open(d.join("kb")).expect("réouverture du store");
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme,
        "le composant réutilisable confirme après reconstruction interprocessus"
    );

    // ── Ce que la reconstruction n'est PAS ──────────────────────────────────
    // Une instance `Attribution` NEUVE dans CE processus ne restaure rien : elle
    // RÉÉMET une autorité. Vérifier la liaison du record historique contre cette
    // instance échoue — et ce n'est pas un défaut du record, c'est la preuve que
    // l'autorité n'est pas durable : elle n'est couplée à aucune base physique.
    let mut autre = Attribution::new();
    while autre.autorite() == enr.autorite_consommateur {
        autre = Attribution::new();
    }
    autre.record(enr.decision_id, def("k1"));
    autre.recevoir(enr.decision_id, obs());
    let tentative = autre.reserver(enr.decision_id).expect("réservation");
    assert_eq!(
        tentative.generation(),
        enr.generation,
        "même génération (1)"
    );
    assert!(
        !enr.verifier_liaison(&tentative),
        "`verifier_liaison` n'est PAS un oracle de durabilité : il compare une \
         autorité RAM à une autorité RAM"
    );
    // La preuve de durabilité est ailleurs : la résolution du BACKEND.
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 4. L'autorité d'`Attribution` COLLISIONNE entre processus
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn enfant_autorite() {
    if !est("autorite") {
        return;
    }
    // La PREMIÈRE instance du processus : c'est celle qu'un redémarrage crée.
    println!("AUTORITE={}", Attribution::new().autorite());
}

#[test]
fn autorite_non_durable_collision_interprocessus() {
    let mut valeurs = Vec::new();
    for i in 0..2 {
        let d = dir(&format!("autorite_{i}"));
        let sortie = Command::new(exe())
            .args(["--exact", "enfant_autorite", "--nocapture"])
            .env("OBRR_MODE", "autorite")
            .env("OBRR_DIR", &d)
            .output()
            .expect("spawn");
        assert!(sortie.status.success(), "le processus enfant a réussi");
        let texte = String::from_utf8_lossy(&sortie.stdout);
        let valeur: u64 = texte
            .lines()
            .find_map(|l| l.strip_prefix("AUTORITE="))
            .expect("autorité imprimée par l'enfant")
            .trim()
            .parse()
            .expect("autorité numérique");
        valeurs.push(valeur);
        let _ = std::fs::remove_dir_all(&d);
    }
    println!("autorités des deux processus : {valeurs:?}");
    assert!(
        valeurs[0] >= 1 && valeurs[0] == valeurs[1],
        "deux processus DISTINCTS réémettent la MÊME autorité ({valeurs:?}) : \
         `PROCHAINE_AUTORITE` repart de 1 à chaque démarrage. Conséquence \
         MESURÉE : l'égalité d'autorité ne certifie jamais une restauration — \
         après redémarrage, l'ancien descripteur et le nouveau sont \
         indistinguables par ce champ."
    );
}

// ───────────────────────────────────────────────────────────────────────────────
// 5. Restauration CONTRÔLÉE : un fichier ne restaure pas une réservation
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn restauration_controlee_pas_de_reservation_implicite() {
    let d = dir("restauration");
    let chemin = d.join("record.obrr");
    let decision_id = 7u128;

    let cycle = cycle_ecrivain(&d, &chemin, 1, decision_id);
    let avant = std::fs::read(&chemin).expect("lecture");
    let empreinte_avant = empreinte_sha256(&avant);

    // « REDÉMARRAGE » : instance NEUVE, aucun cycle en RAM.
    let mut attr = Attribution::new();
    let autorite_apres = attr.autorite();

    // Le record ne restaure AUCUNE réservation : ni cycle, ni tentative.
    assert!(
        attr.etat(decision_id).is_none(),
        "aucun cycle restauré depuis le fichier"
    );
    assert!(
        attr.reserver(decision_id).is_none(),
        "pas de réservation implicite"
    );

    // Génération et autorité sont RÉÉMISES explicitement.
    assert_eq!(attr.record(decision_id, def("k1")), Record::Enregistre);
    assert_eq!(attr.recevoir(decision_id, obs()), Reception::Recu);
    let tentative = attr.reserver(decision_id).expect("réservation explicite");
    assert_eq!(
        tentative.generation(),
        1,
        "cycle neuf : la génération repart de 1"
    );
    assert_eq!(
        tentative.autorite(),
        autorite_apres,
        "autorité de l'instance courante"
    );
    assert_ne!(
        tentative.autorite(),
        cycle.enr.autorite_consommateur,
        "l'autorité du record historique n'est pas réémise par la nouvelle instance"
    );

    // Le record historique reste INTACT et consultable : la lecture ne le
    // réécrit pas. Ses métadonnées sont conservées telles quelles.
    let apres = std::fs::read(&chemin).expect("relecture");
    assert_eq!(
        empreinte_sha256(&apres),
        empreinte_avant,
        "la lecture ne modifie pas le record (aucune réécriture silencieuse)"
    );
    assert_eq!(
        cycle.enr.generation, 1,
        "métadonnée G conservée telle quelle"
    );

    // ── Ce que « restaurer » ne peut pas vouloir dire ───────────────────────
    // L'obligation portée par le record n'est pas « une réservation » : c'est
    // une question adressée au BACKEND. Elle survit à la perte de l'instance,
    // et sa réponse ne sort pas de la RAM.
    let store = SubstrateStore::open(d.join("kb")).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&cycle.enr.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        reconcilier(&cycle.enr, &reference, &res),
        VerdictReconciliation::Confirme,
        "l'effet est attesté par la base, pas par la mémoire"
    );
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 6. Opération INDÉTERMINÉE : l'obligation survit à une mort abrupte
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn enfant_mort_apres_record_avant_publication() {
    if !est("indetermine") {
        return;
    }
    let d = PathBuf::from(std::env::var("OBRR_DIR").expect("OBRR_DIR"));
    let chemin = d.join("record.obrr");
    let decision_id = 21u128;

    let store = SubstrateStore::create(d.join("kb")).expect("store");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store
        .capturer_precondition(id, &["a"])
        .expect("capture précondition");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let reference = prep.reference_tentative();

    let mut attr = Attribution::new();
    attr.record(decision_id, def("k-indetermine"));
    attr.recevoir(decision_id, obs());
    let tentative = attr.reserver(decision_id).expect("réservation");
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 1,
        domaine: "durabilite-test".to_string(),
        ecrivain: reference.auteur(),
        generation: tentative.generation(),
        decision_id: tentative.decision_id(),
        autorite_consommateur: tentative.autorite(),
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: reference.to_bytes(),
    };
    // Le record est CONSTITUÉ…
    ecrire_durable(&chemin, &enr.encoder().unwrap()).expect("écriture du record");
    // … puis l'écrivain meurt ABRUPTEMENT : ni publication, ni `close()`, ni
    // résolution. `abort()` ne déroule aucune pile, n'exécute aucun `Drop`.
    std::process::abort();
}

#[test]
fn operation_indeterminee_obligation_preservee() {
    let d = dir("indetermine");
    let chemin = d.join("record.obrr");
    let mut enfant = lancer(
        "enfant_mort_apres_record_avant_publication",
        "indetermine",
        &d,
    );
    let statut = enfant.wait().expect("attente");
    assert!(!statut.success(), "l'écrivain n'a pas terminé normalement");

    // 1. Le record survit BYTE POUR BYTE à la mort du processus.
    let octets = std::fs::read(&chemin).expect("le record existe après la mort abrupte");
    let empreinte = empreinte_sha256(&octets);
    let etat = qualifier(&octets, "après mort abrupte");
    let EtatEnregistrement::Courant(enr) = etat else {
        panic!("record courant attendu, obtenu : {etat:?}");
    };

    // 2. La question adressée au backend est encore POSABLE : la référence est
    //    dans les octets, pas dans la mémoire du mort.
    let store = SubstrateStore::open(d.join("kb")).expect("réouverture après mort abrupte");
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");
    let verdict = reconcilier(&enr, &reference, &res);

    println!(
        "intégrité après mort abrupte = {:?} ; verdict = {verdict:?}",
        res.integrite
    );

    // 3. L'issue n'est PAS tranchée : jamais « publié », jamais « absent
    //    définitivement ». C'est l'obligation, préservée.
    assert_ne!(
        verdict,
        VerdictReconciliation::Confirme,
        "aucune publication n'a eu lieu : le verdict ne peut pas confirmer"
    );
    match verdict.vers_backend() {
        Some(RésolutionBackend::Indetermine) => {}
        None => {} // verdict nuancé : décision explicite exigée, jamais écrasé
        Some(_) => panic!("un verdict nuancé a été écrasé en {verdict:?}"),
    }

    // 4. L'obligation est PORTÉE par le cycle : tant qu'elle n'est pas résolue,
    //    aucune ré-publication aveugle n'est possible.
    let mut attr = Attribution::new();
    attr.record(enr.decision_id, def("k-indetermine"));
    attr.recevoir(enr.decision_id, obs());
    let tentative = attr.reserver(enr.decision_id).expect("réservation");
    // La transition explicite : c'est `indetermine()` qui INSCRIT l'issue
    // inconnue dans le cycle. Mesuré au passage : `resoudre(.., Indetermine)`
    // répond `true` sur un cycle `EnCours` SANS changer son état — c'est une
    // interrogation, pas une transition.
    assert!(
        attr.indetermine(enr.decision_id, tentative),
        "issue inconnue inscrite"
    );
    assert_eq!(
        attr.etat(enr.decision_id).expect("cycle").etat,
        Etat::Indetermine
    );
    assert!(
        attr.resoudre(enr.decision_id, tentative, RésolutionBackend::Indetermine),
        "le backend ne tranche toujours pas : la réponse reste « indéterminé »"
    );
    assert_eq!(
        attr.etat(enr.decision_id).expect("cycle").etat,
        Etat::Indetermine,
        "`resoudre(.., Indetermine)` ne promeut pas un cycle indéterminé"
    );
    assert!(
        attr.reserver(enr.decision_id).is_none(),
        "pas de ré-publication tant que l'obligation n'est pas résolue"
    );

    // 5. Le record n'a pas été touché par la qualification ni par la résolution.
    let apres = std::fs::read(&chemin).expect("relecture");
    assert_eq!(empreinte_sha256(&apres), empreinte, "record inchangé");

    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 7. V1 : le REFUS ne détruit pas le record — quarantaine explicite
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn record_v1_quarantaine_explicite_non_detruit() {
    let d = dir("v1");
    let chemin = d.join("record-v1.obrr");
    let octets = octets_v1(
        "op-v1",
        7,
        2,
        NodeId(42),
        &[("a", 10)],
        &[0xDE, 0xAD, 0xBE, 0xEF],
    );
    std::fs::write(&chemin, &octets).expect("écriture du record V1");
    let empreinte = empreinte_sha256(&octets);

    // Le lecteur COURANT refuse un V1 — nommément.
    assert_eq!(
        EnregistrementOriginal::decoder(&octets),
        Err(DecodageEnregistrement::VersionInconnue(1)),
        "un V1 est refusé comme record courant, jamais migré silencieusement"
    );

    // La qualification, elle, ne perd RIEN : elle met en quarantaine.
    let etat = relire_qualifie(&chemin).expect("relecture");
    assert!(!etat.est_courant(), "un V1 n'est jamais un record courant");
    assert!(
        etat.refus().is_none(),
        "ce n'est pas une illisibilité : c'est du legacy"
    );
    let EtatEnregistrement::QuarantaineLegacy(legacy) = &etat else {
        panic!("quarantaine legacy attendue, obtenu : {etat:?}");
    };

    // Octets + provenance préservés, VERBATIM.
    assert_eq!(legacy.octets, octets, "octets conservés à l'identique");
    assert_eq!(legacy.provenance.longueur, octets.len());
    assert_eq!(legacy.provenance.empreinte_sha256, empreinte);
    assert_eq!(legacy.provenance.source, chemin.to_string_lossy());
    assert_eq!(legacy.version, 1);

    // La vue V1 porte EXACTEMENT les champs du format V1 — et aucun autre.
    // `decision_id` et `autorite_consommateur` n'existent pas : il n'y a pas de
    // valeur par défaut à leur donner, donc pas d'autorité à fabriquer.
    let vue = legacy.vue.clone().expect("V1 bien formé");
    assert_eq!(vue.k, "op-v1");
    assert_eq!(vue.backend, 7);
    assert_eq!(vue.generation, 2);
    assert_eq!(vue.effets_attendus, vec![("a".to_string(), 10)]);
    // L'écart V1→V2 est exactement : decision_id(16) + autorite_consommateur(8)
    // + le passage de K de `k_len(4)+k` (String V1) à `k(16)` (u128 V2) — soit
    // 24 + (16 − (4 + len("op-v1"))) = 24 + 7 = 31 octets. La vue V1 ne fabrique
    // aucune des valeurs V2 : elle ne porte ni decision_id, ni autorité, ni un
    // K u128 (elle garde la chaîne historique).
    let meme_record_en_v2 = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 1,
        domaine: "durabilite-test".to_string(),
        ecrivain: 7,
        generation: 2,
        decision_id: 3,
        autorite_consommateur: 1,
        cible: NodeId(42),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
    };
    assert_eq!(
        meme_record_en_v2.encoder().unwrap().len(),
        legacy.octets.len() + 51,
        "l'écart V1→V5 est decision_id(16) + autorite_consommateur(8) + K string→u128(7) + domaine(19) + tag valeur(1)"
    );

    // Le FICHIER est intact : le refus n'a rien réécrit, rien supprimé.
    let sur_disque = std::fs::read(&chemin).expect("le fichier existe toujours");
    assert_eq!(
        empreinte_sha256(&sur_disque),
        empreinte,
        "aucune mutation du refus"
    );

    // L'OBLIGATION reste résoluble auprès du backend… si la référence est réelle.
    // Ici elle ne l'est pas (octets arbitraires) : la résolution est REFUSÉE
    // nommément, jamais conclue « absence définitive ».
    let store_dir = d.join("kb-vide");
    std::fs::create_dir_all(&store_dir).expect("répertoire");
    let store = SubstrateStore::create(&store_dir).expect("store");
    let resolution = legacy.resoudre_obligation(&store);
    println!("résolution legacy (référence bidon) = {resolution:?}");
    assert!(
        resolution.is_err()
            || matches!(
                resolution,
                Ok(ResolutionLegacy::EffetNonRetrouveJamaisAbsenceDefinitive)
            ),
        "jamais un `EffetEngage` sur une référence non prouvée"
    );
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn record_v1_engage_resolu_sans_identite_attestee() {
    // Un V1 qui porte une VRAIE référence (produite par une vraie publication) :
    // sa résolution atteste l'EFFET, jamais l'identité logique de l'opération.
    let d = dir("v1-engage");
    let chemin_record = d.join("record-v1.obrr");

    let store = SubstrateStore::create(d.join("kb")).expect("store");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store
        .capturer_precondition(id, &["a"])
        .expect("précondition");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let reference = prep.reference_tentative();
    // Un V1 tel que l'ANCIEN moteur l'aurait écrit : pas de décision, pas
    // d'autorité consommateur.
    let octets = octets_v1(
        "op-v1-engage",
        reference.auteur(),
        1,
        reference.cible(),
        &[("a", 10)],
        &reference.to_bytes(),
    );
    std::fs::write(&chemin_record, &octets).expect("écriture");
    assert_eq!(
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication"),
        IssuePublication::PublieIntegrale
    );

    let etat = relire_qualifie(&chemin_record).expect("relecture");
    let EtatEnregistrement::QuarantaineLegacy(legacy) = etat else {
        panic!("quarantaine legacy attendue");
    };
    let resolution = legacy
        .resoudre_obligation(&store)
        .expect("résolution legacy");
    assert_eq!(
        resolution,
        ResolutionLegacy::EffetEngageIdentiteNonAttestee,
        "l'effet est engagé — et l'identité logique reste NON attestée : \
         c'est le maximum que ce format permet d'affirmer"
    );

    // Et l'effet n'est PAS promu en `Confirme` : il faudrait une décision et une
    // autorité que le V1 ne porte pas.
    let reference2 = ReferenceTentative::from_bytes(&legacy.vue.as_ref().unwrap().reference_bytes)
        .expect("référence");
    let res = store
        .resoudre_reference(&reference2)
        .expect("résolution backend");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 8. Lecture croisée V1 ↔ V2
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn lecture_croisee_v1_v2() {
    let v1 = octets_v1("op-croisee", 7, 1, NodeId(42), &[("a", 10)], &[1, 2, 3]);
    let v2 = record_v2(1, 1, 1).encoder().unwrap();

    let empreinte_v1 = empreinte_sha256(&v1);
    let empreinte_v2 = empreinte_sha256(&v2);

    // V1 → lecteur V1 : accepté, et la vue ne contient que ce que le format porte.
    let vue = decoder_v1(&v1).expect("V1 lu par le lecteur V1");
    assert_eq!(vue.version, 1);
    assert_eq!(vue.k, "op-croisee");

    // V1 → lecteur V2 : REFUSÉ, nommément, et mis en quarantaine par `qualifier`.
    assert_eq!(
        EnregistrementOriginal::decoder(&v1),
        Err(DecodageEnregistrement::VersionInconnue(1))
    );
    assert!(matches!(
        qualifier(&v1, "v1"),
        EtatEnregistrement::QuarantaineLegacy(_)
    ));

    // V2 → lecteur V2 : accepté.
    let enr = EnregistrementOriginal::decoder(&v2).expect("V2 lu par le lecteur V2");
    assert_eq!(enr.k, 1);
    assert_eq!(enr.decision_id, 3);
    assert_eq!(enr.autorite_consommateur, 1);

    // V5 → lecteur V1 : REFUSÉ par la version, AVANT tout décalage d'octets.
    // Sans ce refus, les octets V5 se liraient comme un V1 décalé de 51 octets.
    assert_eq!(
        decoder_v1(&v2),
        Err(DecodageEnregistrement::VersionInconnue(ENREGISTREMENT_VERSION)),
        "le lecteur V1 refuse un V5 : c'est ce qui rend le bump suffisant"
    );

    // Aucune lecture n'a modifié les octets (elles n'ont pas de `&mut`).
    assert_eq!(empreinte_sha256(&v1), empreinte_v1);
    assert_eq!(empreinte_sha256(&v2), empreinte_v2);

    // ── Le point qui justifie le bump ──────────────────────────────────────
    // Un V2 dont l'octet de version est FORGÉ à 1 n'est pas relu comme un V1
    // plausible : le décalage de 31 octets rend les champs suivants incohérents
    // et le lecteur V1 le REFUSE nommément. On mesure le refus, on ne prétend
    // pas à l'authenticité : une réécriture complète et cohérente reste
    // indétectable, le format n'est pas signé.
    let mut forge = v2.clone();
    forge[4..8].copy_from_slice(&1u32.to_le_bytes());
    let lecture_forgee = decoder_v1(&forge);
    println!("V2 version forgée à 1, lu par V1 = {lecture_forgee:?}");
    assert!(
        lecture_forgee.is_err(),
        "un V2 relu comme V1 est refusé (décalage incohérent), jamais accepté en silence"
    );
}

// ───────────────────────────────────────────────────────────────────────────────
// 9. Corruptions SYNTAXIQUEMENT VALIDES : ce que le codec laisse passer
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn corruptions_syntaxiquement_valides_nommees() {
    let d = dir("syntaxe");
    let chemin = d.join("record.obrr");

    // (a) K nul · (b) génération 0 · (c) autorité 0.
    let cas: Vec<(&str, EnregistrementOriginal)> = vec![
        ("K nul", record_v2(0, 1, 1)),
        ("génération 0", record_v2(1, 0, 1)),
        ("autorité 0", record_v2(1, 1, 0)),
    ];

    for (nom, enr) in cas {
        let octets = enr.encoder().unwrap();
        // Le CODEC les accepte : ce sont des octets parfaitement formés. C'est
        // précisément pour cela qu'ils sont dangereux.
        assert!(
            EnregistrementOriginal::decoder(&octets).is_ok(),
            "[{nom}] le codec octet-à-octet ne peut pas voir l'inadmissibilité"
        );
        // La QUALIFICATION, elle, refuse et nomme.
        let etat = qualifier(&octets, nom);
        let EtatEnregistrement::QuarantaineIllisible(illisible) = &etat else {
            panic!("[{nom}] refus nommé attendu, obtenu : {etat:?}");
        };
        assert!(
            matches!(illisible.motif, MotifIllisibilite::CorruptionSyntactique(_)),
            "[{nom}] motif attendu CorruptionSyntactique, obtenu {:?}",
            illisible.motif
        );
        // Refus NOMméné, pas disparition : octets et provenance conservés.
        assert_eq!(illisible.octets, octets, "[{nom}] octets préservés");
        assert_eq!(
            illisible.provenance.empreinte_sha256,
            empreinte_sha256(&octets)
        );
        assert!(!etat.est_courant());

        // Sur le médium : le fichier est intact après qualification.
        std::fs::write(&chemin, &octets).expect("écriture");
        let _ = relire_qualifie(&chemin).expect("relecture");
        assert_eq!(
            empreinte_sha256(&std::fs::read(&chemin).expect("lecture")),
            empreinte_sha256(&octets),
            "[{nom}] la qualification ne réécrit pas le fichier"
        );
    }

    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn bombe_allocation_refusee_nommement() {
    // Un en-tête de 24 octets qui se déclare porteur de 2^32-1 effets : MAGIC et
    // version valides, longueur de K valide. Sans borne, le décodeur réserve
    // `n * size_of::<(String, i64)>()` ≈ 137 Gio et le processus meurt — un
    // refus nommé devient un abandon.
    let mut octets = Vec::new();
    octets.extend_from_slice(b"OBRR");
    octets.extend_from_slice(&ENREGISTREMENT_VERSION.to_le_bytes());
    octets.extend_from_slice(&0u128.to_le_bytes()); // K nul (u128)
    octets.extend_from_slice(&7u64.to_le_bytes()); // backend
    octets.extend_from_slice(&1u64.to_le_bytes()); // generation
    octets.extend_from_slice(&0u128.to_le_bytes()); // decision_id
    octets.extend_from_slice(&1u64.to_le_bytes()); // autorite
    octets.extend_from_slice(&99u64.to_le_bytes()); // cible
    octets.extend_from_slice(&u32::MAX.to_le_bytes()); // n_effets ← la bombe
    // 4 magic + 4 version + 16 k + 8 backend + 8 generation + 16 decision_id
    // + 8 autorite + 8 cible + 4 n_effets.
    assert_eq!(octets.len(), 76);

    let taille_annoncee = u32::MAX as usize * std::mem::size_of::<(String, i64)>();
    println!(
        "allocation annoncée par n_effets = {taille_annoncee} octets ({:.0} Gio) — RELU",
        taille_annoncee as f64 / (1024.0 * 1024.0 * 1024.0)
    );

    let refus = EnregistrementOriginal::decoder(&octets);
    println!("refus mesuré = {refus:?}");
    assert!(
        matches!(refus, Err(DecodageEnregistrement::Malforme(_))),
        "refus NOMMÉ avant toute allocation, obtenu : {refus:?}"
    );
    // La qualification conserve les octets, elle aussi.
    let etat = qualifier(&octets, "bombe");
    assert!(matches!(etat, EtatEnregistrement::QuarantaineIllisible(_)));
    assert_eq!(etat.octets_preserves().len(), octets.len());
}
