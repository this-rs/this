//! # La suite de conformité d'un [`ToolRepo`]
//!
//! Un dépôt d'outils a deux implémentations qui doivent se comporter à l'identique : la
//! version en mémoire ([`MemRepo`](crate::MemRepo)) et celle de l'hôte (un graphe). Sans un
//! contrat exécutable, elles dérivent — et le catalogue, qui ne voit que le port, se met à
//! donner des réponses différentes selon l'endroit où il range ses outils.
//!
//! [`check_repo`] joue ce contrat sur n'importe quelle implémentation. Il PANIQUE à la
//! première violation, avec le nom de la règle : c'est un banc d'essai, pas une API
//! d'exécution. Un hôte l'appelle depuis ses tests, sur un dépôt **vide** :
//!
//! ```ignore
//! this_harness::conformance::check_repo(&|| Rc::new(MonDepot::temporaire()));
//! ```
//!
//! `make` est appelée une fois par règle : chaque règle part d'un dépôt neuf.

use std::rc::Rc;

use serde_json::json;

use crate::catalogue::{
    RecipeStep, ToolDef, ToolRecord, ToolRepo, ToolStats, ToolStatus, steps_hash,
};

fn record(name: &str, description: &str, n: u32, seq: u64) -> ToolRecord {
    let steps = vec![RecipeStep {
        tool: "task_list".to_string(),
        args: json!({ "n": n }),
    }];
    ToolRecord {
        name: name.to_string(),
        versions: vec![ToolDef {
            version: 1,
            description: description.to_string(),
            parameters: json!({"type": "object", "properties": {}, "required": []}),
            steps_hash: steps_hash(&steps),
            steps,
            tags: vec!["conformite".to_string()],
            note: "création".to_string(),
            seq,
            stats: ToolStats::default(),
        }],
        status: ToolStatus::Active,
        dropped: 0,
    }
}

fn retired(mut record: ToolRecord) -> ToolRecord {
    record.status = ToolStatus::Retired {
        reason: "test".to_string(),
        replaced_by: None,
    };
    record
}

