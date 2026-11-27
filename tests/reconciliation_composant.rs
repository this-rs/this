//! Le composant réutilisable `this::reconciliation` — importé hors tests,
//! exercé de bout en bout : encodage versionné, refus de version inconnue,
//! réconciliation contre l'enregistrement original, préservation des types
//! natifs (Int64 / Float64 / Bytes), refus de la conversion arbitraire, refus
//! d'une résolution substituée, et domaine de stockage HORS-BANDE (collision
//! HLC, obrain 62ecd940).

#![cfg(feature = "obrain")]

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    EffetResolu, EstampilleComplete, EtatPropriete, IntegriteTentative, ModePublication,
    Precondition, ResolutionEffet, ResolutionTentative, SubstrateStore,
};
use this::reconciliation::{
    DecodageEnregistrement, ENREGISTREMENT_VERSION, ENREGISTREMENT_VERSION_LEGACY,
    EncodageEnregistrement, EnregistrementOriginal, EnregistrementV1, Provenance, RecordLegacy,
    ResolutionLegacy, VerdictReconciliation, reconcilier,
};

fn dir(tag: &str) -> std::path::PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "reconc_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    d
}

#[test]
fn composant_roundtrip_et_refus_version_inconnue() {
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 42,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 3,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![
            ("a".to_string(), Value::Int64(10)),
            ("b".to_string(), Value::Int64(20)),
        ],
        reference_bytes: vec![0xAB, 0xCD, 0xEF],
    };
    let octets = enr.encoder().expect("encodage");
    let relu = EnregistrementOriginal::decoder(&octets).expect("décodage");
    assert_eq!(relu, enr, "round-trip fidèle du bloc cohérent unique");

    // Version inconnue → refus NOMÉ, pas d'ignorance silencieuse.
    let mut corrompu = octets.clone();
    // version est à l'offset 4 (après magic 4 octets), u32 LE.
    corrompu[4..8].copy_from_slice(&999u32.to_le_bytes());
    assert_eq!(
        EnregistrementOriginal::decoder(&corrompu),
        Err(DecodageEnregistrement::VersionInconnue(999)),
        "version inconnue refusée"
    );

    // Magic invalide → malformé.
    let mut mauvais_magic = octets.clone();
    mauvais_magic[0] = b'X';
    assert!(matches!(
        EnregistrementOriginal::decoder(&mauvais_magic),
        Err(DecodageEnregistrement::Malforme(_))
    ));
}

#[test]
fn composant_confirme_apres_reouverture() {
    let d = dir("nominal");
    let chemin = d.join("kb");
    let (enr_octets, _id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre: Precondition = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 1,
            domaine: "base-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        let _verdict = store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (octets, id)
    };

    // Réouverture : décoder l'enregistrement, résoudre, réconcilier via le COMPOSANT.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&enr.reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme,
        "le composant réutilisable confirme après réouverture"
    );
    assert!(matches!(
        reconcilier(&enr, &reference, &res).vers_backend(),
        Some(this_harness::attribution::RésolutionBackend::EffetConfirme)
    ));
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Le format v2 (valeurs d'effet `i64` nues) est refusé nommément : le bump de
/// version 2→5 est assumé, un enregistrement v2 n'est jamais réinterprété.
#[test]
fn composant_version_v2_refusee() {
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 7,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 1,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: vec![0xAB],
    };
    let octets = enr.encoder().expect("encodage");
    let mut v2 = octets.clone();
    v2[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        EnregistrementOriginal::decoder(&v2),
        Err(DecodageEnregistrement::VersionInconnue(2)),
        "un enregistrement v2 est refusé, jamais réinterprété en v5"
    );
}

