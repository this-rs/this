//! Les scénarios qui comptent, sans réseau et sans horloge.
//!
//! Chaque test est une histoire courte : ce que le modèle fait, ce que
//! l'utilisateur fait pendant ce temps, et ce que le harnais en conclut.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Decision, Policy, Run as AgentRun, Stop};
use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Handle, Harness, Limits, Message, Outcome, Pending,
    Reply, Request, Running, Snapshot, ToolCall, ToolSpec, Toolbox, Toolboxes, Transport,
};

// ---- Les doublures -----------------------------------------------------------

/// Un moteur qui récite un script et note ce qu'on lui a envoyé.
struct Script {
    replies: RefCell<VecDeque<Reply>>,
    seen: RefCell<Vec<Request>>,
}

impl Script {
    fn new(replies: Vec<Reply>) -> Rc<Self> {
        Rc::new(Self {
            replies: RefCell::new(replies.into()),
            seen: RefCell::new(Vec::new()),
        })
    }
}

#[async_trait(?Send)]
impl Transport for Script {
    async fn stream(
        &self,
        request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        self.seen.borrow_mut().push(request.clone());
        let reply = self.replies.borrow_mut().pop_front().unwrap_or(Reply {
            text: "fini".to_string(),
            calls: vec![],
            finish: Finish::Stop,
        });
        for morceau in reply.text.split_inclusive(' ') {
            delta(morceau);
        }
        Ok(reply)
    }
}

/// Un noyau d'outils scriptable.
struct Tools {
    used: RefCell<Vec<String>>,
    respond: Box<dyn Fn(&ToolCall) -> Outcome>,
    specs: Vec<ToolSpec>,
}

impl Tools {
    fn new(respond: impl Fn(&ToolCall) -> Outcome + 'static) -> Rc<Self> {
        Rc::new(Self {
            used: RefCell::new(Vec::new()),
            respond: Box::new(respond),
            specs: vec![ToolSpec {
                name: "install_module".to_string(),
                description: "installe un module".to_string(),
                parameters: json!({"type": "object"}),
            }],
        })
    }
}

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.used.borrow_mut().push(call.name.clone());
        (self.respond)(call)
    }
}

type Log = Rc<RefCell<Vec<Event>>>;

fn build(transport: Rc<Script>, toolbox: Rc<Tools>, limits: Limits) -> (Harness, Handle, Log) {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let seen = log.clone();
    let harness = Harness::new(
        "tu es l'atelier",
        limits,
        transport,
        toolbox,
        move |event| seen.borrow_mut().push(event),
    );
    let handle = harness.handle();
    (harness, handle, log)
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: "install_module".to_string(),
        arguments: json!({}),
    }
}

fn names(log: &Log) -> Vec<&'static str> {
    log.borrow().iter().map(Event::name).collect()
}

fn stop_of(log: &Log) -> Stop {
    log.borrow()
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::Stopped { stop, .. } => Some(stop.clone()),
            _ => None,
        })
        .expect("tout run se termine par un Stopped")
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

// ---- Les scénarios -----------------------------------------------------------

#[test]
fn un_outil_puis_une_conclusion() {
    let transport = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: "c'est installé".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
    ]);
    let tools = Tools::new(|_| Outcome::ok("module installé").observed("v1", "v2"));
    let (harness, _, log) = build(transport, tools, Limits::default());

    assert_eq!(run(harness.send("installe")), Running::Ran(Stop::Done));
    assert_eq!(stop_of(&log), Stop::Done);

    let events = names(&log);
    assert_eq!(events.first(), Some(&"pending"));
    assert!(events.contains(&"tool_use") && events.contains(&"tool_result"));
    assert_eq!(events.last(), Some(&"stopped"));

    // Le résultat de l'outil est bien revenu au modèle, avec son identifiant.
    let transcript = harness.transcript();
    assert!(matches!(&transcript[1], Message::Assistant { calls, .. } if calls.len() == 1));
    assert!(matches!(&transcript[2], Message::Tool { id, .. } if id == "a1"));
}

