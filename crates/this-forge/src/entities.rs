//! Les quatre entités de la forge, déclarées avec `impl_data_entity!` : le framework en
//! tire le descripteur (`Account::descriptor()`…) et le stockage (`DataService`).

use this::descriptor::{DescriptorSet, ModuleDescriptor};
use this::impl_data_entity;

// `name` (champ commun de la macro) porte la clé de recherche de chaque entité.

// name = e-mail normalisé (minuscules)
impl_data_entity!(Account, "account", ["name"], {
    password_hash: String,
});

// name = sha256 du code d'invitation (le code en clair n'est jamais stocké)
impl_data_entity!(Invitation, "invitation", ["name"], {
    uses_left: i64,
    note: Option<String>,
});

// name = sha256 du jeton de rafraîchissement
impl_data_entity!(Session, "session", ["name"], {
    account_id: String,
    expires_at: i64,
});

// Le constructeur généré par la macro prend un argument par champ : le lint est
// légitime pour du code écrit à la main, pas pour une expansion.
mod build_entity {
    #![allow(clippy::too_many_arguments)]
    use this::impl_data_entity;

    // name = nom du module. `status` ∈ queued | building | failed | signed.
    impl_data_entity!(Build, "build", ["name", "account_id"], {
        account_id: String,
        version: String,
        toolchain: String,
        source_sha256: String,
        started_at: Option<String>,
        finished_at: Option<String>,
        duration_ms: i64,
        diagnostics_json: String,
        wasm_sha256: Option<String>,
        wasm_size: Option<i64>,
        signature: Option<String>,
        manifest_b64: Option<String>,
    });
}
pub use build_entity::Build;

/// Le module descripteur de la forge : les quatre entités, vérifiées par le même
/// `DescriptorSet` que n'importe quel module du framework.
pub fn forge_descriptor() -> ModuleDescriptor {
    let mut m = ModuleDescriptor::new("forge", env!("CARGO_PKG_VERSION"));
    m.entities.push(Account::entity_desc_owned());
    m.entities.push(Invitation::entity_desc_owned());
    m.entities.push(Session::entity_desc_owned());
    m.entities.push(Build::entity_desc_owned());
    m.normalize();
    m
}

/// Le descripteur inséré dans un `DescriptorSet` : échoue si le contrat est incohérent.
pub fn forge_descriptor_set() -> anyhow::Result<DescriptorSet> {
    let mut set = DescriptorSet::new();
    set.insert(forge_descriptor())
        .map_err(|e| anyhow::anyhow!("descripteur de la forge incohérent : {e:?}"))?;
    Ok(set)
}

trait OwnedDesc {
    fn entity_desc_owned() -> this::descriptor::EntityDesc;
}
macro_rules! owned {
    ($($t:ty),*) => {$(
        impl OwnedDesc for $t {
            fn entity_desc_owned() -> this::descriptor::EntityDesc {
                <$t as this::descriptor::Described>::entity_desc().clone()
            }
        }
    )*};
}
owned!(Account, Invitation, Session, Build);
