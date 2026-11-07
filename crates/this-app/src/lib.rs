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
//!
//! Ce que les **modules** apportent à l'app (aucune liste en dur) :
//!
//! - [`menu`] — le menu construit depuis les vues déclarées, avec la raison de chaque
//!   vue écartée (module absent, remplacée, identifiant déjà pris) ;
//! - [`module_notices`] — les avis publiés par les modules : niveau, durée, action,
//!   sans doublon par clé, ordonnés, expirés à l'horloge de l'appelant ;
//! - [`module_settings`] — les sections de réglages à valeurs fermées, validées et
//!   persistées ;
//! - [`module_lifecycle`] — installation, activation, désactivation, mise à jour, échec,
//!   chacun dicible à la personne ; l'ensemble des modules actifs alimente les trois autres.
//!
//! **Ce qui n'est plus ici** : la politique d'accès. Elle appartient au contrat du module
//! — `this_descriptor::Capabilities` — et se calcule depuis le descripteur et la session.
//! La réinventer dans l'application était une duplication, pas un contrat.

pub mod boot;
pub mod change;
pub mod chrome;
pub mod device;
pub mod menu;
pub mod module_lifecycle;
pub mod module_notices;
pub mod module_settings;
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
pub use menu::{
    DeclaredView, Entry, Menu, ModuleViews, Omission, Placement, Reason, ViewRef, build_menu,
};
pub use module_lifecycle::{
    Event, Lifecycle, LifecycleError, ModuleState, Modules, Phase, StatusLine,
};
pub use module_notices::{
    Board, Level, ModuleNotice, NoticeAction, NoticeError, Posted, Published,
};
pub use module_settings::{
    Choice, Expected, Field, FieldKind, Registry, Section, SettingValue, SettingsError,
};
pub use notices::{DEFAULT_MAX, DEFAULT_TTL_MS, Live, Notices};
pub use settings::{MemoryStore, Settings, Store, Theme};
pub use surface::Surface;
