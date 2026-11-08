//! Émission WebAssembly (format texte).
//!
//! Le module produit est **fermé** : pas d'import en dehors des primitives
//! d'hôte réellement appelées, une seule mémoire exportée, une seule fonction
//! exportée (`run`). Un module qui n'appelle rien n'importe rien — il
//! s'instancie donc avec un objet d'imports vide.
//!
//! Le texte est émis plutôt qu'un binaire : c'est lisible, ça se relit à la
//! main dans une revue, et la conversion en binaire est un outil standard
//! (`wat2wasm`, la crate `wat`). Le binaire n'apporte rien à la correction.

use crate::ast::{BinOp, Expr, Function, Type, UnOp};
use crate::check;

/// Le module wasm textuel et les primitives qu'il importe.
#[derive(Clone, Debug, PartialEq)]
pub struct Wat {
    pub text: String,
    pub imports: Vec<&'static str>,
}

impl Wat {
    /// Vrai si le programme importe une primitive qu'un navigateur ne fournit pas
    /// (appareil, réseau) : il doit alors s'exécuter dans la coquille, par l'interprète,
    /// où ces appels peuvent être asynchrones et consentis.
    pub fn shell_only(&self) -> bool {
        self.imports
            .iter()
            .any(|import| !check::HOST_NAVIGATEUR.contains(import))
    }
}

impl Wat {
    /// Le nom de la fonction exportée. Un module n'en exporte qu'une.
    pub const EXPORT: &'static str = "run";
}

/// L'import qui porte le budget de boucle. Il n'est pas dans la table des
/// primitives : le modèle ne l'écrit jamais, l'émetteur l'insère dans chaque
/// `while`. Séparé, il ne peut pas être confondu avec une capacité du langage.
const FUEL: &str = "fuel";

struct Emitter {
    lines: Vec<String>,
    indent: usize,
    /// Le nom du programme : un appel à ce nom est une récursion, pas un hôte.
    own: String,
    /// Ce que le programme rend — un appel récursif dans une branche `if` a
    /// besoin de son type pour ouvrir le bloc au bon type.
    returns: Type,
    /// Les chaînes littérales, dans l'ordre d'apparition, avec leur position
    /// dans la mémoire linéaire.
    data: Vec<(u32, Vec<u8>)>,
    imports: Vec<&'static str>,
    locals: Vec<(String, Type)>,
    params: Vec<(String, Type)>,
}

impl Emitter {
    fn line(&mut self, text: impl Into<String>) {
        self.lines
            .push(format!("{}{}", "  ".repeat(self.indent), text.into()));
    }

