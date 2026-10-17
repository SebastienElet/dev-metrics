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

Créez `~/.config/dev-metrics/config.toml` en remplaçant les valeurs d'exemple :

```toml
[github]
token = "votre-token-github"

[bitbucket]
token = "votre-token-bitbucket"
workspace = "votre-workspace"
```

Si `gh` est connecté à GitHub, `[github].token` est facultatif. Pour Bitbucket,
`BITBUCKET_TOKEN` peut remplacer le token du fichier. `--workspace` remplace le
workspace configuré pour une commande. Ne commitez pas vos tokens.

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
