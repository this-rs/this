//! H3 V2 — chaîne d'autorité réelle : la réservation PRÉCÈDE la publication,
//! et le consommateur tient UN enregistrement original qui lie K, l'autorité
//! backend, la génération, la cible, les effets attendus (contenu) et les octets
//! de la référence. La réconciliation compare à CET enregistrement, jamais au
//! seul résultat de `resoudre_reference`.
//!
//! Ce que `resoudre_reference` ne peut PAS faire seul (contre-épreuves du
//! validateur, note 52f8dac1) :
//!   - L1 : `Integrale` est calculé sur la liste DE la référence → une référence
//!     amputée d'une clé rend `Integrale` sur une tentative incomplète.
//!   - L2 : une substitution par une AUTRE référence authentique rend
//!     `Integrale`/`Engage` ; seule la comparaison à l'enregistrement la détecte.
//! La chaîne d'autorité ci-dessous les détecte toutes deux.

#![cfg(feature = "obrain")]

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    IntegriteTentative, ModePublication, Precondition, ReferenceTentative, SubstrateStore,
};
use this::reconciliation::{
    ENREGISTREMENT_VERSION, EnregistrementOriginal, VerdictReconciliation as VerdictChaine,
    reconcilier as reconcilier_chaine,
};
use this_harness::attribution::{Attribution, Definition, Observation, Resultat, Tentative};

fn dir(tag: &str) -> std::path::PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "h3v2chain_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    d
}

/// Cycle ordonné : `record` → `recevoir` → **`reserver` (génération) AVANT
/// publication** → préparation → conservation → publication → perte → résolution
/// → réconciliation contre l'enregistrement.
#[test]
fn h3_v2_chaine_nominale_reserver_avant_publication() {
    let d = dir("nominal");
    let chemin = d.join("kb");

    let mut attr = Attribution::new();
    let decision_id = 42u128;
    attr.record(
        decision_id,
        Definition {
            identite: Some("k1".to_string()),
            prevue: Some(("a".to_string(), "m1".to_string())),
            servie: Some(("a".to_string(), "m1".to_string())),
        },
    );
    attr.recevoir(
        decision_id,
        Observation {
            fournisseur: "a".to_string(),
            modele: "m1".to_string(),
            resultat: Resultat::Succes,
        },
    );
    // RÉSERVATION AVANT PUBLICATION : la génération est assignée ICI.
    let tentative: Tentative = attr.reserver(decision_id).expect("réservation");

    // Préparation + conservation + publication (couche durable).
    let enr = {
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
        let reference_bytes = reference.to_bytes();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 1,
            backend: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0, // première génération de cette décision
            cible: id,
            effets_attendus: vec![("a".to_string(), 10)],
            reference_bytes,
        };
        let _verdict = store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        enr
    };

    // Résolution après réouverture.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("décodage");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        reconcilier_chaine(&enr, &reference, &res, &store2),
        VerdictChaine::Confirme,
        "l'enregistrement original est confirmé (intégral + canonique + contenu conforme)"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
    let _ = (attr, tentative, decision_id); // la réconciliation alimenterait attr.resoudre ici
}

