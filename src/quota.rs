//! Le plafond de dépense par personne et par jour.
//!
//! Compté côté serveur sur l'usage réel renvoyé par l'API — jamais estimé — et vérifié
//! AVANT l'appel : mieux vaut refuser proprement que découvrir la dépense après coup. La
//! jauge est renvoyée avec chaque réponse pour que l'interface montre la position, pas un
//! total abstrait.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::cerveau::Usage;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Jauge {
    pub utilises: i64,
    pub plafond: i64,
}

impl Jauge {
    pub fn reste(&self) -> i64 {
        (self.plafond - self.utilises).max(0)
    }
    /// Position sur dix segments, pour une jauge sans chiffre.
    pub fn segment(&self) -> u8 {
        if self.plafond <= 0 {
            return 10;
        }
        ((self.utilises * 10) / self.plafond).clamp(0, 10) as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlafondAtteint(pub Jauge);

impl std::fmt::Display for PlafondAtteint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plafond de la journée atteint. Ça repart demain.")
    }
}
impl std::error::Error for PlafondAtteint {}

pub fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS usage_jour (
            owner      TEXT NOT NULL,
            jour       TEXT NOT NULL,
            tokens_in  INTEGER NOT NULL DEFAULT 0,
            tokens_out INTEGER NOT NULL DEFAULT 0,
            appels     INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (owner, jour)
        );
        "#,
    )
}

/// Tokens déjà consommés ce jour (entrée + sortie).
pub fn utilises(conn: &Connection, owner: &str, jour: &str) -> i64 {
    conn.query_row(
        "SELECT tokens_in + tokens_out FROM usage_jour WHERE owner = ?1 AND jour = ?2",
        params![owner, jour],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// À appeler avant le cerveau : la jauge si on peut, l'erreur si le plafond est atteint.
pub fn verifier(conn: &Connection, owner: &str, jour: &str, plafond: i64) -> Result<Jauge, PlafondAtteint> {
    let j = Jauge { utilises: utilises(conn, owner, jour), plafond };
    if j.utilises >= plafond {
        Err(PlafondAtteint(j))
    } else {
        Ok(j)
    }
}

/// À appeler après le cerveau, avec l'usage réel. Renvoie la jauge à jour.
pub fn ajouter(conn: &Connection, owner: &str, jour: &str, usage: Usage, plafond: i64) -> rusqlite::Result<Jauge> {
    conn.execute(
        "INSERT INTO usage_jour(owner, jour, tokens_in, tokens_out, appels) VALUES(?1, ?2, ?3, ?4, 1)
         ON CONFLICT(owner, jour) DO UPDATE SET
             tokens_in = tokens_in + ?3, tokens_out = tokens_out + ?4, appels = appels + 1",
        params![owner, jour, usage.tokens_in, usage.tokens_out],
    )?;
    Ok(Jauge { utilises: utilises(conn, owner, jour), plafond })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compte_et_plafonne() {
        let c = Connection::open_in_memory().unwrap();
        init_db(&c).unwrap();
        assert_eq!(verifier(&c, "a", "2026-09-28", 100).unwrap().utilises, 0);
        let j = ajouter(&c, "a", "2026-09-28", Usage { tokens_in: 60, tokens_out: 30, tokens_caches: 0 }, 100).unwrap();
        assert_eq!(j.utilises, 90);
        assert_eq!(j.segment(), 9);
        assert_eq!(j.reste(), 10);
        assert!(verifier(&c, "a", "2026-09-28", 100).is_ok());
        ajouter(&c, "a", "2026-09-28", Usage { tokens_in: 10, tokens_out: 0, tokens_caches: 0 }, 100).unwrap();
        assert_eq!(verifier(&c, "a", "2026-09-28", 100), Err(PlafondAtteint(Jauge { utilises: 100, plafond: 100 })));
        // Une autre personne, un autre jour : compteurs séparés.
        assert!(verifier(&c, "b", "2026-09-28", 100).is_ok());
        assert!(verifier(&c, "a", "2026-09-29", 100).is_ok());
    }
}
