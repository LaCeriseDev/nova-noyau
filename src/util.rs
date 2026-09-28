use time::OffsetDateTime;

/// Secondes depuis l'époque Unix.
pub fn now_secs() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// La date du jour en heure locale, `AAAA-MM-JJ`. C'est la clé des compteurs de quota :
/// « ça repart demain » doit vouloir dire demain pour la personne, pas demain UTC.
pub fn today() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let f = time::macros::format_description!("[year]-[month]-[day]");
    now.format(&f).unwrap_or_else(|_| "1970-01-01".into())
}

/// Coupe une chaîne à `n` caractères (pas octets : on ne casse jamais un accent).
pub fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respecte_les_caracteres() {
        assert_eq!(truncate("éàü", 2), "éà");
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn today_a_la_bonne_forme() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert_eq!(&t[4..5], "-");
    }
}
