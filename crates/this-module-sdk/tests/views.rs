//! Les constructeurs de vues : conformité au contrat `this-descriptor`, refus à la construction,
//! et totalité du moteur de rendu.

use std::num::NonZeroU32;
use this_descriptor::views::ViewDesc;
use this_module_sdk::rt;
use this_module_sdk::views::*;
use this_module_sdk::{Value, ident, json, text};

/// Un arbre qui emploie chaque conteneur et chaque brique que l'API sait construire.
fn gros_arbre() -> View {
    let t = || ident!("tache");
    let filtre = Filter::and(
        Filter::eq(ident!("fait"), false),
        [
            Filter::not(Filter::cmp(ident!("titre"), Op::Contains, "x")),
            Filter::or(
                Filter::cmp(ident!("priorite"), Op::Gte, 2),
                [Filter::cmp(ident!("priorite"), Op::Lt, 1)],
            ),
        ],
    );
    let briques = vec![
        Brick::text(t(), ident!("titre")).filter(filtre.clone()),
        Brick::button(t(), ident!("terminer")),
        Brick::banner(Tone::Warning, text!("attention")),
        Brick::connection(),
        Brick::activity(t(), ident!("phase"), Some(ident!("duree"))),
        Brick::counter(t(), None),
        Brick::badge(t(), ident!("priorite")),
        Brick::list(t(), ident!("titre"))
            .subtitle(ident!("priorite"))
            .empty(text!("vide"))
            .page_size(NonZeroU32::new(20).unwrap())
            .on_click(Click::Navigate {
                view: ident!("detail"),
            })
            .filter(filtre),
        Brick::card(t(), ident!("titre"), [ident!("fait")]),
        Brick::form(
            t(),
            FormField::new(ident!("titre")).label(text!("Titre")),
            [FormField::new(ident!("fait")).read_only()],
        )
        .action(ident!("creer")),
        Brick::field_text(t(), ident!("titre")),
        Brick::field_multiline(t(), ident!("titre")),
        Brick::field_number(t(), ident!("priorite")),
        Brick::field_toggle(t(), ident!("fait")),
        Brick::field_file(t(), ident!("titre")),
        Brick::field_reference(t(), ident!("parent"), t()),
        Brick::relation(t(), ident!("suit"), RelationMode::Edit),
        Brick::chat(t()),
        Brick::notice(Tone::Error, t(), ident!("titre")).action(ident!("terminer")),
    ];
    let feuilles: Vec<Node> = briques.into_iter().map(Node::leaf).collect();
    let (premier, reste) = feuilles.split_first().unwrap();
    let pile = Node::stack(premier.clone(), reste.iter().cloned());
    let corps = Node::tabs(
        Tab::new(text!("A"), pile, []),
        [Tab::new(
            text!("B"),
            Node::row(
                Node::drawer(text!("tiroir"), Node::banner(Tone::Info, text!("x")), []),
                [Node::modal(
                    text!("modale"),
                    Node::banner(Tone::Success, text!("y")),
                    [],
                )],
            ),
            [],
        )],
    );
    View::new(ident!("gros"), Surface::Panel, corps)
        .unwrap()
        .title(text!("Tout"))
        .replaces(ident!("ancien"))
}

#[test]
fn l_aller_retour_json_est_exact() {
    let vue = gros_arbre();
    let ecrit = serde_json::to_value(&vue).unwrap();
    // Relu par les vrais types du contrat (`deny_unknown_fields` : un champ de trop échoue)…
    let relu: ViewDesc = serde_json::from_value(ecrit.clone()).expect("le contrat relit l'arbre");
    // …et réécrit à l'identique : rien n'est perdu ni ajouté.
    assert_eq!(serde_json::to_value(&relu).unwrap(), ecrit);
}

#[test]
fn chaque_surface_et_chaque_ton_se_relisent() {
    for s in [
        Surface::Sidebar,
        Surface::Main,
        Surface::Panel,
        Surface::Header,
        Surface::Chat,
        Surface::Widget,
        Surface::LockWidget,
        Surface::Activity,
        Surface::Island,
    ] {
        let racine = Node::banner(Tone::Error, text!("ok"));
        let v = View::new(ident!("v"), s, racine).unwrap();
        let ecrit = serde_json::to_value(&v).unwrap();
        let relu: ViewDesc = serde_json::from_value(ecrit.clone()).unwrap();
        assert_eq!(serde_json::to_value(&relu).unwrap(), ecrit, "{s:?}");
    }
    for op in [
        Op::Eq,
        Op::Ne,
        Op::Gt,
        Op::Gte,
        Op::Lt,
        Op::Lte,
        Op::Contains,
        Op::StartsWith,
    ] {
        let b = Brick::text(ident!("e"), ident!("f")).filter(Filter::cmp(ident!("f"), op, 1));
        let v = View::new(ident!("v"), Surface::Main, Node::leaf(b)).unwrap();
        let ecrit = serde_json::to_value(&v).unwrap();
        let relu: ViewDesc = serde_json::from_value(ecrit.clone()).unwrap();
        assert_eq!(serde_json::to_value(&relu).unwrap(), ecrit, "{op:?}");
    }
}

