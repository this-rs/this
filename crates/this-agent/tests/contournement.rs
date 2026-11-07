//! Banc adverse : contourner les garanties de `this-agent`.
//!
//! Chaque test reproduit une attaque constatée. Le test DOIT échouer avant la
//! correction (rejouer le rouge), puis passer après. Une attaque qu'on ne peut
//! pas fermer sans casser le contrat est rapportée, pas acceptée par un test.

use this_agent::approval::Approval;
use this_agent::graph::{HarnessDoc, Slot, SlotKind};
use this_agent::substrate::Verdict;
use this_agent::untrusted::{TAG, wrap_with};

// ---- 1. Un nonce vide ou faible brise le cloisonnement ----------------------

/// Extrait le nonce de la balise ouvrante du conteneur rendu.
fn nonce_of(out: &str) -> String {
    let prefix = format!("<{TAG} id=\"");
    assert!(out.starts_with(&prefix), "{out}");
    let rest = &out[prefix.len()..];
    let quote = rest.find('"').expect("attribut id fermé");
    rest[..quote].to_string()
}

/// `wrap_with` documente un nonce de « 32 caractères hexadécimaux » et dit qu'il
/// « ne doit pas être vide » : rien ne l'imposait. Un nonce vide (`id=""`) rend le
/// `strip_nonce` inopérant (aucun secret à retirer) et l'identité du conteneur
/// forçable ; un nonce faible (`abc`, `id`, `!!!`) est devinable. Tout appelant
/// qui passe un nonce hors contrat doit recevoir un nonce fort, pas un trou.
#[test]
fn un_nonce_vide_ou_faible_est_remplace_par_un_nonce_fort() {
    for mauvais in [
        "",
        "abc",
        "id",
        "!!!",
        "0123456789abcdef0123456789abcdeg", // 33 chars, non hex
    ] {
        let out = wrap_with(mauvais, 100, "note", &[("project", "p")], "corps");
        let n = nonce_of(&out);
        assert_eq!(n.len(), 32, "nonce {n:?} pour {mauvais:?}");
        assert!(
            n.chars().all(|c| c.is_ascii_hexdigit()),
            "nonce {n:?} non hexadécimal pour {mauvais:?}"
        );
        // Le nonce fort doit aussi être retiré du corps s'il y paraît : le
        // cloisonnement se fait avec le nonce finalement émis, pas celui reçu.
        assert!(!out.contains(&format!("\n{n}\n")), "{out}");
    }
}

// ---- 2. Un digest vide passe le mode strict --------------------------------

fn slot(kind: SlotKind, module: &str, digest: Option<&str>) -> Slot {
    Slot {
        kind,
        module: module.into(),
        version: "1.0.0".into(),
        entry: "main".into(),
        digest: digest.map(str::to_string),
    }
}

fn doc_avec(digest: Option<&str>) -> HarnessDoc {
    HarnessDoc {
        id: "h".into(),
        name: "atelier".into(),
        version: 1,
        brief: "tu es l'atelier".into(),
        slots: vec![slot(SlotKind::Policy, "pol", digest)],
        max_tours: 12,
        max_relances: 3,
        budget_ms: None,
        approval: None,
        silence_ms: None,
        no_progress_ms: None,
        tool_timeout_ms: None,
        context_budget: None,
    }
}

/// `is_approved_strict` promet « aucun emplacement sans digest de contenu ».
/// Un digest `Some("")` (champ présent mais vide) passait : l'hôte croyait le
/// contenu épinglé alors que rien ne l'était. Le strict doit refuser un digest
/// vide comme un digest absent.
#[test]
fn le_strict_refuse_un_digest_vide_comme_un_digest_absent() {
    // L'hôte approuve une composition dont l'emplacement porte un digest vide :
    // c'est exactement le document qu'un agent peut écrire pour passer le strict.
    let mut d = doc_avec(Some(""));
    let f = d.fingerprint();
    let a = Approval {
        by: "theo".into(),
        at_ms: 0,
        fingerprint: f,
        digests: Default::default(),
    };
    d.approval = Some(a.clone());
    let trusted = vec![a];

    // Le mode tolérant accepte (l'empreinte couvre le champ, même vide).
    assert_eq!(d.is_approved(&trusted), Verdict::Allow);
    // Le mode strict doit refuser : un digest vide n'épingle aucun contenu.
    match d.is_approved_strict(&trusted) {
        Verdict::Refuse(m) => assert!(m.contains("digest"), "{m}"),
        Verdict::Allow => panic!("un digest vide ne doit pas passer le strict"),
    }
}

/// Contrôle : un vrai digest (non vide, hexadécimal) passe le strict.
#[test]
fn le_strict_accepte_un_digest_reel() {
    let mut d = doc_avec(Some("d0"));
    let f = d.fingerprint();
    let a = Approval {
        by: "theo".into(),
        at_ms: 0,
        fingerprint: f,
        digests: Default::default(),
    };
    d.approval = Some(a.clone());
    assert_eq!(d.is_approved_strict(&[a]), Verdict::Allow);
}
