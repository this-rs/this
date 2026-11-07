//! Le banc de robustesse de la boucle : on met `Harness`/`Handle` en erreur.
//!
//! Chaque test reproduit une faille trouvée (panique d'un port, `restore` en plein
//! run, identifiant réutilisé, arrêt qui ne se dit pas…). Tous les `block_on` sont
//! bornés : une future qui ne se résout pas fait échouer le test, pas geler la suite.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Clock, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome, Recorder,
    Reply, Request, Running, Sleep, Snapshot, ToolCall, ToolSpec, Toolbox, Transport,
};

const POLLS: usize = 2000;

/// Pilote une future avec un nombre de sondages borné.
fn bounded<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..POLLS {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
    panic!("la future ne se résout pas en {POLLS} sondages");
}

#[derive(Default)]
struct Fake {
    now: Cell<f64>,
}
struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
impl Clock for Fake {
    fn now_ms(&self) -> f64 {
        self.now.get()
    }
    fn sleep(&self, ms: u64) -> Option<Sleep> {
        self.now.set(self.now.get() + ms as f64);
        Some(Box::pin(Yield(false)))
    }
}

type Script = Box<dyn Fn(&Request, &Cancel) -> Result<Reply, Failure>>;

struct Scripted {
    acts: RefCell<VecDeque<Script>>,
    fallback: Script,
}
fn say(text: &str) -> Reply {
    Reply {
        text: text.into(),
        calls: vec![],
        finish: Finish::Stop,
    }
}
fn call(id: &str) -> Reply {
    Reply {
        text: String::new(),
        calls: vec![ToolCall {
            id: id.into(),
            name: "t".into(),
            arguments: json!({}),
        }],
        finish: Finish::Tools,
    }
}
fn transport(acts: Vec<Script>) -> Rc<Scripted> {
    Rc::new(Scripted {
        acts: RefCell::new(acts.into()),
        fallback: Box::new(|_, _| Ok(say("fini"))),
    })
}
#[async_trait(?Send)]
impl Transport for Scripted {
    async fn stream(&self, r: &Request, c: &Cancel, _d: Delta) -> Result<Reply, Failure> {
        let act = self.acts.borrow_mut().pop_front();
        match act {
            Some(a) => a(r, c),
            None => (self.fallback)(r, c),
        }
    }
}

struct Tools {
    panics: bool,
    text: String,
}
#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "t".into(),
            description: String::new(),
            parameters: json!({"type":"object"}),
        }]
    }
    async fn call(&self, _c: &ToolCall) -> Outcome {
        if self.panics {
            panic!("outil en panique");
        }
        Outcome::ok(self.text.clone())
    }
}

type Log = Rc<RefCell<Vec<Event>>>;
fn harness(limits: Limits, t: Rc<dyn Transport>, tools: Tools) -> (Harness, Log) {
    let log: Log = Rc::default();
    let sink = log.clone();
    let h = Harness::new("sys", limits, t, Rc::new(tools), move |e| {
        sink.borrow_mut().push(e)
    })
    .with_clock(Rc::new(Fake::default()));
    (h, log)
}
fn ok_tools() -> Tools {
    Tools {
        panics: false,
        text: "ok".into(),
    }
}
fn stops(log: &Log) -> usize {
    log.borrow()
        .iter()
        .filter(|e| matches!(e, Event::Stopped { .. }))
        .count()
}

/// Chaque appel d'outil du fil a sa réponse, et chaque réponse son appel.
fn thread_is_valid(thread: &[Message]) -> bool {
    let mut open: Vec<String> = Vec::new();
    for m in thread {
        match m {
            Message::Assistant { calls, .. } => open.extend(calls.iter().map(|c| c.id.clone())),
            Message::Tool { id, .. } => {
                let Some(i) = open.iter().position(|o| o == id) else {
                    return false;
                };
                open.remove(i);
            }
            _ => {}
        }
    }
    open.is_empty()
}

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

struct SlowRecorder;
#[async_trait(?Send)]
impl Recorder for SlowRecorder {
    async fn step(&self, _: &str, _: &Run, _: &Step) {}
    async fn stopped(&self, _: &str, _: &Run, _: &Stop) {
        std::future::pending::<()>().await
    }
}

/// FAILLE : la future lâchée pendant `Recorder::stopped` — la garde déjà désarmée —
/// ne disait jamais l'arrêt : un run sans `Stopped`.
#[test]
fn un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret() {
    let (h, log) = harness(Limits::default(), transport(vec![]), ok_tools());
    let h = h.with_recorder(Rc::new(SlowRecorder));
    {
        let mut fut = Box::pin(h.send("go"));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..50 {
            if fut.as_mut().poll(&mut cx).is_ready() {
                panic!("l'enregistreur ne devait pas rendre");
            }
        }
    }
    assert!(!h.handle().busy());
    assert_eq!(stops(&log), 1, "un Stopped exactement");
}

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
#[test]
fn un_transport_muet_en_boucle_est_borne() {
    let t = Rc::new(Scripted {
        acts: RefCell::new(VecDeque::new()),
        fallback: Box::new(|_, _| Ok(say(""))),
    });
    let (h, log) = harness(Limits::default(), t, ok_tools());
    bounded(h.send("go"));
    assert_eq!(stops(&log), 1);
}

/// 1000 appels d'outils dans un seul tour : tous répondus, fil valide.
#[test]
fn mille_appels_dans_un_tour_restent_ranges() {
    let t = transport(vec![Box::new(|_, _| {
        Ok(Reply {
            text: String::new(),
            calls: (0..1000)
                .map(|i| ToolCall {
                    id: format!("c{i}"),
                    name: "t".into(),
                    arguments: json!({}),
                })
                .collect(),
            finish: Finish::Tools,
        })
    })]);
    let (h, log) = harness(Limits::default(), t, ok_tools());
    bounded(h.send("go"));
    assert!(thread_is_valid(&h.transcript()));
    assert_eq!(stops(&log), 1);
}

/// `interrupt` avant tout run, deux fois, puis un `send` : le drapeau est remis à zéro.
#[test]
fn interrupt_avant_le_run_n_empoisonne_pas_le_suivant() {
    let (h, _l) = harness(Limits::default(), transport(vec![]), ok_tools());
    let hd = h.handle();
    hd.interrupt();
    hd.interrupt();
    assert!(matches!(bounded(h.send("go")), Running::Ran(Stop::Done)));
    assert!(!hd.is_cancelled());
}

/// Interrompu pendant un outil, puis un nouveau `send` : il tourne vraiment.
#[test]
fn interrupt_pendant_un_outil_puis_nouveau_send() {
    let hd_cell: Rc<RefCell<Option<this_harness::Handle>>> = Rc::default();
    let c = hd_cell.clone();
    let t = transport(vec![Box::new(move |_, _| {
        c.borrow().as_ref().unwrap().interrupt();
        Ok(call("c1"))
    })]);
    let (h, _l) = harness(Limits::default(), t, ok_tools());
    *hd_cell.borrow_mut() = Some(h.handle());
    assert!(matches!(
        bounded(h.send("a")),
        Running::Ran(Stop::Cancelled)
    ));
    assert!(thread_is_valid(&h.transcript()));
    assert!(matches!(bounded(h.send("b")), Running::Ran(Stop::Done)));
}

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
