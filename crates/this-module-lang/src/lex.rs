//! Découpage en jetons. Chaque jeton porte sa position : une erreur de syntaxe
//! qui ne dit pas où elle est ne sert à rien à quelqu'un qui écrit un module.

use crate::ast::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Int(i64),
    Str(String),
    Ident(String),
    True,
    False,
    Fn,
    Let,
    Var,
    Set,
    While,
    If,
    Else,
    Arrow,
    Colon,
    Semi,
    Comma,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    Eof,
}

impl Token {
    pub fn describe(&self) -> String {
        match self {
            Token::Int(value) => format!("`{value}`"),
            Token::Str(value) => format!("la chaîne \"{value}\""),
            Token::Ident(name) => format!("`{name}`"),
            Token::True => "`true`".into(),
            Token::False => "`false`".into(),
            Token::Fn => "`fn`".into(),
            Token::Let => "`let`".into(),
            Token::Var => "`var`".into(),
            Token::Set => "`set`".into(),
            Token::While => "`while`".into(),
            Token::If => "`if`".into(),
            Token::Else => "`else`".into(),
            Token::Arrow => "`->`".into(),
            Token::Colon => "`:`".into(),
            Token::Semi => "`;`".into(),
            Token::Comma => "`,`".into(),
            Token::LParen => "`(`".into(),
            Token::RParen => "`)`".into(),
            Token::LBrace => "`{`".into(),
            Token::RBrace => "`}`".into(),
            Token::Plus => "`+`".into(),
            Token::Minus => "`-`".into(),
            Token::Star => "`*`".into(),
            Token::Slash => "`/`".into(),
            Token::Percent => "`%`".into(),
            Token::Eq => "`=`".into(),
            Token::EqEq => "`==`".into(),
            Token::Ne => "`!=`".into(),
            Token::Lt => "`<`".into(),
            Token::Le => "`<=`".into(),
            Token::Gt => "`>`".into(),
            Token::Ge => "`>=`".into(),
            Token::AndAnd => "`&&`".into(),
            Token::OrOr => "`||`".into(),
            Token::Bang => "`!`".into(),
            Token::Eof => "la fin du programme".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lexed {
    pub token: Token,
    pub span: Span,
}

pub fn lex(source: &str) -> Result<Vec<Lexed>, String> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut line = 1u32;
    let mut column = 1u32;

    while let Some(&ch) = chars.peek() {
        let span = Span { line, column };

        match ch {
            '\n' => {
                chars.next();
                line += 1;
                column = 1;
                continue;
            }
            c if c.is_whitespace() => {
                chars.next();
                column += 1;
                continue;
            }
            '/' if matches!(chars.clone().nth(1), Some('/')) => {
                while let Some(&c) = chars.peek() {
                    chars.next();
                    column += 1;
                    if c == '\n' {
                        line += 1;
                        column = 1;
                        break;
                    }
                }
                continue;
            }
            _ => {}
        }

        macro_rules! push {
            ($token:expr, $width:expr) => {{
                tokens.push(Lexed {
                    token: $token,
                    span,
                });
                column += $width;
            }};
        }

        // Deux caractères d'abord : sans ça, `<=` deviendrait `<` puis `=`.
        let lookahead: String = {
            let mut it = chars.clone();
            it.by_ref().take(2).collect()
        };

        match lookahead.as_str() {
            "->" => {
                chars.next();
                chars.next();
                push!(Token::Arrow, 2);
                continue;
            }
            "==" => {
                chars.next();
                chars.next();
                push!(Token::EqEq, 2);
                continue;
            }
            "!=" => {
                chars.next();
                chars.next();
                push!(Token::Ne, 2);
                continue;
            }
            "<=" => {
                chars.next();
                chars.next();
                push!(Token::Le, 2);
                continue;
            }
            ">=" => {
                chars.next();
                chars.next();
                push!(Token::Ge, 2);
                continue;
            }
            "&&" => {
                chars.next();
                chars.next();
                push!(Token::AndAnd, 2);
                continue;
            }
            "||" => {
                chars.next();
                chars.next();
                push!(Token::OrOr, 2);
                continue;
            }
            _ => {}
        }

        let c = chars.next().expect("un caractère a été lu en regardant");

        match c {
            '(' => push!(Token::LParen, 1),
            ')' => push!(Token::RParen, 1),
            '{' => push!(Token::LBrace, 1),
            '}' => push!(Token::RBrace, 1),
            ':' => push!(Token::Colon, 1),
            ';' => push!(Token::Semi, 1),
            ',' => push!(Token::Comma, 1),
            '+' => push!(Token::Plus, 1),
            '-' => push!(Token::Minus, 1),
            '*' => push!(Token::Star, 1),
            '/' => push!(Token::Slash, 1),
            '%' => push!(Token::Percent, 1),
            '<' => push!(Token::Lt, 1),
            '>' => push!(Token::Gt, 1),
            '=' => push!(Token::Eq, 1),
            '!' => push!(Token::Bang, 1),
            '"' => {
                let mut value = String::new();
                let mut width = 1;
                loop {
                    match chars.next() {
                        Some('"') => {
                            width += 1;
                            break;
                        }
                        Some('\n') | None => {
                            return Err(format!(
                                "ligne {line}, colonne {column} : chaîne non terminée"
                            ));
                        }
                        // Trois échappements, pas plus : le guillemet (du JSON dans un
                        // littéral), la barre elle-même, et le saut de ligne. Un autre
                        // `\x` est une faute, pas un caractère : on la dit.
                        Some('\\') => {
                            width += 2;
                            match chars.next() {
                                Some('"') => value.push('"'),
                                Some('\\') => value.push('\\'),
                                Some('n') => value.push('\n'),
                                Some(other) => {
                                    return Err(format!(
                                        "ligne {line}, colonne {column} : échappement inconnu `\\{other}` — seuls \\\" \\\\ et \\n existent"
                                    ));
                                }
                                None => {
                                    return Err(format!(
                                        "ligne {line}, colonne {column} : chaîne non terminée"
                                    ));
                                }
                            }
                        }
                        Some(other) => {
                            value.push(other);
                            width += 1;
                        }
                    }
                }
                push!(Token::Str(value), width);
            }
            c if c.is_ascii_digit() => {
                let mut text = String::from(c);
                let mut width = 1;
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_digit() {
                        text.push(next);
                        chars.next();
                        width += 1;
                    } else {
                        break;
                    }
                }
                let value: i64 = text.parse().map_err(|_| {
                    format!("ligne {line}, colonne {column} : `{text}` dépasse un entier 64 bits")
                })?;
                push!(Token::Int(value), width);
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut text = String::from(c);
                let mut width = 1;
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' {
                        text.push(next);
                        chars.next();
                        width += 1;
                    } else {
                        break;
                    }
                }
                let token = match text.as_str() {
                    "true" => Token::True,
                    "false" => Token::False,
                    "fn" => Token::Fn,
                    "let" => Token::Let,
                    "var" => Token::Var,
                    "set" => Token::Set,
                    "while" => Token::While,
                    "if" => Token::If,
                    "else" => Token::Else,
                    other => Token::Ident(other.to_string()),
                };
                push!(token, width);
            }
            other => {
                return Err(format!(
                    "ligne {line}, colonne {column} : caractère inattendu `{other}`"
                ));
            }
        }
    }

    tokens.push(Lexed {
        token: Token::Eof,
        span: Span { line, column },
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Token> {
        lex(source)
            .unwrap()
            .into_iter()
            .map(|lexed| lexed.token)
            .collect()
    }

    #[test]
    fn the_two_character_operators_are_not_split() {
        assert_eq!(
            kinds("<= >= == != && || ->"),
            vec![
                Token::Le,
                Token::Ge,
                Token::EqEq,
                Token::Ne,
                Token::AndAnd,
                Token::OrOr,
                Token::Arrow,
                Token::Eof
            ]
        );
    }

    #[test]
    fn a_comment_runs_to_the_end_of_the_line() {
        assert_eq!(
            kinds("1 // deux\n2"),
            vec![Token::Int(1), Token::Int(2), Token::Eof]
        );
    }

    #[test]
    fn a_string_keeps_its_content_and_its_position() {
        let lexed = lex("  \"abc\"").unwrap();
        assert_eq!(lexed[0].token, Token::Str("abc".into()));
        // Du JSON dans un littéral : les guillemets s'échappent, et la largeur compte
        // les caractères SOURCE, pas ceux de la valeur.
        let json = lex("\"{\\\"a\\\":1}\"").unwrap();
        assert_eq!(json[0].token, Token::Str("{\"a\":1}".into()));
        assert_eq!(
            lex("\"x\\ny\"").unwrap()[0].token,
            Token::Str("x\ny".into())
        );
        assert_eq!(lex("\"\\\\\"").unwrap()[0].token, Token::Str("\\".into()));
        let e = lex("\"\\q\"").unwrap_err();
        assert!(e.contains("échappement inconnu"), "{e}");
        assert_eq!(lexed[0].span.line, 1);
        assert_eq!(lexed[0].span.column, 3);
    }

    #[test]
    fn an_unterminated_string_is_refused_with_its_position() {
        let error = lex("\"abc").unwrap_err();
        assert!(error.contains("ligne 1"));
        assert!(error.contains("non terminée"));
    }

    #[test]
    fn keywords_are_not_identifiers() {
        assert_eq!(
            kinds("fn let if else true false x"),
            vec![
                Token::Fn,
                Token::Let,
                Token::If,
                Token::Else,
                Token::True,
                Token::False,
                Token::Ident("x".into()),
                Token::Eof
            ]
        );
    }
}
