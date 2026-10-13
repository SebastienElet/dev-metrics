# dev-metrics

CLI Rust pour suivre ses métriques de dev (GitHub, Bitbucket) et générer une note Obsidian hebdomadaire.

Ce dépôt fournit actuellement le squelette CLI. `report` et `schedule` chargent la
configuration puis affichent un message « non implémenté ». Ils ne génèrent aucun
rapport et ne planifient aucune tâche.

## Installation

Installer la toolchain Rust stable avec [rustup](https://rust-lang.org/tools/install/),
puis, depuis la racine du dépôt :

```sh
cargo install --path . --locked
```

Le binaire `dev-metrics` est installé dans le répertoire des binaires Cargo
(par défaut `~/.cargo/bin`), qui doit figurer dans le `PATH`.

## Configuration

```sh
mkdir -p ~/.config/dev-metrics
cp config.example.toml ~/.config/dev-metrics/config.toml
```

La configuration TOML contient un champ obligatoire :

```toml
output_dir = "./reports"
```

`output_dir` désigne le répertoire prévu pour les rapports. Il est uniquement
affiché par les placeholders ; aucun répertoire n'est créé. Un chemin relatif
est relatif au répertoire de travail, et `~` n'est pas développé dans cette valeur.

Par défaut, le fichier est lu depuis `$HOME/.config/dev-metrics/config.toml`,
y compris sur macOS. `--config PATH` remplace ce chemin et peut précéder ou suivre
la sous-commande. Un chemin relatif passé à `--config` est relatif au répertoire
de travail. Si `HOME` est absent ou vide, `--config` est nécessaire.

Un fichier absent, illisible, non UTF-8 ou invalide entraîne une erreur et un code
de sortie non nul. Les champs inconnus sont rejetés. L'aide et la version ne
nécessitent aucun fichier de configuration.

## Usage

```sh
dev-metrics --help
dev-metrics --version
dev-metrics report
dev-metrics schedule
dev-metrics --config config.example.toml report
dev-metrics schedule --config config.example.toml
```

Sans installation :

```sh
cargo run -- --help
cargo run -- --config config.example.toml report
```

## Développement

```sh
rustup component add rustfmt clippy
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Les tests unitaires couvrent le parsing CLI, la configuration d'exemple, la
résolution du chemin et les erreurs de lecture/parsing. Les tests d'intégration
lancent le binaire avec des fichiers temporaires pour vérifier l'aide, les
placeholders, la priorité de `--config` et les codes de sortie.

GitHub Actions exécute les trois contrôles sur Rust stable, sous Ubuntu, à chaque
`push` et `pull_request`.
