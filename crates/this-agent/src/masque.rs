//! # Masquer les secrets d'un texte
//!
//! Une clé d'API, un jeton, une clé privée ou un mot de passe recopiés dans un message,
//! un résultat d'outil ou un motif d'arrêt ne doivent pas atterrir tels quels dans le
//! graphe des runs : ce qui y est écrit est relu, rejoué au modèle, montré à l'écran.
//!
//! Ce module est **pur** (aucune I/O, aucune dépendance hors `serde_json`) et compile en
//! wasm comme en natif : c'est la seule table des motifs, partagée par la page (qui masque
//! avant d'envoyer) et par le magasin (qui masque avant d'écrire, cf.
//! [`crate::graph::run_nodes_with`]).
//!
//! La reconnaissance est volontairement étroite : un préfixe connu (`sk-`, `AKIA`,
//! `ghp_`…) suivi d'une suite assez longue, pris en début de mot ; un en-tête `Bearer` ;
//! un bloc `PRIVATE KEY` ; la valeur d'un champ nommé comme un mot de passe. Un UUID, un
//! SHA-256 ou un base64 ordinaire ressortent intacts.

use serde_json::{Value, json};

/// Ce qui remplace un secret dans tout texte gardé ou répété.
pub const MASQUE: &str = "[secret masqué]";

/// Les noms de champ dont la valeur est un mot de passe (comparés en minuscules ASCII).
const CLES_MOT_DE_PASSE: [&str; 4] = ["password", "passwd", "mot_de_passe", "mot de passe"];

