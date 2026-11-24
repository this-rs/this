//! Crash puis reprise sur un processus NEUF, magasin RÉEL (docs/harnais-reprise-durable.md,
//! §2(b), ticket #284).
//!
//! Chaque test lance un sous-processus (ce binaire de test, ré-invoqué sur `enfant`) qui
//! pilote un vrai `Harness` branché sur le vrai journal durable (`HostRecorder` →
//! `HarnessGraph` → `ObrainStore` sur disque) puis meurt par `std::process::abort()` à un
//! point précis. Le parent, qui n'a rien partagé avec l'enfant que le disque, rouvre le
//! magasin et dit ce que la reprise y voit.
//!
//! L'« effet » est un fichier : l'outil y ajoute une ligne. Le parent le lit comme un
//! oracle de test ; la reprise réelle, elle, ne dispose que du journal.
//!
//! Ce que ces tests NE disent PAS : qu'un effet de résultat inconnu n'est pas rejoué
//! aveuglément (case (c), déjà prouvée : `un_outil_abandonne_a_un_effet_inconnu_pas_un_echec_prouve`
//! et `une_etape_abandonnee_n_arrete_pas_le_run_en_echec`, `src/session.rs`).

// Feature `obrain` : le journal durable (`HarnessGraph`, `HostRecorder`) vit dans le crate
// racine et le magasin est obrain. SANS la feature, ce fichier est vide.
// ATTENTION : le job CI `obrain` n'est pas actif sans le secret OBRAIN_TOKEN (dépôt privé),
// donc ce test ne tourne aujourd'hui qu'en local : `cargo test --features obrain --test reprise_crash`.
#![cfg(feature = "obrain")]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this::server::host::HostRecorder;
use this::storage::{HarnessGraph, ObrainStore};
use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Delta, Failure, Finish, Harness, Limits, Outcome, Recorder, Reply, Request, ToolCall,
    ToolSpec, Toolbox, Transport,
};

const ENV_DIR: &str = "THIS_CRASH_DIR";
const ENV_POINT: &str = "THIS_CRASH_POINT";
const AVANT_EFFET: &str = "avant_effet";
const APRES_EFFET: &str = "apres_effet";
const APRES_CHECKPOINT: &str = "apres_checkpoint";
const RUN_ID: &str = "run-1";

fn magasin(dir: &Path) -> PathBuf {
    dir.join("magasin")
}
fn effet(dir: &Path) -> PathBuf {
    dir.join("effet.log")
}

// ───────────────────────── côté enfant ─────────────────────────

/// Un outil au premier tour, puis la conclusion.
struct Moteur(RefCell<u32>);

#[async_trait(?Send)]
impl Transport for Moteur {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let n = {
            let mut n = self.0.borrow_mut();
            *n += 1;
            *n
        };
        Ok(if n == 1 {
            Reply {
                served: None,
                text: String::new(),
                calls: vec![ToolCall {
                    id: "t1".into(),
                    name: "ecrire".into(),
                    arguments: json!({}),
                }],
                finish: Finish::Tools,
            }
        } else {
            Reply {
                served: None,
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            }
        })
    }
}

/// L'outil : son effet est une ligne ajoutée à `effet.log`.
struct Outil {
    dir: PathBuf,
    point: String,
}

#[async_trait(?Send)]
impl Toolbox for Outil {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "ecrire".into(),
            description: "écrit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        if self.point == AVANT_EFFET {
            std::process::abort();
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(effet(&self.dir))
            .unwrap();
        writeln!(f, "ecrit").unwrap();
        f.sync_all().unwrap();
        if self.point == APRES_EFFET {
            std::process::abort();
        }
        Outcome::ok("ok")
    }
}

/// Le vrai journal, puis (pour `apres_checkpoint`) la mort juste après son commit.
struct MeurtApres {
    reel: HostRecorder,
    point: String,
}

#[async_trait(?Send)]
impl Recorder for MeurtApres {
    async fn step(&self, run_id: &str, run: &Run, step: &Step) {
        self.reel.step(run_id, run, step).await;
        if self.point == APRES_CHECKPOINT {
            std::process::abort();
        }
    }
    async fn stopped(&self, run_id: &str, run: &Run, stop: &Stop) {
        self.reel.stopped(run_id, run, stop).await;
    }
}