#[test]
fn un_message_tenu_part_au_tour_suivant() {
    // L'utilisateur écrit PENDANT que l'outil tourne.
    let transport = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: "terminé".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
        Reply {
            text: "et voilà la suite".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
    ]);
    let holder: Rc<RefCell<Option<Handle>>> = Rc::new(RefCell::new(None));
    let typed = holder.clone();
    let tools = Tools::new(move |_| {
        // Le geste exact de l'interface : on met en file, on n'attend rien.
        if let Some(handle) = typed.borrow().as_ref() {
            handle.enqueue("ajoute aussi une alerte");
        }
        Outcome::ok("ok").observed("v1", "v2")
    });
    let (harness, handle, log) = build(transport, tools, Limits::default());
    *holder.borrow_mut() = Some(handle.clone());

    assert_eq!(run(harness.send("installe")), Running::Ran(Stop::Done));

    // Le message tenu a ouvert un second objectif, sans bloquer le premier.
    let events = log.borrow();
    let users: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::User { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, vec!["installe", "ajoute aussi une alerte"]);
    drop(events);

    // Le moteur a bien reçu le message au tour suivant, pas avant.
    let requests = harness.transcript();
    assert!(
        requests
            .iter()
            .any(|m| m.text().contains("ajoute aussi une alerte"))
    );
    assert_eq!(handle.pending().len(), 0, "un message livré quitte la file");
}

#[test]
fn interrupt_arrete_le_run_et_le_dit() {
    let transport = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: "je n'aurais pas dû parler".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
    ]);
    let holder: Rc<RefCell<Option<Handle>>> = Rc::new(RefCell::new(None));
    let typed = holder.clone();
    let tools = Tools::new(move |_| {
        if let Some(handle) = typed.borrow().as_ref() {
            handle.interrupt();
        }
        Outcome::ok("ok")
    });
    let (harness, handle, log) = build(transport, tools, Limits::default());
    *holder.borrow_mut() = Some(handle.clone());

    assert_eq!(run(harness.send("boucle")), Running::Ran(Stop::Cancelled));
    assert_eq!(stop_of(&log), Stop::Cancelled);
    // Le second tour n'a jamais eu lieu : l'annulation est lue entre deux outils.
    assert_eq!(handle.generation(), 1);
}

