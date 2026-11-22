//! Le banc de robustesse de la boucle : on met `Harness`/`Handle` en erreur.
//!
//! Chaque test reproduit une faille trouvée (panique d'un port, `restore` en plein
//! run, identifiant réutilisé, arrêt qui ne se dit pas…). Tous les `block_on` sont
//! bornés : une future qui ne se résout pas fait échouer le test, pas geler la suite.

mod commun;

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use this_agent::Stop;
use this_harness::{Event, Harness, Limits, Message, Running, Snapshot};

use commun::*;

/// FAILLE : un outil qui panique laissait un `Assistant { calls }` sans `Tool` dans le
/// fil (la réparation était sautée « parce qu'on panique ») : la conversation suivante
/// était refusée par le moteur pour de bon.
#[test]
fn un_outil_qui_panique_ne_laisse_pas_un_fil_invalide() {
    let t = transport(vec![Box::new(|_, _| Ok(call("c1")))]);
    let (h, _log) = harness(
        Limits::default(),
        t,
        Tools {
            panics: true,
            text: String::new(),
        },
    );
    let r = catch_unwind(AssertUnwindSafe(|| bounded(h.send("go"))));
    assert!(r.is_err(), "la panique de l'outil remonte");
    assert!(!h.handle().busy(), "la place est rendue");
    assert!(thread_is_valid(&h.transcript()), "{:?}", h.transcript());
}

/// FAILLE : `restore` rendait un identifiant plus petit que ceux déjà distribués : une
/// interface qui garde l'id d'un message disparu retirait un AUTRE message.
#[test]
fn restore_ne_fait_pas_reculer_les_identifiants() {
    let (h, _l) = harness(Limits::default(), transport(vec![]), ok_tools());
    let hd = h.handle();
    let ids: Vec<u64> = (0..5).map(|i| hd.enqueue(format!("m{i}"))).collect();
    let mut snap = hd.snapshot();
    snap.pending.truncate(1);
    hd.restore(snap);
    let neuf = hd.enqueue("neuf");
    assert!(neuf > *ids.last().unwrap(), "{neuf} réutilise un id passé");
    assert!(!hd.drop_pending(ids[3]) || hd.pending().is_empty());
}

/// FAILLE : `restore` pendant un run remplaçait le fil sous la boucle, qui poussait
/// ensuite des réponses d'outil orphelines. Il est refusé (et le dit) tant que ça tourne.
#[test]
fn restore_pendant_un_run_ne_corrompt_pas_le_fil() {
    let hold: Rc<RefCell<Option<Harness>>> = Rc::default();
    let h2 = hold.clone();
    let t = transport(vec![Box::new(move |_, _| {
        if let Some(h) = h2.borrow().as_ref() {
            h.restore(Snapshot::default());
        }
        Ok(call("c1"))
    })]);
    let (h, log) = harness(Limits::default(), t, ok_tools());
    *hold.borrow_mut() = Some(h.clone());
    bounded(h.send("go"));
    assert!(thread_is_valid(&h.transcript()), "{:?}", h.transcript());
    assert!(
        log.borrow()
            .iter()
            .any(|e| matches!(e, Event::Hint { text, .. } if text.contains("restore"))),
        "le refus se dit"
    );
}

/// FAILLE : `restore` acceptait un `Snapshot` de version inconnue et l'appliquait
/// sans un mot. Un format plus récent est refusé (et le dit), jamais appliqué.
#[test]
fn un_snapshot_de_version_inconnue_est_refuse_avec_un_motif() {
    let (h, log) = harness(Limits::default(), transport(vec![]), ok_tools());
    let hd = h.handle();
    hd.enqueue("avant");
    let mut futur = hd.snapshot();
    futur.version = 999;
    futur.pending.clear();
    hd.restore(futur);
    assert_eq!(
        hd.pending().len(),
        1,
        "un format inconnu ne touche pas à l'état en place"
    );
    assert_eq!(hd.pending()[0].text, "avant");
    assert!(
        log.borrow()
            .iter()
            .any(|e| matches!(e, Event::Hint { text, .. }
                if text.contains("restore") && text.contains("999"))),
        "le refus se dit et nomme la version : {:?}",
        log.borrow()
    );
}

/// FAILLE : la future lâchée pendant `Recorder::stopped` — la garde déjà désarmée —
/// Des bornes dégénérées ne bouclent pas et ne se taisent pas : un seul `Stopped`.
#[test]
fn des_bornes_degenerees_se_terminent_en_disant_leur_arret() {
    let degenerate: Vec<Limits> = vec![
        Limits {
            max_tours: 0,
            ..Limits::default()
        },
        Limits {
            budget_ms: Some(0),
            ..Limits::default()
        },
        Limits {
            silence_ms: Some(0),
            ..Limits::default()
        },
        Limits {
            no_progress_ms: Some(0),
            ..Limits::default()
        },
        Limits {
            tool_timeout_ms: Some(0),
            ..Limits::default()
        },
        Limits {
            context_budget: Some(0),
            ..Limits::default()
        },
        Limits {
            max_relances: u32::MAX,
            ..Limits::default()
        },
        Limits {
            max_tours: u32::MAX,
            max_relances: u32::MAX,
            ..Limits::default()
        },
        Limits {
            max_tours: 5,
            budget_ms: None,
            silence_ms: None,
            no_progress_ms: None,
            tool_timeout_ms: None,
            context_budget: None,
            max_relances: 0,
        },
    ];
    for (i, limits) in degenerate.into_iter().enumerate() {
        let t = transport(vec![]);
        let (h, log) = harness(limits, t, ok_tools());
        let r = catch_unwind(AssertUnwindSafe(|| bounded(h.send("go"))));
        assert!(r.is_ok(), "bornes #{i} : panique");
        assert_eq!(stops(&log), 1, "bornes #{i}");
        assert!(!h.handle().busy(), "bornes #{i}");
    }
}

