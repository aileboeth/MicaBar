# MicaBar

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [Русский](README.ru.md) · [Español](README.es.md) · [Português (Brasil)](README.pt-BR.md) · [Français](README.fr.md) · **Deutsch**

Ein Netzwerkmonitor für Windows im Stil einer „Dynamic Island“. Eine Milchglas-Kapsel oben am Bildschirm zeigt deine
Ausgangs-IP und die Zahl der Verbindungen; lass die Maus darauf ruhen, um zu sehen, welches Programm welchen Port nutzt,
wohin jede Verbindung geht und über welchen Proxy-Knoten — und gleich dort festzulegen, ob sie über den Proxy oder direkt läuft.

## Funktionen

- **Ausgänge pro Gruppe**: die Ausgangs-IP und der Knoten jeder Gruppe deines Abos (Standard-Proxy, KI, Streaming…) plus
  der direkte Ausgang, mit Land und Anbieter / ASN sowie dem IP-Typ (privat / geschäftlich / Rechenzentrum / mobil, mit
  eigener Markierung, wenn sie als Proxy gelistet ist) und dem Inhaber (Netzbetreiber und Provinz bei IPs aus Festlandchina).
- **Wechselnde Kapsel**: Gruppenausgänge, der laufende Song (Cover + Titel), KI-Kontingent, Geschwindigkeit und
  Knotenlatenz wechseln sich ab; was wechselt und wie schnell, ist einstellbar.
- **KI-Kontingent**: Abo-Nutzung (5-Stunden- / Wochenfenster…) und Rücksetzzeit für Claude Code, Codex, Antigravity und
  Grok, jeder Ring mit dem Logo des Tools.
- **Laufende KI-Aufgaben**: solange Claude Code arbeitet, zeigt die Kapsel vorne Projekt und bisherige Dauer; sie wird
  orange, wenn es auf deine Freigabe wartet. Auch oben auf der offenen Insel; ein Klick holt sein Terminal nach vorne.
  Abbruch mit Esc oder Schließen des Terminals entfernt es.
- **Netzwerkdiagnose**: alle 10 s eine leichte Prüfung; zwei Fehlschläge hintereinander lösen eine vollständige Prüfung der
  Kette aus (Adapter → Router → DNS → Internet → Proxy-App → Knoten). Die Kapsel sagt, welches Glied ausgefallen ist („DNS
  gestört“, „Knoten ausgefallen“, „Anschluss ausgefallen“…), und das Panel bietet eine Lösung: DNS-Cache leeren, zu einem
  funktionierenden Knoten wechseln, Proxy-App neu starten… Nach der Erholung steht da, wie lange es weg war. Schaltet ein
  anderes Programm den Systemproxy aus oder um, wirst du gewarnt und kannst ihn mit einem Klick wiederherstellen.
- **Erster Netzzugriff**: eine Karte auf der Insel, wenn ein Programm zum ersten Mal ins Internet geht — „Erlauben“ oder
  „Sperren“ (eine Windows-Firewall-Regel; mit aktiver Ausgangssteuerung wird auch sein Traffic über den Systemproxy
  abgewiesen). Beim ersten Start lernt es 10 Minuten lang deine üblichen Programme; Versionsordner werden
  zusammengefasst, Updates zählen also nicht als neue Programme.
- **Benachrichtigungen**: neue Nachrichten von WeChat, Telegram, Mail und anderen Apps sowie Claude Code / Codex, wenn eine
  Aufgabe fertig ist oder auf Freigabe wartet, erscheinen auf der Insel — als kleines App-Symbol am Ende der Kapsel, als
  Zeile in der Kapsel oder als Karte, ganz nach Wahl; die Benachrichtigungsseite gruppiert sie nach App. Nachrichtentexte
  sind standardmäßig verborgen (nur App + Absender); Nicht stören, Ruhezeiten, Stummschalten pro App und Übernahme der
  eigenen Pop-ups von Apps.
- **Verbindungen pro Programm**: Symbol, Prozessanzahl, Live-Up-/Download, Proxy-Programme zuerst; für jede Verbindung
  lokaler Port → Domain / IP, Land, Anbieter, Knoten und die greifende Regel.