    fn wasm_type(ty: Type) -> &'static str {
        match ty {
            Type::Int => "i64",
            Type::Bool => "i32",
            Type::Str => "i32",
            Type::Unit => "",
        }
    }

    fn import(&mut self, name: &'static str) {
        if !self.imports.contains(&name) {
            self.imports.push(name);
        }
    }

    fn intern(&mut self, text: &str) -> (u32, u32) {
        let bytes = text.as_bytes().to_vec();
        let length = bytes.len() as u32;
        if let Some((offset, _)) = self.data.iter().find(|(_, known)| *known == bytes) {
            return (*offset, length);
        }
        // Une position sur huit : l'adresse 0 reste libre, ce qui rend un
        // pointeur nul détectable.
        let offset = self.data.last().map_or(8, |(previous, bytes)| {
            (previous + bytes.len() as u32 + 7) & !7
        });
        self.data.push((offset, bytes));
        (offset, length)
    }

    fn local_of(&self, name: &str) -> Option<(usize, Type)> {
        self.params
            .iter()
            .enumerate()
            .find(|(_, (candidate, _))| candidate == name)
            .map(|(index, (_, ty))| (index, *ty))
            .or_else(|| {
                self.locals
                    .iter()
                    .position(|(candidate, _)| candidate == name)
                    .map(|index| (index + self.params.len(), self.locals[index].1))
            })
    }

    fn expr(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Int(value) => self.line(format!("i64.const {value}")),
            Expr::Bool(true) => self.line("i32.const 1"),
            Expr::Bool(false) => self.line("i32.const 0"),
            Expr::Str(text) => {
                let (offset, length) = self.intern(text);
                self.line(format!("i32.const {offset}"));
                self.line(format!("i32.const {length}"));
            }
            Expr::Var(name) => {
                let Some((index, ty)) = self.local_of(name) else {
                    return Err(format!("`{name}` n'a pas de local wasm"));
                };
                if ty == Type::Str {
                    return Err(format!(
                        "`{name}` est une chaîne : elle n'a pas de représentation en valeur simple"
                    ));
                }
                self.line(format!("local.get {index}"));
            }
            Expr::Set { name, value } => {
                let Some((index, _)) = self.local_of(name) else {
                    return Err(format!("`{name}` n'a pas de local wasm"));
                };
                self.expr(value)?;
                self.line(format!("local.set {index}"));
            }
            Expr::While { condition, body } => {
                // Le nom des étiquettes suit l'indentation : deux boucles
                // imbriquées ont forcément des indentations différentes, donc
                // des noms différents. Deux boucles voisines les partagent, ce
                // qui est sans effet : aucune ne contient l'autre.
                let label = self.indent;
                self.line(format!("block $break{label}"));
                self.indent += 1;
                self.line(format!("loop $again{label}"));
                self.indent += 1;
                // Le budget de tours : une boucle infinie écrite par un modèle
                // ne doit pas figer l'application qui l'exécute.
                self.import(FUEL);
                self.line("call $fuel");
                self.line("i32.eqz");
                self.line("if");
                self.indent += 1;
                // Budget épuisé : on piège, on ne rend pas un résultat faux.
                // L'interprète, lui, rend une erreur — les deux échouent.
                self.line("unreachable");
                self.indent -= 1;
                self.line("end");
                self.expr(condition)?;
                self.line("i32.eqz");
                self.line(format!("br_if $break{label}"));
                self.expr(body)?;
                self.line(format!("br $again{label}"));
                self.indent -= 1;
                self.line("end");
                self.indent -= 1;
                self.line("end");
            }
            Expr::Unary {
                op: UnOp::Not,
                expr,
            } => {
                self.expr(expr)?;
                self.line("i32.eqz");
            }
            Expr::Unary {
                op: UnOp::Neg,
                expr,
            } => {
                self.line("i64.const 0");
                self.expr(expr)?;
                self.line("i64.sub");
            }
            Expr::Binary { op, left, right } => self.binary(*op, left, right)?,
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(condition)?;
                let result = self.result_type(then_branch);
                match result {
                    Some(ty) => self.line(format!("if (result {})", Self::wasm_type(ty))),
                    None => self.line("if"),
                }
                self.indent += 1;
                self.expr(then_branch)?;
                self.indent -= 1;
                self.line("else");
                self.indent += 1;
                self.expr(else_branch)?;
                self.indent -= 1;
                self.line("end");
            }
            Expr::Seq(items) => {
                for item in items {
                    self.expr(item)?;
                }
            }
            Expr::Call { name, args } => {
                for arg in args {
                    self.expr(arg)?;
                }
                // Un appel à son propre nom est une récursion : la pile wasm
                // borne la profondeur, et un dépassement est un piège, pas un
                // blocage de l'hôte.
                if name == &self.own {
                    self.line(format!("call ${}", Wat::EXPORT));
                } else {
                    // La table HOST est la seule source : une primitive qui y figure a
                    // son import, une qui n'y figure pas n'a pas de wasm. Le typeur l'a
                    // déjà refusée ; ce bras ne fait que le redire.
                    let Some(primitive) = check::host_fn(name) else {
                        return Err(format!("`{name}` n'a pas d'import wasm"));
                    };
                    self.import(primitive.name);
                    self.line(format!("call ${}", primitive.name));
                }
            }
        }
        Ok(())
    }

    /// Le type poussé sur la pile, ou `None` pour une expression sans valeur.
    fn result_type(&self, expr: &Expr) -> Option<Type> {
        match self.type_of(expr) {
            Some(Type::Unit) | None => None,
            Some(ty) => Some(ty),
        }
    }

    /// Le type d'une expression déjà vérifiée. Les variables se résolvent dans
    /// les locaux : sans ça, `a == b` sur deux `Int` choisirait la comparaison
    /// 32 bits et produirait un module invalide.
    fn type_of(&self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Int(_) => Some(Type::Int),
            Expr::Bool(_) => Some(Type::Bool),
            Expr::Str(_) => Some(Type::Str),
            Expr::Var(name) => self.local_of(name).map(|(_, ty)| ty),
            Expr::Unary { op, .. } => match op {
                UnOp::Not => Some(Type::Bool),
                UnOp::Neg => Some(Type::Int),
            },
            Expr::Binary { op, .. } => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => Some(Type::Int),
                _ => Some(Type::Bool),
            },
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => match self.type_of(then_branch) {
                Some(Type::Unit) | None => self.type_of(else_branch),
                other => other,
            },
            Expr::Seq(items) => items.last().and_then(|last| self.type_of(last)),
            Expr::Call { name, .. } => {
                if name == &self.own {
                    return Some(self.returns);
                }
                check::host_fn(name).map(|primitive| primitive.returns)
            }
            Expr::Set { .. } | Expr::While { .. } => Some(Type::Unit),
        }
    }

    fn binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> Result<(), String> {
        // `&&` et `||` court-circuitent : ce n'est pas traduisible par une
        // instruction arithmétique, mais par un `if` qui omet la branche droite.
        match op {
            BinOp::And | BinOp::Or => {
                self.expr(left)?;
                self.line("if (result i32)");
                self.indent += 1;
                if op == BinOp::Or {
                    self.line("i32.const 1");
                } else {
                    self.expr(right)?;
                }
                self.indent -= 1;
                self.line("else");
                self.indent += 1;
                if op == BinOp::Or {
                    self.expr(right)?;
                } else {
                    self.line("i32.const 0");
                }
                self.indent -= 1;
                self.line("end");
                return Ok(());
            }
            _ => {}
        }

        self.expr(left)?;
        self.expr(right)?;
        let instruction = match op {
            BinOp::Add => "i64.add",
            BinOp::Sub => "i64.sub",
            BinOp::Mul => "i64.mul",
            BinOp::Div => "i64.div_s",
            BinOp::Rem => "i64.rem_s",
            BinOp::Lt => "i64.lt_s",
            BinOp::Le => "i64.le_s",
            BinOp::Gt => "i64.gt_s",
            BinOp::Ge => "i64.ge_s",
            BinOp::Eq if self.type_of(left) == Some(Type::Int) => "i64.eq",
            BinOp::Ne if self.type_of(left) == Some(Type::Int) => "i64.ne",
            BinOp::Eq => "i32.eq",
            BinOp::Ne => "i32.ne",
            BinOp::And | BinOp::Or => unreachable!("traité plus haut"),
        };
        self.line(instruction);
        Ok(())
    }
}