/// Int64, Float64 et Bytes survivent au round-trip, SANS conversion arbitraire.
#[test]
fn composant_valeurs_natives_roundtrip() {
    let payload: std::sync::Arc<[u8]> = (10..42u8).collect::<Vec<u8>>().into();
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 11,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 1,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![
            ("i".to_string(), Value::Int64(-42)),
            ("f".to_string(), Value::Float64(3.25)),
            ("b".to_string(), Value::Bytes(payload)),
        ],
        reference_bytes: vec![0xAA],
    };
    let octets = enr.encoder().expect("encodage");
    let relu = EnregistrementOriginal::decoder(&octets).expect("décodage");
    assert_eq!(relu, enr, "Int64 / Float64 / Bytes survivent au round-trip");
}

/// Un type non représentable est REFUSÉ à l'encodage, jamais converti en Int64.
#[test]
fn composant_refuse_conversion_arbitraire() {
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 13,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 1,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![("b".to_string(), Value::Bool(true))],
        reference_bytes: vec![],
    };
    assert!(
        matches!(
            enr.encoder(),
            Err(EncodageEnregistrement::ValeurNonRepresentable(nom)) if nom == "Bool"
        ),
        "Bool refusé nommément, jamais réduit à Int64"
    );
}

/// La valeur canonique courante porte Int64 / Float64 / Bytes TELLE QUELLE —
/// la réconciliation la consomme via `etat_courant` (EtatPropriete), sans
/// relecture indépendante ni conversion.
#[test]
fn composant_valeurs_natives_reconciliation() {
    let d = dir("types");
    let chemin = d.join("kb");
    let payload_attendu: std::sync::Arc<[u8]> = (0..32u8).collect::<Vec<u8>>().into();
    let score_attendu = Value::Float64(0.75);

    let (enr_octets, _id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "payload", Value::Bytes(vec![0u8; 32].into()));
        store.set_node_property(id, "score", Value::Float64(0.25));
        let pre = store
            .capturer_precondition(id, &["payload", "score"])
            .expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[
                    ("payload", Value::Bytes(payload_attendu.clone())),
                    ("score", score_attendu.clone()),
                ],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 17,
            domaine: "base-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![
                ("payload".to_string(), Value::Bytes(payload_attendu.clone())),
                ("score".to_string(), score_attendu.clone()),
            ],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (octets, id)
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    assert_eq!(
        enr.effets_attendus[0].1,
        Value::Bytes(payload_attendu.clone())
    );
    assert_eq!(enr.effets_attendus[1].1, Value::Float64(0.75));
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&enr.reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    // L'observation canonique porte les valeurs telles quelles, sans conversion.
    assert!(matches!(
        &res.par_cle.iter().find(|e| e.cle == "payload").unwrap().etat_courant,
        EtatPropriete::Presente { valeur, .. } if valeur == &Value::Bytes(payload_attendu.clone())
    ));
    assert!(matches!(
        &res.par_cle.iter().find(|e| e.cle == "score").unwrap().etat_courant,
        EtatPropriete::Presente { valeur, .. } if valeur == &Value::Float64(0.75)
    ));
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme,
        "Int64/Float64/Bytes préservés par l'observation canonique"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Une résolution SUBSTITUÉE (clés différentes de la référence) est refusée :
/// elle ne doit pas produire sa propre preuve.
#[test]
fn composant_resolution_substituee_refusee() {
    let d = dir("subst_res");
    let chemin = d.join("kb");
    let (enr, reference) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 19,
            domaine: "base-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        (enr, reference)
    };

    // Résolution FABRIQUÉE : cible/auteur corrects, mais clé "b" au lieu de "a".
    let res = ResolutionTentative {
        cible: reference.cible(),
        auteur_resolvant: reference.auteur(),
        horloge_tentative: reference.horloge(),
        par_cle: vec![EffetResolu {
            cle: "b".to_string(),
            etat: ResolutionEffet::Engage,
            etat_courant: EtatPropriete::Presente {
                estampille: EstampilleComplete {
                    auteur: reference.auteur(),
                    horloge: reference.horloge(),
                },
                valeur: Value::Int64(10),
            },
        }],
        integrite: IntegriteTentative::Integrale,
    };
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Substitution,
        "une résolution substituée (clé différente) est refusée"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Le `domaine` de l'ENREGISTREMENT est un LABEL hors-bande du consommateur