/// Joue le contrat. `make` rend un dépôt VIDE ; elle est appelée une fois par règle.
pub async fn check_repo(make: &dyn Fn() -> Rc<dyn ToolRepo>) {
    // 1. Un dépôt neuf est vide, et ne rend jamais rien d'inventé.
    let r = make();
    assert_eq!(r.len().await.unwrap(), 0, "vide : len");
    assert!(r.is_empty().await.unwrap(), "vide : is_empty");
    assert_eq!(r.active_len().await.unwrap(), 0, "vide : active_len");
    assert!(r.get("rien").await.unwrap().is_none(), "vide : get");
    assert!(r.recent(5).await.unwrap().is_empty(), "vide : recent");
    assert!(
        r.candidates("n'importe quoi", 5).await.unwrap().is_empty(),
        "vide : candidates"
    );
    assert!(
        r.active_with_steps("deadbeef", None)
            .await
            .unwrap()
            .is_none(),
        "vide : active_with_steps"
    );
    assert!(r.capacity() >= 1, "capacity au moins 1");

    // 2. L'horloge est strictement croissante.
    let r = make();
    let (a, b, c) = (
        r.tick().await.unwrap(),
        r.tick().await.unwrap(),
        r.tick().await.unwrap(),
    );
    assert!(a < b && b < c, "tick strictement croissant : {a} {b} {c}");

    // 3. put puis get rend EXACTEMENT ce qui a été écrit (tous les champs).
    let r = make();
    let rec = record(
        "alpha_un",
        "Liste les factures du mois",
        1,
        r.tick().await.unwrap(),
    );
    r.put(&rec).await.expect("put");
    assert_eq!(
        r.get("alpha_un").await.unwrap(),
        Some(rec.clone()),
        "aller-retour complet"
    );
    assert_eq!(
        (r.len().await.unwrap(), r.active_len().await.unwrap()),
        (1, 1),
        "compteurs après un put"
    );

    // 4. put REMPLACE : même nom, pas de doublon, la nouvelle version est lue.
    let mut two = rec.clone();
    let mut v2 = two.versions[0].clone();
    v2.version = 2;
    v2.description = "Liste les factures, version deux".to_string();
    v2.seq = r.tick().await.unwrap();
    two.versions.push(v2);
    r.put(&two).await.expect("put 2");
    assert_eq!(
        r.len().await.unwrap(),
        1,
        "remplacer ne crée pas un second outil"
    );
    assert_eq!(
        r.get("alpha_un").await.unwrap().unwrap().versions.len(),
        2,
        "les versions suivent"
    );
    assert_eq!(
        r.get("alpha_un").await.unwrap().unwrap().current().version,
        2,
        "la courante est la dernière"
    );

    // 5. active_with_steps : par empreinte, `except` exclu, un outil retiré jamais rendu.
    let r = make();
    let a = record("alpha_un", "Premier outil", 1, r.tick().await.unwrap());
    let b = record("bravo_deux", "Second outil", 2, r.tick().await.unwrap());
    r.put(&a).await.unwrap();
    r.put(&b).await.unwrap();
    let h = a.current().steps_hash.clone();
    assert_eq!(
        r.active_with_steps(&h, None).await.unwrap().as_deref(),
        Some("alpha_un"),
        "trouvé par empreinte"
    );
    assert_eq!(
        r.active_with_steps(&h, Some("alpha_un")).await.unwrap(),
        None,
        "except exclu"
    );
    assert_eq!(
        r.active_with_steps("inconnue", None).await.unwrap(),
        None,
        "empreinte inconnue"
    );
    r.put(&retired(a.clone())).await.unwrap();
    assert_eq!(
        r.active_with_steps(&h, None).await.unwrap(),
        None,
        "un outil retiré n'est jamais un doublon"
    );
    assert_eq!(
        (r.len().await.unwrap(), r.active_len().await.unwrap()),
        (2, 1),
        "retiré : len garde, active_len baisse"
    );

    // 6. recent : seq décroissant, borné, actifs seulement.
    let r = make();
    for (i, name) in ["un_outil", "deux_outil", "trois_outil"].iter().enumerate() {
        r.put(&record(
            name,
            &format!("outil {i}"),
            i as u32,
            r.tick().await.unwrap(),
        ))
        .await
        .unwrap();
    }
    let recent = r.recent(2).await.unwrap();
    assert_eq!(recent.len(), 2, "recent borné");
    assert_eq!(
        recent[0].0, "trois_outil",
        "recent : le plus récent d'abord"
    );
    assert!(recent[0].1 > recent[1].1, "recent : seq décroissant");
    r.put(&retired(record("trois_outil", "outil 2", 2, 99)))
        .await
        .unwrap();
    assert!(
        r.recent(10)
            .await
            .unwrap()
            .iter()
            .all(|(n, _)| n != "trois_outil"),
        "recent n'inclut pas un outil retiré"
    );

    // 7. candidates : actifs seulement, le plus proche d'abord, borné par k.
    let r = make();
    r.put(&record(
        "liste_factures",
        "Liste les factures impayées du mois",
        1,
        r.tick().await.unwrap(),
    ))
    .await
    .unwrap();
    r.put(&record(
        "rotation_cles",
        "Fait tourner les clés de sauvegarde",
        2,
        r.tick().await.unwrap(),
    ))
    .await
    .unwrap();
    r.put(&record(
        "export_notes",
        "Exporte les notes en markdown",
        3,
        r.tick().await.unwrap(),
    ))
    .await
    .unwrap();
    let found = r.candidates("factures impayées", 5).await.unwrap();
    assert_eq!(
        found.first().map(String::as_str),
        Some("liste_factures"),
        "le plus proche d'abord : {found:?}"
    );
    assert!(
        r.candidates("factures", 1).await.unwrap().len() <= 1,
        "k borne le résultat"
    );
    assert!(
        r.candidates("zzzzqqqq", 5).await.unwrap().is_empty(),
        "rien de proche : rien"
    );
    r.put(&retired(record(
        "liste_factures",
        "Liste les factures impayées du mois",
        1,
        50,
    )))
    .await
    .unwrap();
    assert!(
        !r.candidates("factures impayées", 5)
            .await
            .unwrap()
            .iter()
            .any(|n| n == "liste_factures"),
        "candidates n'inclut pas un outil retiré"
    );

    // 8. L'horloge survit aux écritures : jamais de retour en arrière.
    let r = make();
    let before = r.tick().await.unwrap();
    r.put(&record("alpha_un", "d", 1, before)).await.unwrap();
    assert!(r.tick().await.unwrap() > before, "tick continue après put");
}