- **Ausgangssteuerung** (Clash Party / Mihomo Party): pro Programm, Domain, Hauptdomain oder „nur dieses Programm zu dieser
  Domain“ — Proxy / direkt / sperren, sofort wirksam ohne Neustart des Kerns; den Knoten für „Proxy“ wechselst und testest
  du direkt auf der Insel.
- **Leck-Warnungen**: geht eine Website, die über den Proxy laufen sollte, direkt, wird die Kapsel rot und eine
  Windows-Benachrichtigung erscheint. Vorbeugung: verfehlte Regeln werden über den Proxy erzwungen; Programmen, die den
  Proxy umgehen, lässt sich der Direktzugriff per Klick sperren (Windows-Firewall).
- **Geschwindigkeit / Traffic / Knoten**: Live-Geschwindigkeitsdiagramm, Traffic Proxy gegen direkt pro Programm (heute,
  7 oder 30 Tage), Latenz und Verlust der Knoten.
- **Systemmonitor**: CPU / GPU / Speicher pro Programm, Prozesse beenden; Autostart-Apps wie im Task-Manager ein- und
  ausschalten.
- **Desktopfenster**: normalerweise nur die Insel und ein Tray-Symbol; das Desktopfenster enthält eine Übersicht
  (Ausgänge, Geschwindigkeit, Traffic, Latenz, Warnungen, KI-Kontingent, aktivste Programme) und alle Einstellungen.
- **Aussehen und Bedienung**: Live-Milchglas (hell / dunkel, wie das System), auf dem Text vor jedem Hintergrund lesbar
  bleibt, Farbvorlagen und eigene Designs; die Formen Kapsel / Notch / Statusleiste an jedem Bildschirmrand angedockt;
  Öffnen per Darüberfahren oder Klick, Durchklick-Modus, eigenes Tastenkürzel, Tastaturbedienung; stört keine
  Vollbildspiele; flüssige Animationen, die der Windows-Animationseinstellung folgen.
- **10 Sprachen der Oberfläche**: 简体中文, 繁體中文, English, 日本語, 한국어, Русский, Español, Português (Brasil),
  Français, Deutsch — standardmäßig wie die Windows-Anzeigesprache.
- **Zwischenablage-Verlauf**: kopierte Texte und Bilder landen auf der Seite „Zwischenablage“ der Insel; ein Klick kopiert sie wieder. Suche, Anheften, automatische Bereinigung nach Alter und Anzahl; was ein Passwortmanager als privat markiert, wird nicht aufgezeichnet.

