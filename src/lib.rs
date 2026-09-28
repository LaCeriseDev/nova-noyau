//! # nova-noyau
//!
//! Ce que NOVA a d'essentiel, et rien d'autre — tel que Mia l'a prouvé en n'en gardant que ça :
//!
//! - [`cerveau`] : un modèle appelé par l'API Messages **sans aucun outil**, qui ne répond
//!   que dans un JSON conforme au contrat qu'on lui impose. Jamais de SQL ni d'UI libre.
//! - [`contrat`] : le contrat d'intent — un schéma, une validation côté serveur, et la
//!   convention d'honnêteté (`cannot`) qui vaut mieux qu'un repli silencieux.
//! - [`chaine`] : la chaîne d'écriture vérifiée — proposer, prévisualiser, confirmer,
//!   exécuter — avec jeton HMAC à usage unique et journal d'audit.
//! - [`eav`] : le « champ qui mange tout » — un schéma qu'on ne fige pas, des attributs
//!   libres typés à l'écriture.
//! - [`acces`] : l'identité par Cloudflare Access, jamais déclarative : la seule source
//!   admise est la signature RS256 du jeton, vérifiée contre les clés de l'équipe.
//! - [`quota`] : le plafond de dépense par personne et par jour, compté sur l'usage réel.
//!
//! Chaque projet garde son domaine (tables, prompts, rendus) ; le noyau, lui, ne se copie
//! plus — il se met à jour d'un coup pour tous.

pub mod acces;
pub mod cerveau;
pub mod chaine;
pub mod contrat;
pub mod eav;
pub mod quota;
pub mod util;

#[cfg(feature = "axum")]
pub mod reponses;

pub use cerveau::{Appel, Cerveau, ErreurCerveau, Reponse, Usage};
pub use chaine::{ChaineEcriture, ErreurChaine, Jeton};
pub use contrat::{valider, Ecart};
pub use quota::{Jauge, PlafondAtteint};
