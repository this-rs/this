//! # L'habillage système : le contrat vit dans le framework
//!
//! Ce module ne contient plus de logique. La décision (ce qu'il faut réserver, la découpe
//! d'un iPhone, les variables CSS) appartient à `this_platform::chrome`, partagé par toutes
//! les coquilles et testé dans le framework. L'exemple le ré-exporte pour que son code garde
//! les mêmes chemins.

pub use this_platform::Platform;
pub use this_platform::chrome::*;