fn escape(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        match byte {
            b'"' => text.push_str("\\\""),
            b'\\' => text.push_str("\\\\"),
            0x20..=0x7e => text.push(*byte as char),
            other => text.push_str(&format!("\\{other:02x}")),
        }
    }
    text
}

/// Émet le module. Le programme doit avoir été vérifié par `check::check`.
/// La déclaration wasm d'une primitive : une chaîne occupe deux i32 (adresse, longueur),
/// un entier un i64, un booléen un i32, et Unit ne rend rien. C'est la même règle que
/// [`Emitter::wasm_type`], appliquée à la signature de la table.
fn import_declaration(primitive: &check::HostFn) -> String {
    let mut params = String::new();
    for ty in primitive.params {
        match ty {
            Type::Str => params.push_str(" i32 i32"),
            Type::Int => params.push_str(" i64"),
            Type::Bool => params.push_str(" i32"),
            Type::Unit => {}
        }
    }
    let params = if params.is_empty() {
        String::new()
    } else {
        format!(" (param{params})")
    };
    let result = match primitive.returns {
        Type::Unit => String::new(),
        ty => format!(" (result {})", Emitter::wasm_type(ty)),
    };
    format!(
        "  (import \"host\" \"{n}\" (func ${n}{params}{result}))\n",
        n = primitive.name
    )
}

