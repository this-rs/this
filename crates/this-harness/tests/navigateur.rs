//! La suite partagée (`tests/commun`) exécutée DANS un navigateur (wasm32).
//!
//! Exécuter via le runner wasm-bindgen-test comme décrit dans le job `wasm32_browser`.
//! Natif : ce fichier est vide
//! (les mêmes scénarios tournent via `tests/robustesse.rs`).
#![cfg(target_arch = "wasm32")]

mod commun;

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn transport_muet() {
    commun::un_transport_muet_en_boucle_est_borne();
}

#[wasm_bindgen_test]
fn mille_appels_dans_un_tour() {
    commun::mille_appels_dans_un_tour_restent_ranges();
}

#[wasm_bindgen_test]
fn interrupt_avant_le_run() {
    commun::interrupt_avant_le_run_n_empoisonne_pas_le_suivant();
}

#[wasm_bindgen_test]
fn interrupt_pendant_un_outil_puis_nouveau_send() {
    commun::interrupt_pendant_un_outil_puis_nouveau_send();
}

#[wasm_bindgen_test]
fn run_lache_pendant_l_enregistreur() {
    commun::un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret();
}
