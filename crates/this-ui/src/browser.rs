//! # Le tableau d'entités, générique
//!
//! Le kit décide **comment** une ligne se dessine (colonnes, alignement, en-tête collant,
//! version « cartes » sous 700 px) ; l'application décide **ce qu'il y a dedans** en
//! produisant des [`Cell`]. Cette séparation est ce qui rend le composant transposable :
//! un autre module fournit ses colonnes et ses cellules, sans toucher au kit.
//!
//! ```ignore
//! let columns = vec![Column { label: "Nom".into(), numeric: false }];
//! let rows = vec![Row { id: "…".into(), cells: vec![Cell::Text("Casque".into())] }];
//! view! { <EntityTable columns=columns rows=rows action_label="Ouvrir" on_open=cb /> }
//! ```

use leptos::prelude::*;

use crate::design::{Tone, badge, meter};

/// Le crayon de l'action de ligne : générique, il ne dépend d'aucune app.
const EDIT: &str = r#"<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 20h4l10-10a2.5 2.5 0 0 0-3.5-3.5L4.5 16.5 4 20Z"/><path d="m13.5 7 3.5 3.5"/></svg>"#;

/// Le contenu d'une cellule. Chaque variante correspond à une façon de lire une valeur.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Text(String),
    Muted(String),
    /// Un montant, aligné à droite en chiffres tabulaires.
    Amount(String),
    /// Une pastille d'état : point + texte.
    Badge {
        text: String,
        tone: Tone,
    },
    /// Une jauge (stock, progression…).
    Meter {
        value: f64,
        max: f64,
        low: f64,
    },
    /// Une référence à une autre entité, rendue sans décoration.
    Reference(String),
    /// Une personne : pastille d'initiales + nom.
    Person {
        name: String,
        initials: String,
    },
    Empty,
}

impl Cell {
    fn render(&self) -> AnyView {
        match self {
            Cell::Text(text) => view! { <span>{text.clone()}</span> }.into_any(),
            Cell::Muted(text) => view! { <span class="muted">{text.clone()}</span> }.into_any(),
            Cell::Amount(text) => view! { <span class="amount">{text.clone()}</span> }.into_any(),
            Cell::Badge { text, tone } => badge(text.clone(), *tone),
            Cell::Meter { value, max, low } => meter(*value, *max, *low),
            Cell::Reference(text) => view! { <span class="ref">{text.clone()}</span> }.into_any(),
            Cell::Person { name, initials } => view! {
                <span class="who">
                    <span class="avatar">{initials.clone()}</span>
                    <span>{name.clone()}</span>
                </span>
            }
            .into_any(),
            Cell::Empty => view! { <span>"—"</span> }.into_any(),
        }
    }
}

/// Une colonne : son libellé et son alignement. Le kit ne connaît pas les champs.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub label: String,
    pub numeric: bool,
}

/// Une ligne prête à afficher.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Identifiant de l'enregistrement, transmis à `on_open`.
    pub id: String,
    pub cells: Vec<Cell>,
}

/// Le tableau : en-tête collant, montants alignés, cartes étiquetées en mobile.
#[component]
pub fn EntityTable(
    columns: Vec<Column>,
    rows: Vec<Row>,
    /// Libellé de l'action de ligne (« Ouvrir », « Détails »…).
    #[prop(into)]
    action_label: String,
    on_open: Callback<String>,
) -> impl IntoView {
    let headers = columns
        .iter()
        .map(|column| {
            let class = if column.numeric { "num" } else { "" };
            view! { <th class=class>{column.label.clone()}</th> }
        })
        .collect_view();

    let labels: Vec<String> = columns.iter().map(|c| c.label.clone()).collect();

    let body = rows
        .into_iter()
        .map(|row| {
            let id_for_click = row.id.clone();
            let cells = row
                .cells
                .into_iter()
                .zip(labels.iter())
                .map(|(cell, label)| {
                    let class = match &cell {
                        Cell::Amount(_) => "num",
                        _ => "",
                    };
                    view! { <td class=class data-label=label.clone()>{cell.render()}</td> }
                })
                .collect_view();
            view! {
                <tr>
                    {cells}
                    <td class="col-actions" data-label="">
                        <button class="btn ghost small" aria-label="Ouvrir l'enregistrement"
                            on:click=move |_| on_open.run(id_for_click.clone())>
                            <span inner_html=EDIT></span>
                            {action_label.clone()}
                        </button>
                    </td>
                </tr>
            }
        })
        .collect_view();

    view! {
        <div class="table-wrap">
            <table class="grid">
                <thead>
                    <tr>{headers}<th class="col-actions"></th></tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </div>
    }
}