/// Un caractère de jeton : ce qui compose une clé d'API.
fn jeton(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// La longueur, en octets, de la suite de `debut` faite de caractères ASCII acceptés.
fn suite(texte: &str, debut: usize, accepte: impl Fn(char) -> bool) -> usize {
    texte[debut..]
        .chars()
        .take_while(|c| c.is_ascii() && accepte(*c))
        .count()
}

/// Vrai si `i` commence un mot : le caractère d'avant n'est pas `dans_le_mot`. Sans cette
/// borne, un `sk-`, un `AKIA` ou un `ghp_` pris au milieu d'un base64 (url-safe) serait lu
/// comme une clé.
fn borne(texte: &str, i: usize, dans_le_mot: impl Fn(char) -> bool) -> bool {
    texte[..i]
        .chars()
        .next_back()
        .is_none_or(|c| !dans_le_mot(c))
}

/// Les secrets reconnus dans `texte` : `(début, fin, motif)` en octets, triés et fusionnés.
/// C'est la SEULE table des motifs : [`secret_dans`] (le refus) et [`masquer`] (le
/// masquage) la lisent toutes deux.
fn secrets(texte: &str) -> Vec<(usize, usize, &'static str)> {
    let mut trouves: Vec<(usize, usize, &'static str)> = Vec::new();
    let prefixes: [(&str, usize, fn(char) -> bool, &'static str); 7] = [
        ("sk-", 20, jeton, "clé d'API sk-…"),
        (
            "AKIA",
            16,
            |c| c.is_ascii_uppercase() || c.is_ascii_digit(),
            "clé AWS AKIA…",
        ),
        ("ghp_", 20, jeton, "jeton GitHub"),
        ("gho_", 20, jeton, "jeton GitHub"),
        ("ghs_", 20, jeton, "jeton GitHub"),
        ("ghu_", 20, jeton, "jeton GitHub"),
        ("github_pat_", 20, jeton, "jeton GitHub"),
    ];
    for (prefixe, minimum, accepte, motif) in prefixes {
        for (i, _) in texte.match_indices(prefixe) {
            let n = suite(texte, i + prefixe.len(), accepte);
            if n >= minimum && borne(texte, i, jeton) {
                trouves.push((i, i + prefixe.len() + n, motif));
            }
        }
    }
    // `Bearer <jeton>` : l'en-tête d'autorisation recopié d'une commande curl.
    for prefixe in ["Bearer ", "bearer "] {
        for (i, _) in texte.match_indices(prefixe) {
            let debut = i + prefixe.len();
            let n = suite(texte, debut, |c| {
                jeton(c) || matches!(c, '.' | '~' | '+' | '/' | '=')
            });
            if n >= 20 && borne(texte, i, |c| c.is_ascii_alphanumeric()) {
                trouves.push((debut, debut + n, "jeton Bearer"));
            }
        }
    }
    for (i, _) in texte.match_indices("-----BEGIN") {
        if !texte[i..].contains("PRIVATE KEY") {
            continue;
        }
        // Jusqu'à la ligne `-----END … -----` ; sans elle, jusqu'à la fin du texte.
        let fin = texte[i..]
            .find("-----END")
            .and_then(|j| {
                let apres = i + j + "-----END".len();
                texte[apres..]
                    .find("-----")
                    .map(|k| apres + k + "-----".len())
            })
            .unwrap_or(texte.len());
        trouves.push((i, fin, "clé privée"));
    }
    // Minuscules ASCII : les positions d'octets restent celles de `texte`.
    let bas = texte.to_ascii_lowercase();
    for cle in CLES_MOT_DE_PASSE {
        for (i, _) in bas.match_indices(cle) {
            if !borne(&bas, i, |c| c.is_ascii_alphanumeric()) {
                continue;
            }
            if let Some((debut, fin)) = valeur_apres_cle(&bas, i + cle.len()) {
                trouves.push((debut, fin, "mot de passe"));
            }
        }
    }
    trouves.sort_by_key(|&(debut, fin, _)| (debut, std::cmp::Reverse(fin)));
    let mut fusionnes: Vec<(usize, usize, &'static str)> = Vec::new();
    for (debut, fin, motif) in trouves {
        match fusionnes.last_mut() {
            Some(dernier) if debut < dernier.1 => dernier.1 = dernier.1.max(fin),
            _ => fusionnes.push((debut, fin, motif)),
        }
    }
    fusionnes
}

/// La valeur qui suit un nom de champ : `= x`, `: x`, `": "x"` (JSON), entre guillemets ou
/// jusqu'au premier blanc ou séparateur. `None` sans `=`/`:` ou sans valeur.
fn valeur_apres_cle(texte: &str, apres: usize) -> Option<(usize, usize)> {
    let blancs = |depuis: usize| depuis + suite(texte, depuis, char::is_whitespace);
    let mut i = blancs(apres);
    if texte[i..].starts_with(['"', '\'']) {
        i = blancs(i + 1);
    }
    if !texte[i..].starts_with(['=', ':']) {
        return None;
    }
    i = blancs(i + 1);
    let reste = &texte[i..];
    let (debut, fin) = match reste.chars().next()? {
        q @ ('"' | '\'') => {
            let debut = i + 1;
            (
                debut,
                texte[debut..].find(q).map_or(texte.len(), |n| debut + n),
            )
        }
        c if c.is_whitespace() => return None,
        _ => {
            let n = reste
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, ',' | ';' | '&' | '}' | '"' | '\'')
                })
                .unwrap_or(reste.len());
            (i, i + n)
        }
    };
    // Une valeur déjà masquée n'est plus un secret : masquer est idempotent, et un texte
    // masqué repassé à `remember` n'est pas refusé.
    (fin > debut && !texte[debut..].starts_with(MASQUE)).then_some((debut, fin))
}

/// Un secret reconnaissable, sans dépendance : le nom du motif, jamais la valeur.
pub fn secret_dans(texte: &str) -> Option<&'static str> {
    secrets(texte).first().map(|&(_, _, motif)| motif)
}

/// Le texte où chaque secret reconnu par [`secret_dans`] est remplacé par [`MASQUE`]. Le
/// reste est intact : un texte sans secret ressort à l'identique. Fonction pure.
pub fn masquer(texte: &str) -> String {
    let mut sortie = String::with_capacity(texte.len());
    let mut curseur = 0;
    for (debut, fin, _) in secrets(texte) {
        sortie.push_str(&texte[curseur..debut]);
        sortie.push_str(MASQUE);
        curseur = fin;
    }
    sortie.push_str(&texte[curseur..]);
    sortie
}

