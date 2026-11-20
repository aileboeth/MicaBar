# MicaBar

**English** · [简体中文](README.zh-CN.md)

A "dynamic island" network monitor for Windows. A frosted-glass pill at the top of the screen shows your exit IP and
connection count; hover it to see which program uses which port, where each connection goes and through which proxy
node — and decide right there whether it goes through the proxy or direct.

- **Exits per proxy group**: the exit IP and node of every group of your subscription (default proxy, AI,
  streaming…) plus the direct exit, with country and ISP / ASN.
- **Rotating pill**: group exits, the song playing (cover + title), AI quota, speed and node latency take turns;
  what rotates and how fast is configurable.
- **AI quota**: subscription usage (5-hour / weekly windows…) and reset times for Claude Code, Codex, Antigravity
  and Grok.
- **Notifications**: new messages from WeChat, Telegram, mail and other apps, and Claude Code / Codex finishing a
  task or waiting for approval show on the island — a small app icon at the pill's end, one line inside the pill, or
  a card, your choice; a notes page groups them by app. Message text is hidden by default (app + sender only);
  do-not-disturb, quiet hours, per-app mute and taking over an app's own popups.
- **Desktop window**: normally only the island and a tray icon; click the tray icon for a desktop window whose home
  page is an overview (exits, speed, traffic, latency, alerts, AI quota, busiest programs), with every page and all
  settings.
- **Any size**: width and height of the pill and the panel via sliders, or by dragging their edges on screen.
- **Per-program connections**: icon, process count, live up/down speed, proxied programs first; for every connection the local port →
  domain / IP, country, ISP, node and the rule that matched.
- **Exit control** (Clash Party / Mihomo Party): per program, domain, main domain, or "only this program to this
  domain" — proxy / direct / reject, applied instantly without restarting the core; switch and test the node used
  for "proxy" from the island.
- **Leak alerts**: when a site that should go through the proxy goes direct, the pill turns red and a Windows
  notification appears. Prevention: rule leaks are forced through the proxy; programs that bypass the proxy can be
  blocked from direct connections with one click (Windows Firewall).
- **Speed / traffic / nodes**: live speed graph, per-program proxy-vs-direct traffic (today, 7 or 30 days, click a
  day for its details), node latency and loss.
- **Looks and feel**: live frosted glass (white / black, follows the system), colour presets and custom themes;
  capsule / notch / line shapes docked to any screen edge; hover or click to open, pass-through mode, custom
  hotkey, keyboard navigation; stays out of the way of fullscreen games.