/// Point d'entrée de l'enfant. Sans les variables d'environnement (exécution normale de la
/// suite), ne fait rien.
#[test]
fn enfant() {
    let (Ok(dir), Ok(point)) = (std::env::var(ENV_DIR), std::env::var(ENV_POINT)) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let store = ObrainStore::open(magasin(&dir)).expect("l'enfant ouvre le magasin");
    let recorder = MeurtApres {
        reel: HostRecorder::new(HarnessGraph::new(store)),
        point: point.clone(),
    };
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Moteur(RefCell::new(0))),
        Rc::new(Outil { dir, point }),
        |_| {},
    )
    .with_recorder(Rc::new(recorder));
    let _ = futures::executor::block_on(harness.send("écris"));
    // Si on arrive ici, le point de coupe n'a pas été atteint : l'enfant ne doit pas
    // passer pour un crash.
    std::process::exit(0);
}

// ───────────────────────── côté parent ─────────────────────────

/// Lance l'enfant, exige qu'il soit mort par SIGABRT (pas une sortie propre, pas une
/// panique), rend le répertoire (supprimé à la chute) et ce que le journal relu dit.
fn crash_a(point: &str) -> (tempfile::TempDir, Option<Run>) {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "enfant", "--test-threads=1", "--nocapture"])
        .env(ENV_DIR, dir.path())
        .env(ENV_POINT, point)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "l'enfant devait mourir : {status:?}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(6),
            "mort attendue par abort() : {status:?}"
        );
    }
    // Processus NEUF : on rouvre le magasin disque, rien d'autre n'est partagé.
    let store = ObrainStore::open(magasin(dir.path())).expect("réouverture du magasin réel");
    let run = HarnessGraph::new(store).load_run(RUN_ID).unwrap();
    (dir, run)
}

fn lignes_d_effet(dir: &Path) -> usize {
    std::fs::read_to_string(effet(dir)).map_or(0, |s| s.lines().count())
}

#[test]
fn crash_avant_l_effet_ni_journal_ni_monde_rejouer_est_sans_risque() {
    let (dir, run) = crash_a(AVANT_EFFET);
    assert!(
        run.is_none(),
        "aucune étape : le journal ne connaît pas ce run"
    );
    assert_eq!(
        lignes_d_effet(dir.path()),
        0,
        "l'outil n'a rien fait au monde"
    );
}

#[test]
fn crash_apres_l_effet_avant_l_acquittement_le_journal_ne_sait_pas() {
    let (dir, run) = crash_a(APRES_EFFET);
    // Le monde a changé…
    assert_eq!(lignes_d_effet(dir.path()), 1, "l'effet a eu lieu");
    // …et le journal est indiscernable du cas « avant l'effet » : ni run, ni étape. La
    // reprise ne peut donc PAS savoir que l'effet a été tenté (le cœur n'écrit pas
    // d'intention avant l'appel : docs/harnais-reprise-durable.md §2(b), point 2). Ce test
    // épingle cette limite connue, il ne prétend pas la combler.
    assert!(
        run.is_none(),
        "le journal ne dit rien de l'effet : la reprise l'ignore"
    );
}

#[test]
fn crash_apres_checkpoint_l_etape_est_relue_et_le_run_n_est_pas_fini() {
    let (dir, run) = crash_a(APRES_CHECKPOINT);
    assert_eq!(lignes_d_effet(dir.path()), 1, "l'effet a eu lieu une fois");
    let run = run.expect("l'étape a été commitée avant la mort : le run se relit");
    assert_eq!(run.steps.len(), 1);
    let etape = &run.steps[0];
    assert_eq!(etape.tool, "ecrire");
    assert!(
        !etape.refused && etape.failure.is_none(),
        "l'étape dit : effet accompli"
    );
    // Le run n'a pas dit son arrêt (`_stop` = "running") : la reprise sait qu'il a été
    // interrompu, et que l'étape 0 est faite — elle ne doit pas la refaire.
    assert!(
        run.stop.is_none(),
        "run interrompu, pas terminé : {:?}",
        run.stop
    );
}