pub fn emit(function: &Function) -> Result<Wat, String> {
    let returns = check::check(function)?;

    let mut emitter = Emitter {
        lines: Vec::new(),
        indent: 0,
        own: function.name.clone(),
        returns,
        data: Vec::new(),
        imports: Vec::new(),
        locals: function
            .bindings
            .iter()
            .map(|b| (b.name.clone(), b.ty))
            .collect(),
        params: function
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty))
            .collect(),
    };

    // Le corps d'abord : c'est là qu'on découvre les imports et les chaînes,
    // dont dépendent les sections mémoire et data écrites avant lui.
    emitter.indent = 2;
    for binding in &function.bindings {
        emitter.expr(&binding.value)?;
        let Some((index, _)) = emitter.local_of(&binding.name) else {
            return Err(format!("`{}` n'a pas de local wasm", binding.name));
        };
        emitter.line(format!("local.set {index}"));
    }
    emitter.expr(&function.body)?;
    let body_lines = std::mem::take(&mut emitter.lines);
    emitter.indent = 0;

    let mut module = String::from("(module\n");
    for import in &emitter.imports {
        if *import == FUEL {
            // Le budget de boucle. Ce n'est pas une primitive du langage : le
            // modèle ne l'écrit pas, l'émetteur l'insère dans chaque `while`.
            module.push_str("  (import \"host\" \"fuel\" (func $fuel (result i32)))\n");
        } else if let Some(primitive) = check::host_fn(import) {
            module.push_str(&import_declaration(primitive));
        }
    }
    if !emitter.data.is_empty() {
        module.push_str("  (memory (export \"memory\") 1)\n");
        for (offset, bytes) in &emitter.data {
            module.push_str(&format!(
                "  (data (i32.const {offset}) \"{}\")\n",
                escape(bytes)
            ));
        }
    }

    let mut signature = format!("  (func ${} (export \"{}\")", Wat::EXPORT, Wat::EXPORT);
    for (index, param) in emitter.params.iter().enumerate() {
        signature.push_str(&format!(
            " (param ${} {})",
            index,
            Emitter::wasm_type(param.1)
        ));
    }
    if returns != Type::Unit {
        signature.push_str(&format!(" (result {})", Emitter::wasm_type(returns)));
    }
    module.push_str(&signature);
    module.push('\n');
    // Les locaux suivent les paramètres : c'est l'ordre d'indexation wasm.
    for (index, (_, ty)) in emitter.locals.iter().enumerate() {
        module.push_str(&format!(
            "    (local ${} {})\n",
            index + emitter.params.len(),
            Emitter::wasm_type(*ty)
        ));
    }
    for line in body_lines {
        module.push_str(&line);
        module.push('\n');
    }
    module.push_str("  )\n)\n");

    Ok(Wat {
        text: module,
        imports: emitter.imports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn wat(source: &str) -> Wat {
        emit(&parse(source).unwrap()).unwrap()
    }

    #[test]
    fn a_pure_module_imports_nothing() {
        let module = wat("fn run(a: Int) -> Int { a + 1 }");
        assert!(module.imports.is_empty());
        assert!(!module.text.contains("(import"));
        assert!(!module.text.contains("(memory"));
        assert!(
            module
                .text
                .contains("(func $run (export \"run\") (param $0 i64) (result i64)")
        );
        assert!(module.text.contains("i64.const 1"));
        assert!(module.text.contains("i64.add"));
    }

    #[test]
    fn only_the_primitives_actually_called_are_imported() {
        assert_eq!(wat("fn run() -> Int { now() }").imports, vec!["now"]);
        assert_eq!(
            wat("fn run() -> Unit { emit(\"a\") }").imports,
            vec!["emit"]
        );
    }

    #[test]
    fn a_string_literal_lands_in_memory_at_a_non_zero_offset() {
        let module = wat("fn run() -> Unit { emit(\"créé\") }");
        assert!(module.text.contains("(memory (export \"memory\") 1)"));
        // « créé » fait quatre caractères et SIX octets : la longueur émise est
        // celle des octets, sinon l'hôte lirait deux octets de trop.
        assert!(
            module
                .text
                .contains("(data (i32.const 8) \"cr\\c3\\a9\\c3\\a9\")"),
            "{}",
            module.text
        );
        assert!(module.text.contains("i32.const 6"), "{}", module.text);
    }

    #[test]
    fn two_identical_strings_share_one_slot() {
        let module = wat("fn run(b: Bool) -> Unit { if b { emit(\"x\") } else { emit(\"x\") } }");
        assert_eq!(module.text.matches("(data ").count(), 1, "{}", module.text);
    }

    #[test]
    fn a_binding_becomes_a_local_after_the_parameters() {
        let module = wat("fn run(a: Int) -> Int { let b: Int = a * 2; b }");
        assert!(module.text.contains("(local $1 i64)"), "{}", module.text);
        assert!(module.text.contains("local.set 1"), "{}", module.text);
    }

    #[test]
    fn an_if_carries_its_result_type() {
        let module = wat("fn run(b: Bool) -> Int { if b { 1 } else { 2 } }");
        assert!(module.text.contains("if (result i64)"), "{}", module.text);
        assert!(module.text.contains("else"), "{}", module.text);
    }

    #[test]
    fn and_omits_the_right_branch_when_the_left_is_false() {
        let module = wat("fn run(a: Int) -> Bool { a > 0 && now() > 0 }");
        // La forme attendue : un `if` sur la gauche, la droite seulement dans la
        // branche « vrai », et un zéro dans la branche « faux ».
        let and_block = module
            .text
            .split("if (result i32)")
            .nth(1)
            .unwrap()
            .to_string();
        let before_else = and_block.split("else").next().unwrap();
        assert!(before_else.contains("call $now"), "{}", module.text);
        let after_else = and_block.split("else").nth(1).unwrap();
        assert!(!after_else.contains("call $now"), "{}", module.text);
    }

    #[test]
    fn a_unit_function_has_no_result_type() {
        let module = wat("fn run() -> Unit { emit(\"a\") }");
        assert!(!module.text.contains("(result"), "{}", module.text);
        assert!(
            module.text.contains("(func $run (export \"run\")"),
            "{}",
            module.text
        );
    }

    /// Le piège que ce test existe pour attraper : `a == b` sur deux `Int`
    /// doit produire `i64.eq`. Une version antérieure émettait `i32.eq` parce
    /// que le type d'une variable était inconnu à l'émission — le module était
    /// alors rejeté par n'importe quel moteur, sans que rien ici ne le signale.
    #[test]
    fn equality_picks_the_width_of_its_operands() {
        let ints = wat("fn run(a: Int, b: Int) -> Bool { a == b }");
        assert!(ints.text.contains("i64.eq"), "{}", ints.text);
        assert!(!ints.text.contains("i32.eq"), "{}", ints.text);

        let bools = wat("fn run(a: Bool, b: Bool) -> Bool { a != b }");
        assert!(bools.text.contains("i32.ne"), "{}", bools.text);
        assert!(!bools.text.contains("i64.ne"), "{}", bools.text);
    }

    #[test]
    fn an_ill_typed_program_is_refused_before_emitting() {
        let function = parse("fn run() -> Bool { 1 }").unwrap();
        assert!(emit(&function).is_err());
    }

    #[test]
    fn une_primitive_d_appareil_a_son_import_et_marque_la_coquille() {
        let function =
            parse("fn run() -> Int { device_int(\"healthSteps\", \"{}\", \"count\") }").unwrap();
        let wat = emit(&function).unwrap();
        assert_eq!(wat.imports, vec!["device_int"]);
        assert!(wat.text.contains(
            "(import \"host\" \"device_int\" (func $device_int (param i32 i32 i32 i32 i32 i32) (result i64)))"
        ));
        assert!(wat.shell_only());
        let pur = emit(&parse("fn run() -> Int { emit(\"a\"); now() }").unwrap()).unwrap();
        assert!(!pur.shell_only());
        assert!(
            pur.text
                .contains("(import \"host\" \"emit\" (func $emit (param i32 i32)))")
        );
        assert!(
            pur.text
                .contains("(import \"host\" \"now\" (func $now (result i64)))")
        );
    }
}
