# dev-metrics

CLI Rust pour suivre ses métriques de développement (GitHub, Bitbucket).

`report` collecte et calcule les métriques sur 7/30 jours ou une période libre,
avec comparaison à la période précédente. Sa sortie par défaut est un JSON compact
versionné pour un bot LLM ; un tableau complet et un résumé Markdown sont disponibles.
`github` et `bitbucket` exposent également l'activité brute en JSON ou tableau.
Les résultats vont sur stdout. Aucune planification ni sortie Obsidian n'est fournie.

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

La configuration TOML peut être vide ou préciser les sources et exclusions :

```toml
[github]
token = "github_pat_..."

[bitbucket]
workspace = "mon-workspace"
token = "api_token_..."
```

L'ancien champ `output_dir` reste accepté, mais est facultatif et ignoré.
Aucun fichier ni répertoire de rapport n'est créé ; utilisez une redirection shell
pour conserver la sortie. La sous-commande placeholder `schedule` est supprimée.

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
de sortie non nul. Les champs inconnus sont rejetés. L'aide, la version et `schema` ne
nécessitent aucun fichier de configuration ni accès réseau.

## Usage

```sh
dev-metrics --help
dev-metrics --version
dev-metrics report
dev-metrics schema
dev-metrics --config config.example.toml report
dev-metrics report --until 2026-10-30 --format json
dev-metrics report --since 2026-10-01 --until 2026-10-15 --format markdown
dev-metrics report --source github,bitbucket --workspace mon-workspace --format table
dev-metrics github --from 2026-10-01 --to 2026-10-07
dev-metrics github --from 2026-10-01 --to 2026-10-07 --user octocat --format table
dev-metrics bitbucket --workspace mon-workspace --from 2026-10-01 --to 2026-10-07
dev-metrics bitbucket --from 2026-10-01 --to 2026-10-07 --user '{uuid}' --format table
```

Sans installation :

```sh
cargo run -- --help
cargo run -- --config config.example.toml report
cargo run -- --config config.example.toml github --from 2026-10-01 --to 2026-10-07
```

## Rapport pour un bot ou un humain

`report` utilise GitHub par défaut. `--source github,bitbucket` active les deux
sources ; `--source bitbucket` utilise seulement Bitbucket. Les doublons de sources
sont ignorés. `--github-user` et `--bitbucket-user` remplacent le compte authentifié
de chaque source ; `--workspace` remplace le workspace Bitbucket configuré.
Les options d'identité/workspace exigent la source correspondante dans `--source`.
L'authentification suit les mêmes règles que les commandes d'activité brute.

Sans `--since`, le rapport contient les fenêtres 7 et 30 jours, dans cet ordre.
`--until` fixe leur fin ; par défaut, c'est la date du jour en UTC. Avec `--since`,
le rapport contient une seule fenêtre allant de `--since` à `--until`, comparée à
la période immédiatement précédente de même durée. `--until` reste facultatif.
Les bornes sont des **dates ISO 8601 `YYYY-MM-DD`, inclusives en UTC**, pas des
timestamps. Une date invalide, une plage inversée ou une comparaison dépassant
les années 0000–9999 échoue. Par exemple, le 28–30 octobre est comparé au 25–27
octobre ; une journée inclut les événements de 00:00:00 à la fin de ce jour UTC.

La collecte couvre les périodes courantes et précédentes en une passe par source :
60 jours pour 7/30 jours, deux fois la durée pour une période libre. Une longue
période peut dépasser les limites des APIs documentées ci-dessous ; ces échecs
ne produisent aucun rapport partiel. Les exclusions de `[metrics]` s'appliquent
aux rapports, sans modifier l'activité brute.

### Contrat JSON v1

`--format json` est le défaut : un objet JSON compact sur une ligne, suivi d'un
saut de ligne. Le contrat est [report-v1.schema.json](schemas/report-v1.schema.json),
également imprimé par `dev-metrics schema` (Draft 2020-12, autonome, sans résolution
réseau). Cette commande décrit le rapport ; les contrats d'activité brute sont
liés dans les sections GitHub et Bitbucket.

