//! La ligne de commande : lecture des arguments, `--version`/`--help`, erreurs lisibles,
//! `invite`, `serve` (port occupé, puis un vrai démarrage), et le binaire lui-même.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

use this_forge::cli::{self, Command};
use this_forge::sign::sha256_hex;
use this_forge::store::Store;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

async fn run(a: &[&str]) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let r = cli::run(&args(a), &mut out).await;
    (r, String::from_utf8(out).unwrap())
}

fn err(r: anyhow::Result<()>) -> String {
    format!("{:#}", r.expect_err("devait échouer"))
}

#[tokio::test]
async fn version_and_help_print_without_touching_the_disk() {
    for flag in ["--version", "-V", "version"] {
        let (r, out) = run(&[flag]).await;
        r.unwrap();
        assert_eq!(out, format!("this-forge {}\n", env!("CARGO_PKG_VERSION")));
    }
    for flag in ["--help", "-h", "help"] {
        let (r, out) = run(&[flag]).await;
        r.unwrap();
        assert_eq!(out, cli::USAGE);
        assert!(out.contains("this-forge serve") && out.contains("this-forge invite"));
    }
    assert!(!Path::new("./forge-data").exists(), "aucune donnée créée");
}

#[tokio::test]
async fn an_unknown_subcommand_is_refused_readably() {
    let (r, out) = run(&["deploy"]).await;
    assert_eq!(err(r), "commande inconnue `deploy` : serve | invite");
    assert!(out.is_empty());
}

#[test]
fn without_arguments_it_serves_with_safe_defaults() {
    let Command::Serve { cfg, listen } = cli::parse(&[]).unwrap() else {
        panic!("serve attendu")
    };
    assert_eq!(listen.to_string(), "127.0.0.1:8480");
    // `--data-dir` absent : le dossier par défaut, et la clé dedans.
    assert_eq!(cfg.data_dir, Path::new("./forge-data"));
    assert_eq!(
        cfg.signing_key_path,
        Path::new("./forge-data/forge-signing.key")
    );
    assert_eq!(
        (cfg.image.as_str(), cfg.cpus, cfg.memory.as_str()),
        ("this-forge-builder:1.93", 6, "4g")
    );
    assert_eq!((cfg.cpu_shares, cfg.blkio_weight), (1024, 500));
}

#[test]
fn every_option_reaches_the_config() {
    let Command::Serve { cfg, listen } = cli::parse(&args(&[
        "serve",
        "--listen",
        "0.0.0.0:9000",
        "--data-dir",
        "/srv/forge",
        "--signing-key",
        "/etc/forge.key",
        "--image",
        "img:2",
        "--cpus",
        "2",
        "--memory",
        "1g",
        "--cpu-shares",
        "256",
        "--blkio-weight",
        "100",
    ]))
    .unwrap() else {
        panic!("serve attendu")
    };
    assert_eq!(listen.to_string(), "0.0.0.0:9000");
    assert_eq!(cfg.data_dir, Path::new("/srv/forge"));
    assert_eq!(cfg.signing_key_path, Path::new("/etc/forge.key"));
    assert_eq!(
        (cfg.image.as_str(), cfg.cpus, cfg.memory.as_str()),
        ("img:2", 2, "1g")
    );
    assert_eq!((cfg.cpu_shares, cfg.blkio_weight), (256, 100));

    let Command::Invite { cfg, uses, note } = cli::parse(&args(&[
        "invite",
        "--data-dir",
        "/d",
        "--uses",
        "5",
        "--note",
        "pour Léa",
    ]))
    .unwrap() else {
        panic!("invite attendu")
    };
    assert_eq!(cfg.data_dir, Path::new("/d"));
    assert_eq!((uses, note.as_deref()), (5, Some("pour Léa")));

    let Command::Invite { uses, note, .. } = cli::parse(&args(&["invite"])).unwrap() else {
        panic!("invite attendu")
    };
    assert_eq!(
        (uses, note),
        (1, None),
        "une invitation à usage unique par défaut"
    );
}

