# MicaBar

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [Русский](README.ru.md) · [Español](README.es.md) · [Português (Brasil)](README.pt-BR.md) · **Français** · [Deutsch](README.de.md)

Un moniteur réseau pour Windows façon « îlot dynamique ». Une capsule en verre givré en haut de l’écran affiche votre IP
de sortie et le nombre de connexions ; posez la souris dessus pour voir quel programme utilise quel port, où va chaque
connexion et par quel nœud proxy — et décider sur-le-champ si elle passe par le proxy ou en direct.

## Fonctionnalités

- **Sorties par groupe** : l’IP de sortie et le nœud de chaque groupe de votre abonnement (proxy par défaut, IA,
  streaming…) plus la sortie directe, avec pays et fournisseur / ASN, ainsi que le type d’IP (résidentiel / pro /
  hébergement / mobile, avec une mention à part si elle est listée comme proxy) et son propriétaire (opérateur et province
  pour les IP de Chine continentale).
- **Capsule tournante** : sorties par groupe, morceau en lecture (pochette + titre), quota IA, vitesse et latence du nœud
  défilent tour à tour ; le contenu et le rythme sont réglables.
- **Quota IA** : utilisation de l’abonnement (fenêtres de 5 heures / hebdomadaire…) et heure de réinitialisation pour
  Claude Code, Codex, Antigravity et Grok, chaque anneau avec le logo de l’outil.
- **Tâches IA en cours** : pendant que Claude Code travaille, le début de la capsule affiche le projet et le temps écoulé ;
  elle passe à l’orange quand il attend votre accord. Aussi en haut de l’îlot ouvert ; un clic ramène son terminal au
  premier plan. Interrompre avec Échap ou fermer le terminal le fait disparaître.
- **Diagnostic réseau** : une sonde légère toutes les 10 s ; deux échecs de suite lancent une vérification complète de la
  chaîne (carte réseau → routeur → DNS → internet → app proxy → nœud). La capsule indique quel maillon a lâché (« DNS en
  panne », « Nœud en panne », « Ligne coupée »…) et le panneau propose une réparation : vider le cache DNS, passer sur un
  nœud qui marche, redémarrer l’app proxy… Au retour, la durée de la coupure s’affiche. Si un autre programme désactive ou
  modifie le proxy système, vous êtes prévenu avec un rétablissement en un clic.
- **Premier accès au réseau** : une carte sur l’îlot quand un programme accède à internet pour la première fois —
  « Autoriser » ou « Bloquer » (une règle du Pare-feu Windows ; avec le contrôle de sortie actif, son trafic via le proxy
  système est aussi rejeté). Le premier lancement apprend vos programmes habituels pendant 10 minutes ; les dossiers de
  version sont regroupés, donc une mise à jour ne compte pas comme un nouveau programme.
- **Notifications** : les nouveaux messages de WeChat, Telegram, de la messagerie et d’autres apps, ainsi que Claude Code /
  Codex qui terminent une tâche ou attendent votre accord, s’affichent sur l’îlot — une petite icône au bout de la capsule,
  une ligne dans la capsule ou une carte, au choix ; la page Notifications les regroupe par app. Le texte des messages est
  masqué par défaut (app + expéditeur seulement) ; ne pas déranger, heures calmes, sourdine par app et prise en main des
  fenêtres propres aux apps.
- **Connexions par programme** : icône, nombre de processus, débit montant / descendant en direct, ceux passant par le
  proxy d’abord ; pour chaque connexion, port local → domaine / IP, pays, fournisseur, nœud et la règle appliquée.
- **Contrôle de sortie** (Clash Party / Mihomo Party) : par programme, domaine, domaine principal ou « seulement ce
  programme vers ce domaine » — proxy / direct / bloquer, appliqué immédiatement sans redémarrer le noyau ; changez et
  testez le nœud utilisé pour « Proxy » depuis l’îlot.
- **Alertes de fuite** : quand un site censé passer par le proxy part en direct, la capsule devient rouge et une
  notification Windows apparaît. Prévention : les règles manquées sont forcées par le proxy ; les programmes qui
  contournent le proxy peuvent être empêchés de sortir en direct en un clic (Pare-feu Windows).
