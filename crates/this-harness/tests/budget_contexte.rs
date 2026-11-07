//! La borne de contexte : un fil trop long est rogné par les plus anciens,
//! jamais au milieu d'un appel d'outil, et jamais sans le dire.

use serde_json::json;
use this_harness::{Message, ToolCall, estimate_tokens, fit};

fn appel(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "lire".into(),
        arguments: json!({"chemin": "a.txt"}),
    }
}

fn assistant_appels(ids: &[&str]) -> Message {
    Message::Assistant {
        content: String::new(),
        calls: ids.iter().map(|i| appel(i)).collect(),
    }
}

fn gros(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

fn pas_d_orphelin(fil: &[Message]) {
    let mut ouverts: Vec<String> = Vec::new();
    for m in fil {
        match m {
            Message::Assistant { calls, .. } => {
                ouverts = calls.iter().map(|c| c.id.clone()).collect()
            }
            Message::Tool { id, .. } => assert!(ouverts.contains(id), "réponse orpheline {id}"),
            _ => ouverts.clear(),
        }
    }
}

#[test]
fn estimation_croit_avec_le_texte_et_compte_les_appels() {
    let court = estimate_tokens(&[Message::user("a")]);
    let long = estimate_tokens(&[Message::user(gros('a', 400))]);
    assert!(long >= court + 99);
    let sans = estimate_tokens(&[Message::assistant("")]);
    let avec = estimate_tokens(&[assistant_appels(&["x"])]);
    assert!(avec > sans);
    assert_eq!(estimate_tokens(&[]), 0);
}

#[test]
fn un_fil_qui_tient_est_rendu_intact() {
    let fil = vec![Message::user("a"), Message::assistant("b")];
    let r = fit(&fil, 1_000_000);
    assert_eq!(r.kept, fil);
    assert!(r.dropped.is_empty());
    assert!(!r.dropped.over_budget);
}

#[test]
fn on_retire_les_plus_anciens_et_on_garde_l_ordre() {
    let fil = vec![
        Message::user(gros('a', 400)),
        Message::assistant(gros('b', 400)),
        Message::user(gros('c', 40)),
        Message::assistant(gros('d', 40)),
        Message::user(gros('e', 40)),
    ];
    let budget = estimate_tokens(&fil[2..]);
    let r = fit(&fil, budget);
    assert_eq!(r.kept, fil[2..].to_vec());
    assert_eq!(r.dropped.messages, 2);
    assert_eq!(r.dropped.tokens, estimate_tokens(&fil[..2]));
    assert!(!r.dropped.over_budget);
}

#[test]
fn le_groupe_d_outils_disparait_entier() {
    let fil = vec![
        Message::user("q1"),
        assistant_appels(&["a", "b"]),
        Message::tool("a", gros('x', 2000)),
        Message::tool("b", gros('y', 2000)),
        Message::assistant("fait"),
        Message::user("q2"),
    ];
    // Assez pour la fin, pas pour le groupe (4 premiers messages).
    let budget = estimate_tokens(&fil[4..]) + 10;
    let r = fit(&fil, budget);
    assert_eq!(r.kept, fil[4..].to_vec());
    assert_eq!(r.dropped.messages, 4);
    pas_d_orphelin(&r.kept);
}

#[test]
fn le_groupe_reste_entier_quand_il_tient() {
    let fil = vec![
        Message::user("q1"),
        assistant_appels(&["a"]),
        Message::tool("a", "ok"),
        Message::user("q2"),
    ];
    let r = fit(&fil, estimate_tokens(&fil[1..]));
    assert_eq!(r.kept, fil[1..].to_vec());
    pas_d_orphelin(&r.kept);
}

#[test]
fn le_dernier_message_utilisateur_est_garde_et_l_ecart_signale() {
    let fil = vec![Message::assistant("vieux"), Message::user(gros('q', 4000))];
    let r = fit(&fil, 10);
    assert_eq!(r.kept, fil[1..].to_vec());
    assert!(r.dropped.over_budget);
    assert_eq!(r.dropped.excess, estimate_tokens(&r.kept) - 10);
    assert_eq!(r.dropped.messages, 1);
}

#[test]
fn le_message_systeme_n_est_pas_de_l_historique() {
    let fil = vec![
        Message::system("consignes"),
        Message::user(gros('a', 800)),
        Message::assistant(gros('b', 800)),
        Message::user("q"),
    ];
    let r = fit(&fil, estimate_tokens(&[fil[0].clone(), fil[3].clone()]));
    assert_eq!(r.kept, vec![fil[0].clone(), fil[3].clone()]);
    assert!(!r.dropped.over_budget);
}

#[test]
fn les_cas_limites_ne_paniquent_pas() {
    assert!(fit(&[], 0).kept.is_empty());
    assert!(fit(&[], 100).dropped.is_empty());
    let seul = vec![Message::user("q")];
    let r = fit(&seul, 0);
    assert_eq!(r.kept, seul);
    assert!(r.dropped.over_budget);
    let r = fit(&seul, usize::MAX);
    assert_eq!(r.kept, seul);
    // Pas de message utilisateur du tout, budget nul.
    let r = fit(&[Message::assistant("x")], 0);
    assert!(r.kept.is_empty());
    assert!(!r.dropped.over_budget);
}

#[test]
fn un_groupe_d_outils_en_tete_de_fil_sans_budget_disparait() {
    let fil = vec![
        assistant_appels(&["a"]),
        Message::tool("a", "ok"),
        Message::user("q"),
    ];
    let r = fit(&fil, 0);
    assert_eq!(r.kept, fil[2..].to_vec());
    pas_d_orphelin(&r.kept);
}

#[test]
fn des_assistants_avec_appels_qui_se_suivent_forment_des_groupes_distincts() {
    let fil = vec![
        assistant_appels(&["a"]),
        Message::tool("a", gros('x', 2000)),
        assistant_appels(&["b"]),
        Message::tool("b", "court"),
        Message::user("q"),
    ];
    let r = fit(&fil, estimate_tokens(&fil[2..]));
    assert_eq!(r.kept, fil[2..].to_vec());
    pas_d_orphelin(&r.kept);
}

#[test]
fn deterministe() {
    let fil = vec![Message::user(gros('a', 900)), Message::user("q")];
    assert_eq!(fit(&fil, 20), fit(&fil, 20));
}
