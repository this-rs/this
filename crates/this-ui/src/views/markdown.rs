//! Le markdown des bulles : le strict nécessaire, sans dépendance.
//!
//! Ce module ne connaît ni Leptos ni le DOM : il prend du texte, rend du HTML. C'est
//! volontaire, et c'est ce qui permet à `cargo test` de le mesurer sur l'hôte, sans wasm.
//!
//! Il vivait dans l'app (l'Atelier) tant que le fil n'était pas déclaré. Depuis que
//! `brick:message` porte `markdown: true`, c'est le RENDU DU CONTRAT : il est ici, dans le
//! moteur de vues, pour que deux hôtes ne finissent pas par rendre deux markdowns.
//!
//! ```
//! use this_ui::views::markdown::markdown;
//! assert_eq!(markdown("**fort**"), "<p><strong>fort</strong></p>");
//! ```
//!
//! Rien n'est injecté : tout le texte est échappé **avant** d'être transformé,
//! et les seules balises écrites sont celles produites ici. On ne rend pas le
//! texte d'un modèle en lui faisant confiance.
//!
//! ## Ce qui est rendu
//!
//! Blocs de code (```), code en ligne, gras (`**`), titres (`#`, `##`, `###`),
//! listes à puces (`- `, `* `), tableaux GFM (`| a | b |` + `|---|:--:|`).
//!
//! ## Ce qui n'est PAS rendu
//!
//! Les liens, les listes numérotées, les citations, les filets : ils restent du
//! texte. Un rendu qui prétend tout connaître finit par avaler une phrase.
//!
//! ## Les tableaux
//!
//! Un tableau n'existe que si la ligne de séparation suit l'en-tête — c'est la
//! règle GFM, et elle protège le texte ordinaire : une phrase contenant une
//! barre (« soit A | B ») reste une phrase. Les alignements `:--`, `:--:`,
//! `--:` sont honorés ; une barre échappée (`\|`) ne coupe pas la cellule, et
//! les barres à l'intérieur d'un code en ligne non plus.

/// Rend le markdown d'une bulle en HTML.
pub fn markdown(texte: &str) -> String {
    let mut sortie = String::new();
    let mut dans_code = false;
    let mut dans_liste = false;
    let lignes: Vec<&str> = texte.lines().collect();
    let mut i = 0;
    while i < lignes.len() {
        let ligne = lignes[i];
        if ligne.trim_start().starts_with("```") {
            if dans_liste {
                sortie.push_str("</ul>");
                dans_liste = false;
            }
            sortie.push_str(if dans_code {
                "</code></pre>"
            } else {
                "<pre><code>"
            });
            dans_code = !dans_code;
            i += 1;
            continue;
        }
        if dans_code {
            sortie.push_str(&echappe(ligne));
            sortie.push('\n');
            i += 1;
            continue;
        }

        // Un tableau : une ligne d'en-tête qui porte des barres, suivie d'une
        // ligne de séparation de même largeur. Sans séparation, une barre est
        // du texte — c'est le cas qui trompait : toute ligne devenait un
        // paragraphe et les barres restaient visibles.
        if let Some(entete) = cellules(ligne) {
            let suivante = lignes.get(i + 1).copied();
            // Voie stricte (GFM) : une ligne de séparation donne le tableau ET ses alignements.
            let alignes = suivante
                .and_then(alignements)
                .filter(|aligns| entete.len() == aligns.len());
            // Voie TOLÉRANTE, mesurée : un moteur écrit souvent l'en-tête et les rangées SANS
            // la ligne de séparation. Le GFM strict dit « ce n'est pas un tableau », et la
            // personne voit des barres brutes au lieu d'une grille — c'est exactement le
            // défaut rapporté. Deux lignes de même largeur (au moins deux colonnes) sont un
            // tableau : c'est ce que l'auteur voulait dire.
            let tolere = alignes.is_none()
                && entete.len() >= 2
                && suivante
                    .and_then(cellules)
                    .is_some_and(|rangee| rangee.len() == entete.len());
            if alignes.is_some() || tolere {
                let aligns = alignes.unwrap_or_else(|| vec![Alignement::Defaut; entete.len()]);
                if dans_liste {
                    sortie.push_str("</ul>");
                    dans_liste = false;
                }
                // En voie tolérante la ligne suivante est une RANGÉE, pas une séparation :
                // elle entre dans le corps.
                i += if tolere { 1 } else { 2 };
                let mut corps: Vec<Vec<String>> = Vec::new();
                while i < lignes.len() {
                    let brute = lignes[i];
                    if brute.trim().is_empty() || brute.trim_start().starts_with("```") {
                        break;
                    }
                    let Some(rangee) = cellules(brute) else {
                        break;
                    };
                    corps.push(rangee);
                    i += 1;
                }
                sortie.push_str(&rend_table(&entete, &aligns, &corps));
                continue;
            }
        }

        let nu = ligne.trim_end();
        let est_puce = nu.trim_start().starts_with("- ") || nu.trim_start().starts_with("* ");
        if est_puce && !dans_liste {
            sortie.push_str("<ul>");
            dans_liste = true;
        } else if !est_puce && dans_liste {
            sortie.push_str("</ul>");
            dans_liste = false;
        }
        if nu.is_empty() {
            i += 1;
            continue;
        }
        if est_puce {
            let contenu = nu.trim_start()[2..].to_string();
            sortie.push_str(&format!("<li>{}</li>", en_ligne(&echappe(&contenu))));
        } else if let Some(titre) = nu.strip_prefix("### ") {
            sortie.push_str(&format!("<h4>{}</h4>", en_ligne(&echappe(titre))));
        } else if let Some(titre) = nu.strip_prefix("## ") {
            sortie.push_str(&format!("<h3>{}</h3>", en_ligne(&echappe(titre))));
        } else if let Some(titre) = nu.strip_prefix("# ") {
            sortie.push_str(&format!("<h2>{}</h2>", en_ligne(&echappe(titre))));
        } else {
            sortie.push_str(&format!("<p>{}</p>", en_ligne(&echappe(nu))));
        }
        i += 1;
    }
    if dans_liste {
        sortie.push_str("</ul>");
    }
    if dans_code {
        sortie.push_str("</code></pre>");
    }
    sortie
}

