# dev-metrics

CLI Rust pour suivre ses métriques de développement (GitHub, Bitbucket).

La commande `github` collecte l'activité brute sur une période et la restitue en
JSON versionné ou en tableau. `report` et `schedule` restent des placeholders :
les métriques agrégées sur 7/30 jours, leurs deltas et la planification ne sont pas
encore implémentés. La source Bitbucket reste à ajouter.

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

[github]
token = "github_pat_..."
```

`output_dir` est conservé pour les placeholders. Il est uniquement
affiché par les placeholders ; aucun répertoire n'est créé. Un chemin relatif
est relatif au répertoire de travail, et `~` n'est pas développé dans cette valeur.

Par défaut, le fichier est lu depuis `$HOME/.config/dev-metrics/config.toml`,
y compris sur macOS. `--config PATH` remplace ce chemin et peut précéder ou suivre
la sous-commande. Un chemin relatif passé à `--config` est relatif au répertoire
de travail. Si `HOME` est absent ou vide, `--config` est nécessaire.

La table `[github]` et son `token` sont optionnels. La commande `github` utilise
d'abord `gh auth token --hostname github.com`, puis le token configuré si `gh`
est absent, en erreur, trop lent ou ne retourne pas de token valide. Un token
refusé par l'API entraîne une erreur sans changer de compte. Sans `--user`, le
compte authentifié est obtenu via `/user`. Un token doit donner accès aux dépôts
souhaités : lecture des pull requests, issues et contenus pour les dépôts privés.
Ne commitez pas votre configuration contenant un token.

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
dev-metrics github --from 2026-10-01 --to 2026-10-07
dev-metrics github --from 2026-10-01 --to 2026-10-07 --user octocat --format table
```

Sans installation :

```sh
cargo run -- --help
cargo run -- --config config.example.toml report
cargo run -- --config config.example.toml github --from 2026-10-01 --to 2026-10-07
```

## Données GitHub

Les bornes `--from` et `--to` sont des dates `YYYY-MM-DD`, inclusives en UTC.
« Ouvertes » désigne les PRs créées pendant la période, quel que soit leur état
actuel ; « mergées » désigne les PRs de l'utilisateur mergées pendant la période.
Les PRs anciennes recevant ou faisant l'objet d'une review pendant la période
sont également collectées. Le champ `author` permet de distinguer les PRs de
l'utilisateur de celles des autres.

Le JSON par défaut respecte [github-activity-v1.schema.json](schemas/github-activity-v1.schema.json)
et comporte `schema_version: 1`, `source`, `user`, `period` et les listes :

- `pull_requests` : métadonnées, dates de création/merge, première review reçue,
  additions/deletions et `files` avec chemins, ancien chemin en cas de renommage,
  additions et deletions par fichier.
- `reviews_given` : reviews soumises par l'utilisateur sur les PRs des autres.
- `reviews_received` : reviews soumises par d'autres sur ses PRs.
- `comments` : commentaires de discussion sur issues/PRs et commentaires de review
  laissés par l'utilisateur, distingués par `kind`.
- `commits` : SHA, message, date d'auteur, statistiques et fichiers modifiés.

Chaque PR, review, commentaire et commit conserve `source: "github"` et
`repository` (`owner/repo`). Les timestamps sont normalisés en UTC. La première
review reçue est cherchée dans tout l'historique disponible de la PR ; les
reviews personnelles et les reviews en attente sont exclues. Un auteur supprimé
est représenté par `null` sur les PRs et reviews ; une review soumise d'un auteur
supprimé reste une review reçue sur une PR de l'utilisateur.

Les statistiques restent brutes : les lockfiles et fichiers générés sont conservés
pour permettre leur exclusion configurable lors du calcul des métriques (#4).
Les lignes des PRs et celles des commits décrivent des vues différentes du même
travail et ne doivent pas être additionnées. Les commentaires de commits et les
corps des commentaires ne sont pas collectés.

`--format table` affiche ces données brutes, y compris les dates historiques utiles
pour les PRs retenues. Il ne calcule pas encore les fenêtres 7/30 jours et leurs
deltas (#4/#5). Une évolution incompatible du contrat JSON nécessitera une nouvelle
version du schéma. Les listes vides sont des tableaux vides, les dates absentes
valent `null`.

### Limites de l'API

La recherche GitHub ne voit que les données indexées accessibles au token ; la
[recherche des commits](https://docs.github.com/en/rest/search/search#search-commits)
porte sur les branches par défaut. Les changements d'état, commits ou commentaires
supprimés ne sont pas reconstruits. Les statistiques des PRs reflètent leur diff
au moment de la collecte, pas une photographie historique à la fin de la période.

Les recherches sont paginées et refusées lorsqu'elles sont incomplètes ou dépassent
la [limite GitHub de 1 000 résultats](https://docs.github.com/en/rest/search/search).
Les fichiers, reviews et commentaires sont également paginés. Une PR dépassant
3 000 fichiers est refusée ; un commit atteignant 3 000 fichiers est refusé car
l'API ne permet pas de distinguer ce cas d'une troncature. Un écart entre les
statistiques globales et celles des fichiers, ou une pagination incohérente,
entraîne une erreur.

La collecte effectue des requêtes séquentielles et peut rencontrer les limites de
débit sur les périodes chargées. Chaque requête a un timeout de 15 secondes
(connexion : 5 secondes), sans retry automatique ; `gh` est limité à 5 secondes.
Les erreurs HTTP/réseau ou de désérialisation échouent sans imprimer de résultat
partiel ni le contenu des tokens ou réponses en erreur.

## Développement

```sh
rustup component add rustfmt clippy
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Les tests couvrent le parsing CLI, la configuration et ses erreurs, puis lancent
le binaire pour vérifier l'aide, les placeholders et les codes de sortie. La
source GitHub utilise des réponses HTTP simulées avec `mockito` pour exercer la
collecte, la pagination, les dates, les erreurs et les sorties. Les snapshots
JSON/tableau et le moteur `jsonschema` vérifient le contrat de sortie. Les tests
d'authentification utilisent des exécutables temporaires sous Unix, sans accéder
aux identifiants de la machine. Ils ne prouvent pas l'exhaustivité de l'index GitHub.

GitHub Actions exécute les trois contrôles sur Rust stable, sous Ubuntu, à chaque
`push` et `pull_request`.