#[test]
fn unreadable_values_name_the_option_and_the_value() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["serve", "--cpus", "six"],
            "--cpus : valeur illisible `six`",
        ),
        (
            &["serve", "--cpu-shares", "-1"],
            "--cpu-shares : valeur illisible `-1`",
        ),
        (
            &["serve", "--blkio-weight", "99999"],
            "--blkio-weight : valeur illisible `99999`",
        ),
        (
            &["invite", "--uses", "beaucoup"],
            "--uses : valeur illisible `beaucoup`",
        ),
        (
            &["serve", "--listen", "localhost"],
            "--listen : adresse illisible `localhost`",
        ),
    ];
    for (a, expected) in cases {
        let e = format!("{:#}", cli::parse(&args(a)).expect_err("devait échouer"));
        assert!(e.starts_with(expected), "{a:?} → {e}");
    }
}

#[test]
fn invite_prints_a_code_whose_hash_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap().to_string();
    // Son propre runtime : le worker de la forge garde l'environnement LMDB ouvert tant
    // que le runtime vit ; on le jette avant de rouvrir le stockage, comme un processus.
    let (r, out) = tokio::runtime::Runtime::new().unwrap().block_on(run(&[
        "invite",
        "--data-dir",
        &data,
        "--uses",
        "2",
        "--note",
        "n",
    ]));
    r.unwrap();
    let code = out.trim_end();
    assert!(code.starts_with("forge-"), "{out}");
    assert_eq!(out.lines().count(), 1, "le code seul, pour un script");

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let store = Store::lmdb(dir.path()).unwrap();
        let inv = store.invitations.list().await.unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].name, sha256_hex(code.as_bytes()));
        assert_eq!((inv[0].uses_left, inv[0].note.as_deref()), (2, Some("n")));
    });
}

#[tokio::test]
async fn a_signing_key_that_cannot_be_written_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("fichier");
    std::fs::write(&blocker, "x").unwrap();
    let key = blocker.join("sous-un-fichier.key");
    let (r, out) = run(&[
        "invite",
        "--data-dir",
        dir.path().to_str().unwrap(),
        "--signing-key",
        key.to_str().unwrap(),
    ])
    .await;
    assert!(r.is_err());
    assert!(out.is_empty(), "aucun code imprimé");
}

#[tokio::test]
async fn serve_on_a_busy_port_says_which_address() {
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = busy.local_addr().unwrap().to_string();
    let dir = tempfile::tempdir().unwrap();
    let (r, _) = run(&[
        "serve",
        "--listen",
        &addr,
        "--data-dir",
        dir.path().to_str().unwrap(),
    ])
    .await;
    let e = err(r);
    assert!(
        e.starts_with(&format!("impossible d'écouter sur {addr}")),
        "{e}"
    );
    drop(busy);
}

/// `GET path` en HTTP/1.1 brut : rend (statut, corps) ou `None` si personne n'écoute.
fn http_get(addr: &str, path: &str) -> Option<(u16, String)> {
    let mut s = std::net::TcpStream::connect(addr).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut raw = String::new();
    s.read_to_string(&mut raw).ok()?;
    let status = raw.split_whitespace().nth(1)?.parse().ok()?;
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b.to_string())?;
    Some((status, body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_answers_with_its_public_key() {
    let dir = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let server = tokio::spawn({
        let a = args(&[
            "serve",
            "--listen",
            &addr,
            "--data-dir",
            dir.path().to_str().unwrap(),
        ]);
        async move { cli::run(&a, &mut std::io::sink()).await }
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let (status, body) = loop {
        let a = addr.clone();
        if let Some(r) = tokio::task::spawn_blocking(move || http_get(&a, "/v1/forge/key"))
            .await
            .unwrap()
        {
            break r;
        }
        assert!(
            !server.is_finished(),
            "serve s'est arrêté : {:?}",
            server.await
        );
        assert!(Instant::now() < deadline, "serve n'écoute pas après 5 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(status, 200, "{body}");
    let keys =
        this_forge::sign::ForgeKeys::load_or_create(&dir.path().join("forge-signing.key")).unwrap();
    assert!(body.contains(&keys.public_hex()), "{body}");
    server.abort();
}

#[test]
fn the_binary_runs_the_same_cli() {
    let bin = env!("CARGO_BIN_EXE_this-forge");
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("this-forge {}\n", env!("CARGO_PKG_VERSION"))
    );

    let out = std::process::Command::new(bin)
        .arg("deploy")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "une commande inconnue sort en erreur"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("commande inconnue `deploy`"), "{stderr}");
}
