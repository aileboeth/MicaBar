# MicaBar

**English** · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [Русский](README.ru.md) · [Español](README.es.md) · [Português (Brasil)](README.pt-BR.md) · [Français](README.fr.md) · [Deutsch](README.de.md)

A "dynamic island" network monitor for Windows. A frosted-glass pill at the top of the screen shows your exit IP and
connection count; rest the mouse on it to see which program uses which port, where each connection goes and through
which proxy node — and decide right there whether it goes through the proxy or direct.

## Features

- **Exits per proxy group**: the exit IP and node of every group of your subscription (default proxy, AI,
  streaming…) plus the direct exit, with country and ISP / ASN, and the IP's type (home / business / hosting /
  mobile, plus a mark when it is listed as a proxy) and owner (carrier and province for mainland China IPs).
- **Rotating pill**: group exits, the song playing (cover + title), AI quota, speed and node latency take turns;
  what rotates and how fast is configurable.
- **AI quota**: subscription usage (5-hour / weekly windows…) and reset times for Claude Code, Codex, Antigravity
  and Grok, each ring with the tool's own mark.
- **AI tasks at work**: while Claude Code works, the front of the pill shows the project and the time so far; it turns
  orange when it waits for your approval. Also at the top of the open island; one click brings its terminal to the
  front. Interrupting with Esc or closing the terminal clears it.
- **Network diagnosis**: a light probe every 10 s; two failures in a row run a full check along the chain (adapter →
  router → DNS → internet → proxy app → node). The pill says which link broke ("DNS broken", "Node down",
  "Broadband down"…) and the sheet offers a fix: flush the DNS cache, switch to a working node, restart the proxy
  app… Recovery shows how long it was down. A system proxy switched off or changed by another program is reported
  with a one-click restore.
- **First network access**: a card on the island when a program reaches the internet for the first time — "Allow"
  or "Block" (a Windows Firewall rule; with exit control on, its traffic through the system proxy is rejected too).
  The first run learns your usual programs for 10 minutes; version folders are folded, so updates don't count as new.
- **Notifications**: new messages from WeChat, Telegram, mail and other apps, and Claude Code / Codex finishing a
  task or waiting for approval, show on the island — a small app icon at the pill's end, one line inside the pill, or
  a card, your choice; the Notifications page groups them by app. Message text is hidden by default (app + sender
  only); do not disturb, quiet hours, per-app mute and taking over an app's own pop-ups.
- **Per-program connections**: icon, process count, live up/down speed, proxied programs first; for every connection
  the local port → domain / IP, country, ISP, node and the rule that matched.
- **Exit control** (Clash Party / Mihomo Party): per program, domain, main domain, or "only this program to this
  domain" — proxy / direct / block, applied instantly without restarting the core; switch and test the node used
  for "Proxy" from the island.
- **Leak alerts**: when a site that should go through the proxy goes direct, the pill turns red and a Windows
  notification appears. Prevention: rule misses are forced through the proxy; programs that bypass the proxy can be
  blocked from direct connections with one click (Windows Firewall).
- **Speed / traffic / nodes**: live speed chart, per-program proxy-vs-direct traffic (today, 7 or 30 days), node
  latency and loss.
- **System monitor**: CPU / GPU / memory per program, end processes; switch startup apps on and off like Task Manager.
- **Desktop window**: normally only the island and a tray icon; the desktop window has an overview (exits, speed,
  traffic, latency, alerts, AI quota, busiest programs) and all settings.
- **Looks and feel**: live frosted glass (light / dark, follows the system) that keeps text readable over any
  background, colour presets and custom themes; capsule / notch / indicator-bar shapes docked to any screen edge;
  hover or click to open, click-through mode, custom hotkey, keyboard navigation; stays out of the way of full-screen
  games; fluid animations that follow the Windows animation setting.
- **10 interface languages**: 简体中文, 繁體中文, English, 日本語, 한국어, Русский, Español, Português (Brasil),
  Français, Deutsch — follows the Windows display language by default.

