//! # `this-app` — les contrats purs d'une app this-rs
//!
//! Ce qui reste quand on retire le DOM, Tauri et HTTP : testable sur l'hôte, sans
//! navigateur.
//!
//! - [`boot`] — la séquence de démarrage : un splash qui dit ce qu'il fait, qui ne
//!   clignote pas (durée minimale), et qui reste affiché s'il échoue.
//! - [`settings`] — les réglages de l'app (thème, mouvement réduit) et leur persistance
//!   derrière un trait de stockage, pour que la même logique serve `localStorage`,
//!   un fichier ou un store natif.
//! - [`change`] — le fil de changements : ce que le domaine publie après une écriture,
//!   ce qu'une interface applique pour corriger un enregistrement sans tout recharger.
//! - [`device`] — ce que la coquille sait faire (capacités, permissions système, avis) ;
//!   le kit **demande**, chaque coquille répond.
//! - [`notices`] — la file d'avis : une clé, une borne, une horloge passée par l'appelant.
//! - [`surface`] — où tourne l'app (desktop, mobile, web) et par quel transport.
//! - [`chrome`] — ce que le système pose sur la fenêtre (feux macOS, contrôles Windows,
//!   encoche, Dynamic Island, barre d'accueil) et ce que l'interface doit leur réserver.
//!
//! **Ce qui n'est plus ici** : la politique d'accès. Elle appartient au contrat du module
//! — `this_descriptor::Capabilities` — et se calcule depuis le descripteur et la session.
//! La réinventer dans l'application était une duplication, pas un contrat.

pub mod boot;
pub mod change;
pub mod chrome;
pub mod device;
pub mod notices;
pub mod settings;
pub mod surface;

pub use boot::{Boot, BootState};
pub use change::{Change, ChangeOp};
pub use chrome::{
    ChromeEnv, ChromeLayout, Cutout, CutoutSource, Insets, Platform, TitleBar, WindowState,
    layout as chrome_layout,
};
pub use device::{Capability, Device, NoDevice, Notice, PermissionState, Tone};
pub use notices::{DEFAULT_MAX, DEFAULT_TTL_MS, Live, Notices};
pub use settings::{MemoryStore, Settings, Store, Theme};
pub use surface::Surface;
