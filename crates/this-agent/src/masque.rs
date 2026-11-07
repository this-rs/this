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
//! un bloc `PRIVATE KEY` ; la valeur littérale d'un champ nommé comme un secret. Un UUID, un
//! SHA-256 ou un base64 ordinaire ressortent intacts.

use serde_json::{Value, json};

/// Ce qui remplace un secret dans tout texte gardé ou répété.
pub const MASQUE: &str = "[secret masqué]";

/// Les noms de champ dont la valeur est un secret (comparés en minuscules ASCII) : mot de
/// passe, secret partagé, jeton, clé d'API.
const CLES_MOT_DE_PASSE: [&str; 9] = [
    "password",
    "passwd",
    "pwd",
    "mot_de_passe",
    "mot de passe",
    "secret",
    "token",
    "api_key",
    "apikey",
];

/// Les mots qui, après `password =`/`password:`, ne sont pas une valeur : des constantes du
/// langage et des noms de types primitifs.
const PAS_DES_VALEURS: [&str; 22] = [
    "null",
    "true",
    "false",
    "none",
    "nil",
    "undefined",
    "str",
    "string",
    "bool",
    "char",
    "int",
    "number",
    "any",
    "object",
    "bytes",
    "i32",
    "i64",
    "u8",
    "u32",
    "u64",
    "usize",
    "f64",
];

/// Vrai si un nom de champ JSON désigne un secret : le nom exact (`password`) ou un suffixe
/// séparé (`db_password`, `x-api-key`). `passwordHint` ou `password_policy` n'en sont pas :
/// pas de `contains`.
fn champ_secret(nom: &str) -> bool {
    let nom = nom.to_ascii_lowercase().replace(['-', ' ', '.'], "_");
    CLES_MOT_DE_PASSE.iter().any(|cle| {
        let cle = cle.replace(' ', "_");
        nom == cle || nom.ends_with(&format!("_{cle}"))
    })
}

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

/// Les sauts de ligne qu'une clé recopiée peut traverser : réels, ou échappés dans un JSON.
const COUPURES: [&str; 4] = ["\r\n", "\n", "\\r\\n", "\\n"];