/// Un transport qui répond `Ok` vide en boucle est arrêté par les tours, pas par la patience.
/// 1000 appels d'outils dans un seul tour : tous répondus, fil valide.
/// `interrupt` avant tout run, deux fois, puis un `send` : le drapeau est remis à zéro.
/// Interrompu pendant un outil, puis un nouveau `send` : il tourne vraiment.
/// Opérations sur un id absent, message vide : rien ne panique, rien ne se met en file.
#[test]
fn ids_absents_et_messages_vides() {
    let (h, _l) = harness(Limits::default(), transport(vec![]), ok_tools());
    let hd = h.handle();
    assert!(!hd.bump(99) && !hd.drop_pending(99) && !hd.edit_pending(99, "x"));
    hd.enqueue("");
    hd.enqueue("   ");
    assert!(hd.pending().is_empty());
    assert!(matches!(bounded(h.send("")), Running::Idle));
    assert!(!hd.busy());
}

/// FAILLE : `edit_pending` pouvait vider un message en file : il partait ensuite comme
/// un tour utilisateur vide (envoyé au moteur, qui le refuse).
#[test]
fn editer_un_message_en_file_en_vide_ne_l_envoie_pas_vide() {
    let (h, _l) = harness(Limits::default(), transport(vec![]), ok_tools());
    let hd = h.handle();
    let id = hd.enqueue("a");
    hd.edit_pending(id, "  ");
    bounded(h.send(""));
    assert!(
        !h.transcript()
            .iter()
            .any(|m| matches!(m, Message::User { content, .. } if content.trim().is_empty())),
        "{:?}",
        h.transcript()
    );
}

/// Un puits qui panique tue le run, pas le harnais : la place est rendue et le suivant tourne.
#[test]
fn un_puits_qui_panique_ne_bloque_pas_la_session() {
    let boom = Rc::new(Cell::new(true));
    let b = boom.clone();
    let h = Harness::new(
        "sys",
        Limits::default(),
        transport(vec![]),
        Rc::new(ok_tools()),
        move |e| {
            if matches!(e, Event::Turn { .. }) && b.replace(false) {
                panic!("puits");
            }
        },
    )
    .with_clock(Rc::new(Fake::default()));
    assert!(catch_unwind(AssertUnwindSafe(|| bounded(h.send("a")))).is_err());
    assert!(!h.handle().busy());
    assert!(matches!(bounded(h.send("b")), Running::Ran(Stop::Done)));
}

/// 10 000 messages en file puis leur vidage : doit rester linéaire en événements.
#[test]
fn dix_mille_messages_en_file() {
    let n = Rc::new(Cell::new(0usize));
    let c = n.clone();
    let h = Harness::new(
        "sys",
        Limits::default(),
        transport(vec![]),
        Rc::new(ok_tools()),
        move |e| {
            if let Event::Pending { messages } = e {
                c.set(c.get() + messages.len());
            }
        },
    )
    .with_clock(Rc::new(Fake::default()));
    let hd = h.handle();
    for i in 0..10_000 {
        hd.enqueue(format!("m{i}"));
    }
    assert!(hd.pending().len() <= 1000);
    assert!(n.get() < 2_000_000, "Pending cumulé : {}", n.get());
}

/// Un outil qui rend 10 Mo : le run se termine, l'indice de relance reste borné.
#[test]
fn une_sortie_d_outil_de_10_mo() {
    let t = transport(vec![Box::new(|_, _| Ok(call("c1")))]);
    let (h, log) = harness(
        Limits::default(),
        t,
        Tools {
            panics: false,
            text: "x".repeat(10 * 1024 * 1024),
        },
    );
    bounded(h.send("go"));
    assert_eq!(stops(&log), 1);
}

#[test]
fn un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret() {
    commun::un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret();
}

#[test]
fn un_transport_muet_en_boucle_est_borne() {
    commun::un_transport_muet_en_boucle_est_borne();
}

#[test]
fn mille_appels_dans_un_tour_restent_ranges() {
    commun::mille_appels_dans_un_tour_restent_ranges();
}

#[test]
fn interrupt_avant_le_run_n_empoisonne_pas_le_suivant() {
    commun::interrupt_avant_le_run_n_empoisonne_pas_le_suivant();
}

#[test]
fn interrupt_pendant_un_outil_puis_nouveau_send() {
    commun::interrupt_pendant_un_outil_puis_nouveau_send();
}
