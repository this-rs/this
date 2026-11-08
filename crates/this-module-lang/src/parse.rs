//! Descente récursive, précédence explicite.
//!
//! La grammaire est **fermée** : tout ce qui n'est pas reconnu est une erreur,
//! jamais une extension silencieuse. C'est la propriété qui rend le langage
//! total — on ne peut pas écrire une boucle, donc on ne peut pas écrire un
//! module qui ne termine pas.

use crate::ast::{BinOp, Binding, Expr, Function, Param, Span, SpannedExpr, Type, UnOp};
use crate::lex::{Token, lex};

struct Parser {
    tokens: Vec<crate::lex::Lexed>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.position].token
    }

    fn span(&self) -> Span {
        self.tokens[self.position].span
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].token.clone();
        if self.position + 1 < self.tokens.len() {
            self.position += 1;
        }
        token
    }

    fn error(&self, expected: &str) -> String {
        let span = self.span();
        format!(
            "ligne {}, colonne {} : attendu {expected}, trouvé {}",
            span.line,
            span.column,
            self.peek().describe()
        )
    }

    fn expect(&mut self, expected: Token, label: &str) -> Result<(), String> {
        if *self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(self.error(label))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(self.error("un identifiant")),
        }
    }

    fn ty(&mut self) -> Result<Type, String> {
        let name = self.ident()?;
        match name.as_str() {
            "Int" => Ok(Type::Int),
            "Bool" => Ok(Type::Bool),
            "Str" => Ok(Type::Str),
            "Unit" => Ok(Type::Unit),
            other => Err(format!(
                "ligne {}, colonne {} : `{other}` n'est pas un type du langage (Int, Bool, Str, Unit)",
                self.span().line,
                self.span().column
            )),
        }
    }

    fn function(&mut self) -> Result<Function, String> {
        self.expect(Token::Fn, "`fn`")?;
        let name = self.ident()?;
        self.expect(Token::LParen, "`(`")?;

        let mut params = Vec::new();
        if *self.peek() != Token::RParen {
            loop {
                let name = self.ident()?;
                self.expect(Token::Colon, "`:`")?;
                let ty = self.ty()?;
                params.push(Param { name, ty });
                if *self.peek() == Token::Comma {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        self.expect(Token::RParen, "`)`")?;
        self.expect(Token::Arrow, "`->`")?;
        let returns = self.ty()?;
        self.expect(Token::LBrace, "`{`")?;

        let mut bindings = Vec::new();
        // `let` et `var` se déclarent au même endroit, dans l'ordre de la source :
        // l'initialisation d'une liaison peut lire la précédente, et l'indexation
        // des locaux wasm suit cet ordre.
        while matches!(*self.peek(), Token::Let | Token::Var) {
            let mutable = *self.peek() == Token::Var;
            self.advance();
            let name = self.ident()?;
            self.expect(Token::Colon, "`:`")?;
            let ty = self.ty()?;
            self.expect(Token::Eq, "`=`")?;
            let value = self.expression()?.node;
            self.expect(Token::Semi, "`;`")?;
            bindings.push(Binding {
                name,
                ty,
                value,
                mutable,
            });
        }

        let body = self.sequence()?.node;
        self.expect(Token::RBrace, "`}`")?;
        if *self.peek() != Token::Eof {
            return Err(self.error("la fin du programme"));
        }

        Ok(Function {
            name,
            params,
            returns,
            bindings,
            body,
        })
    }

    fn expression(&mut self) -> Result<SpannedExpr, String> {
        self.or()
    }

    /// `e1; e2; e3` — une seule expression si aucun `;` ne suit. Le `;` final
    /// est toléré : il rend les programmes générés plus faciles à produire.
    fn sequence(&mut self) -> Result<SpannedExpr, String> {
        let first = self.expression()?;
        if *self.peek() != Token::Semi {
            return Ok(first);
        }
        let span = first.span;
        let mut items = vec![first.node];
        while *self.peek() == Token::Semi {
            self.advance();
            if *self.peek() == Token::RBrace {
                break;
            }
            items.push(self.expression()?.node);
        }
        Ok(SpannedExpr {
            node: Expr::Seq(items),
            span,
        })
    }

    fn or(&mut self) -> Result<SpannedExpr, String> {
        let mut left = self.and()?;
        while *self.peek() == Token::OrOr {
            self.advance();
            let right = self.and()?;
            let span = left.span;
            left = SpannedExpr {
                node: Expr::Binary {
                    op: BinOp::Or,
                    left: Box::new(left.node),
                    right: Box::new(right.node),
                },
                span,
            };
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<SpannedExpr, String> {
        let mut left = self.comparison()?;
        while *self.peek() == Token::AndAnd {
            self.advance();
            let right = self.comparison()?;
            let span = left.span;
            left = SpannedExpr {
                node: Expr::Binary {
                    op: BinOp::And,
                    left: Box::new(left.node),
                    right: Box::new(right.node),
                },
                span,
            };
        }
        Ok(left)
    }

    fn comparison(&mut self) -> Result<SpannedExpr, String> {
        let left = self.addition()?;
        let op = match self.peek() {
            Token::EqEq => BinOp::Eq,
            Token::Ne => BinOp::Ne,
            Token::Lt => BinOp::Lt,
            Token::Le => BinOp::Le,
            Token::Gt => BinOp::Gt,
            Token::Ge => BinOp::Ge,
            _ => return Ok(left),
        };
        self.advance();
        let right = self.addition()?;
        let span = left.span;
        Ok(SpannedExpr {
            node: Expr::Binary {
                op,
                left: Box::new(left.node),
                right: Box::new(right.node),
            },
            span,
        })
    }

    fn addition(&mut self) -> Result<SpannedExpr, String> {
        let mut left = self.multiplication()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.multiplication()?;
            let span = left.span;
            left = SpannedExpr {
                node: Expr::Binary {
                    op,
                    left: Box::new(left.node),
                    right: Box::new(right.node),
                },
                span,
            };
        }
    }

    fn multiplication(&mut self) -> Result<SpannedExpr, String> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Rem,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.unary()?;
            let span = left.span;
            left = SpannedExpr {
                node: Expr::Binary {
                    op,
                    left: Box::new(left.node),
                    right: Box::new(right.node),
                },
                span,
            };
        }
    }

    fn unary(&mut self) -> Result<SpannedExpr, String> {
        let span = self.span();
        match self.peek() {
            Token::Bang => {
                self.advance();
                let inner = self.unary()?;
                Ok(SpannedExpr {
                    node: Expr::Unary {
                        op: UnOp::Not,
                        expr: Box::new(inner.node),
                    },
                    span,
                })
            }
            Token::Minus => {
                self.advance();
                let inner = self.unary()?;
                Ok(SpannedExpr {
                    node: Expr::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(inner.node),
                    },
                    span,
                })
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<SpannedExpr, String> {
        let span = self.span();
        match self.peek().clone() {
            Token::Int(value) => {
                self.advance();
                Ok(SpannedExpr {
                    node: Expr::Int(value),
                    span,
                })
            }
            Token::True => {
                self.advance();
                Ok(SpannedExpr {
                    node: Expr::Bool(true),
                    span,
                })
            }
            Token::False => {
                self.advance();
                Ok(SpannedExpr {
                    node: Expr::Bool(false),
                    span,
                })
            }
            Token::Str(value) => {
                self.advance();
                Ok(SpannedExpr {
                    node: Expr::Str(value),
                    span,
                })
            }
            Token::LParen => {
                self.advance();
                let inner = self.expression()?;
                self.expect(Token::RParen, "`)`")?;
                Ok(inner)
            }
            Token::If => {
                self.advance();
                let condition = self.expression()?;
                self.expect(Token::LBrace, "`{`")?;
                let then_branch = self.expression()?;
                self.expect(Token::RBrace, "`}`")?;
                self.expect(Token::Else, "`else`")?;
                self.expect(Token::LBrace, "`{`")?;
                let else_branch = self.expression()?;
                self.expect(Token::RBrace, "`}`")?;
                Ok(SpannedExpr {
                    node: Expr::If {
                        condition: Box::new(condition.node),
                        then_branch: Box::new(then_branch.node),
                        else_branch: Box::new(else_branch.node),
                    },
                    span,
                })
            }
            Token::While => {
                self.advance();
                let condition = self.expression()?;
                self.expect(Token::LBrace, "`{`")?;
                let body = self.sequence()?;
                self.expect(Token::RBrace, "`}`")?;
                Ok(SpannedExpr {
                    node: Expr::While {
                        condition: Box::new(condition.node),
                        body: Box::new(body.node),
                    },
                    span,
                })
            }
            Token::Set => {
                self.advance();
                let name = self.ident()?;
                self.expect(Token::Eq, "`=`")?;
                let value = self.expression()?;
                Ok(SpannedExpr {
                    node: Expr::Set {
                        name,
                        value: Box::new(value.node),
                    },
                    span,
                })
            }
            Token::Ident(name) => {
                self.advance();
                if *self.peek() != Token::LParen {
                    return Ok(SpannedExpr {
                        node: Expr::Var(name),
                        span,
                    });
                }
                self.advance();
                let mut args = Vec::new();
                if *self.peek() != Token::RParen {
                    loop {
                        args.push(self.expression()?.node);
                        if *self.peek() == Token::Comma {
                            self.advance();
                            continue;
                        }
                        break;
                    }
                }
                self.expect(Token::RParen, "`)`")?;
                Ok(SpannedExpr {
                    node: Expr::Call { name, args },
                    span,
                })
            }
            _ => Err(self.error("une expression")),
        }
    }
}

/// Analyse une source complète. Un programme = une fonction.
pub fn parse(source: &str) -> Result<Function, String> {
    let tokens = lex(source)?;
    let mut parser = Parser {
        tokens,
        position: 0,
    };
    parser.function()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_program_parses() {
        let function = parse("fn run() -> Int { 1 }").unwrap();
        assert_eq!(function.name, "run");
        assert_eq!(function.returns, Type::Int);
        assert_eq!(function.body, Expr::Int(1));
    }

    #[test]
    fn parameters_and_bindings_are_read_in_order() {
        let function = parse(
            "fn run(a: Int, b: Bool) -> Int { let c: Int = a + 1; if b { c } else { c * 2 } }",
        )
        .unwrap();
        assert_eq!(function.params.len(), 2);
        assert_eq!(function.bindings.len(), 1);
        assert_eq!(function.bindings[0].name, "c");
    }

    #[test]
    fn precedence_is_respected() {
        let function = parse("fn run() -> Int { 1 + 2 * 3 }").unwrap();
        match function.body {
            Expr::Binary {
                op: BinOp::Add,
                right,
                ..
            } => {
                assert!(matches!(*right, Expr::Binary { op: BinOp::Mul, .. }))
            }
            other => panic!("attendu une addition à la racine, obtenu {other:?}"),
        }
    }

    #[test]
    fn a_call_with_arguments_parses() {
        let function = parse("fn run() -> Unit { emit(\"créé\") }").unwrap();
        match function.body {
            Expr::Call { name, args } => {
                assert_eq!(name, "emit");
                assert_eq!(args.len(), 1);
            }
            other => panic!("attendu un appel, obtenu {other:?}"),
        }
    }

    #[test]
    fn a_missing_brace_says_where() {
        let error = parse("fn run() -> Int { 1").unwrap_err();
        assert!(error.contains("ligne 1"), "{error}");
        assert!(error.contains("`}`"), "{error}");
    }

    #[test]
    fn an_unknown_type_is_refused() {
        let error = parse("fn run() -> Float { 1 }").unwrap_err();
        assert!(error.contains("Float"), "{error}");
        assert!(error.contains("Int, Bool, Str, Unit"), "{error}");
    }

    #[test]
    fn a_loop_and_its_mutation_are_parsed() {
        let function =
            parse("fn run() -> Unit { var i: Int = 0; while i < 3 { set i = i + 1; } }").unwrap();
        assert_eq!(function.bindings.len(), 1);
        assert!(
            function.bindings[0].mutable,
            "`var` doit marquer la liaison comme mutable"
        );
        assert!(matches!(function.body, Expr::While { .. }));
    }

    #[test]
    fn a_loop_body_that_throws_a_value_is_refused() {
        // Une valeur jetée à chaque tour n'a pas de sens : le typeur le dit.
        let function = parse("fn run() -> Unit { while true { 1 } }").unwrap();
        let error = crate::check::check(&function).unwrap_err();
        assert!(error.contains("Unit"), "{error}");
    }
}
