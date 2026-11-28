//! Le préalable de la tranche mémoire, mesuré aux DEUX niveaux d'API distincts.
//!
//! Le verdict d'un `close()` n'est pas le même selon le niveau :
//!
//! - **`SubstrateStore` (API brute)** : `open` n'a PAS de garde « une base
//!   inscriptible par processus » — c'est l'ouvreur `ObrainDB` qui la porte.
//!   `close()` rend `flush()` puis marque `ferme` si le flush a réussi : son
//!   `Ok` EST donc un signal de flush réussi (en mode écriture).
//! - **`ObrainDB` (le chemin réel de l'Atelier, via `ObrainStore`)** : la garde
//!   introduite par `dcba1fab` vit dans `open_substrate_inner` (registre des
//!   substrats vivants) — une seconde ouverture en ÉCRITURE du même chemin dans
//!   le même processus est REFUSÉE tant qu'une instance vit. `close()`
//!   désinscrit la base du registre (réouverture permise), PUIS avale l'erreur
//!   de `sub.flush()` (`tracing::error!` puis continue) : son `Ok(())` ne
//!   prouve donc PAS la réussite du flush.
//!
//! Ce témoin mesure les trois cas que l'adaptateur doit connaître, au niveau
//! `ObrainDB`, avec des assertions — pas des `println!`.
#![cfg(feature = "obrain")]

use std::path::PathBuf;

use obrain_engine::ObrainDB;

fn repertoire() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "this-substrate-vie-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

#[test]
fn une_seconde_ouverture_en_ecriture_est_refusee_tant_que_l_instance_vit() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire de test");

    let _db1 = ObrainDB::open(&chemin).expect("première ouverture");

    // La garde dcba1fab : une base, une instance inscriptible par processus.
    let seconde = ObrainDB::open(&chemin);
    assert!(
        seconde.is_err(),
        "une seconde ouverture en écriture du même chemin doit être refusée"
    );

    let _ = std::fs::remove_dir_all(&chemin);
}

#[test]
fn apres_close_la_reouverture_est_permise_meme_avec_un_ancien_handle_conserve() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire de test");

    let db1 = ObrainDB::open(&chemin).expect("première ouverture");
    // `db1` est conservé (l'ancien handle reste en portée) : on ferme puis on
    // rouvre. La désinscription du registre, pas le drop, est ce qui libère.
    db1.close().expect("close");

    let rouvert = ObrainDB::open(&chemin);
    assert!(
        rouvert.is_ok(),
        "la réouverture après close doit être permise même avec un ancien handle conservé"
    );

    let _ = std::fs::remove_dir_all(&chemin);
}

#[test]
fn commit_wal_est_accessible_et_rend_ok_sur_un_store_sain() {
    // Ce test ne provoque AUCUNE erreur : il vérifie que `commit_wal()` est
    // accessible depuis le consommateur et rend Ok sur un store sain.
    //
    // Le défaut de propagation de flush de `ObrainDB::close()` (il AVALE
    // l'erreur de `sub.flush()` et rend `Ok(())`) reste un constat
    // d'INSPECTION (`database/mod.rs`, `close`), pas une mesure : le reproduire
    // exigerait une injection de panne de flush, qui n'existe pas. La garantie
    // de durabilité s'obtient par `commit_wal()`, qui rend l'erreur de flush.
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire de test");
    let db = ObrainDB::open(&chemin).expect("ouverture");
    // `commit_wal` rend l'erreur de flush (contrat lu dans `database/mod.rs`).
    // On l'exerce pour établir qu'il EST accessible et rend Ok sur un store sain.
    db.commit_wal()
        .expect("commit_wal sur un store sain doit réussir");
    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&chemin);
}