Le rapport contient :

- `schema_version: 1` ;
- `windows` : une ou deux fenêtres, avec `days`, `current_period`,
  `previous_period` (bornes `from`/`to`) et `metrics` ;
- `by_source` : les mêmes fenêtres par source, GitHub puis Bitbucket ;
- `by_repository` : les mêmes fenêtres par couple source/dépôt, triées par
  source puis nom de dépôt.

Chaque métrique contient `current`, `previous`, `delta` et `delta_percent`.
Les durées JSON sont en **secondes**, les proportions en **pourcentage 0–100** ;
les deltas de proportion sont en **points de pourcentage**. Les volumes sont des
nombres non négatifs ; les médianes/proportions sans observations et les deltas
indéfinis valent `null`, jamais une chaîne ni un champ omis. Les volumes nuls
restent `0` ; un précédent nul donne `delta_percent: null`, sauf si le courant
est aussi nul (`0`). Les répartitions vides sont des tableaux vides. Une source
sélectionnée sans activité conserve ses fenêtres à zéro/`null`.

Les clés et l'ordre des listes sont déterministes pour les mêmes entrées ;
aucune date d'exécution variable n'est ajoutée. Le schéma est strict : toute
évolution incompatible (y compris un champ refusé par le schéma v1, un changement
de type, d'unité ou de sens) doit incrémenter `schema_version` et publier le
schéma correspondant. Les snapshots et validations JSON Schema font partie de
`cargo test` ; la compatibilité s'évalue aussi lors de la revue du changement.
Le contrat des commandes d'activité brute reste en v1, indépendamment de celui
du rapport. Leur JSON est désormais compact également.

Exemple d'invocation par un bot :

```sh
dev-metrics schema > report.schema.json
dev-metrics report --until 2026-10-30 > report.json 2> error.json
```

Le bot doit vérifier le code de sortie avant de lire le rapport. Les valeurs
numériques JSON proviennent du moteur `f64` existant : elles ne constituent pas
un contrat d'entiers exacts au-delà de 2^53.

### Tableau et résumé Markdown

`--format table` affiche toutes les métriques, leurs périodes et deltas, d'abord
pour l'ensemble puis par source et par dépôt. `--format markdown` fournit un
résumé global court avec les mêmes métriques et comparaisons ; les répartitions
détaillées restent disponibles en JSON/tableau. Dans ces deux rendus, les durées
sont en heures, les nombres arrondis à deux décimales et `—` signifie non calculable.
La légende rappelle les deltas en points pour la part des PRs > 400 lignes.
Voir les snapshots [tableau](tests/fixtures/report.table) et
[Markdown](tests/fixtures/report.md), construits sur un jeu de données fixe.

### Erreurs et codes de sortie

Les erreurs de toutes les commandes, y compris celles du parsing CLI, sont des
objets JSON compacts sur stderr : `code` stable, `message` lisible, `details` objet.
Les messages ne sont pas une API à parser. `details.source` identifie la source
pour une erreur de collecte brute/authentification ; `details.sources` liste les
sources sélectionnées pour un échec de rapport. Les autres détails sont `{}`.
L'aide et la version sont du texte sur stdout avec succès.

| Sortie | Codes JSON | Signification |
| --- | --- | --- |
| 0 | — | Succès, aide ou version |
| 1 | `OUTPUT_ERROR` | Échec de sérialisation ou d'écriture sur stdout, y compris pipe fermé |
| 2 | `INVALID_ARGUMENTS`, `INVALID_PERIOD` | Arguments, format ou période invalides |
| 3 | `CONFIGURATION_ERROR` | Fichier/configuration indisponible ou invalide, workspace absent/invalide |
| 4 | `AUTHENTICATION_ERROR` | Identifiant local absent/invalide ou initialisation du client impossible |
| 5 | `COLLECTION_ERROR`, `REPORT_ERROR` | Collecte brute ou collecte/calcul du rapport en échec |

Un refus HTTP d'un token par l'API relève de la collecte (5), sans changement de
compte. Les erreurs de configuration/authentification ne reproduisent pas les
tokens ; les erreurs réseau/API ne reproduisent pas les corps des réponses.
Aucune sortie de succès n'est écrite avant la fin du calcul et du rendu. Un échec
d'écriture peut toutefois laisser une sortie tronquée : vérifiez toujours le
code de sortie. Si stderr est lui-même fermé, le code non nul reste le signal.

```json
{"code":"AUTHENTICATION_ERROR","message":"Authentification GitHub absente : gh auth token indisponible et aucun token valide dans [github]","details":{"source":"github"}}
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
pour leur exclusion configurable dans le moteur de calcul des métriques.
Les lignes des PRs et celles des commits décrivent des vues différentes du même
travail et ne doivent pas être additionnées. Les commentaires de commits et les
corps des commentaires ne sont pas collectés.

`--format table` affiche ces données brutes, y compris les dates historiques utiles
pour les PRs retenues. Utilisez `report --format table` pour les métriques et deltas. Une évolution incompatible du contrat JSON nécessitera une nouvelle
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

## Données Bitbucket Cloud

`bitbucket` utilise l'API REST 2.0. `--workspace` remplace le slug configuré dans
`[bitbucket].workspace`. Le token provient d'abord de `BITBUCKET_TOKEN`, puis de
`[bitbucket].token`. Une valeur explicite vide ou invalide échoue sans fallback.
Il s'agit d'un [token API Bitbucket accepté en Bearer](https://support.atlassian.com/bitbucket-cloud/docs/using-api-tokens/),
pas d'un ancien App password. Les permissions de lecture des dépôts et des pull
requests sont nécessaires. Pour un token API, prévoir `read:repository:bitbucket`,
`read:pullrequest:bitbucket` et [`read:user:bitbucket`](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-users/),
ce dernier étant requis pour `/user` comme pour `/users/{user}`.

Sans `--user`, `/user` identifie le compte authentifié. `--user` accepte un UUID
Bitbucket entre accolades ou un Account ID Atlassian (par exemple
`557058:c0b72ad0-1cb5-4018-9cdc-0cde8492c443`), résolu par `/users/{user}`.
Les [nicknames ne permettent pas d'identifier un compte](https://developer.atlassian.com/cloud/bitbucket/bitbucket-api-changes-gdpr/).
Le JSON conserve les UUIDs des comptes, pour éviter les ambiguïtés des noms affichés.
Seuls les dépôts accessibles du workspace demandé sont parcourus.

Les périodes et modèles sont partagés avec GitHub. La sortie respecte
[bitbucket-activity-v1.schema.json](schemas/bitbucket-activity-v1.schema.json) ; les
deux schémas de source référencent le contrat commun
[activity-v1.schema.json](schemas/activity-v1.schema.json), à fournir au résolveur
JSON Schema lors d'une validation hors ligne. Les sorties GitHub restent en v1.
Chaque événement Bitbucket porte `source: "bitbucket"` et `repository`.

Les PRs sont parcourues dans tous les états (`OPEN`, `MERGED`, `DECLINED`,
`SUPERSEDED`) et retenues selon leur création, merge, review ou commentaire sur
la période. Le timestamp de merge est la première entrée `MERGED` du journal
d'activité, jamais `updated_on`. Une PR mergée sans cette entrée échoue.
Les reviews correspondent aux approbations, demandes de changements et
commentaires de PR des autres auteurs ; chaque action compte séparément.
Les commentaires inline et de discussion de l'utilisateur sont également
exposés dans `comments`. Les actions de l'auteur de la PR sont exclues des
reviews. La première review reçue est recherchée dans tout l'historique accessible.

Bitbucket ne fournit pas d'ID natif pour chaque action d'approbation : `reviews.id`
est un ordinal synthétique, à partir de 1, dans les actions de la PR triées par
date, UUID, état puis URL. Cet ordinal est local à la PR et à l'historique retourné ;
il peut changer si une action historique disparaît. Les IDs de commentaires sont
ceux de l'API. Les auteurs absents valent `null` ; les commentaires supprimés ou
encore en attente de soumission (`pending`) sont exclus, et les approbations conservées d'auteurs supprimés restent des reviews reçues.

Les commits sont parcourus sur toutes les branches et tags accessibles via
[`/commits`](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-commits/#api-repositories-workspace-repo-slug-commits-get).
Seuls les commits attribués par l'API à l'UUID de l'utilisateur et datés dans la
période sont retenus : aucune identité n'est déduite d'un nom ou d'une adresse email.
Le diffstat d'un commit compare son premier parent. Les diffstats paginés des PRs
et commits conservent tous les chemins, renommages, suppressions et statistiques,
y compris les lockfiles/générés, pour les exclusions du moteur de métriques.

La pagination suit les liens `next`, y compris les curseurs opaques. Les
redirections du diffstat de PR sont suivies uniquement dans la même origine et
sous le préfixe de l'API configurée. Les liens externes, cycles, pages incomplètes
avec un total annoncé, doublons et statistiques invalides échouent. Les événements
d'activité inconnus échouent pour éviter une omission silencieuse.
Les requêtes sont séquentielles, sans retry, avec les mêmes timeouts HTTP que GitHub.
Aucune sortie partielle ni réponse d'erreur brute n'est imprimée.

La collecte parcourt l'historique accessible des dépôts et PRs du workspace et
peut coûter beaucoup de requêtes sur un grand workspace. Elle ne reconstruit pas
les événements supprimés/inaccessibles. Les diffstats de PR reflètent le diff
actuel, pas une photographie historique. `--format table` restitue l'activité
brute ; `report --source bitbucket` expose les métriques et deltas.

## Calcul des métriques

La bibliothèque expose `metrics::calculate(activities, through, config)` pour des
activités déjà collectées et `metrics::collect(sources, through, config)` pour
collecter puis calculer. `calculate_period(activities, period, config)` et
`collect_period(sources, period, config)` acceptent une `Period` libre et renvoient
une seule fenêtre, avec la même logique de calcul. `MetricsReport::json()`,
`table()` et `markdown()` produisent les rendus sans recalculer les métriques.
`through` est une date de fin explicite (`NaiveDate`),
inclusive en UTC. `SourceRequest` associe un `&dyn ActivitySource` à un utilisateur
optionnel ; `GitHubSource` et `BitbucketSource` implémentent ce trait en réutilisant
leur collecte existante. Une identité absente désigne le compte authentifié.

Une collecte par source couvre les 60 jours nécessaires aux deux fenêtres et à
leurs périodes précédentes. Pour une fin au 30 octobre 2026 :

| Fenêtre | Période courante | Période précédente |
| --- | --- | --- |
| 7 jours | 24–30 octobre | 17–23 octobre |
| 30 jours | 1–30 octobre | 1–30 septembre |

`calculate` exige que chaque activité couvre ces 60 jours (`calculate_period` exige
la couverture de la période libre et de sa précédente) et refuse plusieurs
collectes pour la même source afin d'éviter le double comptage. Une erreur de
collecte, un délai négatif ou un dépassement des compteurs de lignes échoue sans
retourner de rapport partiel. Une entrée vide produit des volumes nuls.

`MetricsReport` fournit une ou deux fenêtres selon la période demandée, puis les
mêmes calculs par source et par
couple `(source, repository)`. GitHub précède Bitbucket ; les dépôts sont triés par
nom au sein de chaque source. Une source fournie sans activité conserve ses
fenêtres vides ; les dépôts proviennent des événements collectés, y compris ceux
avec uniquement des reviews ou commentaires. Les agrégats recalculent les
médianes et proportions sur toutes les observations, sans moyenner les résultats
par dépôt ou par source. Les identités sont comparées sans tenir compte de la
casse sur GitHub et exactement sur Bitbucket.

Les conventions de calcul sont :

- **Volume** : PRs de l'utilisateur créées ou mergées dans la fenêtre ; commits
  attribués à l'utilisateur, selon leur date d'auteur. Les lignes ajoutées et
  supprimées viennent des fichiers de ces commits, après exclusions. Les lignes
  des PRs ne s'ajoutent pas au volume des commits.
- **Vitesse** : médiane ouverture → merge des PRs de l'utilisateur mergées dans
  la fenêtre ; médiane ouverture → première review des PRs de l'utilisateur dont
  la première review reçue tombe dans la fenêtre, même si leur ouverture est
  antérieure. Les durées sont exprimées en secondes. Les événements postérieurs
  à la fin de la fenêtre sont exclus.
- **Taille** : médiane des additions + suppressions, après exclusions, sur les
  PRs de l'utilisateur créées dans la fenêtre. La part des PRs strictement
  supérieures à 400 lignes est exprimée en pourcentage (0–100). Une PR dont tous
  les fichiers sont exclus conserve une taille de zéro.
- **Reviews** : nombre d'actions dans `reviews_given` sur les PRs des autres,
  nombre de commentaires dans `comments` et nombre d'actions dans
  `reviews_received` sur ses PRs, filtrés par leur date. Les conventions de chaque
  source sont conservées : un commentaire de PR Bitbucket peut compter à la fois
  comme review faite et comme commentaire laissé.

Chaque `Comparison` contient `current`, `previous`, `delta` (courant − précédent)
et `delta_percent` ((courant − précédent) / précédent × 100). Pour une proportion,
`delta` est une différence en points de pourcentage. Une médiane ou proportion
sans observations est absente (`None`), ainsi que ses deltas si l'une des valeurs
comparées est absente. Un précédent nul donne un pourcentage absent, sauf si le
courant est aussi nul : le delta en pourcentage vaut alors zéro.

### Exclusions configurables

La section TOML optionnelle `[metrics]` alimente `Config.metrics` :

```toml
[metrics]
excluded_files = ["Cargo.lock", "package-lock.json", "generated/*", "*.snap"]
```

La liste explicite **remplace** les défauts ; `excluded_files = []` désactive
les exclusions. Sans liste explicite, les défauts de `MetricsConfig::default()`
excluent les lockfiles courants Rust, JavaScript, PHP, Python, Ruby, Go et .NET,
les fichiers `*.min.js`, `*.min.css`, `*.generated.*` et les répertoires
`generated`, `dist`, `build` à toute profondeur. Cette liste est une convention
par nom, pas une détection exhaustive des fichiers générés.

Un motif sans `/` s'applique au nom du fichier à toute profondeur ; un motif avec
`/` s'applique au chemin relatif complet. `*` représente zéro ou plusieurs
caractères, y compris `/` ; `?` représente un caractère Unicode. Les autres
caractères sont littéraux, sans classes ni négation. La casse est significative.
Un fichier renommé est exclu si son ancien ou son nouveau chemin correspond.
Les données brutes et les exclusions utilisées par un appelant restent intactes.

## Développement

```sh
rustup component add rustfmt clippy
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Les tests couvrent le parsing CLI, la configuration et ses erreurs, puis lancent
le binaire pour vérifier l'aide, le schéma, les erreurs JSON et les codes de sortie. Les
sources GitHub et Bitbucket utilisent des réponses HTTP simulées avec `mockito` pour exercer la
collecte, la pagination, les dates, les erreurs, les redirections Bitbucket et les sorties. Les snapshots
JSON/tableau/Markdown et le moteur `jsonschema` vérifient les contrats de sortie,
avec des exemples invalides rejetés par le schéma. Les tests de commande utilisent
des sources in-memory et une date fixe pour exercer le raccordement de `report`. Les tests
d'authentification utilisent des exécutables temporaires sous Unix, sans accéder
aux identifiants de la machine. Les tests unitaires de métriques utilisent des jeux
de données fixes pour les fenêtres, deltas, médianes, exclusions et répartitions ;
les tests HTTP simulés exercent aussi le raccordement au trait commun de collecte. Ils ne prouvent pas l'exhaustivité des données accessibles ni une collecte Bitbucket réelle.

GitHub Actions exécute les trois contrôles sur Rust stable, sous Ubuntu, à chaque
`push` et `pull_request`.
