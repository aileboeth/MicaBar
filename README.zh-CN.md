# MicaBar

[English](README.md) · **简体中文**

Windows 上的「灵动岛」式网络监视器。屏幕顶部一颗磨砂玻璃胶囊，平时显示出口 IP 和连接数；鼠标停上去展开，
看每个程序在用什么端口、连到哪里、走的哪个代理节点，并且能直接指定它走代理还是直连。

- **按分组看出口**：订阅里每个分组（默认代理、AI、流媒体……）各自的出口 IP 和所用节点，外加直连出口，带国家、运营商 / ASN。
- **胶囊轮换显示**：各分组出口、正在播放的歌（封面 + 歌名）、AI 额度、网速、节点延迟依次轮换，内容和间隔都可设置。
- **AI 额度**：Claude Code、Codex、Antigravity、Grok 的订阅用量（5 小时 / 每周等）和重置时间。
- **桌面管理窗口**：平时只有岛和托盘图标；点托盘图标打开桌面窗口，首页是总览（出口、网速、流量、延迟、预警、正在播放、AI 额度、最活跃的程序），所有页面和设置都在这里。
- **大小随意调**：胶囊和展开面板的宽高可以用滑块调，也可以在屏幕上直接拖边缘。
- **每个程序的连接**：程序图标、进程数、实时上下行，走代理的程序排在前面；每条连接的本地端口 → 域名 / IP、国家、运营商、节点、命中的规则。
- **出口控制**（Clash Party / Mihomo Party）：按程序、域名、主域，或「仅这个程序访问这个域名」选择 代理 / 直连 / 拦截，
  即时生效，不重启内核；「代理」使用的节点可在岛上切换、测速。
- **泄漏预警**：本该走代理的网站直连了就报警（胶囊变红 + 系统通知），可以提前拦截：
  规则漏网时强制走代理；程序绕过代理时一键用 Windows 防火墙禁止它直连。
- **网速 / 流量 / 节点**：网速曲线、按程序的「代理 / 直连」流量统计（今天、7 天）、节点延迟与丢包。
- **好看也好用**：实时磨砂玻璃（白 / 黑，跟随系统）、多套配色和自定义主题；胶囊 / 刘海 / 细线三种外形，可停靠任意一边；
  悬停或点击展开、穿透模式、自定义快捷键、键盘操作；全屏游戏时不打扰。