/// L'alignement d'une colonne, lu sur la ligne de séparation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Alignement {
    Defaut,
    Gauche,
    Centre,
    Droite,
}

/// L'attribut de classe d'une cellule, vide quand l'alignement est celui par
/// défaut — une classe inutile dans le HTML rend le DOM plus dur à lire.
fn classe(alignement: Option<&Alignement>) -> &'static str {
    match alignement {
        Some(Alignement::Gauche) => " class=\"a-gauche\"",
        Some(Alignement::Centre) => " class=\"a-centre\"",
        Some(Alignement::Droite) => " class=\"a-droite\"",
        _ => "",
    }
}

/// Rendu d'un tableau complet. Une rangée courte est complétée de cellules
/// vides, une rangée longue est coupée : le nombre de colonnes est celui de
/// l'en-tête, sinon la grille se décale et le tableau devient illisible.
fn rend_table(entete: &[String], aligns: &[Alignement], corps: &[Vec<String>]) -> String {
    let vide = String::new();
    let mut out = String::from("<table><thead><tr>");
    for (n, cellule) in entete.iter().enumerate() {
        out.push_str(&format!(
            "<th{}>{}</th>",
            classe(aligns.get(n)),
            en_ligne(&echappe(cellule))
        ));
    }
    out.push_str("</tr></thead>");
    if !corps.is_empty() {
        out.push_str("<tbody>");
        for rangee in corps {
            out.push_str("<tr>");
            for n in 0..entete.len() {
                let cellule = rangee.get(n).unwrap_or(&vide);
                out.push_str(&format!(
                    "<td{}>{}</td>",
                    classe(aligns.get(n)),
                    en_ligne(&echappe(cellule))
                ));
            }
            out.push_str("</tr>");
        }
        out.push_str("</tbody>");
    }
    out.push_str("</table>");
    out
}