> Aussehen und Bedienung sind von [fly-island](https://github.com/flyl1u/fly-island) inspiriert, einige Funktionsideen von
> [BrimDeck](https://github.com/MoFeng2223/BrimDeck); der gesamte Code ist eigenständig geschrieben.

## Installation

Herunterladen unter [Releases](https://github.com/aileboeth/MicaBar/releases):

- `MicaBar_<Version>_x64-setup.exe` — Installer (`C:\Program Files\MicaBar`), mit Updates aus der App;
- `MicaBar_<Version>_portable.exe` — eine einzelne portable exe, ohne automatisches Update.

Benötigt Windows 10 2004+ / Windows 11 (x64) und die WebView2-Laufzeit (in Windows 11 enthalten; der Installer ergänzt sie
bei Bedarf). MicaBar läuft mit Administratorrechten (die Geschwindigkeit pro Programm braucht ETW-Ereignisse des Kernels)
und fragt daher beim Start per UAC; mit aktiviertem „Mit Windows starten“ startet es über eine geplante Aufgabe ohne Abfrage.

## Bedienung

Beim ersten Start führt dich eine halbminütige Tour auf der Insel durch die unten beschriebenen Grundlagen; wiederhole sie
jederzeit über Einstellungen → Info → „Tour wiederholen“.

### Aus- und einklappen

| Aktion | Ergebnis |
| --- | --- |
| Maus etwa 0,5 s auf der Kapsel ruhen lassen | Sie klappt zum Panel auf (Standardmodus „Beim Darüberfahren“; die Wartezeit steht unter Einstellungen → Allgemein → Bedienung) |
| Maus vom Panel wegbewegen | Sie klappt ein |
| Auf die Kapsel klicken / daneben klicken | Aus- / Einklappen im Modus „Per Klick“ (umschalten unter Einstellungen → Allgemein → Bedienung → „Wie ausklappen“) |
| `Strg+Alt+N` | Jederzeit aus- / einklappen (zum Ändern in den Einstellungen auf das Tastenkürzel-Feld klicken und die neue Kombination drücken) |
| `Esc` | Klappt das Panel ein; schließt vorher ein offenes unteres Panel oder Menü |

- Im Modus „Beim Darüberfahren“ blockiert die eingeklappte Kapsel nie Klicks: Solange die Maus darauf ruht, wird sie
  durchsichtig, damit du den verdeckten Tab siehst, und der Klick geht an das Fenster darunter.
- **Durchklick-Modus**: Die Maus berührt die Insel überhaupt nicht, nur das Tastenkürzel öffnet sie — praktisch, wenn du sie
  nie versehentlich treffen willst.
- Ist eine Vollbild-App (Spiel, Video) vorne, bleibt die Kapsel sichtbar, klappt aber nie von selbst auf und blockiert keine
  Klicks; sie lässt sich auch ganz ausblenden.
- Optional „Bei maximiertem Fenster zur Statusleiste verkleinern“: Solange ein maximiertes Fenster vorne ist, wird die
  Kapsel zu einer dünnen Linie und kommt zurück, wenn du das Fenster wechselst.

### Ziehen, werfen und andocken

Die Insel dockt am oberen, unteren, linken oder rechten Bildschirmrand an — am Anfang, in der Mitte oder am Ende dieses Randes.

1. **Greifen**: Bei ausgeklapptem Panel **drücke irgendwo auf das Panel und halte etwa 0,26 s still** (die Dauer steht unter
   Einstellungen → Allgemein → Bedienung → „Langes Drücken zum Greifen der Insel“). Eingeklappt **halte `Alt` und drücke dann
   lange auf die Kapsel** (mit gedrückter Alt-Taste wird die Kapsel nicht durchsichtig, also greift das Drücken die Kapsel
   statt des Fensters darunter).
2. **Am Rand entlang werfen**: Gegriffen, gib ihr einen Schwung nach links oder rechts und lass los — die Insel gleitet an
   den Anfang, die Mitte oder das Ende dieses Randes.
3. **An einen anderen Rand bringen**: Gegriffen, zieh sie zur Bildschirmmitte, dann folgt sie der Maus; lass sie nahe einem
   anderen Rand los, und sie dockt dort an. Am linken / rechten Rand wird die Kapsel zur senkrechten Notch.

Wenn du nicht ziehen willst, wähle Rand und Position darauf unter Einstellungen → Eingeklappte Ansicht.

### Größe ändern

- Rechtsklick auf das Tray-Symbol → „Inselgröße anpassen…“, die Schaltfläche „Inselgröße anpassen“ unten links im
  Desktopfenster oder Einstellungen → Eingeklappte / Ausgeklappte Ansicht → „Zum Anpassen auf dem Bildschirm ziehen“: Kapsel
  und Panel auf dem Bildschirm bekommen ziehbare Ränder; zieh daran, um Breite und Höhe zu ändern, und klicke dann auf
  „Fertig“ (`Esc` bricht ab).
- Oder nutze die Schieberegler unter Einstellungen → Eingeklappte / Ausgeklappte Ansicht: Kapselhöhe, Breite (0 = wächst
  mit dem Inhalt), Panelbreite, maximale Höhe und Skalierung der Oberfläche.

### Tastatur

Bei ausgeklapptem Panel: `Tab` / `Umschalt+Tab` bewegen den Fokus der Reihe nach; liegt der Fokus auf den Seitensymbolen
oben oder auf den Tabs der Netzwerkseite, wechseln `←` / `→`; `↑` / `↓` bewegen sich zwischen Zeilen und Schaltflächen der
Liste; `Enter` / `Leertaste` klappen das aktuelle Programm auf; `Esc` klappt ein. Listen, die sich in den Einstellungen
sortieren lassen, nehmen auch `Alt+↑` / `Alt+↓` auf dem gewählten Eintrag an.

### Die Seiten

Die Symbole oben im Panel wechseln die Seite (Netzwerk, System, Musik, KI-Kontingent, Benachrichtigungen); welche Seiten
erscheinen und in welcher Reihenfolge, stellst du unter Einstellungen → Ausgeklappte Ansicht ein, und die Insel merkt sich
die zuletzt offene Seite.

**Netzwerk**

- Ganz oben stehen die **Ausgangskarten**. Links die Proxy-Ausgänge: eine Zeile pro Ausgangs-IP mit Typ-Markierung
  (Privat / Geschäftlich / Rechenzentrum / Mobil / Proxy-markiert), in der zweiten Zeile Knoten und Inhaber, rechts daneben
  die Gruppen, die diese IP nutzen; was nicht passt, wird zu „+N“ zusammengefasst — klicke darauf, um alle zu sehen, und
  auf „Einklappen“, um sie wieder zusammenzufassen. Rechts der direkte Ausgang (Netzbetreiber, Adapter, Reverse-DNS,
  letzte Prüfung). Fahre mit der Maus über eine IP oder Markierung für registrierten Inhaber, Anbieter, ASN und Datenquelle.
  ↻ oben rechts prüft die Ausgänge erneut; das Pulssymbol daneben öffnet die **Netzwerkdiagnose**.
- Die Zeile unter den Karten zeigt Proxy-App, Systemproxy, TUN, lokalen Port und Controller.
- Tabs: **Verb. / Ports / Ausgang / Warnungen / Geschwindigkeit / Traffic / Knoten** (bei Platzmangel mit dem Mausrad
  scrollen; Reihenfolge in den Einstellungen änderbar). Das Suchfeld rechts filtert nach Programm, Domain, IP oder Port.
  - **Verbindungen**: eine Zeile pro Programm, das online ist (Proxy-Programme zuerst). Klicke auf eine Zeile, um ihre
    Verbindungen aufzuklappen; die Schaltfläche „Ausgang“ rechts wählt Proxy / Direkt / Sperren für das ganze Programm, das
    „⋯“ neben einer Verbindung nur für diese Verbindung (diese Domain, diese IP oder „nur dieses Programm dorthin“). Die
    gewählte Schaltfläche erneut klicken = zurück zu den Regeln.
  - **Ports**: lauschende Ports dieses PCs; solche, die auf allen Adaptern lauschen (aus dem LAN erreichbar), sind markiert.
  - **Ausgang**: Ausgangssteuerung ein- / ausschalten, Ausgangsregeln ansehen und löschen, den Knoten für „Proxy“ wechseln
    und testen.
  - **Warnungen**: Proxy-Lecks und erste Netzzugriffe; Direktzugriff sperren, eine Domain ignorieren, wieder erlauben.
  - **Geschwindigkeit**: das Live-Geschwindigkeitsdiagramm. Fahre mit der Maus nach links / rechts darüber, dann rastet es
    auf jede Sekunde ein und die großen Zahlen folgen; klicke ein Programm unter dem Diagramm an, um ein Panel mit seinem
    eigenen Diagramm und seinen Zahlen hochzuziehen. Zieh das Panel nach oben / unten, wirf es nach unten zum Schließen; es
    bietet „Ihre Verbindungen ansehen“ und „Nur sie im großen Diagramm zeigen“.
  - **Traffic**: Traffic Proxy / direkt pro Programm für heute / 7 Tage / 30 Tage; klicke auf einen Balken für die Details
    dieses Tages.
  - **Knoten**: Latenz und Verlust jedes Ausgangsknotens; „Jetzt testen“ startet sofort eine Runde.
- **Netzwerkdiagnose** (das Pulssymbol oder die rote Meldung auf der Kapsel, wenn du offline bist): prüft vom Adapter bis
  zum Knoten, sagt dir, welches Glied ausgefallen ist, und bietet darunter Reparatur-Schaltflächen (DNS-Cache leeren,
  Systemproxy wiederherstellen, Proxy-App öffnen / neu starten, zu einem funktionierenden Knoten wechseln…).

**System**: Auslastung jedes CPU- / GPU-Kerns, Speicher und VRAM; darunter wechselst du zwischen „Apps mit der höchsten
Last“ (nach CPU oder Speicher sortiert; ✕ beendet einen Prozess nach Rückfrage — Kernprozesse von Windows und MicaBar
selbst werden nie beendet) und „Autostart-Apps“ (derselbe Schalter wie „Autostart-Apps“ im Task-Manager; Ausschalten
löscht nichts).

**Musik**: der laufende Song (NetEase Cloud Music, QQ Music, Spotify, Videos im Browser…) mit Cover, Titel, Balken im
Takt der Musik und Zurück / Abspielen-Pause / Weiter.

**KI-Kontingent**: eine Karte pro Tool mit dem Verbrauch jedes Kontingentfensters und der Rücksetzzeit; „Jetzt
aktualisieren“ fragt sofort ab (die offiziellen APIs drosseln stark; Claude höchstens alle 10 Minuten).

**Benachrichtigungen**: nach App gruppiert; klicke auf eine, um ihre App zu öffnen, „Alle gelesen“, „Alle löschen“; ein
**Rechtsklick** auf eine Gruppe schaltet diese App stumm oder löscht die Gruppe.

### Was die Kapsel dir zeigt

- Was die Kapsel anzeigt (Ausgangs-IP, Geschwindigkeit, KI-Kontingentringe, Uhr, CPU / Speicher…), hakst du unter
  Einstellungen → Eingeklappte Ansicht → „Inhalt“ an und sortierst es dort; was nicht passt, landet im letzten Feld und
  wechselt sich ab.
- Bist du offline, wurde der Systemproxy geändert oder arbeitet Claude Code bzw. wartet auf deine Freigabe, erscheint die
  passende Meldung vorne in der Kapsel; ein Klick öffnet die Diagnose oder wechselt zu diesem Terminal.
- Geänderte Ausgangs-IP, ein Knoten ohne Antwort oder mit hoher Latenz und Traffic-Spitzen erscheinen kurz 5 Sekunden auf
  der Kapsel (Schalter und Schwellenwerte unter Einstellungen → Netzwerk → „Kurzhinweise“).
- Bei Lecks wird die Kapsel rot; ungelesene Benachrichtigungen zeigen rechts das App-Symbol und einen roten Punkt.

### Desktopfenster, Einstellungen und Tray

- **Linksklick auf das Tray-Symbol** oder das **Zahnrad** oben rechts im Panel öffnet das Desktopfenster: Übersicht,
  Verbindungen, Lauschende Ports, Ausgangssteuerung, Leck-Warnungen, Geschwindigkeit, Traffic, Knoten, System,
  KI-Kontingent, Benachrichtigungen und Einstellungen.
- **Rechtsklick auf das Tray-Symbol**: MicaBar öffnen, Einstellungen, Inselgröße anpassen, Ausgangs-IP erneut prüfen,
  Ansicht (wie das System / helles Milchglas / dunkles Milchglas), Protokoll öffnen, Beenden.
- Die Einstellungen sind in der Seitenleiste gruppiert (Grundlagen: Allgemein, Eingeklappte Ansicht, Ausgeklappte Ansicht,
  Panel-Design; Ausgänge & Hinweise; Kontingent & Ringe; Wiedergabe; Benachrichtigungen; Info), und jede Funktion lässt sich
  abschalten; Formen und Designs wählst du über Vorschaukarten, Listen mit ⠿ sortierst du per Ziehen. „Alle Einstellungen
  zurücksetzen“ steht ganz unten.
- **Sprache der Oberfläche**: Einstellungen → Allgemein → „Sprache“; der Standard „Wie das System“ nutzt die
  Windows-Anzeigesprache. Nach dem Wechsel laden alle Fenster neu, und auch das Tray-Menü wechselt.

## Ausgänge pro Gruppe

Mit aktivierter Ausgangssteuerung öffnet die Überschreibung für jede Proxy-Gruppe einen Port, der nur auf diesem PC lauscht
(ab `127.0.0.1:47800`), und dessen Traffic direkt an diese Gruppe geht. MicaBar prüft die Ausgangs-IP über jeden Port und
weiß so, wo „Standard“, „KI“ oder „YouTube“ hinausgehen. Standardmäßig zeigt es die Fallback-Gruppe plus die Gruppen mit
jüngstem Traffic (bis zu 4); Gruppen lassen sich auch von Hand wählen.

## Ausgangssteuerung

Wird Clash Party / Mihomo Party erkannt, bietet die Insel an, die Ausgangssteuerung zu aktivieren. Das Aktivieren sichert
die Überschreibungsliste von Clash Party, fügt eine globale Überschreibung „MicaBar 出口控制“ hinzu und startet Clash Party
einmal neu (etwa 3 s offline); lädt der Kern sie nicht, wird alles wiederhergestellt. Danach wählst du ein Ziel über
„Ausgang“ in der Zeile eines Programms oder „⋯“ an einer Verbindung. Genauere Regeln gewinnen, daher funktionieren „dieses
Programm direkt“ und „nur dieses Programm zu *.example.com über Proxy“ zusammen. Die Überschreibung entfernst du unten im
Tab „Ausgang“.

Im Systemproxy-Modus lassen sich nur Programme steuern, die den Systemproxy beachten; aktiviere TUN, um alle zu erfassen.

## Unterstützte Proxy-Clients

| Client | Proxy-Ausgang / lokaler Port | Knoten, Gruppen, greifende Regeln | Ausgangssteuerung / Leckvorbeugung |
|---|---|---|---|
| Clash Party / Mihomo Party | ✓ | ✓ | ✓ |
| Clash Verge Rev | ✓ | ✓ (über seinen Controller) | — |
| v2rayN, Throne (NekoRay), Hiddify, FlClash, Karing | ✓ (Systemproxy-Port oder ihr üblicher lokaler Port) | — | — |

Systemproxy, TUN-Adapter und Verbindungen pro Programm werden bei jedem Client angezeigt.

## Benachrichtigungen

Benachrichtigungen kommen aus drei Quellen, alle lokal gelesen:

- **Blinken in der Taskleiste**: WeChat 4.x meldet eine neue Nachricht nur durch Blinken seiner Taskleisten-Schaltfläche
  (keine Windows-Benachrichtigung, kein lesbares Fenster), daher kann die Insel nur „WeChat · N neue Nachrichten“ sagen; ein
  Klick öffnet WeChat. Andere blinkende Apps funktionieren genauso.
- **Eigene Pop-ups von Apps / Windows-Benachrichtigungen**: Mit ausgeschalteten Windows-Benachrichtigungen zeigt Telegram
  eigene Eckfenster; erscheint eines, liest MicaBar einmal die Chatliste von Telegram (Windows UI Automation) und wählt die
  Chats, deren Zahl neuer Nachrichten gestiegen ist. Apps mit Windows-Benachrichtigungen werden aus der
  Benachrichtigungsdatenbank des Systems gelesen. „Pop-ups übernehmen“ blendet das eigene Pop-up der App (oder von Windows)
  aus, sodass nur die Insel es zeigt.
- Apps mit Windows-Benachrichtigungen (WhatsApp, Teams, Slack, Discord, Signal, Zoom, LINE…) erscheinen mit eigenem Namen
  und Symbol; eine Nachricht, die sowohl die Taskleiste blinken lässt als auch eine Benachrichtigung auslöst, zählt einmal.
- Das Öffnen der App markiert ihre Benachrichtigungen als gelesen und entfernt den roten Punkt; eine, die eintrifft,
  während du diese App nutzt, wird nicht angekündigt.
- **KI-Fertig-Meldungen**: „Verbinden“ unter Einstellungen → Benachrichtigungen fügt `~/.claude/settings.json` von Claude
  Code oder `~/.codex/config.toml` von Codex einen Hook hinzu (vorher gesichert, per Klick entfernbar); ist eine Aufgabe
  fertig oder braucht sie eine Freigabe, meldet er das per `curl` an `127.0.0.1:47900` (mit zufälligem Token).

## Zwischenablage-Verlauf

- Kopierte Texte und Bilder (Screenshots usw.) stehen zeitlich geordnet auf der Seite „Zwischenablage“ des Panels, angeheftete oben; denselben Inhalt erneut kopieren schiebt ihn nur nach oben.
- Ein Klick auf einen Eintrag legt ihn nur wieder in die Zwischenablage – einfügen Sie selbst mit Strg+V. Mit der Maus über einem Eintrag lässt er sich anheften oder löschen; oben ist die Suche.
- Einstellungen → Zwischenablage: Bilder mit aufzeichnen, den Verlauf auf der Festplatte dieses PCs speichern (mit dem Schlüssel des aktuellen Windows-Kontos verschlüsselt, bleibt nach Neustart) oder nur im Speicher, wie viele Tage und Einträge behalten werden. Angeheftete Einträge werden nie automatisch entfernt.
- Passwortmanager wie Bitwarden, KeePass und 1Password markieren kopierte Passwörter als „nicht aufzeichnen“; solche Inhalte werden standardmäßig übersprungen.
- Ein Zwischenablage-Symbol am rechten Ende der Kapsel hüpft bei jeder neuen Kopie (abschaltbar unter Einstellungen → Eingeklappte Ansicht).

## Leck-Warnungen

Die Bibliothek „sollte über Proxy“ = eine eingebaute Liste gängiger Dienste (~140 Hauptdomains) + die gfw-Liste (täglich
aktualisiert) + eigene Einträge / importierte Listen (reine Domains, Regelformate von v2fly und Clash). Zwei Arten von Lecks:

- **Regel verfehlt**: Die Verbindung ging durch die Proxy-App, aber eine Regel schickte sie direkt. Mit „Vorher abfangen“
  werden solche Domains zum Proxy geschickt, bevor sie die direkten Regeln des Abos erreichen; die Dienstgruppen des Abos
  bleiben unberührt.
- **Proxy umgangen**: Das Programm hat sich direkt verbunden, ohne durch die Proxy-App zu gehen (ignoriert den Systemproxy,
  oder die Verbindung ist älter als TUN). Die Warnkarte kann die Direktverbindungen des Programms sperren (eine
  Windows-Firewall-Regel nur für öffentliche Adressen; das Programm erreicht weiterhin den lokalen Proxy-Port) und das später
  wieder aufheben.

## Datenschutz: wohin sich MicaBar verbindet

Alle Statistiken (Verbindungen, Traffic, Regeln, Warnungen) bleiben auf diesem PC in `%LOCALAPPDATA%\MicaBar\`. Die einzigen
Anfragen, die MicaBar selbst stellt:

| Zweck | Adresse | Wann |
| --- | --- | --- |
| Proxy-Ausgangs-IP | ipinfo.io → api.ipify.org → ifconfig.me (über den lokalen Proxy-Port; Gruppenausgänge über den Loopback-Port jeder Gruppe) | beim Start, bei Netz- / Proxy-Wechsel, alle 5 Min. |
| Direkte Ausgangs-IP | 4.ipw.cn → myip.ipip.net → ip.3322.net (an den physischen Adapter gebunden; aufgelöst über 223.5.5.5 / 119.29.29.29) | ebenso |
| IP-Typ und Inhaber | proxycheck.io (HTTPS) und ip-api.com (kostenlos nur HTTP), mit jeder Ausgangs-IP (auch der direkten) als Anfrage; über den lokalen Port des Proxy-Clients | wenn eine neue Ausgangs-IP auftaucht; 24 Std. Cache pro IP; abschaltbar unter Einstellungen → Netzwerk |
| Knotenlatenz | gemessen vom mihomo-Kern gegen `www.gstatic.com/generate_204` | alle 15–60 s (Ressourcenprofil) |
| gfw-Liste | cdn.jsdelivr.net / raw.githubusercontent.com (Loyalsoldier/v2ray-rules-dat) | täglich, wenn aktiviert |
| Update-Suche | latest.json von github.com/aileboeth/MicaBar | täglich, wenn aktiviert |
| KI-Kontingent | api.anthropic.com (Claude Code), chatgpt.com (Codex), cli-chat-proxy.grok.com (Grok), der lokale Sprachserver von Antigravity | alle 10 Min. (einstellbar), wenn aktiviert; immer über den lokalen Port des Proxy-Clients, nie direkt (kein Port = keine Abfrage) |
| Netzwerkdiagnose | direkt: die HTTP-Auflösungs-APIs von 119.29.29.29 / 223.5.5.5 (Abfrage von qq.com); über den Proxy: `www.gstatic.com/generate_204`; DNS: eine frische Auflösung von www.baidu.com über den System-DNS; die vollständige Prüfung pingt zusätzlich den Router | wenn aktiviert: eine kleine Anfrage alle 10 s (direkt), alle 30 s über den Proxy und an den DNS; die vollständige Prüfung nur bei Fehlschlägen |

Land / ASN stammen aus den Offline-Datenbanken des Proxy-Clients (`country.mmdb`, `ASN.mmdb`); die IPs der Verbindungen
werden nirgendwohin gesendet. Das KI-Kontingent nutzt die Anmeldung, die jedes Tool auf diesem PC hält, und schickt sie
nur an den eigenen Dienst dieses Tools; sie wird nie gespeichert, protokolliert oder erneuert. „Wiedergabe“ liest die
Mediensteuerung von Windows ohne Netzwerkzugriff. Auch Benachrichtigungen greifen nicht aufs Netz zu: Die des Tages liegen
nur im Arbeitsspeicher, Nachrichtentexte werden nie auf die Festplatte oder ins Protokoll geschrieben (und standardmäßig
nicht auf der Insel gezeigt); der Loopback-Port `127.0.0.1:47900` nimmt nur Aufrufe der KI-Hooks mit Token an. Das Geheimnis
des mihomo-Controllers wird zur Laufzeit aus dessen Konfiguration gelesen und nie gespeichert oder weitergegeben.

Der Zwischenablage-Verlauf geht nie ins Internet: Einträge bleiben auf diesem PC (`%LOCALAPPDATA%\MicaBar\clipboard`, verschlüsselt) oder nur im Speicher und nie im Protokoll.

## Aus dem Quellcode bauen

Benötigt Rust (rustup), Visual Studio C++ Build Tools, Node.js + pnpm.

```powershell
pnpm install
pnpm build            # eine exe: src-tauri\target\release\micabar.exe
pnpm release          # Installer + portabel + Update-Manifest -> dist\release-<Version>\ (benötigt keys\micabar.key)
```

Aufbau: `src-tauri\src\` (Rust-Backend: Socket-Tabellen, ETW, mihomo-Controller, Ausgangsprüfung, Leckschutz,
Fensterverhalten, Aufnahme fürs Milchglas…), `ui\` (reines HTML / CSS / JS ohne Bundler; `ui\i18n\` enthält die
Übersetzungen, mit dem chinesischen Ausgangstext im Code als Schlüssel), `tools\` (Release-Skript, Treiber für die Test-VM).
Implementierungsnotizen (Chinesisch): [docs/internals.zh-CN.md](docs/internals.zh-CN.md).

## Lizenz

[MIT](LICENSE). Flaggensymbole von [flag-icons](https://github.com/lipis/flag-icons) (MIT, siehe `ui/flags/LICENSE`). Logos der KI-Tools von [lobe-icons](https://github.com/lobehub/lobe-icons) (MIT, siehe `ui/brands/LICENSE`); die Marken gehören ihren Inhabern.

## Haftungsausschluss

MicaBar ist freie Open-Source-Software, die der Autor in seiner Freizeit entwickelt, bereitgestellt „wie besehen“, ohne
jede Gewährleistung. Es liest Netzwerkverbindungen, Prozesse, Systembenachrichtigungen und den Anmeldestatus von KI-Tools
auf diesem PC; diese Daten werden nur auf diesem PC verwendet. Ausgangssteuerung und Leckschutz ändern die Überschreibung
des Proxy-Clients, fügen Firewall-Regeln hinzu und können den Proxy-Client neu starten. Der Autor haftet nicht für Schäden
durch die Nutzung oder Nichtnutzbarkeit der Software (Datenverlust, Netzwerkausfälle, eingeschränkte Konten…). Nutze sie im
Einklang mit den örtlichen Gesetzen und den Bedingungen der jeweiligen Dienste. MicaBar steht in keiner Verbindung zu Clash,
mihomo, Anthropic, OpenAI, Google, xAI oder anderen hier genannten Projekten und Unternehmen; ihre Namen und Marken gehören
ihren Inhabern.
