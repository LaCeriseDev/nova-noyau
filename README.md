# nova-noyau

Le noyau NOVA, et rien d'autre — ce que Mia a prouvé essentiel en n'en gardant que ça.

| module | ce qu'il garantit |
|---|---|
| `cerveau` | l'API Messages **sans outil**, une sortie JSON bornée par un schéma, l'usage réel compté |
| `contrat` | le contrat d'intent validé **côté serveur**, et la convention d'honnêteté `cannot` |
| `chaine` | proposer → prévisualiser → confirmer → exécuter : jeton HMAC lié à l'action, à la personne, à une échéance, à usage unique ; journal d'audit |
| `eav` | le « champ qui mange tout » : attributs libres typés à l'écriture, jamais de perte d'information |
| `acces` | l'identité par Cloudflare Access, jamais déclarative : signature RS256 vérifiée, allowlist locale, refus tracés |
| `quota` | plafond par personne et par jour, vérifié avant l'appel, jauge en dix segments |
| `reponses` | (feature `axum`) 401 / 400 / 429 / 424 / 403 identiques dans tous les services |

Le noyau ne connaît aucun domaine : pas de table métier, pas de prompt, pas de rendu. Chaque
projet garde ça chez lui. Le noyau, lui, ne se copie plus d'un projet à l'autre : une
correction ici vaut pour tous.

```toml
[dependencies]
nova-noyau = { git = "https://github.com/LaCeriseDev/nova-noyau", tag = "v0.1.0" }
```

```rust
let cerveau = Cerveau::nouveau(api_key, "claude-opus-5", "low");
let jauge = quota::verifier(&conn, &qui.email, &util::today(), plafond)?;      // 429 sinon
let r = cerveau.envoyer(&Appel::nouveau().systeme(consigne).fil(&fil).utilisateur(msg).schema(contrat)).await?;
let intent = r.json.unwrap();
if let Some(raison) = contrat::est_refus(&intent) { /* dire non, proprement */ }
assert!(contrat::valider(&contrat, &intent).is_empty());                       // on ne croit pas le réseau
let jeton = chaine.proposer(intent, &qui.email);                               // à montrer, puis à confirmer
quota::ajouter(&conn, &qui.email, &util::today(), r.usage, plafond)?;
```

Tests : `cargo test` (27). Sans compilateur C sur la machine : `docker run --rm -v $PWD:/w -w /w rust:1.90-bookworm cargo test`.

Provenance : réécrit depuis Mia (projet personnel). La chaîne d'écriture est écrite depuis sa
spécification, pas reprise d'un dépôt tiers.