/// [`masquer`] appliqué à chaque chaîne d'une valeur JSON. La forme est gardée (un objet
/// reste un objet), et la valeur d'un champ nommé comme un mot de passe (`{"password":
/// "…"}`) est masquée entière : seule, la chaîne n'aurait rien de reconnaissable.
pub fn masquer_valeur(valeur: &Value) -> Value {
    match valeur {
        Value::String(texte) => Value::String(masquer(texte)),
        Value::Array(liste) => Value::Array(liste.iter().map(masquer_valeur).collect()),
        Value::Object(objet) => Value::Object(
            objet
                .iter()
                .map(|(cle, v)| {
                    let bas = cle.to_ascii_lowercase();
                    let nommee = CLES_MOT_DE_PASSE.iter().any(|c| bas.contains(c));
                    let v = match v {
                        Value::String(s) if nommee && !s.trim().is_empty() => json!(MASQUE),
                        autre => masquer_valeur(autre),
                    };
                    (cle.clone(), v)
                })
                .collect(),
        ),
        autre => autre.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textes_ordinaires_ne_sont_pas_des_secrets() {
        for texte in [
            "le risk-score dépend du module",
            "le champ password est obligatoire",
            "AKIA est le préfixe des clés AWS",
            "-----BEGIN PUBLIC KEY-----",
        ] {
            assert_eq!(secret_dans(texte), None, "{texte}");
        }
    }

    #[test]
    fn masquer_remplace_chaque_secret_et_garde_le_reste() {
        let cas = [
            (
                "ma clé sk-ant-api03-abcdefghijklmnopqrstuvwxyz ici",
                "ma clé [secret masqué] ici",
            ),
            ("AKIAIOSFODNN7EXAMPLE pour S3", "[secret masqué] pour S3"),
            (
                "jeton ghp_abcdefghijklmnopqrstuvwxyz0123.",
                "jeton [secret masqué].",
            ),
            (
                "connexion avec password=hunter2 puis",
                "connexion avec password=[secret masqué] puis",
            ),
            ("Password : s3cret", "Password : [secret masqué]"),
            (
                r#"{"text": "le password": "hunter2", "x"#,
                r#"{"text": "le password": "[secret masqué]", "x"#,
            ),
            (
                "curl -H 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz.0123' x",
                "curl -H 'Authorization: Bearer [secret masqué]' x",
            ),
            (
                "avant -----BEGIN RSA PRIVATE KEY-----\nMIIEabc\n-----END RSA PRIVATE KEY----- après",
                "avant [secret masqué] après",
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nMIIE sans fin",
                "[secret masqué]",
            ),
        ];
        for (texte, attendu) in cas {
            assert_eq!(masquer(texte), attendu, "{texte}");
            assert!(
                secret_dans(texte).is_some(),
                "même table de motifs : {texte}"
            );
            assert_eq!(
                secret_dans(&masquer(texte)),
                None,
                "plus rien après masquage"
            );
        }
        let deux = masquer("sk-aaaaaaaaaaaaaaaaaaaaaaaa et ghp_bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(deux, "[secret masqué] et [secret masqué]");
    }

    #[test]
    fn masquer_ne_touche_ni_uuid_ni_sha256_ni_base64_de_wasm() {
        let ordinaires = [
            "note 3f2b8c1e-9a4d-4e6f-b7a1-0c9d8e7f6a5b".to_string(),
            "sha256 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".to_string(),
            // Le base64 d'un module wasm (`\0asm`), standard puis url-safe avec des `sk-`,
            // `AKIA` et `ghp_` pris AU MILIEU des jetons.
            "AGFzbQEAAAABBgFgAX8BfwMCAQAHBwEDZmliAAAKHgEcACAAQQJIBH8gAAUgAEEBaxAAIABBAmsQAGoLCw=="
                .to_string(),
            "AGFzbQEsk-AAAABBgFgAX8BfwMCAQAHBwEDZmliAAAKAKIAQQJIBH8gAAUgAEEBaxAAIABBAmsQAG9ghp_Lgw2bQEsk-AAAABBgFgAX8Bfw"
                .to_string(),
            "le risk-score dépend du module".to_string(),
            "le champ password est obligatoire".to_string(),
            "-----BEGIN PUBLIC KEY-----\nMIIB".to_string(),
        ];
        for texte in ordinaires {
            assert_eq!(masquer(&texte), texte);
            assert_eq!(secret_dans(&texte), None, "{texte}");
        }
    }

    #[test]
    fn masquer_valeur_garde_la_forme_et_masque_les_champs_mot_de_passe() {
        let v = json!({
            "text": "ma clé sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
            "db_password": "hunter2",
            "liste": ["AKIAIOSFODNN7EXAMPLE", 3, null],
            "id": "3f2b8c1e-9a4d-4e6f-b7a1-0c9d8e7f6a5b",
        });
        let m = masquer_valeur(&v);
        assert_eq!(m["text"], "ma clé [secret masqué]");
        assert_eq!(m["db_password"], MASQUE);
        assert_eq!(m["liste"], json!(["[secret masqué]", 3, null]));
        assert_eq!(m["id"], v["id"]);
        let brut = m.to_string();
        assert!(
            !brut.contains("hunter2") && !brut.contains("EXAMPLE") && !brut.contains("abcdefghij")
        );
    }
}