- **Vitesse / trafic / nœuds** : graphique de vitesse en direct, trafic proxy contre direct par programme (aujourd’hui,
  7 ou 30 jours), latence et pertes des nœuds.
- **Moniteur système** : CPU / GPU / mémoire par programme, arrêt de processus ; activez et désactivez les apps au
  démarrage comme dans le Gestionnaire des tâches.
- **Fenêtre de bureau** : d’ordinaire seuls l’îlot et une icône dans la zone de notification ; la fenêtre de bureau
  contient une vue d’ensemble (sorties, vitesse, trafic, latence, alertes, quota IA, programmes les plus actifs) et tous
  les réglages.
- **Apparence et confort** : verre givré en direct (clair / sombre, comme le système) qui garde le texte lisible sur
  n’importe quel fond, palettes prêtes et thèmes personnalisés ; formes capsule / encoche / barre indicatrice ancrées à
  n’importe quel bord de l’écran ; ouverture au survol ou au clic, mode traversant, raccourci personnalisé, navigation au
  clavier ; ne gêne pas les jeux en plein écran ; animations fluides qui suivent le réglage d’animation de Windows.
- **10 langues d’interface** : 简体中文, 繁體中文, English, 日本語, 한국어, Русский, Español, Português (Brasil),
  Français, Deutsch — suit la langue d’affichage de Windows par défaut.
- **Historique du presse-papiers** : le texte et les images copiés sont gardés sur la page « Presse-papiers » de l’îlot ; un clic les recopie. Recherche, épinglage, nettoyage automatique par âge et par nombre ; ce qu’un gestionnaire de mots de passe marque comme privé n’est pas enregistré.