/// L1 : une référence AMPUTÉE d'une clé (tentative a,b, référence a seul) rendrait
/// `Integrale` au stockage. La chaîne la détecte comme `Incomplet`.
#[test]
fn h3_v2_chaine_detecte_cle_amputee() {
    let d = dir("amputee");
    let chemin = d.join("kb");
    let enr = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        store.set_node_property(id, "b", Value::Int64(2));
        let pre = store
            .capturer_precondition(id, &["a", "b"])
            .expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10)), ("b", Value::Int64(20))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 1,
            backend: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), 10), ("b".to_string(), 20)],
            reference_bytes: reference.to_bytes(),
        };
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        enr
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    // Forger une référence AMPUTÉE : on re-encode avec un seul effet.
    // On repart des octets originaux, on les décode, on reconstruit une référence
    // à [a] seul en réutilisant le codec public (auteur/horloge/cible lisibles).
    let mut ampute = enr.reference_bytes.clone();
    // En-tête = magic(4) | version(1) | auteur(8) | horloge(8) | cible(8) | n_effets(4).
    // n_effets (u32 LE) est à l'offset 29..33 ; on le force à 1.
    ampute[29..33].copy_from_slice(&1u32.to_le_bytes());
    // On tronque au premier effet : en-tête 33 + len(4) + « a »(1) = 38 octets.
    ampute.truncate(38);
    let reference = ReferenceTentative::from_bytes(&ampute).expect("référence amputée bien formée");

    // Le stockage la juge Intégrale (L1)…
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Integrale,
        "L1 reproduite : amputée → Integrale"
    );
    // … mais la chaîne d'autorité la refuse.
    assert_eq!(
        reconcilier_chaine(&enr, &reference, &res, &store2),
        VerdictChaine::Incomplet,
        "la chaîne détecte la clé amputée, le stockage ne le peut pas"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// L2 : une substitution par une AUTRE référence authentique rend Integrale au
/// stockage. La chaîne la détecte comme `Substitution`.
#[test]
fn h3_v2_chaine_detecte_substitution() {
    let d = dir("substitution");
    let chemin = d.join("kb");
    // Tentative 1 : écrit a=10 (l'enregistrement original).
    let (enr, id) = {
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
            k: 1,
            backend: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), 10)],
            reference_bytes: reference.to_bytes(),
        };
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication 1");
        store.close().expect("fermeture 1");
        (enr, id)
    };

    // Tentative 2 : écrit a=99 (référence authentique, autre horloge).
    let autre_reference_bytes = {
        let store = SubstrateStore::open(&chemin).expect("réouverture");
        let pre = store.capturer_precondition(id, &["a"]).expect("capture 2");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(99))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation 2");
        let bytes = prep.reference_tentative().to_bytes();
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication 2");
        store.close().expect("fermeture 2");
        bytes
    };

    let store3 = SubstrateStore::open(&chemin).expect("réouverture 3");
    let reference_substituee =
        ReferenceTentative::from_bytes(&autre_reference_bytes).expect("décodage");
    let res = store3
        .resoudre_reference(&reference_substituee)
        .expect("résolution");
    // Le stockage juge la référence substituée valide (L2)…
    assert_eq!(
        res.integrite,
        IntegriteTentative::Integrale,
        "L2 reproduite : substituée → Integrale"
    );
    // … mais la chaîne la refuse car elle n'est PAS celle de l'enregistrement.
    assert_eq!(
        reconcilier_chaine(&enr, &reference_substituee, &res, &store3),
        VerdictChaine::Substitution,
        "la chaîne détecte la substitution d'une référence authentique"
    );
    store3.close().expect("fermeture 3");
    let _ = std::fs::remove_dir_all(&d);
}

/// Contenu incompatible : engagé puis masqué par une valeur DIFFÉRENTE → la
/// chaîne distingue l'engagement historique de la compatibilité du contenu.
#[test]
fn h3_v2_chaine_contenu_incompatible() {
    let d = dir("contenu");
    let chemin = d.join("kb");
    let (enr, id) = {
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
            k: 1,
            backend: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), 10)],
            reference_bytes: reference.to_bytes(),
        };
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (enr, id)
    };

    // Un écrivain ultérieur masque avec une valeur DIFFÉRENTE (99).
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    store2.set_node_property(id, "a", Value::Int64(99));
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("décodage");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Integrale,
        "engagé (historique) intégral"
    );
    assert_eq!(
        reconcilier_chaine(&enr, &reference, &res, &store2),
        VerdictChaine::Incompatible,
        "engagé puis masqué par une valeur différente → contenu incompatible"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// « Un historique intégral masqué n'est pas nécessairement incompatible » :
/// un masque qui réécrit la MÊME valeur est `EngageMasqueCompatible`, pas `Incompatible`.
#[test]
fn h3_v2_chaine_masque_compatible_n_est_pas_incompatible() {
    let d = dir("masque_compatible");
    let chemin = d.join("kb");
    let (enr, id) = {
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
            k: 1,
            backend: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), 10)],
            reference_bytes: reference.to_bytes(),
        };
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (enr, id)
    };

    // Masque avec la MÊME valeur (10) : estampille change, contenu conforme.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    store2.set_node_property(id, "a", Value::Int64(10));
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("décodage");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        reconcilier_chaine(&enr, &reference, &res, &store2),
        VerdictChaine::EngageMasqueCompatible,
        "masqué mais contenu conforme : engagé, pas incompatible"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Témoin V1 restauré : deux tentatives indépendantes de contenu identique
