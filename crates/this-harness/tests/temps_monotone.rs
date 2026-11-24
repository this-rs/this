//! #285 (case e) : le temps MONOTONE est distingué des dates civiles.
//!
//! `Clock::now_ms()` est une horloge CIVILE (epoch) : un saut NTP/DST la fait bondir ou
//! reculer. Si la comptabilité des bornes (budget, silence, plafonds) l'utilisait, un saut
//! fausserait les délais. Le harnais mesure désormais sur `Clock::now_mono_ms()`, et ces
//! tests le prouvent : une horloge dont l'heure civile bondit de 1 000 s en plein run ne
//! déclenche ni la garde de silence, ni le budget — parce que le temps MONOTONE, lui, n'a
//! avancé que de la vraie durée (100 ms).

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_trait::async_trait;

use this_harness::{
    Cancel, Clock, Delta, Failure, Finish, Harness, Limits, Outcome, Reply, Request, Running,
    Sleep, Stop, ToolSpec, Toolbox, Transport,
};

// ---- L'horloge à deux aiguilles : civile (dates) et monotone (mesures) ------------

/// Une horloge qui DISTINGUE les deux temps : `now_ms` (civile, peut sauter) et
/// `now_mono_ms` (monotone, n'avance que par `sleep`). `saut_civil` simule un saut
/// d'horloge (NTP, DST) sans toucher au temps monotone.
struct Horloge {
    civil: Cell<f64>,
    mono: Cell<f64>,
}

impl Horloge {
    fn neuve() -> Rc<Self> {
        Rc::new(Self {
            civil: Cell::new(0.0),
            mono: Cell::new(0.0),
        })
    }

    /// Un saut de l'horloge civile, sans rien changer au temps monotone.
    fn saut_civil(&self, delta_ms: f64) {
        self.civil.set(self.civil.get() + delta_ms);
    }
}

/// Cède la main une fois, puis rend : l'autre branche de la course est sondée.
struct CedeUneFois(bool);

impl Future for CedeUneFois {
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

impl Clock for Horloge {
    fn now_ms(&self) -> f64 {
        self.civil.get()
    }

    fn now_mono_ms(&self) -> f64 {
        self.mono.get()
    }

    fn sleep(&self, ms: u64) -> Option<Sleep> {
        self.mono.set(self.mono.get() + ms as f64);
        self.civil.set(self.civil.get() + ms as f64);
        Some(Box::pin(CedeUneFois(false)))
    }
}

// ---- Le moteur scripté ------------------------------------------------------------

/// Parle une fois, saute l'heure civile d'une heure, parle encore 100 ms (monotones) plus
/// tard, puis conclut. Si une borne lisait l'heure civile, le saut la ferait sonner.
struct Moteur {
    horloge: Rc<Horloge>,
}

fn reply(text: &str) -> Reply {
    Reply {
        usage: None,
        served: None,
        text: text.to_string(),
        calls: vec![],
        finish: Finish::Stop,
    }
}

#[async_trait(?Send)]
impl Transport for Moteur {
    async fn stream(
        &self,
        _request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        delta("a"); // un signe de vie à t = 0 (monotone)
        self.horloge.saut_civil(1_000_000.0); // l'heure civile bondit d'environ 16 min
        self.horloge.sleep(100).expect("minuteur").await; // 100 ms MONOTONES passent
        delta("b"); // toujours en vie
        Ok(reply("ok"))
    }
}

struct Vide;

#[async_trait(?Send)]
impl Toolbox for Vide {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![]
    }
    async fn call(&self, _: &this_harness::ToolCall) -> Outcome {
        Outcome::ok("")
    }
}

fn jouer(horloge: Rc<Horloge>, moteur: Rc<Moteur>, limits: Limits) -> Running {
    let harness =
        Harness::new("consigne", limits, moteur, Rc::new(Vide), |_| {}).with_clock(horloge);
    futures::executor::block_on(harness.send("bonjour"))
}

// ---- Un saut civil ne déclenche pas la garde de silence ----------------------------

#[test]
fn un_saut_d_horloge_civile_ne_declenche_pas_la_garde_de_silence() {
    let horloge = Horloge::neuve();
    let moteur = Rc::new(Moteur {
        horloge: horloge.clone(),
    });
    let limits = Limits {
        silence_ms: Some(1_000),
        // isoler la borne de silence : aucune autre garde ne doit intervenir
        no_progress_ms: None,
        tool_timeout_ms: None,
        budget_ms: None,
        ..Limits::default()
    };
    let fin = jouer(horloge.clone(), moteur, limits);

    // Le saut civil a bien eu lieu (le test n'est pas vide)…
    assert!(
        horloge.civil.get() >= 1_000_000.0,
        "l'heure civile aurait dû bondir : {}",
        horloge.civil.get()
    );
    // … et il n'a PAS fait sonner la garde de silence : le run va au bout.
    assert_eq!(
        fin,
        Running::Ran(Stop::Done),
        "un saut de l'heure civile ne doit pas couper un run en « silence »"
    );
}

// ---- Un saut civil n'épuise pas le budget -----------------------------------------

#[test]
fn un_saut_d_horloge_civile_n_epuise_pas_le_budget() {
    let horloge = Horloge::neuve();
    let moteur = Rc::new(Moteur {
        horloge: horloge.clone(),
    });
    let limits = Limits {
        // Assez grand pour absorber le pas de la garde (qui fait avancer l'horloge
        // factice d'une seconde au plus), assez petit pour que le saut civil (1 000 s)
        // le dépasse de loin si une borne lisait l'heure civile.
        budget_ms: Some(10_000),
        silence_ms: None,
        no_progress_ms: None,
        tool_timeout_ms: None,
        ..Limits::default()
    };
    let fin = jouer(horloge.clone(), moteur, limits);

    assert!(
        horloge.civil.get() >= 1_000_000.0,
        "l'heure civile aurait dû bondir : {}",
        horloge.civil.get()
    );
    assert_eq!(
        fin,
        Running::Ran(Stop::Done),
        "un saut de l'heure civile ne doit pas épuiser le budget du run"
    );
}
