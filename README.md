# dev-metrics

`dev-metrics` rassemble l'activité de développement de GitHub et Bitbucket Cloud.
Il compare les métriques sur 7 et 30 jours, ou sur une période choisie, et peut
aussi afficher l'activité brute. Les résultats sont disponibles en JSON, en tableau
ou, pour les rapports, en résumé Markdown.

## Installation

Avec [Rust et Cargo](https://rustup.rs/) installés :

```sh
cargo install --git https://github.com/SebastienElet/dev-metrics
```

## Configuration

Exécutez `dev-metrics config init` ou créez `~/.config/dev-metrics/config.toml` en remplaçant les valeurs d'exemple :

```toml
[github]
token = "votre-token-github"

[bitbucket]
token = "votre-token-bitbucket"
email = "prenom.nom@domaine.com"
workspace = "votre-workspace"
```

Si `gh` est connecté à GitHub, `[github].token` est facultatif. Pour Bitbucket,
`BITBUCKET_TOKEN` peut remplacer le token du fichier. `--workspace` remplace le
workspace configuré pour une commande. Ne commitez pas vos tokens.

## Tokens

**GitHub** — [Créer un fine-grained PAT](https://github.com/settings/personal-access-tokens/new) limité aux dépôts concernés.
Permissions en lecture : Metadata (incluse), Contents et Pull requests.
Alternative classic : `repo` pour les dépôts privés ou `public_repo` pour les publics ; `read:org` si vos organisations le demandent.

**Bitbucket Cloud** — [Créer un API token](https://id.atlassian.com/manage-profile/security/api-tokens) avec scopes.
Permissions en lecture : `read:user:bitbucket`, `read:workspace:bitbucket`, `read:repository:bitbucket`, `read:pullrequest:bitbucket`.
`bitbucket.email` est l'email Atlassian pour l'authentification Basic ; sans lui, le token utilise Bearer. `config init --bitbucket-username EMAIL` remplit aussi cette clé.
`bitbucket.username` désigne le compte dont les PR sont suivies (slug, Account ID ou UUID), pas l'email d'authentification.
Vérifiez les accès avec `dev-metrics config check` (ou `--format json`) ; la commande sonde un dépôt accessible par source.

## Exemples

Après configuration des sources utilisées :

```sh
dev-metrics report --format table
dev-metrics report --source github,bitbucket --format markdown
dev-metrics schema
```

Pour les autres options et les commandes d'activité brute, consultez
`dev-metrics --help` puis l'aide de chaque commande. `dev-metrics schema` affiche
le schéma JSON du rapport pour les intégrations et les agents.

`--quiet` désactive la progression ; `--verbose` détaille les appels API sur stderr, uniquement dans un terminal (`--quiet` prioritaire).

La collecte GitHub regroupe les PR en lots GraphQL, avec au plus huit requêtes
simultanées. `--verbose` affiche les candidats, les appels par endpoint et les attentes
de limite de débit. Le cache local `~/.cache/dev-metrics` réutilise les PR et issues
inchangées (`updated_at`) et les commits par SHA ; `--no-cache` désactive sa lecture
et son écriture. Les recherches restent fraîches à chaque lancement.

Les limites de débit GitHub déclenchent jusqu’à quatre tentatives, avec un budget
d’attente de 300 s par requête ; en cas d’arrêt, l’heure de reprise est indiquée
lorsqu’elle peut être déterminée.
Un détail indisponible écarte la PR, le commit ou l’issue concerné ; l’avertissement
figure dans le JSON, dans `--verbose` et dans le compte final des avertissements.