> The look and interaction are inspired by [fly-island](https://github.com/flyl1u/fly-island), some feature ideas
> by [BrimDeck](https://github.com/MoFeng2223/BrimDeck); all code is written independently.

## Install

Download from [Releases](https://github.com/aileboeth/MicaBar/releases):

- `MicaBar_<version>_x64-setup.exe` — installer (`C:\Program Files\MicaBar`), with in-app updates;
- `MicaBar_<version>_portable.exe` — single portable exe, no auto-update.

Requires Windows 10 2004+ / Windows 11 (x64) and the WebView2 runtime (built into Windows 11; the installer adds
it when missing). MicaBar runs elevated (per-program speed needs kernel ETW events), so it asks for UAC on start;
with "Start with Windows" enabled it starts from a scheduled task without a prompt.

## How to use

On the first start, a half-minute tour on the island walks you through the basics below; replay it any time from
Settings → About → "Replay tour".

### Expand and collapse

| Action | Result |
| --- | --- |
| Rest the mouse on the pill for about 0.5 s | It expands into the panel (the default "Hover" mode; the delay is in Settings → General → Interaction) |
| Move the mouse off the panel | It collapses |
| Click the pill / click outside | Expand / collapse in "Click" mode (switch under Settings → General → Interaction → "How to expand") |
| `Ctrl+Alt+N` | Expand / collapse any time (change it: click the hotkey box in Settings and press the new combination) |
| `Esc` | Collapse the panel; closes an open sheet or menu first |

- In "Hover" mode the collapsed pill never blocks clicks: while the mouse rests on it the pill turns see-through so
  you can see the tab it covers, and a click goes to the window below.
- **Click-through mode**: the mouse can't touch the island at all, only the hotkey opens it — handy when you never
  want to hit it by accident.
- While a full-screen app (game, video) is in front, the pill stays visible but never auto-expands or blocks clicks;
  it can also be set to hide completely.
- Optional "Shrink to the indicator bar when a window is maximized": while a maximized window is in front, the pill
  becomes a thin line and comes back when you switch away.

### Drag, fling and dock

The island docks to the top, bottom, left or right edge of the screen, at the start, middle or end of that edge.

1. **Grab it**: with the panel expanded, **press and hold still for about 0.26 s** anywhere on the panel (the time is
   in Settings → General → Interaction → "Long press to grab the island"). When it's collapsed, **hold `Alt` and then
   long-press the pill** (with Alt held the pill stops being see-through, so the press grabs it instead of the window
   below).
2. **Fling along the edge**: once grabbed, flick it left or right and let go — the island slides to the start,
   middle or end of that edge.
3. **Move to another edge**: once grabbed, pull it toward the middle of the screen and it follows the mouse; release
   it near another edge and it docks there. On the left / right edge the capsule becomes a vertical notch.

If you'd rather not drag, pick the edge and the position on it in Settings → Collapsed look.

### Resize

- Tray icon right-click → "Resize the island…", the "Resize the island" button at the bottom left of the desktop
  window, or Settings → Collapsed look / Expanded look → "Drag on screen to resize": the pill and panel on screen get
  draggable edges; drag them to change width and height, then click "Done" (`Esc` cancels).
- Or use the sliders in Settings → Collapsed look / Expanded look: pill height, width (0 = grows with the content),
  panel width, max height and interface scale.

### Keyboard

With the panel expanded: `Tab` / `Shift+Tab` move the focus in order; with the focus on the page icons at the top or
on the Network tabs, `←` / `→` switch; `↑` / `↓` move between rows and buttons of the list; `Enter` / `Space` expand
the current program; `Esc` collapses. Lists you can reorder in Settings also take `Alt+↑` / `Alt+↓` on the selected item.

### The pages

The icons along the top of the panel switch pages (Network, System, Music, AI quota, Notifications); which pages show
and their order are in Settings → Expanded look, and the island remembers the page you were on.

**Network**

- At the top are the **exit cards**. On the left the proxy exits: one row per exit IP with its type tag (Home /
  Business / Hosting / Mobile / Flagged proxy), the node and owner on the second line, and the groups using that IP on
  the right — what doesn't fit folds into "+N"; click it to see them all and "Collapse" to fold again. On the right the
  direct exit (carrier, adapter, reverse DNS, last check). Rest the mouse on an IP or tag for the registered owner,
  provider, ASN and data source. ↻ at the top right checks the exits again; the pulse icon next to it opens the
  **network diagnosis**.
- The line under the cards shows the proxy app, system proxy, TUN, local port and controller.
- Tabs: **Conns / Listen / Exit / Alerts / Speed / Traffic / Nodes** (scroll them with the mouse wheel when they don't
  fit; reorder them in Settings). The search box on the right filters by program, domain, IP or port.
  - **Conns**: one row per program that is online (proxied ones first). Click a row to expand its connections; the
    "Exit" button on the right picks Proxy / Direct / Block for the whole program, the "⋯" next to a connection only
    for that connection (this domain, this IP, or "only this program to it"). Clicking the selected button again =
    back to the rules.
  - **Listen**: ports listening on this PC; those listening on all adapters (reachable from the LAN) are marked.
  - **Exit**: enable / disable exit control, see and delete exit rules, switch and test the node used for "Proxy".
  - **Alerts**: proxy leaks and first network access; block direct connections, ignore a domain, allow again.
  - **Speed**: the live speed chart. Move the mouse left / right over it and it snaps to each second while the big
    numbers follow; click a program under the chart to pull up a sheet with its own chart and numbers. Drag the sheet
    up / down, fling it down to close; it has "See its connections" and "Show only it in the big chart".
  - **Traffic**: proxy / direct traffic per program for today / 7 days / 30 days; click a bar for that day's details.
  - **Nodes**: latency and loss of each exit node; "Test now" runs a round right away.
- **Network diagnosis** (the pulse icon, or the red alert on the pill when you're offline): checks from the adapter
  to the node, tells you which link broke and offers fix buttons below (flush the DNS cache, restore the system proxy,
  open / restart the proxy app, switch to a working node…).

**System**: usage of every CPU / GPU core, memory and VRAM; below it switch between "Top apps" (sorted by CPU or
memory; ✕ ends a process after asking — Windows core processes and MicaBar itself are never ended) and "Startup apps"
(the same switch as Task Manager's "Startup apps"; turning one off deletes nothing).

**Music**: the song playing (NetEase Cloud Music, QQ Music, Spotify, videos in a browser…) with cover, title, bars
moving with the music, and previous / play-pause / next.

**AI quota**: one card per AI tool with how much of each quota window is used and when it resets; "Refresh now"
checks right away (the official APIs rate-limit hard; Claude at most every 10 minutes).

**Notifications**: grouped by app; click one to open its app, "Mark all read", "Clear all"; **right-click** a group to
mute that app or clear the group.

### What the pill tells you

- What the pill shows (exit IP, speed, AI quota rings, clock, CPU / memory…) is ticked and reordered in Settings →
  Collapsed look → "Content"; items that don't fit join the last cell and take turns.
- When you're offline, the system proxy was changed, or Claude Code is working or waiting for your approval, the
  matching alert appears at the front of the pill; click it to open the diagnosis or switch to that terminal.
- Exit IP changes, a node not responding or getting slow, and traffic spikes show briefly on the pill for 5 seconds
  (switches and thresholds in Settings → Network → "Short alerts").
- The pill turns red when there are leaks; unread notifications show their app icon and a red dot on the right.

### Desktop window, settings and tray

- **Left-click the tray icon** or the **gear** at the top right of the panel to open the desktop window: Overview,
  Connections, Listening ports, Exit control, Leak alerts, Speed, Traffic, Nodes, System, AI quota, Notifications and
  Settings.
- **Right-click the tray icon**: Open MicaBar, Settings, Resize the island, Check exit IP again, Look (follow system /
  light frost / dark frost), Open log, Quit.
- Settings are grouped in the sidebar (Basics: General, Collapsed look, Expanded look, Panel theme; Network; AI usage;
  Music; Notifications; About) and every feature can be switched off; shapes and themes are picked from preview
  cards, and lists with ⠿ are reordered by dragging. "Reset all settings" is at the bottom.
- **Interface language**: Settings → General → "Language"; the default "Follow system" uses the Windows display
  language. After a change all windows reload and the tray menu switches too.

## Exits per group

With exit control enabled, the override opens one loopback-only port per proxy group (from `127.0.0.1:47800`) whose
traffic goes straight to that group. MicaBar checks the exit IP through each port, so it knows where "default",
"AI" or "YouTube" leave from. By default it shows the fallback group plus the groups with recent traffic (up to 4);
groups can also be picked by hand.

## Exit control

When Clash Party / Mihomo Party is detected, the island offers to enable exit control. Enabling backs up Clash
Party's override list, adds a global override "MicaBar 出口控制" and restarts Clash Party once (about 3 s offline);
if the core fails to load it, everything is restored. Afterwards pick a target from "Exit" on a program row or "⋯"
on a connection. More specific rules win, so "this program direct" and "only this program to *.example.com via
proxy" work together. The override can be removed from the bottom of the "Exit" tab.

In system-proxy mode only programs that honour the system proxy can be controlled; enable TUN to cover all of them.

## Supported proxy clients

| Client | Proxy exit / local port | Nodes, groups, matched rules | Exit control / leak prevention |
|---|---|---|---|
| Clash Party / Mihomo Party | ✓ | ✓ | ✓ |
| Clash Verge Rev | ✓ | ✓ (through its controller) | — |
| v2rayN, Throne (NekoRay), Hiddify, FlClash, Karing | ✓ (system proxy port or their usual local port) | — | — |

System proxy, TUN adapter and per-program connections are shown whatever the client.

## Notifications

Notifications come from three places, all read locally:

- **Taskbar flashes**: WeChat 4.x signals a new message only by flashing its taskbar button (no Windows
  notification, no readable window), so the island can only say "WeChat · N new messages"; clicking opens WeChat.
  Other apps that flash work the same way.
- **App-drawn pop-ups / Windows notifications**: with Windows notifications off, Telegram shows its own corner
  pop-ups; when one appears MicaBar reads Telegram's chat list once (Windows UI Automation) and picks the chats whose
  new-message count went up. Apps using Windows notifications are read from the system's notification database.
  "Take over pop-ups" hides the app's (or Windows') own pop-up so only the island shows it.
- Apps using Windows notifications (WhatsApp, Teams, Slack, Discord, Signal, Zoom, LINE…) show with their own name
  and icon; a message that both flashes the taskbar and raises a notification counts once.
- Opening the app marks its notifications read and clears the red dot; one that arrives while you are using that
  app is not announced.
- **AI done alerts**: "Connect" in Settings → Notifications adds a hook to Claude Code's `~/.claude/settings.json` or
  Codex's `~/.codex/config.toml` (backed up first, removable with one click); when a task finishes or needs approval
  it notifies `127.0.0.1:47900` with `curl` (with a random token).

## Leak alerts

The "should go through the proxy" library = a built-in list of common services (~140 main domains) + the gfw list
(updated daily) + your own entries / imported lists (plain domains, v2fly and Clash rule formats). Two kinds of leaks:

- **Rule miss**: the connection went through the proxy app but a rule sent it direct. With "Catch it early" on, such
  domains are sent to the proxy before they reach the subscription's direct rules; per-service groups of the
  subscription are left alone.
- **Bypass**: the program connected directly without going through the proxy app (ignores the system proxy, or the
  connection predates TUN). The alert card can block the program's direct connections (a Windows Firewall rule for
  public addresses only; the program still reaches the local proxy port) and undo it later.

## Privacy: what MicaBar connects to

All statistics (connections, traffic, rules, alerts) stay on this PC in `%LOCALAPPDATA%\MicaBar\`. The only
requests MicaBar makes itself:

| Purpose | Address | When |
| --- | --- | --- |
| Proxy exit IP | ipinfo.io → api.ipify.org → ifconfig.me (through the local proxy port; group exits through each group's loopback port) | start, network / proxy change, every 5 min |
| Direct exit IP | 4.ipw.cn → myip.ipip.net → ip.3322.net (bound to the physical NIC; resolved via 223.5.5.5 / 119.29.29.29) | same |
| IP type and owner | proxycheck.io (HTTPS) and ip-api.com (HTTP only on the free tier), with each exit IP (the direct one too) as the query; through the proxy client's local port | when a new exit IP shows up; cached 24 h per IP; can be turned off in Settings → Network |
| Node latency | measured by the mihomo core against `www.gstatic.com/generate_204` | every 15–60 s (resource profile) |
| gfw list | cdn.jsdelivr.net / raw.githubusercontent.com (Loyalsoldier/v2ray-rules-dat) | daily, when enabled |
| Update check | latest.json of github.com/aileboeth/MicaBar | daily, when enabled |
| AI quota | api.anthropic.com (Claude Code), chatgpt.com (Codex), cli-chat-proxy.grok.com (Grok), the local Antigravity language server | every 10 min (configurable), when enabled; always through the proxy client's local port, never direct (no port found = no query) |
| Network diagnosis | direct: the HTTP resolve APIs of 119.29.29.29 / 223.5.5.5 (looking up qq.com); through the proxy: `www.gstatic.com/generate_204`; DNS: a fresh lookup of www.baidu.com through the system DNS; a full check also pings the router | when enabled: one small request every 10 s (direct), every 30 s through the proxy and to DNS; the full check only when they fail |

Country / ASN lookups use the offline databases shipped with the proxy client (`country.mmdb`, `ASN.mmdb`);
connection IPs are never sent anywhere. AI quota uses the sign-in each tool keeps on this PC and sends it only to
that tool's own service; it is never stored, logged or renewed. Now playing reads the Windows media controls and
makes no network request. Notifications make no network request either: today's notifications are kept in memory
only, message text is never written to disk or the log (and is not shown on the island by default); the loopback
port `127.0.0.1:47900` only accepts the AI hooks' calls carrying the token. The mihomo controller secret is read from
its config at runtime and never stored or sent.

## Build from source

Needs Rust (rustup), Visual Studio C++ Build Tools, Node.js + pnpm.

```powershell
pnpm install
pnpm build            # single exe: src-tauri\target\release\micabar.exe
pnpm release          # installer + portable + update manifest -> dist\release-<version>\ (needs keys\micabar.key)
```

Layout: `src-tauri\src\` (Rust backend: socket tables, ETW, mihomo controller, exit checks, leak guard, window
behaviour, glass capture…), `ui\` (plain HTML / CSS / JS, no bundler; `ui\i18n\` holds the translations, keyed by the
Chinese source text in the code), `tools\` (release script, test-VM driver).
Implementation notes (Chinese): [docs/internals.zh-CN.md](docs/internals.zh-CN.md).

## License

[MIT](LICENSE). Flag icons from [flag-icons](https://github.com/lipis/flag-icons) (MIT, see `ui/flags/LICENSE`). AI tool marks from [lobe-icons](https://github.com/lobehub/lobe-icons) (MIT, see `ui/brands/LICENSE`); the marks belong to their owners.

## Disclaimer

MicaBar is free open-source software made by its author in their spare time, provided "as is", without warranty of
any kind. It reads network connections, processes, system notifications and the sign-in state of AI tools on this
PC; this data is used on this PC only. Exit control and the leak guard change your proxy client's override
configuration, add firewall rules and may restart the proxy client. The author is not liable for any loss from using
or being unable to use the software (data loss, network outages, restricted accounts…). Use it in line with local laws
and each service's terms. MicaBar is not affiliated with Clash, mihomo, Anthropic, OpenAI, Google, xAI or any other
project or company named here; their names and trademarks belong to their owners.