/// (`String`), DISTINCT du `domaine_id` NATIF (u128) que la référence porte
/// depuis OBRT v2. `reconcilier` ne compare PAS ce label : deux enregistrements
/// de même écrivain/cible/référence mais de labels DIFFÉRENTS réconcilient à
/// l'IDENTIQUE. Le REFUS d'un domaine ÉTRANGER est natif, côté substrate
/// (`resoudre_reference` rend `Indeterminee` si `reference.domaine()` diffère du
/// `domaine_id` du store) — voir `composant_domaine_etranger_refuse`.
#[test]
fn composant_domaine_est_hors_bande() {
    let d = dir("domaine_hb");
    let chemin = d.join("kb");
    let (enr_octets, _id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 23,
            domaine: "base-a".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (octets, id)
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&enr.reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");

    // Même écrivain/cible/référence/effets, domaine DIFFÉRENT.
    let mut enr_b = enr.clone();
    enr_b.domaine = "base-b".to_string();

    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme
    );
    assert_eq!(
        reconcilier(&enr_b, &reference, &res),
        reconcilier(&enr, &reference, &res),
        "domaine différent → verdict IDENTIQUE : la preuve du domaine est hors-bande"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Construit une référence OBRT v2 à la main (contre-épreuves) :
/// `magic(4) | version=2 | auteur(8 LE) | horloge(8 LE) | cible(8 LE) |
/// domaine(16 BE) | n(4 LE) | [len(4 LE) | clé]*n`.
fn reference_v2(
    auteur: u64,
    horloge: u64,
    cible: NodeId,
    domaine: u128,
    cles: &[&str],
) -> obrain_substrate::ReferenceTentative {
    let mut o = Vec::new();
    o.extend_from_slice(b"OBRT");
    o.push(2u8);
    o.extend_from_slice(&auteur.to_le_bytes());
    o.extend_from_slice(&horloge.to_le_bytes());
    o.extend_from_slice(&cible.0.to_le_bytes());
    o.extend_from_slice(&domaine.to_be_bytes());
    o.extend_from_slice(&(cles.len() as u32).to_le_bytes());
    for c in cles {
        o.extend_from_slice(&(c.len() as u32).to_le_bytes());
        o.extend_from_slice(c.as_bytes());
    }
    obrain_substrate::ReferenceTentative::from_bytes(&o).expect("référence v2 bien formée")
}

/// OBRT v2 : la référence porte le `domaine_id` NATIF de la base (u128), encodé
/// dans `to_bytes`. La référence reste OPAQUE : `to_bytes`/`from_bytes` font un
/// round-trip fidèle, mais le codec ne valide que la SYNTAXE — une fabrication
/// bien formée (domaine forgé) est décodée SANS refus (l'authenticité est
/// ailleurs : la résolution sous verrou).
#[test]
fn composant_reference_obrt_v2_opaque() {
    let d = dir("obrt_v2");
    let chemin = d.join("kb");
    let store = SubstrateStore::create(&chemin).expect("création");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).expect("capture");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let reference = prep.reference_tentative();

    // v2 : la référence porte le domaine NATIF de sa base, non nul.
    assert_ne!(reference.domaine(), 0, "un domaine v2 n'est jamais nul");
    assert_eq!(
        reference.domaine(),
        store.domaine_id(),
        "domaine == domaine_id du store"
    );

    // Opaque : round-trip fidèle des octets.
    let octets = reference.to_bytes();
    assert_eq!(&octets[..4], b"OBRT", "magic OBRT");
    assert_eq!(octets[4], 2, "version v2");
    let relue = obrain_substrate::ReferenceTentative::from_bytes(&octets).expect("décodage");
    assert_eq!(relue, reference, "round-trip fidèle");

    // Le codec valide la SYNTAXE, pas l'authenticité : une fabrication bien
    // formée (domaine forgé) est décodée sans refus.
    let mut forge = octets.clone();
    // domaine aux octets 4(magic)+1(version)+8(auteur)+8(horloge)+8(cible)=29,
    // sur 16 octets big-endian : on flippe un bit du PREMIER octet de domaine.
    forge[29] ^= 1;
    let relue_forgee =
        obrain_substrate::ReferenceTentative::from_bytes(&forge).expect("décodage forgé");
    assert_ne!(
        relue_forgee.domaine(),
        reference.domaine(),
        "un domaine forgé est décodé (syntaxe) mais ne correspond plus"
    );
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

/// Une référence v1 (legacy, SANS domaine) reste décodable par `from_bytes` :
/// `domaine() == 0` (« domaine non spécifié »). Le codec v2 continue de LIRE la
/// v1, il ne la produit plus (`to_bytes` rend toujours v2).
#[test]
fn composant_reference_legacy_v1_decode() {
    // v1 : magic(4) | version=1 | auteur(8 LE) | horloge(8 LE) | cible(8 LE) |
    // n(4 LE) | [len(4 LE) | clé]*n — SANS domaine.
    let mut v1 = Vec::new();
    v1.extend_from_slice(b"OBRT");
    v1.push(1u8);
    v1.extend_from_slice(&7u64.to_le_bytes());
    v1.extend_from_slice(&42u64.to_le_bytes());
    v1.extend_from_slice(&99u64.to_le_bytes());
    v1.extend_from_slice(&1u32.to_le_bytes());
    v1.extend_from_slice(&1u32.to_le_bytes());
    v1.extend_from_slice(b"a");

    let reference = obrain_substrate::ReferenceTentative::from_bytes(&v1).expect("v1 lisible");
    assert_eq!(
        reference.domaine(),
        0,
        "v1 legacy = domaine non spécifié (0)"
    );
    assert_eq!(reference.auteur(), 7);
    assert_eq!(reference.cible().0, 99);
    assert_eq!(reference.effets(), ["a".to_string()].as_slice());

    // La v1 n'est plus PRODUITE : `to_bytes` d'une référence v1 décodée rend v2.
    let reenc = reference.to_bytes();
    assert_eq!(reenc[4], 2, "to_bytes produit v2, même pour une v1 décodée");
}

/// Domaine ÉTRANGER (refus) : une référence v2 porte le `domaine_id` de SA base.
/// Après un fork DÉCLARÉ, la référence (ancien domaine) devient étrangère à sa
/// propre base, alors même que l'effet y est engagé : la garde native rend
/// `Indeterminee` SANS consulter le contenu, et `reconcilier` la préserve en
/// `Indetermine` — un `Engage` croisé entre deux domaines est impossible.
#[test]
fn composant_domaine_etranger_refuse() {
    let d = dir("domaine_etranger");
    let chemin = d.join("kb");
    let (enr_octets, reference_bytes) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let domaine_avant = reference.domaine();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 29,
            domaine: "base-a".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        let domaine_apres = store.forker_domaine().expect("fork domaine");
        assert_ne!(domaine_apres, domaine_avant, "le fork change le domaine_id");
        store.close().expect("fermeture");
        (octets, reference.to_bytes())
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Indeterminee,
        "domaine étranger → Indeterminee (jamais un Engage croisé)"
    );
    assert!(
        res.par_cle
            .iter()
            .all(|e| e.etat == ResolutionEffet::Indetermine)
    );

    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Indetermine,
        "un domaine étranger n'est JAMAIS un Confirme"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Conservation de l'INDÉTERMINATION : une tentative préparée mais JAMAIS
/// publiée rend `Indetermine` (le backend ne connaît plus `JamaisEngage`), et
/// `reconcilier` ne la transforme JAMAIS en « absence définitive ». `Indetermine`
/// couvre « jamais engagé », « refusé » et « preuve compactée ».
#[test]
fn composant_conservation_indetermination() {
    let d = dir("indet");
    let chemin = d.join("kb");
    let (enr_octets, reference_bytes) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 31,
            domaine: "indet-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        // prep N'EST JAMAIS publiée : on l'abandonne (drop).
        drop(prep);
        store.close().expect("fermeture");
        (octets, reference.to_bytes())
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Indeterminee,
        "jamais publiée → Indeterminee (plus de JamaisEngage)"
    );
    assert!(
        res.par_cle
            .iter()
            .all(|e| e.etat == ResolutionEffet::Indetermine)
    );

    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Indetermine,
        "jamais publiée → Indetermine, jamais une absence définitive"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Obligations APRÈS INTERRUPTION : un record V1 (legacy) en quarantaine
/// conserve son OBLIGATION. Après fermeture/réouverture (interruption), la
/// référence réelle qu'il porte reste résoluble auprès du backend, et rapporte
/// l'EFFET engagé sans jamais attester l'identité logique (que le V1 ne porte
/// pas).
#[test]
fn composant_obligations_apres_interruption() {
    let d = dir("obligations");
    let chemin = d.join("kb");
    let (reference_bytes, auteur, cible) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let (auteur, cible) = (reference.auteur(), reference.cible());
        let bytes = reference.to_bytes();
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (bytes, auteur, cible)
    };

    // INTERRUPTION : le store est fermé. Le record V1 est reconstruit depuis le
    // médium (octets conservés verbatim) ; son obligation est intacte.
    let octets_record: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let legacy = RecordLegacy {
        version: ENREGISTREMENT_VERSION_LEGACY,
        octets: octets_record.clone(),
        provenance: Provenance::de(&octets_record, "<interruption>"),
        vue: Some(EnregistrementV1 {
            version: ENREGISTREMENT_VERSION_LEGACY,
            k: "op-v1".to_string(),
            backend: auteur,
            generation: 1,
            cible,
            effets_attendus: vec![("a".to_string(), 10)],
            reference_bytes: reference_bytes.clone(),
        }),
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let resolution = legacy
        .resoudre_obligation(&store2)
        .expect("obligation résoluble");
    assert_eq!(
        resolution,
        ResolutionLegacy::EffetEngageIdentiteNonAttestee,
        "après interruption, l'obligation est préservée : effet engagé, identité non attestée"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Intégrité multi-effets : l'agrégat `IntegriteTentative` est calculé PAR EFFET
/// (jamais déduit d'un seul). Une référence à deux clés dont UNE seule est
/// réellement engagée rend `Partielle` — et `reconcilier` le préserve
/// (`Partielle`), sans jamais le lire comme un `Confirme` ni une absence
/// définitive.
#[test]
fn composant_integrite_multi_effets_partielle() {
    let d = dir("multi_effets");
    let chemin = d.join("kb");
    let (enr_octets, reference_bytes) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let domaine = reference.domaine();
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        // Référence FORGÉE à DEUX clés [a, b] : seule "a" est réellement engagée
        // par cette tentative ; "b" n'a jamais été écrite par elle.
        let reference2 = reference_v2(
            reference.auteur(),
            reference.horloge(),
            reference.cible(),
            domaine,
            &["a", "b"],
        );
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 37,
            domaine: "multi".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![
                ("a".to_string(), Value::Int64(10)),
                ("b".to_string(), Value::Int64(20)),
            ],
            reference_bytes: reference2.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        store.close().expect("fermeture");
        (octets, reference2.to_bytes())
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Partielle,
        "une clé engagée sur deux → Partielle"
    );

    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Partielle,
        "l'agrégat est PAR EFFET : Partielle n'est ni Confirme ni absence définitive"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}
