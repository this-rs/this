# compteur — un module qui calcule son arbre de vues

`export!(Compteur, render = rendre)` ajoute l'export `render` (ABI de vue, `this-module-harness::render`).
Sans `render = …`, le module n'exporte pas `render` (`cargo build --no-default-features` le montre).

```rust
use this_module_sdk::views::*;
use this_module_sdk::{ident, text};

pub fn rendre(req: serde_json::Value) -> Result<View, ViewError> {
    let n = req["ouvertes"].as_u64().unwrap_or(0);
    let bandeau = if n == 0 {
        Node::banner(Tone::Success, text!("Tout est fait"))
    } else {
        Node::banner(Tone::Info, Text::new(format!("{n} à faire"))?)
    };
    let liste = Node::leaf(Brick::list(ident!("tache"), ident!("titre"))
        .filter(Filter::eq(ident!("fait"), false)));
    View::new(ident!("accueil"), Surface::Main, Node::stack(bandeau, [liste]))
}
```

`ident!`/`text!` sont vérifiés à la compilation ; un conteneur vide ou une brique incomplète ne s'écrit pas.
Un échec de rendu (`Err`, requête illisible) devient une vue d'erreur valide, jamais un trap.
