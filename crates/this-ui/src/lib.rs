//! # `ui-shell` — les composants que toute app this-rs en Leptos refait
//!
//! Ils ne connaissent ni la boutique, ni le module : uniquement `this-app` (les contrats
//! purs) et le DOM.
//!
//! - [`Splash`] — l'écran de démarrage : ce qui se charge, ce qui a échoué, un bouton
//!   pour réessayer. Présentation pure : la temporisation vit dans `this_app::Boot`.
//! - [`install_chrome`] — l'habillage système (feux macOS, plein écran, encoche, Dynamic
//!   Island, barre d'accueil) traduit en variables CSS `--chrome-*` et attributs `data-*`.
//! - [`behave_like_app`] — la coquille se comporte comme une app (pas de zoom, pas de
//!   sélection accidentelle, pas de rebond) ; le site, lui, reste une page.
//! - [`design`] — les primitives du design system (ton, pastille, jauge, tuile, vide).
//! - [`browser`] — le tableau d'entités : le kit décide comment, l'app décide quoi.
//! - [`model`] — le modèle d'affichage d'un module, dérivé du descripteur typé.
//! - [`device`] — la coquille web : capacités, permissions système, notifications.
//! - [`toasts`] — le rendu de la file d'avis.
//! - [`store`] — le magasin d'enregistrements, une piste réactive par entité.
//! - [`Panel`] — la coquille d'un panneau latéral (voile, en-tête, corps, fermeture).
//! - [`SettingsPanel`] — apparence (thème, mouvement) + faits d'accès fournis par l'app.
//! - [`storage`] / [`theme`] — la persistance `localStorage` et l'application au `<html>`.

pub mod app_mode;
pub mod browser;
pub mod chrome;
pub mod design;
pub mod device;
pub mod lifecycle;
pub mod model;
pub mod panel;
pub mod settings_panel;
pub mod splash;
pub mod storage;
pub mod store;
pub mod theme;
pub mod toasts;

pub use app_mode::behave_like_app;
pub use browser::{Cell, Column, EntityTable, Row};
pub use chrome::{drag_region, install_chrome, use_chrome};
pub use design::{Tone, badge, empty, kpi, meter};
pub use device::BrowserDevice;
pub use lifecycle::{is_online, on_resume};
pub use model::{Entity, Field, Kind, Link, Module, count, money_f};
pub use panel::Panel;
pub use settings_panel::SettingsPanel;
pub use splash::Splash;
pub use storage::LocalStorage;
pub use store::Store;
pub use theme::{apply_settings, now_ms};
pub use this_platform as platform;
pub use toasts::ToastHost;