/// Les cellules d'une ligne de tableau, ou `None` si la ligne n'a aucune barre.
///
/// Une barre en tête ou en queue borde le tableau, elle ne délimite pas une
/// cellule vide. `\|` est une barre littérale ; les barres à l'intérieur d'un
/// code en ligne (`a | b`) ne coupent pas la cellule.
fn cellules(ligne: &str) -> Option<Vec<String>> {
    let nu = ligne.trim();
    if !nu.contains('|') {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    let mut courant = String::new();
    let mut dans_code = false;
    let mut echappement = false;
    let mut barres = 0usize;
    for c in nu.chars() {
        if echappement {
            courant.push(c);
            echappement = false;
            continue;
        }
        match c {
            '\\' => echappement = true,
            '`' => {
                dans_code = !dans_code;
                courant.push(c);
            }
            '|' if !dans_code => {
                barres += 1;
                out.push(courant.trim().to_string());
                courant.clear();
            }
            _ => courant.push(c),
        }
    }
    out.push(courant.trim().to_string());
    if barres == 0 {
        return None;
    }
    if nu.starts_with('|') {
        out.remove(0);
    }
    if nu.ends_with('|') {
        out.pop();
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Les alignements d'une ligne de séparation (`|---|:--:|--:|`), ou `None` si
/// la ligne n'en est pas une : chaque cellule doit être au moins un tiret,
/// éventuellement bordé de deux-points.
fn alignements(ligne: &str) -> Option<Vec<Alignement>> {
    let brut = cellules(ligne)?;
    let mut out = Vec::with_capacity(brut.len());
    for cellule in brut {
        let c = cellule.trim();
        let gauche = c.starts_with(':');
        let droite = c.ends_with(':');
        let tirets = c
            .trim_start_matches(':')
            .trim_end_matches(':')
            .replace(' ', "");
        if tirets.is_empty() || !tirets.chars().all(|c| c == '-') {
            return None;
        }
        out.push(match (gauche, droite) {
            (true, true) => Alignement::Centre,
            (true, false) => Alignement::Gauche,
            (false, true) => Alignement::Droite,
            (false, false) => Alignement::Defaut,
        });
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Échappe le HTML d'un texte : c'est fait avant tout le reste, donc rien de
/// ce qu'écrit un modèle ne peut devenir une balise.
fn echappe(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Le markdown d'une seule ligne : code en ligne d'abord (son contenu ne doit
/// pas être réinterprété), puis gras.
fn en_ligne(s: &str) -> String {
    let mut out = String::new();
    let mut reste = s;
    while let Some(debut) = reste.find('`') {
        out.push_str(&gras_et_italique(&reste[..debut]));
        match reste[debut + 1..].find('`') {
            Some(fin) => {
                out.push_str("<code>");
                out.push_str(&reste[debut + 1..debut + 1 + fin]);
                out.push_str("</code>");
                reste = &reste[debut + 1 + fin + 1..];
            }
            None => {
                out.push('`');
                reste = &reste[debut + 1..];
            }
        }
    }
    out.push_str(&gras_et_italique(reste));
    out
}

/// Le gras (`**`) — rien d'autre pour l'instant : l'italique par `*` seul
/// avalait les astérisques d'une expression écrite en clair.
fn gras_et_italique(s: &str) -> String {
    let mut out = String::new();
    let mut reste = s;
    while let Some(debut) = reste.find("**") {
        out.push_str(&reste[..debut]);
        match reste[debut + 2..].find("**") {
            Some(fin) => {
                out.push_str("<strong>");
                out.push_str(&reste[debut + 2..debut + 2 + fin]);
                out.push_str("</strong>");
                reste = &reste[debut + 2 + fin + 2..];
            }
            None => {
                out.push_str("**");
                reste = &reste[debut + 2..];
            }
        }
    }
    out.push_str(reste);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sans_espaces(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn un_tableau_devient_une_table() {
        let html =
            markdown("| nom | etat |\n|---|---|\n| vendredi | pret |\n| lundi | en cours |\n");
        assert!(html.contains("<table><thead><tr>"), "{html}");
        assert!(html.contains("<th>nom</th>"), "{html}");
        assert!(html.contains("<td>vendredi</td>"), "{html}");
        assert!(html.contains("<td>en cours</td>"), "{html}");
        assert!(
            !html.contains("<p>|"),
            "les barres restent du texte : {html}"
        );
    }

    #[test]
    fn un_tableau_sans_barres_de_bord_est_un_tableau() {
        let html = markdown("nom | etat\n--- | ---\nA | prêt\n");
        assert!(html.contains("<th>nom</th>"), "{html}");
        assert!(html.contains("<td>prêt</td>"), "{html}");
    }

    #[test]
    fn un_tableau_sans_ligne_de_separation_est_quand_meme_un_tableau() {
        // Mesure : un moteur écrit souvent l'en-tête et les rangées sans `|---|`. Le GFM
        // strict affichait des barres brutes ; on lit la grille que l'auteur voulait.
        let html = markdown("| Capacité | État |\n| bluetooth | connecte |\n| nfc | écrit |");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<th>Capacité</th>"), "{html}");
        assert!(html.contains("<td>connecte</td>"), "{html}");
        assert!(!html.contains("| bluetooth"), "{html}");
        // Le tableau se termine à la première ligne qui n'est pas une rangée.
        let suite = markdown("| a | b |\n| 1 | 2 |\nfin");
        assert!(suite.contains("</table><p>fin</p>"), "{suite}");
    }

    #[test]
    fn une_seule_ligne_a_barres_reste_du_texte() {
        // Une ligne seule ne fait pas une grille : sans deuxième rangée, c'est une phrase.
        let html = markdown("| a | b |");
        assert!(!html.contains("<table"), "{html}");
    }

    #[test]
    fn une_barre_seule_reste_du_texte() {
        // Pas de ligne de séparation : c'est une phrase, pas un tableau.
        let html = markdown("soit A | B dans le plan\n");
        assert!(!html.contains("<table"), "{html}");
        assert!(html.contains("A | B"), "{html}");
    }

    #[test]
    fn les_alignements_sont_lus() {
        let html = markdown("| g | c | d |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n");
        assert!(html.contains("<th class=\"a-gauche\">g</th>"), "{html}");
        assert!(html.contains("<th class=\"a-centre\">c</th>"), "{html}");
        assert!(html.contains("<td class=\"a-droite\">3</td>"), "{html}");
    }

    #[test]
    fn une_rangee_courte_est_completee_et_une_longue_coupee() {
        let html = markdown("| a | b |\n|---|---|\n| 1 |\n| 2 | 3 | 4 |\n");
        assert!(html.contains("<tr><td>1</td><td></td></tr>"), "{html}");
        assert!(html.contains("<tr><td>2</td><td>3</td></tr>"), "{html}");
        assert!(!html.contains("<td>4</td>"), "{html}");
    }

    #[test]
    fn les_barres_echappees_et_le_code_ne_coupent_pas() {
        let html = markdown("| x | y |\n|---|---|\n| `a | b` | c \\| d |\n");
        assert!(html.contains("<td><code>a | b</code></td>"), "{html}");
        assert!(html.contains("<td>c | d</td>"), "{html}");
    }

    #[test]
    fn le_gras_et_le_code_marchent_dans_une_cellule() {
        let html = markdown("| a |\n|---|\n| **fort** et `code` |\n");
        assert!(
            html.contains("<td><strong>fort</strong> et <code>code</code></td>"),
            "{html}"
        );
    }

    #[test]
    fn un_tableau_dans_un_bloc_de_code_reste_litteral() {
        let html = markdown("```\n| a | b |\n|---|---|\n```\n");
        assert!(!html.contains("<table"), "{html}");
        assert!(html.contains("<pre><code>"), "{html}");
        assert!(html.contains("| a | b |"), "{html}");
    }

    #[test]
    fn un_tableau_ferme_la_liste_qui_le_precede() {
        let html = markdown("- un\n| a |\n|---|\n| 1 |\n");
        assert!(html.contains("</ul><table>"), "{html}");
    }

    #[test]
    fn une_ligne_vide_termine_le_tableau() {
        let html = markdown("| a |\n|---|\n| 1 |\n\naprès le tableau\n");
        assert!(html.contains("</table><p>après le tableau</p>"), "{html}");
    }

    #[test]
    fn le_html_ecrit_par_le_modele_est_echappe_meme_dans_une_cellule() {
        let html = markdown("| a |\n|---|\n| <script>alert(1)</script> |\n");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn les_balises_de_tableau_sont_equilibrees() {
        let html = markdown("| a | b |\n|:-:|--:|\n| 1 | 2 |\n| 3 | 4 |\n");
        let ouvrants = html.matches("<tr>").count();
        let fermants = html.matches("</tr>").count();
        assert_eq!(ouvrants, fermants, "{html}");
        assert_eq!(html.matches("<table>").count(), 1, "{html}");
        assert_eq!(html.matches("</table>").count(), 1, "{html}");
        assert_eq!(
            html.matches("<tbody>").count(),
            html.matches("</tbody>").count()
        );
    }

    #[test]
    fn une_seule_colonne_est_permise() {
        let html = markdown("| a |\n|---|\n| 1 |\n");
        assert!(html.contains("<th>a</th>"), "{html}");
        assert!(html.contains("<td>1</td>"), "{html}");
    }

    #[test]
    fn un_separateur_invalide_n_aligne_rien() {
        // `| : |` n'est pas une ligne de séparation : elle ne donne aucun alignement. Depuis
        // la tolérance du 08/11, le bloc reste un TABLEAU (deux rangées de deux cellules) —
        // mais la cellule `:` est une donnée, pas une consigne d'alignement.
        let html = markdown("| a | b |\n| : | x |\n");
        assert!(html.contains("<table>"), "{html}");
        assert!(
            !html.contains("a-gauche") && !html.contains("a-centre"),
            "{html}"
        );
        assert!(html.contains("<td>:</td>"), "{html}");
    }

    #[test]
    fn rien_ne_disparait_du_texte() {
        // Deux paragraphes ordinaires, un titre, une puce : le rendu de base.
        let html = sans_espaces(&markdown("# Titre\n\nune phrase\n- puce\n"));
        assert!(html.contains("<h2>Titre</h2>"), "{html}");
        assert!(html.contains("<p>unephrase</p>"), "{html}");
        assert!(html.contains("<li>puce</li>"), "{html}");
    }
}
