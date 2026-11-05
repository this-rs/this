//! Ask a model to write a module — in the language *this* framework defines.
//!
//! ## Why this lives in the framework
//!
//! Only the framework knows its own language: the token set, the types, the host
//! primitives, the fact that there is no loop and no state. A host that wants a
//! model to write a module would otherwise have to re-describe all of that, and
//! would drift the moment the language changes. The description is therefore
//! written once, here, next to the compiler that enforces it.
//!
//! ## Why the model is a trait
//!
//! The framework has no opinion on which endpoint answers: OpenAI-compatible,
//! local, or a fake in a test. [`Completion`] is the seam — the same pattern as
//! [`crate::executor::Executor`], and for the same reason.
//!
//! ## What `draft` guarantees
//!
//! A drafted program **compiles**. The answer is parsed, then handed to the real
//! compiler; if it does not pass the type checker, `draft` fails with the
//! compiler's message and keeps the source. A model that invents a construct gets
//! caught here rather than in the user's face.

use crate::Error;

/// The seam: how a request reaches a model.
///
/// Asynchronous, and `Send` only off `wasm32`, because a browser completion goes
/// through a `!Send` future while a server's handlers need `Send`.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Completion {
    /// `system` describes the language; `request` is what the user asked for.
    async fn complete(&self, system: &str, request: &str) -> Result<String, String>;
}

/// Asks a model for a program, and refuses what does not compile.
pub struct Drafter<C: Completion> {
    completion: C,
}

impl<C: Completion> Drafter<C> {
    pub fn new(completion: C) -> Self {
        Self { completion }
    }

    /// Draft a program for `request`, or explain why nothing usable came back.
    pub async fn draft(&self, request: &str) -> Result<String, Error> {
        let answer = self
            .completion
            .complete(system_prompt(), request)
            .await
            .map_err(Error::Model)?;
        let source = extract_program(&answer).ok_or_else(|| Error::NoProgram(answer.clone()))?;

        // Le framework sait quelle langue il parle : il peut donc refuser.
        this_module_lang::compile(&source).map_err(|message| Error::NotCompiling {
            program: source.clone(),
            message,
        })?;
        Ok(source)
    }
}

/// The description a model receives. It is deliberately a specification, not a
/// tutorial: the compiler is the authority, this only says what it accepts.
pub fn system_prompt() -> &'static str {
    r#"You write programs in the this-rs module language. Answer with ONE fenced code block, nothing else.

GRAMMAR
  fn <name>(<param>: <Type>, ...) -> <Type> { <statements> }
  let <name>: <Type> = <expression>;
  if <expression> { <statements> } else { <statements> }
  the last expression of a block is its value

TYPES
  Int (64-bit), Bool, Str, Unit

OPERATORS
  arithmetic: + - * / %      comparison: == != < <= > >=
  boolean: && || !           grouping: ( )

HOST PRIMITIVES (effects — visible to the caller)
  emit(text: Str) -> Unit    writes a line
  now() -> Int               current time in milliseconds

PURE PRIMITIVES
  str_len(s: Str) -> Int
  str_eq(a: Str, b: Str) -> Bool
  str_starts_with(s: Str, prefix: Str) -> Bool
  str_ends_with(s: Str, suffix: Str) -> Bool
  str_contains(s: Str, needle: Str) -> Bool

WHAT DOES NOT EXIST — do not emit these
  no loops (no while, for, loop), no recursion, no mutation, no reassignment
  no floating point, no arrays, no structs, no generics, no closures
  no string concatenation with +, no string literals indexing
  no I/O other than emit

ENTRY POINT
  the program must define `fn run(...)`. Its parameters are the inputs, its return
  type is the answer.

EXAMPLE
  fn run(a: Int) -> Int {
      let doubled: Int = a * 2;
      emit("calcul fait");
      if doubled > 100 { doubled } else { doubled + 1 }
  }"#
}

/// Pull the program out of the model's answer.
///
/// A fenced block wins. Falling back to plain text only when the whole answer is
/// already a program keeps a chatty model from being accepted by accident.
fn extract_program(answer: &str) -> Option<String> {
    if let Some(start) = answer.find("```") {
        let rest = &answer[start + 3..];
        // La première ligne d'une clôture peut être le nom du langage.
        let body = match rest.find('\n') {
            Some(newline) => &rest[newline + 1..],
            None => rest,
        };
        if let Some(end) = body.find("```") {
            let program = body[..end].trim();
            if program.starts_with("fn ") {
                return Some(program.to_string());
            }
        }
    }

    let trimmed = answer.trim();
    if trimmed.starts_with("fn ") {
        return Some(trimmed.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un modèle qui rend ce qu'on lui donne.
    struct Fixed(&'static str);

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl Completion for Fixed {
        async fn complete(&self, _system: &str, _request: &str) -> Result<String, String> {
            Ok(self.0.to_string())
        }
    }

    async fn draft_from(answer: &'static str) -> Result<String, Error> {
        Drafter::new(Fixed(answer)).draft("une somme").await
    }

    #[tokio::test]
    async fn a_fenced_program_is_extracted_and_compiles() {
        let answer = "Voici :\n```text\nfn run(a: Int) -> Int { a + 1 }\n```\nBon test !";
        assert_eq!(
            draft_from(answer).await.unwrap(),
            "fn run(a: Int) -> Int { a + 1 }"
        );
    }

    #[tokio::test]
    async fn a_bare_program_is_accepted() {
        assert_eq!(
            draft_from("fn run() -> Int { 40 + 2 }").await.unwrap(),
            "fn run() -> Int { 40 + 2 }"
        );
    }

    #[tokio::test]
    async fn prose_without_a_program_is_refused() {
        match draft_from("Je ne peux pas vous aider sur ce point.").await {
            Err(Error::NoProgram(_)) => {}
            other => panic!("une réponse sans programme doit être refusée, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_program_that_does_not_compile_is_refused_with_the_compiler_message() {
        // Une boucle n'existe pas dans ce langage : le modèle a inventé.
        let answer = "```\nfn run() -> Int { while true { 1 } }\n```";
        match draft_from(answer).await {
            Err(Error::NotCompiling { program, message }) => {
                assert!(program.contains("while"));
                assert!(
                    !message.is_empty(),
                    "le message du compilateur doit être rendu"
                );
            }
            other => panic!("un programme qui ne compile pas doit être refusé, reçu {other:?}"),
        }
    }

    #[test]
    fn the_prompt_names_what_does_not_exist() {
        let prompt = system_prompt();
        for absent in ["no loops", "no recursion", "no floating point", "emit"] {
            assert!(prompt.contains(absent), "le prompt doit dire : {absent}");
        }
    }
}