> 外观和交互参考了 [fly-island](https://github.com/flyl1u/fly-island)，部分功能思路参考了
> [BrimDeck](https://github.com/MoFeng2223/BrimDeck)；代码全部独立编写。

## 安装

在 [Releases](https://github.com/aileboeth/MicaBar/releases) 下载：

- `MicaBar_<版本>_x64-setup.exe`：安装版，装到 `C:\Program Files\MicaBar`，支持应用内自动更新；
- `MicaBar_<版本>_portable.exe`：便携版，单个 exe，不能自动更新。

要求 Windows 10 2004+ / Windows 11（x64）和 WebView2 运行时（Windows 11 自带；安装版会自动安装）。
程序需要管理员权限（按程序统计网速要用内核 ETW 事件），启动时会弹 UAC；开了「开机自启」后用计划任务启动，不再弹。

## 使用

- 鼠标停在胶囊上 0.3 秒展开（可改成点击）；快捷键默认 `Ctrl+Alt+N`。
- 左键点托盘图标打开桌面窗口；右键菜单：打开、设置、调整岛的大小、重新检测出口 IP、外观、打开日志、退出。
- 面板右上角的齿轮打开桌面窗口的设置页：胶囊里显示什么、轮换内容、大小、提示阈值、外形、停靠位置、主题、
  AI 额度、正在播放、开机自启（系统页还能开关其他程序的开机自启）、资源档位等，每一项都能开关。

### 分组出口

启用出口控制后，覆写会给每个代理分组开一个只监听本机的端口（`127.0.0.1:47800` 起），从这个端口进来的流量直接交给
对应分组。MicaBar 从各端口分别查一次出口 IP，就知道「默认代理」「AI」「YouTube」各自从哪里出去。默认显示兜底分组 +
最近有流量的分组（最多 4 个），也可以在设置里手动勾选。

### 出口控制

第一次检测到 Clash Party / Mihomo Party 时，岛上会提示启用。启用会：备份 Clash Party 的覆写列表，加一个全局覆写
「MicaBar 出口控制」，并重启 Clash Party 一次（断网约 3 秒）；加载失败会自动还原。之后在程序右边的「出口」
或连接右边的「⋯」里选择即可。更具体的规则优先：「整个程序直连」+「仅它访问 *.example.com 走代理」两条可以同时生效。
在「出口」页底部可以停用并移除覆写。

系统代理模式下只能管住遵守系统代理的程序；要管住所有程序请开启 TUN。

### 泄漏预警

「应走代理」规则库 = 内置常见服务清单（约 140 个主域）+ 订阅 gfw 列表（每天更新）+ 自定义 / 导入（支持纯域名、
v2fly、Clash 规则格式）。两种泄漏：

- **规则漏网**：经过了代理软件，但被分流规则判成直连。开启「提前拦截」后，这类域名在落进直连规则之前就被转到代理，
  订阅里按服务分好的分组不受影响。
- **绕过代理**：程序没经过代理软件直接连了出去（不走系统代理，或连接早于 TUN）。预警卡上可以一键
  「禁止 xxx 直连」（Windows 防火墙规则，只拦公网地址，程序仍能通过本机代理端口上网），可随时解除。

## 隐私：MicaBar 会连接哪些地方

所有统计（连接、流量、规则、预警记录）都只存在本机 `%LOCALAPPDATA%\MicaBar\`。程序主动发起的网络请求只有：

| 用途 | 地址 | 时机 |
| --- | --- | --- |
| 代理出口 IP | ipinfo.io → api.ipify.org → ifconfig.me（经本地代理端口；分组出口经各分组的本机端口） | 启动、网络 / 代理变化、每 5 分钟 |
| 直连出口 IP | 4.ipw.cn → myip.ipip.net → ip.3322.net（绑定物理网卡；域名经 223.5.5.5 / 119.29.29.29 解析） | 同上 |
| 节点延迟 | 由 mihomo 内核测 `www.gstatic.com/generate_204` | 15~60 秒一次（资源档位） |
| gfw 列表 | cdn.jsdelivr.net / raw.githubusercontent.com（Loyalsoldier/v2ray-rules-dat） | 开启时每天一次 |
| 检查更新 | github.com/aileboeth/MicaBar 的 latest.json | 开启时每天一次 |
| AI 额度 | api.anthropic.com（Claude Code）、chatgpt.com（Codex）、cli-chat-proxy.grok.com（Grok）、本机的 Antigravity 语言服务器 | 开启时每 10 分钟（可改）；一律经代理软件的本地端口发出，绝不直连（找不到端口就不查） |

国家 / ASN 用的是代理客户端自带的离线库（`country.mmdb`、`ASN.mmdb`），不会把连接的 IP 发给任何服务。
AI 额度用各工具在本机已登录的凭据，只发给那个工具自己的服务，不保存、不写日志、不续期。
正在播放读的是 Windows 媒体控制接口，不联网。
mihomo 控制接口的密钥只在运行时从配置文件读取，不会保存或外传。

## 从源码编译

需要 Rust（rustup）、Visual Studio C++ Build Tools、Node.js + pnpm。

```powershell
pnpm install
pnpm build            # 单个 exe：src-tauri\target\release\micabar.exe
pnpm release          # 安装包 + 便携版 + 自动更新清单 -> dist\release-<版本>\（需要更新签名私钥 keys\micabar.key）
```

源码结构：`src-tauri\src\`（Rust 后端：连接表、ETW、mihomo 控制接口、出口检测、守卫、窗口控制、磨砂截屏……），
`ui\`（无需打包的纯 HTML / CSS / JS 前端），`tools\`（发布脚本、测试虚拟机驱动）。
实现细节见 [docs/internals.zh-CN.md](docs/internals.zh-CN.md)。

## 许可

[MIT](LICENSE)。国旗图标来自 [flag-icons](https://github.com/lipis/flag-icons)（MIT，见 `ui/flags/LICENSE`）。