> L’apparence et l’interaction s’inspirent de [fly-island](https://github.com/flyl1u/fly-island), certaines idées de
> fonctionnalités de [BrimDeck](https://github.com/MoFeng2223/BrimDeck) ; tout le code est écrit de manière indépendante.

## Installation

Téléchargez depuis [Releases](https://github.com/aileboeth/MicaBar/releases) :

- `MicaBar_<version>_x64-setup.exe` — installateur (`C:\Program Files\MicaBar`), avec mises à jour intégrées ;
- `MicaBar_<version>_portable.exe` — un seul exe portable, sans mise à jour automatique.

Nécessite Windows 10 2004+ / Windows 11 (x64) et le runtime WebView2 (intégré à Windows 11 ; l’installateur l’ajoute s’il
manque). MicaBar tourne en administrateur (la vitesse par programme exige les événements ETW du noyau), il demande donc
l’UAC au démarrage ; avec « Lancer avec Windows » activé, il démarre par une tâche planifiée, sans invite.

## Utilisation

Au premier lancement, une visite guidée d’une demi-minute sur l’îlot vous montre les bases décrites ci-dessous ; revoyez-la
quand vous voulez via Réglages → À propos → « Revoir la visite ».

### Déplier et replier

| Action | Résultat |
| --- | --- |
| Laisser la souris sur la capsule environ 0,5 s | Elle se déplie en panneau (mode « Au survol » par défaut ; le délai se règle dans Réglages → Général → Interaction) |
| Sortir la souris du panneau | Elle se replie |
| Cliquer sur la capsule / cliquer à côté | Déplier / replier en mode « Au clic » (à changer dans Réglages → Général → Interaction → « Comment déplier ») |
| `Ctrl+Alt+N` | Déplier / replier à tout moment (pour le changer, cliquez sur le champ du raccourci dans les Réglages et appuyez sur la nouvelle combinaison) |
| `Échap` | Replie le panneau ; ferme d’abord un panneau inférieur ou un menu ouvert |

- En mode « Au survol », la capsule repliée ne bloque jamais les clics : tant que la souris est dessus, elle devient
  transparente pour laisser voir l’onglet qu’elle cache, et le clic va à la fenêtre du dessous.
- **Mode traversant** : la souris ne touche jamais l’îlot, seul le raccourci l’ouvre — pratique pour ne jamais l’activer
  par erreur.
- Quand une app plein écran (jeu, vidéo) est au premier plan, la capsule reste visible mais ne se déplie jamais toute seule
  et ne bloque pas les clics ; elle peut aussi être masquée complètement.
- Option « Réduire en barre indicatrice quand une fenêtre est agrandie » : tant qu’une fenêtre agrandie est au premier
  plan, la capsule devient un trait fin et revient quand vous changez de fenêtre.

### Glisser, lancer et ancrer

L’îlot s’ancre au bord haut, bas, gauche ou droit de l’écran, au début, au milieu ou à la fin de ce bord.

1. **Le saisir** : panneau déplié, **appuyez et restez immobile environ 0,26 s** n’importe où sur le panneau (la durée se
   règle dans Réglages → Général → Interaction → « Appui long pour saisir l’îlot »). Replié, **maintenez `Alt` puis faites
   un appui long sur la capsule** (avec Alt maintenu, la capsule ne devient plus transparente : l’appui saisit la capsule et
   non la fenêtre du dessous).
2. **Le lancer le long du bord** : une fois saisi, donnez un coup vers la gauche ou la droite et lâchez — l’îlot glisse au
   début, au milieu ou à la fin de ce bord.
3. **Le changer de bord** : une fois saisi, tirez-le vers le milieu de l’écran et il suit la souris ; lâchez-le près d’un
   autre bord et il s’y ancre. Sur les bords gauche / droit, la capsule devient une encoche verticale.

Si vous préférez ne pas glisser, choisissez le bord et la position sur ce bord dans Réglages → Aspect replié.

### Redimensionner

- Clic droit sur l’icône de la zone de notification → « Redimensionner l’îlot… », le bouton « Redimensionner l’îlot » en
  bas à gauche de la fenêtre de bureau, ou Réglages → Aspect replié / Aspect déplié → « Glissez à l’écran pour ajuster » :
  la capsule et le panneau à l’écran ont des bords déplaçables ; tirez-les pour changer largeur et hauteur, puis cliquez sur
  « Terminé » (`Échap` annule).
- Ou utilisez les curseurs de Réglages → Aspect replié / Aspect déplié : hauteur de la capsule, largeur (0 = s’élargit avec
  le contenu), largeur du panneau, hauteur max et échelle de l’interface.

### Clavier

Panneau déplié : `Tab` / `Maj+Tab` déplacent le focus dans l’ordre ; avec le focus sur les icônes de page en haut ou sur
les onglets de Réseau, `←` / `→` changent ; `↑` / `↓` se déplacent entre les lignes et les boutons de la liste ; `Entrée`
/ `Espace` déplient le programme courant ; `Échap` replie. Les listes réordonnables des Réglages acceptent aussi `Alt+↑` /
`Alt+↓` sur l’élément sélectionné.

### Les pages

Les icônes en haut du panneau changent de page (Réseau, Système, Musique, Quota IA, Notifications) ; quelles pages
s’affichent et dans quel ordre se règle dans Réglages → Aspect déplié, et l’îlot se souvient de la page où vous étiez.

**Réseau**

- En haut, les **cartes de sortie**. À gauche les sorties proxy : une ligne par IP de sortie avec son étiquette de type
  (Résidentiel / Pro / Hébergement / Mobile / Signalée proxy), le nœud et le propriétaire en deuxième ligne, et à droite les
  groupes qui utilisent cette IP ; ce qui ne tient pas se replie en « +N » — cliquez pour tout voir et sur « Replier » pour
  replier. À droite la sortie directe (opérateur, carte réseau, DNS inverse, dernière vérification). Posez la souris sur
  une IP ou une étiquette pour voir le propriétaire enregistré, le fournisseur, l’ASN et la source des données. ↻ en haut à
  droite revérifie les sorties ; l’icône de pouls à côté ouvre le **diagnostic réseau**.
- La ligne sous les cartes montre l’app proxy, le proxy système, TUN, le port local et le contrôleur.
- Onglets : **Conn. / Écoute / Sortie / Alertes / Vitesse / Trafic / Nœuds** (faites-les défiler avec la molette s’ils ne
  tiennent pas ; réordonnez-les dans les Réglages). Le champ de recherche à droite filtre par programme, domaine, IP ou port.
  - **Connexions** : une ligne par programme en ligne (ceux via proxy d’abord). Cliquez sur une ligne pour voir ses
    connexions ; le bouton « Sortie » à droite choisit Proxy / Direct / Bloquer pour tout le programme, et le « ⋯ » à côté
    d’une connexion seulement pour elle (ce domaine, cette IP, ou « seulement ce programme vers lui »). Recliquer sur le
    bouton sélectionné = revenir aux règles.
  - **Écoute** : les ports en écoute sur ce PC ; ceux qui écoutent sur toutes les cartes (accessibles depuis le réseau
    local) sont signalés.
  - **Sortie** : activer / désactiver le contrôle de sortie, voir et supprimer les règles de sortie, changer et tester le
    nœud utilisé pour « Proxy ».
  - **Alertes** : fuites de proxy et premiers accès au réseau ; bloquer la sortie directe, ignorer un domaine, autoriser à
    nouveau.
  - **Vitesse** : le graphique de vitesse en direct. Déplacez la souris à gauche / droite : il s’aimante à chaque seconde
    et les grands chiffres suivent ; cliquez sur un programme sous le graphique pour faire monter un panneau avec son propre
    graphique et ses chiffres. Faites glisser ce panneau vers le haut / bas, lancez-le vers le bas pour le fermer ; il
    propose « Voir ses connexions » et « Afficher seulement elle dans le grand graphique ».
  - **Trafic** : trafic proxy / direct par programme pour aujourd’hui / 7 jours / 30 jours ; cliquez sur une barre pour le
    détail de ce jour.
  - **Nœuds** : latence et pertes de chaque nœud de sortie ; « Tester maintenant » lance une série tout de suite.
- **Diagnostic réseau** (l’icône de pouls, ou l’alerte rouge sur la capsule quand vous êtes hors ligne) : vérifie de la
  carte réseau au nœud, indique quel maillon a lâché et propose des boutons de réparation en dessous (vider le cache DNS,
  rétablir le proxy système, ouvrir / redémarrer l’app proxy, passer sur un nœud qui marche…).

**Système** : utilisation de chaque cœur CPU / GPU, mémoire et VRAM ; en dessous, basculez entre « Apps les plus
gourmandes » (triées par CPU ou mémoire ; ✕ termine un processus après confirmation — les processus essentiels de Windows
et MicaBar lui-même ne sont jamais terminés) et « Apps au démarrage » (le même interrupteur que « Applications de
démarrage » du Gestionnaire des tâches ; en désactiver une ne supprime rien).

**Musique** : le morceau en lecture (NetEase Cloud Music, QQ Music, Spotify, vidéos du navigateur…) avec pochette, titre,
barres qui suivent la musique et précédent / lecture-pause / suivant.

**Quota IA** : une carte par outil avec la part utilisée de chaque fenêtre de quota et l’heure de réinitialisation ;
« Actualiser maintenant » interroge tout de suite (les API officielles limitent fortement ; Claude au plus toutes les 10 minutes).

**Notifications** : regroupées par app ; cliquez sur une notification pour ouvrir son app, « Tout marquer comme lu »,
« Tout effacer » ; un **clic droit** sur un groupe permet de mettre l’app en sourdine ou d’effacer le groupe.

### Ce que vous dit la capsule

- Le contenu de la capsule (IP de sortie, vitesse, anneaux de quota IA, horloge, CPU / mémoire…) se coche et se réordonne
  dans Réglages → Aspect replié → « Contenu » ; ce qui ne tient pas rejoint la dernière case et défile.
- Quand vous êtes hors ligne, que le proxy système a été modifié, ou que Claude Code travaille ou attend votre accord,
  l’alerte correspondante apparaît au début de la capsule ; cliquez dessus pour ouvrir le diagnostic ou passer à ce terminal.
- Les changements d’IP de sortie, un nœud qui ne répond pas ou ralentit et les pics de trafic s’affichent brièvement
  5 secondes sur la capsule (interrupteurs et seuils dans Réglages → Réseau → « Alertes brèves »).
- La capsule devient rouge en cas de fuite ; les notifications non lues affichent l’icône de leur app et un point rouge à droite.

### Fenêtre de bureau, réglages et zone de notification

- **Clic gauche sur l’icône de la zone de notification** ou sur la **roue dentée** en haut à droite du panneau pour ouvrir
  la fenêtre de bureau : Vue d’ensemble, Connexions, Ports en écoute, Contrôle de sortie, Alertes de fuite, Vitesse,
  Trafic, Nœuds, Système, Quota IA, Notifications et Réglages.
- **Clic droit sur l’icône** : Ouvrir MicaBar, Réglages, Redimensionner l’îlot, Revérifier l’IP de sortie, Aspect (comme le
  système / givre clair / givre sombre), Ouvrir le journal, Quitter.
- Les réglages sont regroupés dans la barre latérale (Essentiel : Général, Aspect replié, Aspect déplié, Thème du panneau ;
  Sorties et alertes ; Quota et anneaux ; En lecture ; Notifications ; À propos) et chaque fonction peut être désactivée ;
  formes et thèmes se choisissent sur des cartes d’aperçu, et les listes avec ⠿ se réordonnent en les faisant glisser.
  « Réinitialiser tous les réglages » se trouve tout en bas.
- **Langue de l’interface** : Réglages → Général → « Langue » ; « Comme le système » par défaut utilise la langue
  d’affichage de Windows. Après un changement, toutes les fenêtres se rechargent et le menu de la zone de notification change aussi.

## Sorties par groupe

Avec le contrôle de sortie activé, la surcharge ouvre pour chaque groupe proxy un port qui n’écoute que sur ce PC (à partir
de `127.0.0.1:47800`), dont le trafic va directement à ce groupe. MicaBar vérifie l’IP de sortie par chaque port et sait
ainsi par où sortent « défaut », « IA » ou « YouTube ». Par défaut, il affiche le groupe de secours et les groupes avec du
trafic récent (jusqu’à 4) ; les groupes peuvent aussi être choisis à la main.

## Contrôle de sortie

Quand Clash Party / Mihomo Party est détecté, l’îlot propose d’activer le contrôle de sortie. L’activation sauvegarde la
liste des surcharges de Clash Party, ajoute une surcharge globale « MicaBar 出口控制 » et redémarre Clash Party une fois
(environ 3 s de coupure) ; si le noyau ne parvient pas à la charger, tout est rétabli. Ensuite, choisissez une cible via
« Sortie » sur la ligne d’un programme ou « ⋯ » sur une connexion. Les règles plus précises l’emportent, si bien que « ce
programme en direct » et « seulement ce programme vers *.example.com via le proxy » fonctionnent ensemble. La surcharge
se retire en bas de l’onglet « Sortie ».

En mode proxy système, seuls les programmes qui respectent le proxy système peuvent être contrôlés ; activez TUN pour les
couvrir tous.

## Clients proxy pris en charge

| Client | Sortie proxy / port local | Nœuds, groupes, règles appliquées | Contrôle de sortie / prévention des fuites |
|---|---|---|---|
| Clash Party / Mihomo Party | ✓ | ✓ | ✓ |
| Clash Verge Rev | ✓ | ✓ (via son contrôleur) | — |
| v2rayN, Throne (NekoRay), Hiddify, FlClash, Karing | ✓ (port du proxy système ou leur port local habituel) | — | — |

Le proxy système, la carte TUN et les connexions par programme s’affichent quel que soit le client.

## Notifications

Les notifications viennent de trois sources, toutes lues localement :

- **Clignotement de la barre des tâches** : WeChat 4.x signale un nouveau message uniquement en faisant clignoter son
  bouton dans la barre des tâches (pas de notification Windows, pas de fenêtre lisible), l’îlot ne peut donc dire que
  « WeChat · N nouveaux messages » ; un clic ouvre WeChat. Les autres apps qui clignotent fonctionnent pareil.
- **Fenêtres propres aux apps / notifications Windows** : notifications Windows coupées, Telegram affiche ses propres
  petites fenêtres dans le coin ; quand l’une apparaît, MicaBar lit une fois la liste des discussions de Telegram (Windows
  UI Automation) et repère celles dont le nombre de nouveaux messages a augmenté. Les apps utilisant les notifications
  Windows sont lues dans la base de notifications du système. « Prendre la main sur les fenêtres » masque la fenêtre de
  l’app (ou de Windows) pour que seul l’îlot l’affiche.
- Les apps utilisant les notifications Windows (WhatsApp, Teams, Slack, Discord, Signal, Zoom, LINE…) s’affichent avec leur
  propre nom et icône ; un message qui fait clignoter la barre des tâches et déclenche une notification ne compte qu’une fois.
- Ouvrir l’app marque ses notifications comme lues et efface le point rouge ; une notification qui arrive pendant que vous
  utilisez cette app n’est pas annoncée.
- **Alertes de fin d’IA** : « Connecter » dans Réglages → Notifications ajoute un hook dans `~/.claude/settings.json` de
  Claude Code ou `~/.codex/config.toml` de Codex (sauvegarde préalable, retrait en un clic) ; quand une tâche se termine ou
  attend un accord, il prévient `127.0.0.1:47900` avec `curl` (avec un jeton aléatoire).

## Historique du presse-papiers

- Le texte et les images copiés (captures d’écran, etc.) s’affichent par ordre chronologique sur la page « Presse-papiers » du panneau, les éléments épinglés en haut ; recopier le même contenu le remonte simplement en haut.
- Un clic sur un élément le remet seulement dans le presse-papiers — vous collez vous-même avec Ctrl+V. Survolez un élément pour l’épingler ou le supprimer ; la recherche est en haut.
- Réglages → Presse-papiers : enregistrer aussi les images, garder l’historique sur le disque de ce PC (chiffré avec la clé du compte Windows actuel, conservé après redémarrage) ou en mémoire seulement, combien de jours et d’éléments conserver. Les éléments épinglés ne sont jamais supprimés automatiquement.
- Les gestionnaires de mots de passe comme Bitwarden, KeePass et 1Password marquent les mots de passe copiés « ne pas enregistrer » ; ce contenu est ignoré par défaut.
- Une icône de presse-papiers au bout de la capsule rebondit à chaque nouvelle copie (désactivable dans Réglages → Aspect replié).

## Alertes de fuite

La bibliothèque « doit passer par le proxy » = une liste intégrée de services courants (~140 domaines principaux) + la
liste gfw (mise à jour chaque jour) + vos propres entrées / listes importées (domaines bruts, formats de règles v2fly et
Clash). Deux sortes de fuites :

- **Règle manquée** : la connexion est passée par l’app proxy mais une règle l’a envoyée en direct. Avec « Anticiper »
  activé, ces domaines partent au proxy avant d’atteindre les règles directes de l’abonnement ; les groupes par service de
  l’abonnement ne sont pas touchés.
- **Hors proxy** : le programme s’est connecté directement sans passer par l’app proxy (il ignore le proxy système, ou la
  connexion est antérieure au TUN). La carte d’alerte permet de bloquer ses connexions directes (une règle du Pare-feu
  Windows pour les adresses publiques uniquement ; le programme atteint toujours le port local du proxy) puis de lever le
  blocage plus tard.

## Confidentialité : à quoi MicaBar se connecte

Toutes les statistiques (connexions, trafic, règles, alertes) restent sur ce PC dans `%LOCALAPPDATA%\MicaBar\`. Les seules
requêtes que MicaBar fait de lui-même :

| Usage | Adresse | Quand |
| --- | --- | --- |
| IP de sortie proxy | ipinfo.io → api.ipify.org → ifconfig.me (via le port local du proxy ; sorties par groupe via le port loopback de chaque groupe) | au démarrage, à chaque changement de réseau / proxy, toutes les 5 min |
| IP de sortie directe | 4.ipw.cn → myip.ipip.net → ip.3322.net (liée à la carte physique ; résolution via 223.5.5.5 / 119.29.29.29) | idem |
| Type et propriétaire de l’IP | proxycheck.io (HTTPS) et ip-api.com (HTTP seulement en version gratuite), avec chaque IP de sortie (la directe aussi) en paramètre ; via le port local du client proxy | quand une nouvelle IP de sortie apparaît ; cache 24 h par IP ; désactivable dans Réglages → Réseau |
| Latence des nœuds | mesurée par le noyau mihomo sur `www.gstatic.com/generate_204` | toutes les 15–60 s (profil de ressources) |
| Liste gfw | cdn.jsdelivr.net / raw.githubusercontent.com (Loyalsoldier/v2ray-rules-dat) | une fois par jour, si activée |
| Mises à jour | latest.json de github.com/aileboeth/MicaBar | une fois par jour, si activé |
| Quota IA | api.anthropic.com (Claude Code), chatgpt.com (Codex), cli-chat-proxy.grok.com (Grok), le serveur de langage local d’Antigravity | toutes les 10 min (réglable), si activé ; toujours via le port local du client proxy, jamais en direct (pas de port = pas de requête) |
| Diagnostic réseau | direct : les API HTTP de résolution de 119.29.29.29 / 223.5.5.5 (requête sur qq.com) ; via le proxy : `www.gstatic.com/generate_204` ; DNS : une résolution neuve de www.baidu.com par le DNS système ; la vérification complète pingue aussi le routeur | si activé : une petite requête toutes les 10 s (direct), toutes les 30 s via le proxy et vers le DNS ; la vérification complète seulement en cas d’échec |

Pays / ASN proviennent des bases hors ligne fournies avec le client proxy (`country.mmdb`, `ASN.mmdb`) ; les IP des
connexions ne sont jamais envoyées nulle part. Le quota IA utilise la connexion que chaque outil garde sur ce PC et ne
l’envoie qu’au service de cet outil ; elle n’est jamais enregistrée, journalisée ni renouvelée. « En lecture » lit les
contrôles multimédias de Windows sans requête réseau. Les notifications non plus : celles du jour restent en mémoire, le
texte des messages n’est jamais écrit sur le disque ni dans le journal (et n’est pas affiché sur l’îlot par défaut) ; le
port loopback `127.0.0.1:47900` n’accepte que les appels des hooks IA portant le jeton. Le secret du contrôleur mihomo est
lu dans sa configuration à l’exécution et n’est jamais enregistré ni transmis.

L’historique du presse-papiers ne se connecte jamais à Internet : les éléments restent sur ce PC (`%LOCALAPPDATA%\MicaBar\clipboard`, chiffré) ou en mémoire seulement, et jamais dans le journal.

## Compiler depuis les sources

Nécessite Rust (rustup), Visual Studio C++ Build Tools, Node.js + pnpm.

```powershell
pnpm install
pnpm build            # un seul exe : src-tauri\target\release\micabar.exe
pnpm release          # installateur + portable + manifeste de mise à jour -> dist\release-<version>\ (nécessite keys\micabar.key)
```

Organisation : `src-tauri\src\` (backend Rust : tables de sockets, ETW, contrôleur mihomo, vérification des sorties,
protection contre les fuites, comportement de la fenêtre, capture pour le givre…), `ui\` (HTML / CSS / JS bruts, sans
bundler ; `ui\i18n\` contient les traductions, indexées par le texte chinois du code), `tools\` (script de publication,
pilote de la machine virtuelle de test).
Notes d’implémentation (en chinois) : [docs/internals.zh-CN.md](docs/internals.zh-CN.md).

## Licence

[MIT](LICENSE). Icônes de drapeaux issues de [flag-icons](https://github.com/lipis/flag-icons) (MIT, voir `ui/flags/LICENSE`). Logos des outils IA issus de [lobe-icons](https://github.com/lobehub/lobe-icons) (MIT, voir `ui/brands/LICENSE`) ; les marques appartiennent à leurs propriétaires.

## Avertissement

MicaBar est un logiciel libre et open source que son auteur développe sur son temps libre, fourni « en l’état », sans
garantie d’aucune sorte. Il lit les connexions réseau, les processus, les notifications système et l’état de connexion des
outils IA sur ce PC ; ces données ne servent que sur ce PC. Le contrôle de sortie et la protection contre les fuites
modifient la surcharge du client proxy, ajoutent des règles de pare-feu et peuvent redémarrer le client proxy. L’auteur
décline toute responsabilité en cas de perte liée à l’utilisation ou à l’impossibilité d’utiliser le logiciel (perte de
données, coupures réseau, comptes restreints…). Utilisez-le dans le respect des lois locales et des conditions de chaque
service. MicaBar n’est affilié ni à Clash, ni à mihomo, Anthropic, OpenAI, Google, xAI, ni à aucun autre projet ou société
cités ; leurs noms et marques appartiennent à leurs propriétaires.