/// Comme [`suite`], mais une clé coupée par un saut de ligne (copie d'un terminal étroit,
/// `sk-ant-api03-abcdefgh⏎ijkl…`) est suivie au-delà : `(octets couverts, caractères de
/// jeton)`. Le premier mot de la ligne suivante est pris avec elle — masquer un mot de trop
/// vaut mieux que laisser la fin d'une clé.
fn suite_coupee(texte: &str, debut: usize, accepte: fn(char) -> bool) -> (usize, usize) {
    let mut fin = debut;
    let mut compte = 0;
    loop {
        let n = suite(texte, fin, accepte);
        fin += n;
        compte += n;
        if n == 0 {
            break;
        }
        let pont = COUPURES.iter().find_map(|coupure| {
            let apres = fin + coupure.len();
            (texte[fin..].starts_with(coupure) && suite(texte, apres, accepte) > 0).then_some(apres)
        });
        match pont {
            Some(apres) => fin = apres,
            None => break,
        }
    }
    (fin - debut, compte)
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

/// Un motif de secret : préfixe, longueur minimale du corps, test d'un caractère du corps, libellé.
type Motif = (&'static str, usize, fn(char) -> bool, &'static str);

/// Les secrets reconnus dans `texte` : `(début, fin, motif)` en octets, triés et fusionnés.
/// C'est la SEULE table des motifs : [`secret_dans`] (le refus) et [`masquer`] (le
/// masquage) la lisent toutes deux.
fn secrets(texte: &str) -> Vec<(usize, usize, &'static str)> {
    let mut trouves: Vec<(usize, usize, &'static str)> = Vec::new();
    let prefixes: [Motif; 7] = [
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
            // `borne` : le caractère d'avant n'est pas de jeton — un `=` (`cle=sk-…`), un
            // blanc ou un guillemet ouvrent bien un mot.
            let (octets, n) = suite_coupee(texte, i + prefixe.len(), accepte);
            if n >= minimum && borne(texte, i, jeton) {
                trouves.push((i, i + prefixe.len() + octets, motif));
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
        // « PRIVATE KEY » sur la ligne même du BEGIN : un `BEGIN CERTIFICATE` suivi, plus
        // loin, d'une autre clé n'est pas une clé privée.
        let ligne = &texte[i..];
        let ligne = COUPURES
            .iter()
            .filter_map(|coupure| ligne.find(coupure))
            .min()
            .map_or(ligne, |n| &ligne[..n]);
        if !ligne.contains("PRIVATE KEY") {
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
            if let Some((debut, fin)) = valeur_apres_cle(texte, i + cle.len()) {
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

/// Vrai si `mot` est un chemin d'identifiants (`self.password`, `cfg.db.pwd`) : un accès à un
/// champ, pas un littéral.
fn acces_de_champ(mot: &str) -> bool {
    let morceaux: Vec<&str> = mot.split('.').collect();
    morceaux.len() > 1
        && morceaux.iter().all(|m| {
            m.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// La valeur LITTÉRALE qui suit un nom de champ : `= x`, `: x`, `": "x"` (JSON), `\": \"x\"`
/// (JSON échappé), entre guillemets ou jusqu'au premier blanc ou séparateur. `None` sans
/// `=`/`:`, sans valeur, ou quand ce qui suit est du code : comparaison (`==`), appel
/// (`get_password()`), constante (`null`, `true`), type (`String`, `&str`), accès de champ
/// (`self.password`). Hors guillemets et hors forme serrée `cle=valeur`, la valeur doit porter
/// un chiffre ou un signe : « le mot de passe : oublié » n'est pas un secret, `Password :
/// s3cret` en est un.
fn valeur_apres_cle(texte: &str, apres: usize) -> Option<(usize, usize)> {
    let blancs = |depuis: usize| depuis + suite(texte, depuis, char::is_whitespace);
    let mut i = apres;
    // Le guillemet fermant le nom (`"password"` ou `\"password\"`), collé au nom.
    if texte[i..].starts_with("\\\"") {
        i += 2;
    } else if texte[i..].starts_with(['"', '\'']) {
        i += 1;
    }
    let colle_avant = i == apres;
    i = blancs(i);
    let colle_avant = colle_avant && i == apres;
    let separateur = texte[i..].chars().next()?;
    match separateur {
        '=' if texte[i + 1..].starts_with(['=', '~']) => return None,
        ':' if texte[i + 1..].starts_with(':') => return None,
        '=' | ':' => {}
        _ => return None,
    }
    i += 1;
    // `=>` (table de hachage) et `:=` (affectation) introduisent aussi une valeur.
    if texte[i..].starts_with(['>', '=']) {
        i += 1;
    }
    let serre = separateur == '=' && colle_avant && i == blancs(i);
    i = blancs(i);
    let reste = &texte[i..];
    let (debut, fin) = if reste.starts_with("\\\"") {
        let debut = i + 2;
        let fin = texte[debut..]
            .find(['\\', '"'])
            .map_or(texte.len(), |n| debut + n);
        (debut, fin)
    } else {
        match reste.chars().next()? {
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
                        c.is_whitespace()
                            || matches!(
                                c,
                                ',' | ';' | '&' | '}' | '"' | '\'' | '(' | ')' | ']' | '<' | '>'
                            )
                    })
                    .unwrap_or(reste.len());
                let mot = reste[..n].trim_end_matches(['.', '!', '?', ':']);
                if !litteral_nu(mot, &reste[n..], serre) {
                    return None;
                }
                (i, i + mot.len())
            }
        }
    };
    // Une valeur déjà masquée n'est plus un secret : masquer est idempotent, et un texte
    // masqué repassé à `remember` n'est pas refusé.
    (fin > debut && !texte[debut..].starts_with(MASQUE)).then_some((debut, fin))
}

/// Vrai si `mot`, une valeur sans guillemets suivie de `suite`, est un littéral plutôt que du
/// code ou de la prose (voir [`valeur_apres_cle`]).
fn litteral_nu(mot: &str, suite: &str, serre: bool) -> bool {
    let Some(premier) = mot.chars().next() else {
        return false;
    };
    let appel = suite.starts_with('(');
    let constante = PAS_DES_VALEURS.contains(&mot.to_ascii_lowercase().as_str());
    let type_ = matches!(premier, '&' | '[' | '*' | '$' | '{')
        || (premier.is_ascii_uppercase()
            && !mot.chars().any(|c| c.is_ascii_digit())
            && mot
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'));
    let signe = mot
        .chars()
        .any(|c| c.is_ascii_digit() || (c.is_ascii_punctuation() && c != '_'));
    let chemin = premier == '/' || mot.starts_with("./") || mot.starts_with("~/");
    !appel
        && !constante
        && !type_
        && !acces_de_champ(mot)
        && !mot.contains("::")
        && (serre || (signe && !chemin))
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
/// reste un objet), et la valeur d'un champ nommé comme un secret ([`champ_secret`] :
/// `{"password": "…"}`, `{"db_password": "…"}`) est masquée entière : seule, la chaîne
/// n'aurait rien de reconnaissable.
pub fn masquer_valeur(valeur: &Value) -> Value {
    match valeur {
        Value::String(texte) => Value::String(masquer(texte)),
        Value::Array(liste) => Value::Array(liste.iter().map(masquer_valeur).collect()),
        Value::Object(objet) => Value::Object(
            objet
                .iter()
                .map(|(cle, v)| {
                    let v = match v {
                        Value::String(s) if champ_secret(cle) && !s.trim().is_empty() => {
                            json!(MASQUE)
                        }
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

    /// Masqué ET reconnu (même table) : le refus et le masquage disent la même chose.
    fn masque_en(texte: &str, attendu: &str) {
        assert_eq!(masquer(texte), attendu, "{texte}");
        assert!(secret_dans(texte).is_some(), "{texte}");
        assert_eq!(secret_dans(&masquer(texte)), None, "{texte}");
    }

    /// Ni masqué ni refusé.
    fn intact(texte: &str) {
        assert_eq!(masquer(texte), texte);
        assert_eq!(secret_dans(texte), None, "{texte}");
    }

    // ---- Revue Y10 #1 : faux positifs (chaque cas a son positif voisin) ----------------

    #[test]
    fn secret_type_rust_n_est_pas_une_valeur() {
        intact("struct Compte { password: String, token: Option<String> }");
        intact("fn connecter(password: &str, pwd: str)");
    }

    #[test]
    fn secret_litteral_apres_deux_points_est_masque() {
        masque_en(
            "password: \"hunter2\", token: s3cr3t!",
            "password: \"[secret masqué]\", token: [secret masqué]!",
        );
    }

    #[test]
    fn secret_appel_de_fonction_n_est_pas_une_valeur() {
        intact("let password = get_password();");
        intact("let token = self.token.clone();");
    }

    #[test]
    fn secret_litteral_affecte_est_masque() {
        masque_en(
            "let password = \"hunter2\";",
            "let password = \"[secret masqué]\";",
        );
    }

    #[test]
    fn secret_comparaison_n_est_pas_une_valeur() {
        intact("if password == expected { ok() }");
        intact("assert!(token != secret)");
    }

    #[test]
    fn secret_comparaison_a_un_litteral_reste_masquee_si_affectee() {
        masque_en("password=hunter2 == ok", "password=[secret masqué] == ok");
    }

    #[test]
    fn secret_constantes_json_ne_sont_pas_des_valeurs() {
        intact(r#"{"password": null, "token": true, "secret": false}"#);
        intact("password = None");
    }

    #[test]
    fn secret_chaine_json_est_masquee() {
        masque_en(
            r#"{"password": "null-pas-null"}"#,
            r#"{"password": "[secret masqué]"}"#,
        );
    }

    #[test]
    fn secret_champ_json_par_contains_n_est_pas_un_secret() {
        let v = json!({"passwordHint": "ton chien", "password_policy": "strict",
                       "tokenizer": "bpe", "secretary": "Ada"});
        assert_eq!(masquer_valeur(&v), v);
        intact(r#"{"passwordHint": "ton chien", "password_policy": "strict"}"#);
    }

    #[test]
    fn secret_champ_json_exact_ou_suffixe_est_masque() {
        let v = json!({"password": "a", "db_password": "b", "access_token": "c",
                       "x-api-key": "d", "client_secret": "e", "pwd": "f"});
        let m = masquer_valeur(&v);
        for cle in [
            "password",
            "db_password",
            "access_token",
            "x-api-key",
            "client_secret",
            "pwd",
        ] {
            assert_eq!(m[cle], MASQUE, "{cle}");
        }
    }

    #[test]
    fn secret_prose_mot_de_passe_oublie_n_est_pas_un_secret() {
        intact("le mot de passe : oublié ?");
        intact("Le token : expiré, le secret : bien gardé.");
    }

    #[test]
    fn secret_prose_mot_de_passe_avec_valeur_est_masque() {
        masque_en(
            "le mot de passe : Tr0ub4dor#3",
            "le mot de passe : [secret masqué]",
        );
    }

    // ---- Revue Y10 #2 : faux négatifs ---------------------------------------------------

    #[test]
    fn secret_json_echappe_est_masque() {
        masque_en(
            r#"{\"password\": \"hunter2\"}"#,
            r#"{\"password\": \"[secret masqué]\"}"#,
        );
        masque_en(
            r#"{\"api_key\":\"abc123\",\"x\":1}"#,
            r#"{\"api_key\":\"[secret masqué]\",\"x\":1}"#,
        );
    }

    #[test]
    fn secret_json_echappe_sans_valeur_n_est_pas_un_secret() {
        intact(r#"{\"password\": null, \"passwordHint\": \"chien\"}"#);
    }

    #[test]
    fn secret_cle_coupee_par_un_saut_de_ligne_est_masquee() {
        masque_en(
            "clé sk-ant-api03-abcdefgh\nijklmnopqrstuvwxyz0123 fin",
            "clé [secret masqué] fin",
        );
        masque_en(
            "jeton ghp_abcdefghij\r\nklmnopqrstuvwxyz ici",
            "jeton [secret masqué] ici",
        );
        // Coupée dans un JSON sérialisé : le saut est un `\n` échappé.
        masque_en(
            r#"{"text":"ghp_abcdefghij\nklmnopqrstuvwxyz"}"#,
            r#"{"text":"[secret masqué]"}"#,
        );
    }

    #[test]
    fn secret_prefixe_court_puis_saut_de_ligne_n_est_pas_une_cle() {
        intact("le préfixe sk-\nest celui des clés");
        intact("ghp_abc\ndef est trop court");
        intact("risk-score\nsuivant");
    }

    #[test]
    fn secret_pwd_secret_token_api_key_sont_masques() {
        masque_en(
            "pwd=hunter2 secret=abc token=xyz api_key=k3y",
            "pwd=[secret masqué] secret=[secret masqué] token=[secret masqué] api_key=[secret masqué]",
        );
        masque_en("export API_KEY=zzz", "export API_KEY=[secret masqué]");
    }

    #[test]
    fn secret_pwd_secret_token_api_key_sans_valeur_ne_sont_pas_des_secrets() {
        intact("cd $(pwd) ; les secrets du token ; max_tokens: 4096 est une borne");
        intact("let token = fetch_token(); if secret == x {}");
    }

    // ---- Revue Y10 NIT 11 : PRIVATE KEY sur la ligne même du BEGIN ----------------------

    #[test]
    fn secret_certificat_suivi_d_une_cle_plus_loin_n_est_pas_masque_entier() {
        let texte = "-----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----\nnote : PRIVATE KEY absente";
        intact(texte);
    }

    #[test]
    fn secret_cle_privee_sur_la_ligne_du_begin_est_masquee() {
        masque_en(
            "cert -----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----\n-----BEGIN EC PRIVATE KEY-----\nMHc\n-----END EC PRIVATE KEY----- fin",
            "cert -----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----\n[secret masqué] fin",
        );
    }
}