> The look and interaction are inspired by [fly-island](https://github.com/flyl1u/fly-island), some feature ideas
> by [BrimDeck](https://github.com/MoFeng2223/BrimDeck); all code is written independently.

The interface is in Chinese (Simplified).

## Install

Download from [Releases](https://github.com/aileboeth/MicaBar/releases):

- `MicaBar_<version>_x64-setup.exe` — installer (`C:\Program Files\MicaBar`), with in-app updates;
- `MicaBar_<version>_portable.exe` — single portable exe, no auto-update.

Requires Windows 10 2004+ / Windows 11 (x64) and the WebView2 runtime (built into Windows 11; the installer adds
it when missing). MicaBar runs elevated (per-program speed needs kernel ETW events), so it asks for UAC on start;
with "start with Windows" enabled it starts from a scheduled task without a prompt.

## Usage

- Rest the mouse on the pill for 0.3 s to open it (or switch to click); default hotkey `Ctrl+Alt+N`.
- Left-click the tray icon for the desktop window; right-click menu: open, settings, resize the island, re-check
  exit IPs, appearance, open log, quit.
- The gear in the panel opens the settings page of the desktop window: pill items, rotation, sizes, alert
  thresholds, shape, docking edge, theme, AI quota, now playing, autostart, resource profile and more — every
  feature can be switched off.

### Exits per group

With exit control enabled, the override opens one loopback-only port per proxy group (from `127.0.0.1:47800`) whose
traffic goes straight to that group. MicaBar checks the exit IP through each port, so it knows where "default",
"AI" or "YouTube" leave from. By default it shows the fallback group plus the groups with recent traffic (up to 4);
groups can also be picked by hand.

### Exit control

When Clash Party / Mihomo Party is detected, the island offers to enable exit control. Enabling backs up Clash
Party's override list, adds a global override "MicaBar 出口控制" and restarts Clash Party once (about 3 s offline);
if the core fails to load it, everything is restored. Afterwards pick a target from "出口" on a program row or "⋯"
on a connection. More specific rules win, so "this program direct" and "only this program to *.example.com via
proxy" work together. The override can be removed from the bottom of the "出口" tab.

In system-proxy mode only programs that honour the system proxy can be controlled; enable TUN to cover all of them.

### Supported proxy clients

| Client | Proxy exit / local port | Nodes, groups, matched rules | Exit control / leak prevention |
|---|---|---|---|
| Clash Party / Mihomo Party | ✓ | ✓ | ✓ |
| Clash Verge Rev | ✓ | ✓ (through its controller) | — |
| v2rayN, Throne (NekoRay), Hiddify, FlClash, Karing | ✓ (system proxy port or their usual local port) | — | — |

System proxy, TUN adapter and per-program connections are shown whatever the client.

### Notifications

Notes come from three places, all read locally:

- **Taskbar flashes**: WeChat 4.x signals a new message only by flashing its taskbar button (no Windows
  notification, no readable window), so the island can only say "WeChat · N new messages"; clicking opens WeChat.
  Other apps that flash work the same way.
- **App-drawn popups / Windows notifications**: with Windows notifications off, Telegram shows its own corner
  popups; when one appears MicaBar reads Telegram's chat list once (Windows UI Automation) and picks the chats whose
  new-message count went up. Apps using Windows notifications are read from the system's notification database.
  "Take over" hides the app's (or Windows') own popup so only the island shows it.
- Apps using Windows notifications (WhatsApp, Teams, Slack, Discord, Signal, Zoom, LINE…) show with their own name
  and icon; a message that both flashes the taskbar and raises a notification counts once.
- Opening the app marks its notes read and clears the red dot; a note that arrives while you are using that app
  is not announced.
- **AI tools**: "接入" in the settings adds a hook to Claude Code's `~/.claude/settings.json` or Codex's
  `~/.codex/config.toml` (backed up first, removable with one click); when a task finishes or needs approval it
  notifies `127.0.0.1:47900` with `curl` (with a random token).

### Leak alerts

The "should go through the proxy" library = a built-in list of common services (~140 domains) + the gfw list
(updated daily) + your own entries / imported lists (plain domains, v2fly and Clash rule formats). Two kinds of leaks:

- **Rule leak**: the connection went through the proxy client but a rule sent it direct. With "提前拦截" on, such
  domains are sent to the proxy before they reach the subscription's direct rules; per-service groups of the
  subscription are left alone.
- **Bypass**: the program connected directly without going through the proxy client (ignores the system proxy, or
  the connection predates TUN). The alert card can block the program's direct connections (a Windows Firewall rule
  for public addresses only; the program still reaches the local proxy port) and undo it later.

## Privacy: what MicaBar connects to

All statistics (connections, traffic, rules, alerts) stay on this PC in `%LOCALAPPDATA%\MicaBar\`. The only
requests MicaBar makes itself:

| Purpose | Address | When |
| --- | --- | --- |
| Proxy exit IP | ipinfo.io → api.ipify.org → ifconfig.me (through the local proxy port; group exits through each group's loopback port) | start, network / proxy change, every 5 min |
| Direct exit IP | 4.ipw.cn → myip.ipip.net → ip.3322.net (bound to the physical NIC; resolved via 223.5.5.5 / 119.29.29.29) | same |
| Node latency | measured by the mihomo core against `www.gstatic.com/generate_204` | every 15–60 s (resource profile) |
| gfw list | cdn.jsdelivr.net / raw.githubusercontent.com (Loyalsoldier/v2ray-rules-dat) | daily, when enabled |
| Update check | latest.json of github.com/aileboeth/MicaBar | daily, when enabled |
| AI quota | api.anthropic.com (Claude Code), chatgpt.com (Codex), cli-chat-proxy.grok.com (Grok), the local Antigravity language server | every 10 min (configurable), when enabled; always through the proxy client's local port, never direct (no port found = no query) |

Country / ASN lookups use the offline databases shipped with the proxy client (`country.mmdb`, `ASN.mmdb`);
connection IPs are never sent anywhere. AI quota uses the sign-in each tool keeps on this PC and sends it only to
that tool's own service; it is never stored, logged or renewed. Now playing reads the Windows media controls and
makes no network request. Notifications make no network request either: today's notes are kept in memory only,
message text is never written to disk or the log (and is not shown on the island by default); the loopback port
`127.0.0.1:47900` only accepts the AI hooks' calls carrying the token. The mihomo controller secret is read from
its config at runtime and never stored or sent.

## Build from source

Needs Rust (rustup), Visual Studio C++ Build Tools, Node.js + pnpm.

```powershell
pnpm install
pnpm build            # single exe: src-tauri\target\release\micabar.exe
pnpm release          # installer + portable + update manifest -> dist\release-<version>\ (needs keys\micabar.key)
```

Layout: `src-tauri\src\` (Rust backend: socket tables, ETW, mihomo controller, exit checks, leak guard, window
behaviour, glass capture…), `ui\` (plain HTML / CSS / JS, no bundler), `tools\` (release script, test-VM driver).
Implementation notes (Chinese): [docs/internals.zh-CN.md](docs/internals.zh-CN.md).

## License

[MIT](LICENSE). Flag icons from [flag-icons](https://github.com/lipis/flag-icons) (MIT, see `ui/flags/LICENSE`).