/// portent des références physiques DISTINCTES (pas d'idempotence par contenu).
#[test]
fn h3_v2_deux_tentatives_references_distinctes() {
    let d = dir("deuxref");
    let chemin = d.join("kb");
    let store = SubstrateStore::create(&chemin).expect("création");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).expect("capture");
    let prep1 = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation 1");
    let r1 = prep1.reference_tentative();
    store
        .publier_si_precondition_intacte(prep1)
        .expect("publication 1");

    let pre2 = store.capturer_precondition(id, &["a"]).expect("capture 2");
    let prep2 = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(20))],
            &pre2,
            ModePublication::Strict,
        )
        .expect("préparation 2");
    let r2 = prep2.reference_tentative();
    assert_ne!(
        (r1.auteur(), r1.horloge()),
        (r2.auteur(), r2.horloge()),
        "deux tentatives = deux références physiques distinctes (pas d'idempotence par contenu)"
    );
    assert_ne!(r1.to_bytes(), r2.to_bytes());
    store
        .publier_si_precondition_intacte(prep2)
        .expect("publication 2");
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

/// Durabilité : l'enregistrement original est reconstruit depuis un MÉDIUM durable
/// (fichiers), après disparition du consommateur. Une variable RAM d'octets
/// sérialisés ne suffit pas : on vérifie la reconstruction effective de K, génération,
/// autorité (backend), cible, référence et contenu attendu, puis la réconciliation.
#[test]
fn h3_v2_enregistrement_reconstruit_apres_disparition() {
    let d = dir("durable");
    let chemin = d.join("kb");
    let rec_file = d.join("enregistrement.txt");
    let ref_file = d.join("reference.bin");

    // 1. Décision → préparation → conservation → PERSISTANCE sur fichier (pas RAM).
    {
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
        let (backend, cible, horloge) =
            (reference.auteur(), reference.cible().0, reference.horloge());
        // Référence (octets opaques) sur un fichier binaire.
        std::fs::write(&ref_file, reference.to_bytes()).expect("persistance référence");
        // Métadonnées (K, backend, génération, cible, effets attendus) sur un fichier texte.
        let meta = format!(
            "k=1\nbackend={backend}\ngeneration=1\ncible={cible}\nhorloge={horloge}\neffets=a:10\n"
        );
        std::fs::write(&rec_file, &meta).expect("persistance enregistrement");
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
    }
    // 2. DISPARITION du consommateur : tout état RAM est hors de portée.

    // 3. RECONSTRUCTION depuis le médium durable.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference_bytes = std::fs::read(&ref_file).expect("lecture référence");
    let meta = std::fs::read_to_string(&rec_file).expect("lecture enregistrement");
    let champ = |nom: &str| -> String {
        meta.lines()
            .find_map(|l| l.strip_prefix(&format!("{nom}=")).map(str::to_string))
            .unwrap_or_else(|| panic!("champ {nom} absent de l'enregistrement reconstruit"))
    };
    // Vérifier chaque champ reconstruit contre la décision originale.
    assert_eq!(champ("k"), "1", "K reconstruit");
    assert_eq!(champ("generation"), "1", "génération reconstruite");
    assert_eq!(champ("effets"), "a:10", "contenu attendu reconstruit");

    let reference = ReferenceTentative::from_bytes(&reference_bytes).expect("décodage");
    // Autorité backend + cible reconstruits doivent égaler ceux de la référence (cohérence durable).
    assert_eq!(
        reference.auteur().to_string(),
        champ("backend"),
        "autorité backend reconstruite"
    );
    assert_eq!(
        reference.cible().0.to_string(),
        champ("cible"),
        "cible reconstruite"
    );

    // 4. Résolution + réconciliation contre l'enregistrement RECONSTRUIT.
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: champ("k").parse().expect("k u128"),
        backend: champ("backend").parse().expect("backend u64"),
        generation: champ("generation").parse().expect("generation u64"),
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(champ("cible").parse().expect("cible u64")),
        effets_attendus: vec![("a".to_string(), 10)],
        reference_bytes: reference.to_bytes(),
    };
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        reconcilier_chaine(&enr, &reference, &res, &store2),
        VerdictChaine::Confirme,
        "l'enregistrement reconstruit après disparition confirme la décision originale"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}
