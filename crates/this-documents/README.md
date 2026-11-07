# this-documents

Le cœur pur d'une pipeline de documents : reconnaître un format par ses octets, en tirer du texte, le découper en morceaux qui **pavent** la source (leur concaténation redonne le texte), retrouver un extrait dans la source, et produire des enregistrements (`DocumentRecord`, `ChunkRecord`) prêts à être écrits dans un graphe.

## Ce que le crate ne fait pas

Aucun I/O, aucun async, aucun graphe, aucune horloge : il consomme des octets et rend des données. Stocker les octets, écrire les nœuds, exposer des outils à un agent, c'est le travail de l'application. Il n'extrait pas XLSX ni PPTX, ne fait pas d'OCR, et ne lit pas le texte d'une image.

## Features

- *(défaut)* : texte brut, reconnaissance des images.
- `docx` : Word, via `zip` et `quick-xml`.
- `pdf` : texte par page, via `pdf_oxide` (pur Rust, nettement plus lourd).

Un format reconnu mais non compilé donne `ExtractError::FeatureDisabled`, distinct de `UnsupportedFormat`.

## Usage

```rust
let texte = ExtractorRegistry::with_builtins().extract(&octets, Some("note.md"))?;
let morceaux = chunk_text(&texte.text, &ChunkConfig::default());
let place = locate(&texte.text, "une citation", &AlignConfig::default()); // None si introuvable
```

Ajouter un format : voir `atelier/PIECES-JOINTES.md`, §5.
