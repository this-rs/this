//! Le profil d'exécution d'un transport : le défaut ne promet rien, et une déclaration
//! explicite est honorée ou refusée nommément (#285). Mesuré natif ; le profil `wasm` est
//! testé sur sa valeur déclarée, pas sur une exécution wasm (voir la note en fin de fichier).

use async_trait::async_trait;
use this_harness::{
    Besoin, Cancel, Delta, Failure, Isolation, ProfilRuntime, Refus, Reply, Request, Transport,
};

/// Un transport minimal : il ne déclare rien, il prend le défaut.
struct Minimal;

#[async_trait(?Send)]
impl Transport for Minimal {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        unreachable!("le profil se lit sans tour")
    }
}

/// Un transport qui déclare le profil d'une coquille native complète.
struct Natif;

#[async_trait(?Send)]
impl Transport for Natif {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        unreachable!("le profil se lit sans tour")
    }
    fn profil_runtime(&self) -> ProfilRuntime {
        ProfilRuntime::natif()
    }
}

/// Un transport qui déclare une page wasm sans minuteur.
struct Wasm;

#[async_trait(?Send)]
impl Transport for Wasm {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        unreachable!("le profil se lit sans tour")
    }
    fn profil_runtime(&self) -> ProfilRuntime {
        ProfilRuntime::wasm_sans_minuteur()
    }
}

#[test]
fn un_transport_minimal_ne_promet_rien_par_defaut() {
    let p = Minimal.profil_runtime();
    assert_eq!(p, ProfilRuntime::minimal());
    assert_eq!(p.exiger(Besoin::Delai), Err(Refus::SansMinuteur));
    assert_eq!(p.exiger(Besoin::Annulation), Err(Refus::SansAnnulation));
    assert_eq!(
        p.exiger(Besoin::IsolationDure),
        Err(Refus::IsolationInsuffisante(Isolation::Aucune))
    );
    assert_eq!(p.exiger(Besoin::Supervision), Err(Refus::SansSupervision));
}

#[test]
fn une_coquille_native_declaree_honore_tout() {
    let p = Natif.profil_runtime();
    for b in [
        Besoin::Delai,
        Besoin::Annulation,
        Besoin::IsolationDure,
        Besoin::Supervision,
    ] {
        assert_eq!(p.exiger(b), Ok(()), "{b:?}");
    }
}

#[test]
fn une_page_wasm_declaree_refuse_le_delai_et_l_isolation_dure() {
    let p = Wasm.profil_runtime();
    assert_eq!(p.exiger(Besoin::Delai), Err(Refus::SansMinuteur));
    assert_eq!(p.exiger(Besoin::Annulation), Ok(()));
    assert_eq!(
        p.exiger(Besoin::IsolationDure),
        Err(Refus::IsolationInsuffisante(Isolation::CatchUnwind))
    );
}

// Note : ces trois tests s'exécutent sur la cible native. Le profil wasm est vérifié ici sur sa
// VALEUR déclarée ; qu'une page wasm tienne réellement ce profil n'est pas exécuté par cette suite.