#[test]
fn un_refus_relance_avec_le_motif() {
    let transport = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: "je m'arrête là".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
    ]);
    let tools = Tools::new(|_| Outcome::refused("refusé : même version", "même version"));
    let (harness, _, log) = build(transport, tools, Limits::default());

    let stop = match run(harness.send("installe")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    // Le refus relance, puis le modèle conclut : le run finit « terminé »,
    // mais le motif a été dit au modèle — c'est ce qui l'a fait changer.
    assert_eq!(stop, Stop::Done);
    let hints: Vec<String> = log
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Hint { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(hints.len(), 1);
    assert!(
        hints[0].contains("même version"),
        "le motif doit être relu : {hints:?}"
    );
}

/// Un outil qui PLANTE n'est ni un succès, ni un refus : le harnais doit le voir, le dire au
/// modèle avec la cause, et l'événement doit le porter à l'interface.
#[test]
fn un_echec_d_outil_relance_avec_la_cause() {
    let transport = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: "je m'arrête là".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
    ]);
    let tools = Tools::new(|_| Outcome::failed("échec : module introuvable", "module introuvable"));
    let (harness, _, log) = build(transport, tools, Limits::default());

    let stop = match run(harness.send("installe")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    assert_eq!(stop, Stop::Done);

    let hints: Vec<String> = log
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Hint { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(hints.len(), 1, "un échec relance : {hints:?}");
    assert!(
        hints[0].contains("module introuvable"),
        "la cause doit être relue : {hints:?}"
    );
    assert!(
        hints[0].contains("échoué"),
        "et dite comme un échec, pas comme un refus : {hints:?}"
    );

    let result = log
        .borrow()
        .iter()
        .find_map(|event| match event {
            Event::ToolResult {
                failed, refused, ..
            } => Some((*failed, *refused)),
            _ => None,
        })
        .expect("un ToolResult");
    assert_eq!(
        result,
        (true, false),
        "l'interface voit un échec, pas un refus"
    );
}

#[test]
fn une_reponse_tronquee_n_est_pas_une_conclusion() {
    let transport = Script::new(vec![Reply {
        text: "voilà ce que j'ai écrit".into(),
        calls: vec![],
        finish: Finish::Length,
    }]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (harness, _, log) = build(transport, tools, Limits::default());

    match run(harness.send("parle")) {
        Running::Ran(Stop::Failed(motif)) => assert!(motif.contains("tronquée"), "{motif}"),
        autre => panic!("une réponse coupée ne doit pas passer pour finie : {autre:?}"),
    }
    assert!(matches!(stop_of(&log), Stop::Failed(_)));
}

#[test]
fn un_tour_muet_relance_puis_echoue_en_le_disant() {
    let vide = Reply {
        text: String::new(),
        calls: vec![],
        finish: Finish::Stop,
    };
    let transport = Script::new(vec![vide.clone(), vide.clone(), vide]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (harness, _, log) = build(
        transport,
        tools,
        Limits {
            max_tours: 10,
            max_relances: 2,
            budget_ms: None,
        },
    );

    match run(harness.send("fais quelque chose")) {
        Running::Ran(Stop::Failed(motif)) => {
            assert!(motif.contains("ni texte ni outil"), "{motif}")
        }
        autre => panic!("attendu un échec nommé, obtenu {autre:?}"),
    }
    let hints = log.borrow().iter().filter(|e| e.name() == "hint").count();
    assert_eq!(hints, 2, "chaque relance dit pourquoi elle a lieu");
}

#[test]
fn une_action_sans_effet_n_est_pas_un_progres() {
    let transport = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: "c'est fait".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
        Reply {
            text: "vraiment fait".into(),
            calls: vec![],
            finish: Finish::Stop,
        },
    ]);
    // L'outil répond « ok » et ne change rien : deux empreintes identiques.
    let tools = Tools::new(|_| Outcome::ok("ok").observed("A", "A"));
    let (harness, _, log) = build(transport, tools, Limits::default());

    run(harness.send("change le chrome"));
    let result = log.borrow().iter().any(|event| {
        matches!(
            event,
            Event::ToolResult {
                unchanged: true,
                ..
            }
        )
    });
    assert!(result, "l'observation doit remonter jusqu'à l'interface");
}

#[test]
fn le_budget_de_tours_s_arrete_en_le_disant() {
    let tour = Reply {
        text: String::new(),
        calls: vec![call("a1")],
        finish: Finish::Tools,
    };
    let transport = Script::new(vec![tour.clone(), tour.clone(), tour]);
    let tools = Tools::new(|_| Outcome::ok("ok").observed("a", "b"));
    let (harness, _, log) = build(
        transport,
        tools,
        Limits {
            max_tours: 2,
            max_relances: 0,
            budget_ms: None,
        },
    );

    assert_eq!(run(harness.send("boucle")), Running::Ran(Stop::Budget));
    assert!(
        log.borrow()
            .iter()
            .any(|event| matches!(event, Event::Stopped { .. }))
    );
}

#[test]
fn la_file_se_manipule_sans_perdre_l_ordre() {
    let transport = Script::new(vec![]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (_, handle, _) = build(transport, tools, Limits::default());

    let a = handle.enqueue("un");
    let b = handle.enqueue("deux");
    let _c = handle.enqueue("trois");
    assert_eq!(handle.pending().len(), 3);

    assert!(handle.edit_pending(b, "DEUX"));
    assert!(handle.bump(b));
    assert_eq!(handle.pending()[0].text, "DEUX");
    assert!(handle.drop_pending(a));
    assert_eq!(handle.pending().len(), 2);
    assert!(
        !handle.drop_pending(4242),
        "retirer deux fois ne casse rien"
    );
}

#[test]
fn un_etat_sauvegarde_se_repare_et_se_rejoue() {
    let transport = Script::new(vec![]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (harness, handle, _) = build(transport, tools, Limits::default());
    handle.enqueue("message tapé juste avant la fermeture");

    let saved: Snapshot = harness.snapshot();
    let texte = serde_json::to_string(&saved).expect("un état se sérialise");
    let relu: Snapshot = serde_json::from_str(&texte).expect("et se relit");

    let (autre, autre_handle, _) = build(
        Script::new(vec![]),
        Tools::new(|_| Outcome::ok("ok")),
        Limits::default(),
    );
    autre.restore(relu);
    let restaure = autre_handle.pending();
    assert_eq!(restaure.len(), saved.pending.len());
    for (apres, avant) in restaure.iter().zip(&saved.pending) {
        assert_eq!((apres.id, &apres.text), (avant.id, &avant.text));
        // `serde_json` relit un `f64` à un ULP près sans la fonctionnalité `float_roundtrip` :
        // on compare l'instant, pas ses bits — sinon le test échoue une fois sur vingt.
        assert!(
            (apres.at_ms - avant.at_ms).abs() < 1e-3,
            "l'instant d'arrivée doit survivre à la sauvegarde : {} / {}",
            apres.at_ms,
            avant.at_ms
        );
    }
    assert_eq!(
        autre_handle.pending()[0].text,
        "message tapé juste avant la fermeture"
    );
}

#[test]
fn le_flux_arrive_morceau_par_morceau() {
    let transport = Script::new(vec![Reply {
        text: "un deux trois".into(),
        calls: vec![],
        finish: Finish::Stop,
    }]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (harness, _, log) = build(transport, tools, Limits::default());
    run(harness.send("parle"));

    let morceaux: Vec<String> = log
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Delta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        morceaux.len() > 1,
        "le texte doit arriver en flux, pas d'un bloc"
    );
    assert_eq!(morceaux.concat(), "un deux trois");
}

#[test]
fn un_arret_de_transport_est_une_panne_typee() {
    struct Down;
    #[async_trait(?Send)]
    impl Transport for Down {
        async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
            Err(Failure::http(503))
        }
    }
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let seen = log.clone();
    let harness = Harness::new(
        "s",
        Limits::default(),
        Rc::new(Down),
        Tools::new(|_| Outcome::ok("ok")),
        move |event| seen.borrow_mut().push(event),
    );

    match run(harness.send("parle")) {
        Running::Ran(Stop::Failed(motif)) => assert!(motif.contains("503"), "{motif}"),
        autre => panic!("attendu Failed, obtenu {autre:?}"),
    }
    let code = log.borrow().iter().find_map(|event| match event {
        Event::Stopped { code, .. } => code.clone(),
        _ => None,
    });
    assert_eq!(
        code.as_deref(),
        Some("http"),
        "une panne porte un code stable"
    );
}

#[test]
fn la_consigne_systeme_part_toujours_avec_le_fil() {
    // Le bug d'origine : le harnais portait la consigne, aucun transport ne
    // l'envoyait. Le modèle tournait sans brief — sans la liste de ses outils.
    // `Request` n'expose plus que `thread()`, qui la met en tête.
    let transport = Script::new(vec![Reply {
        text: "fini".into(),
        calls: vec![],
        finish: Finish::Stop,
    }]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (harness, _, _) = build(transport.clone(), tools, Limits::default());
    run(harness.send("installe un module"));

    let request = transport
        .seen
        .borrow()
        .first()
        .cloned()
        .expect("un tour a eu lieu");
    let thread = request.thread();
    assert!(matches!(&thread[0], Message::System { content } if content == "tu es l'atelier"));
    assert_eq!(thread.len(), 2, "consigne + le message de l'utilisateur");
    assert!(thread[1].is_user());
    // Aucun doublon : la consigne n'est pas aussi dans le fil.
    assert_eq!(
        thread
            .iter()
            .filter(|m| matches!(m, Message::System { .. }))
            .count(),
        1
    );
}

#[test]
fn un_tour_coupe_ne_laisse_pas_d_appel_sans_reponse() {
    // Un assistant qui annonce `tool_calls` sans qu'aucun résultat ne suive
    // rend le fil invalide : le moteur refuse la conversation suivante. C'est
    // le genre de détail qui casse tout *après* la coupure.
    let transport = Script::new(vec![Reply {
        text: "je vais installer".into(),
        calls: vec![call("a1")],
        finish: Finish::Length,
    }]);
    let tools = Tools::new(|_| Outcome::ok("ok"));
    let (harness, _, _) = build(transport, tools, Limits::default());
    run(harness.send("fais"));

    let fil = harness.transcript();
    assert!(
        !fil.iter().any(|m| matches!(m, Message::Tool { .. })),
        "aucun résultat d'outil ne doit apparaître"
    );
    let assistant = fil
        .iter()
        .find_map(|m| match m {
            Message::Assistant { content, calls } => Some((content.clone(), calls.len())),
            _ => None,
        })
        .expect("le texte entendu est conservé");
    assert_eq!(assistant.0, "je vais installer");
    assert_eq!(assistant.1, 0, "les appels non joués sont retirés du fil");
}

#[test]
fn un_pending_se_relit_dans_le_json() {
    let entry = Pending {
        id: 3,
        text: "coucou".into(),
        at_ms: 12.0,
    };
    let texte = serde_json::to_string(&entry).unwrap();
    let relu: Pending = serde_json::from_str(&texte).unwrap();
    assert_eq!(relu, entry);
}

// ---- la composition : une politique branchée change l'issue ------------------

/// Une politique qui refuse de relancer sur un refus : elle arrête le run et
/// nomme la raison, au lieu de rendre la main au modèle.
struct Strict;

impl Policy for Strict {
    fn after_turn(&self, run: &mut AgentRun, spoke: bool, called: bool) -> Decision {
        // Le refus est lu AVANT le raccourci « il a appelé un outil » : c'est
        // toute la différence avec la politique par défaut, qui continue
        // toujours quand un outil a été appelé.
        if let Some(motif) = run.derniere_refusee().and_then(|etape| etape.motif.clone()) {
            return Decision::Halt(Stop::Refused(motif));
        }
        if called {
            return Decision::Continue;
        }
        match spoke {
            true => Decision::Halt(Stop::Done),
            false => Decision::Continue,
        }
    }
}

/// Le harnais est composable : la même boucle, les mêmes transports, les mêmes
/// outils — et une politique différente change l'issue du run.
///
/// Par défaut un refus relance (le modèle peut rebondir) ; avec une politique
/// stricte, le refus arrête tout de suite et **le modèle n'est pas rappelé**.
#[test]
fn une_politique_branchee_change_l_issue_du_run() {
    let script = || {
        Script::new(vec![
            Reply {
                text: String::new(),
                calls: vec![call("a1")],
                finish: Finish::Tools,
            },
            Reply {
                text: "je m'arrête là".into(),
                calls: vec![],
                finish: Finish::Stop,
            },
        ])
    };
    let tools = || Tools::new(|_| Outcome::refused("refusé : même version", "même version"));

    // 1. Politique par défaut : le refus relance, le tour suivant conclut.
    let (harness, _, _) = build(script(), tools(), Limits::default());
    let stop = match run(harness.send("installe")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    assert_eq!(
        stop,
        Stop::Done,
        "par défaut, un refus ne termine pas le run"
    );

    // 2. Même script, même outil, politique stricte : on s'arrête au refus.
    let transport = script();
    let seen = transport.clone();
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let harness = Harness::new(
        "tu es l'atelier",
        Limits::default(),
        transport,
        tools(),
        move |event| sink.borrow_mut().push(event),
    )
    .with_policy(Rc::new(Strict));

    let stop = match run(harness.send("installe")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    assert!(
        matches!(stop, Stop::Refused(_)),
        "attendu Refused, obtenu {stop:?}"
    );
    assert_eq!(
        seen.seen.borrow().len(),
        1,
        "la politique stricte évite le second tour"
    );
    assert_eq!(stop_of(&log), stop, "l'arrêt est dit, pas constaté");
}

// ---- la composition : les outils viennent de plusieurs sources ----------------

/// Un toolbox minimal, piloté par une closure, dont la surface peut changer.
struct Part {
    tools: RefCell<Vec<ToolSpec>>,
    answer: &'static str,
    used: Rc<RefCell<Vec<String>>>,
}

impl Part {
    fn new(names: &[&str], answer: &'static str) -> Rc<Self> {
        Rc::new(Self {
            tools: RefCell::new(
                names
                    .iter()
                    .map(|name| ToolSpec {
                        name: (*name).to_string(),
                        description: "outil de test".to_string(),
                        parameters: json!({"type": "object"}),
                    })
                    .collect(),
            ),
            answer,
            used: Rc::new(RefCell::new(Vec::new())),
        })
    }
}

#[async_trait(?Send)]
impl Toolbox for Part {
    fn specs(&self) -> Vec<ToolSpec> {
        self.tools.borrow().clone()
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.used.borrow_mut().push(call.name.clone());
        Outcome::ok(format!("{} dit {}", self.answer, call.name))
    }
}

/// Face la surface composite : deux sources, un seul harnais.
#[test]
fn deux_sources_d_outils_se_composent_sans_se_voir() {
    let hote = Part::new(&["install_module"], "hôte");
    let module = Part::new(&["list_customers"], "module");
    let both = Toolboxes::new().with(hote.clone()).with(module.clone());

    let names: Vec<String> = both.specs().into_iter().map(|spec| spec.name).collect();
    assert_eq!(names, ["install_module", "list_customers"]);

    // Chaque appel part au bon endroit.
    let a = run(both.call(&ToolCall {
        id: "1".into(),
        name: "install_module".into(),
        arguments: json!({}),
    }));
    let b = run(both.call(&ToolCall {
        id: "2".into(),
        name: "list_customers".into(),
        arguments: json!({}),
    }));
    assert_eq!(a.text, "hôte dit install_module");
    assert_eq!(b.text, "module dit list_customers");
    assert_eq!(*hote.used.borrow(), ["install_module"]);
    assert_eq!(*module.used.borrow(), ["list_customers"]);

    // Un outil que personne ne déclare est refusé, et la raison est nommée.
    let unknown = run(both.call(&ToolCall {
        id: "3".into(),
        name: "efface_tout".into(),
        arguments: json!({}),
    }));
    assert!(
        unknown.refused.is_some(),
        "un outil inconnu ne s'exécute pas"
    );
}

/// Un module installé **pendant** un run devient routable au tour suivant :
/// la table n'est pas figée à la construction.
#[test]
fn un_outil_installe_pendant_le_run_devient_routable() {
    let part = Part::new(&[], "module");
    let both = Toolboxes::new().with(part.clone());

    let unknown = run(both.call(&ToolCall {
        id: "1".into(),
        name: "list_customers".into(),
        arguments: json!({}),
    }));
    assert!(unknown.refused.is_some(), "rien n'est encore installé");

    // Le module s'installe : sa surface apparaît.
    part.tools.borrow_mut().push(ToolSpec {
        name: "list_customers".to_string(),
        description: "lu dans le module".to_string(),
        parameters: json!({"type": "object"}),
    });

    let routed = run(both.call(&ToolCall {
        id: "2".into(),
        name: "list_customers".into(),
        arguments: json!({}),
    }));
    assert_eq!(routed.text, "module dit list_customers");
    assert_eq!(both.specs().len(), 1);
}

// ---- le harness vient du graphe ----------------------------------------------

/// Une composition lue dans le graphe produit un harness qui tourne : le brief
/// et les bornes viennent du document, la politique d'un emplacement.
#[test]
fn un_harness_compose_depuis_le_graphe_porte_son_brief_et_ses_bornes() {
    use this_agent::graph::{HarnessDoc, Slot, SlotKind};

    let doc = HarnessDoc {
        id: "h-atelier".into(),
        name: "atelier".into(),
        version: 1,
        brief: "tu es l'atelier, et tu écris des modules".into(),
        slots: vec![Slot {
            kind: SlotKind::Policy,
            module: "harness-default".into(),
            version: "0.1.0".into(),
            entry: "policy".into(),
        }],
        max_tours: 2,
        max_relances: 0,
        budget_ms: None,
    };

    let script = Script::new(vec![
        Reply {
            text: String::new(),
            calls: vec![call("a1")],
            finish: Finish::Tools,
        },
        Reply {
            text: String::new(),
            calls: vec![call("a2")],
            finish: Finish::Tools,
        },
    ]);
    let seen = script.clone();
    let tools = Tools::new(|_| Outcome::ok("fait"));
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();

    let harness = Harness::from_doc(
        &doc,
        script,
        tools,
        Rc::new(this_agent::DefaultPolicy),
        move |event| sink.borrow_mut().push(event),
    );

    let stop = match run(harness.send("installe")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };

    // Le brief du document est bien le message système du premier tour.
    let first = seen
        .seen
        .borrow()
        .first()
        .cloned()
        .expect("un tour a eu lieu");
    let thread = first.thread();
    assert!(
        matches!(&thread[0], Message::System { content } if content == &doc.brief),
        "le brief du document est la consigne système"
    );

    // Les bornes du document s'appliquent : 2 tours, puis Budget.
    assert_eq!(stop, Stop::Budget, "max_tours vient de la composition");
    assert_eq!(seen.seen.borrow().len(), 2);
}