#[test]
fn un_nom_ou_un_texte_invalide_est_refuse_sans_panic() {
    for mauvais in ["", "Tache", "1a", "a-b", "a b", "é"] {
        assert_eq!(
            Ident::new(mauvais),
            Err(ViewError::BadIdent(mauvais.into()))
        );
    }
    assert!(Ident::new("tache_2").is_ok());
    assert_eq!(Text::new(""), Err(ViewError::BlankText));
    assert_eq!(Text::new(" \n\t"), Err(ViewError::BlankText));
    assert!(Text::new("ok").is_ok());
}

fn imbrique(niveaux: usize) -> Node {
    let mut n = Node::banner(Tone::Info, text!("fond"));
    for _ in 0..niveaux {
        n = Node::stack(n, []);
    }
    n
}

#[test]
fn la_profondeur_est_bornee_comme_le_contrat() {
    // La feuille est au niveau MAX+1 quand on empile MAX conteneurs : refusé ; MAX-1 passe.
    let ok = View::new(ident!("v"), Surface::Main, imbrique(MAX_VIEW_DEPTH - 1)).unwrap();
    let ecrit = serde_json::to_value(&ok).unwrap();
    // Le contrat accepte exactement ce que le SDK accepte : même borne.
    let relu: ViewDesc = serde_json::from_value(ecrit).unwrap();
    let hote = this_descriptor::ModuleDescriptor::from_json(
        &json!({"name":"m","version":"0.1.0","views":[relu]}).to_string(),
    )
    .unwrap();
    assert!(
        this_descriptor::views::issues(&hote).is_empty(),
        "{:?}",
        this_descriptor::views::issues(&hote)
    );
    assert_eq!(
        View::new(ident!("v"), Surface::Main, imbrique(MAX_VIEW_DEPTH + 1)).unwrap_err(),
        ViewError::TooDeep
    );
}

#[test]
fn deux_onglets_homonymes_sont_refuses() {
    let tab = || Tab::new(text!("A"), Node::banner(Tone::Info, text!("x")), []);
    let r = View::new(ident!("v"), Surface::Main, Node::tabs(tab(), [tab()]));
    assert_eq!(r.unwrap_err(), ViewError::DuplicateTab("A".into()));
}

#[test]
fn l_en_tete_refuse_ce_que_le_contrat_refuse() {
    let liste = Node::leaf(Brick::list(ident!("e"), ident!("f")));
    assert_eq!(
        View::new(ident!("v"), Surface::Header, liste).unwrap_err(),
        ViewError::HeaderBrick("list")
    );
    assert!(
        View::new(
            ident!("v"),
            Surface::Header,
            Node::banner(Tone::Info, text!("ok"))
        )
        .is_ok()
    );
}

/// Un arbre d'erreur est lui aussi conforme : l'hôte l'accepte et le dessine.
fn conforme(octets: &[u8]) -> Vec<ViewDesc> {
    serde_json::from_slice(octets).expect("tableau de vues conforme au contrat")
}

#[test]
fn une_requete_illisible_devient_une_vue_d_erreur() {
    let sortie = rt::render_bytes(b"{pas du json", |_| -> View {
        unreachable!("jamais appelée")
    });
    let vues = conforme(&sortie);
    assert_eq!(vues.len(), 1);
    assert_eq!(vues[0].name, "render_error");
    assert!(String::from_utf8(sortie).unwrap().contains("illisible"));
}

#[test]
fn un_rendu_en_echec_devient_une_vue_d_erreur() {
    let sortie = rt::render_bytes(b"{}", |_| Err::<View, _>(ViewError::NoView));
    assert!(
        String::from_utf8(sortie.clone())
            .unwrap()
            .contains("aucune vue rendue")
    );
    conforme(&sortie);
    let vide = rt::render_bytes(b"{}", |_| Vec::<View>::new());
    assert!(
        String::from_utf8(vide.clone())
            .unwrap()
            .contains("aucune vue")
    );
    conforme(&vide);
}

#[test]
fn une_requete_vide_est_un_null_et_un_message_blanc_ne_casse_rien() {
    let sortie = rt::render_bytes(b"", |v: Value| {
        assert!(v.is_null());
        Err::<View, _>("   ")
    });
    // Le message est blanc : la bannière retombe sur un texte par défaut, toujours valide.
    let vues = conforme(&sortie);
    assert_eq!(vues[0].name, "render_error");
}

#[test]
fn un_rendu_reussi_rend_un_tableau_de_vues() {
    let v = View::new(
        ident!("a"),
        Surface::Main,
        Node::banner(Tone::Info, text!("x")),
    )
    .unwrap();
    let sortie = rt::render_bytes(b"{}", |_| v.clone());
    assert_eq!(conforme(&sortie)[0].name, "a");
}
