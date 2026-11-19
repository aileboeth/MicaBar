# MicaBar 实现说明

## 数据来源

| 内容 | 来源 |
| --- | --- |
| 连接表 + PID | IP Helper `GetExtendedTcpTable` / `GetExtendedUdpTable` |
| 每程序网速 | ETW `Microsoft-Windows-Kernel-Network`（需要管理员） |
| 程序名 / 图标 | `QueryFullProcessImageNameW`、文件版本信息 FileDescription、`SHDefExtractIconW` |
| 域名 | mihomo 连接的 host / sniffHost；Windows DNS 缓存反查；ETW `Microsoft-Windows-DNS-Client` 事件 3008 的实时解析（TUN 下 DNS 缓存几乎是空的） |
| 国家 / ASN | 代理客户端自带的 `country.mmdb`（mihomo Meta-geoip0 或 MaxMind 格式）和 `ASN.mmdb`，或 exe 旁边的 `geo\` 目录 |
| 节点 / 规则 | mihomo 控制接口 `/connections`（命名管道 `\\.\pipe\MihomoParty\...` 或 external-controller） |
| 出口 IP | 代理：ipinfo.io → api.ipify.org → ifconfig.me；直连：4.ipw.cn → myip.ipip.net → ip.3322.net |

直连出口检测绑定物理网卡的地址发出（Windows 按源地址选网卡），域名先用 223.5.5.5 DoH / 119.29.29.29 解析，
所以 TUN 和分流规则（比如把「查 IP」网站送去代理的规则组）都影响不到它。

支持识别的代理客户端：Clash Party / Mihomo Party、Clash Verge、v2rayN、sing-box、xray 等（按进程名）。
节点和规则信息目前只有 mihomo 内核提供；出口控制只支持 Clash Party / Mihomo Party。

## 出口控制

| 范围 | mihomo 规则 | 层级 |
| --- | --- | --- |
| 仅本程序访问某域名 / IP | `AND,((PROCESS-NAME,x),(DOMAIN,y))` / `IP-CIDR` | 1 |
| 仅本程序访问某主域 | `AND,((PROCESS-NAME,x),(DOMAIN-SUFFIX,y))` | 2 |
| 某域名 / IP | `DOMAIN` / `IP-CIDR,…,no-resolve` | 3 |
| 某主域 `*.x.com` | `DOMAIN-SUFFIX` | 4 |
| 整个程序 | `PROCESS-NAME` | 5 |

层级小的先匹配。每个层级 × 目标（拦截 / 直连 / 代理）是一个 file 规则集 `micabar-<层级>-<目标>`，
文件在内核目录（mihomo 的 `-d`）下的 `micabar\`。「代理」= 覆写自带的 `MicaBar` 分组（`include-all`）。

启用流程：Clash Party 把覆写列表 `override.yaml` 缓存在内存里、之后会整份写回，所以运行中改文件会被冲掉。
启用时：备份 `override.yaml` 到 `%LOCALAPPDATA%\MicaBar\backup\` → 结束 Clash Party 和内核 → 写入全局
JS 覆写 `override\micabar.js` 并登记 → 用原来的权限重新打开 Clash Party（原来不是管理员时经 explorer.exe 降权）
→ 等内核报告规则集都已加载；超时则结束、还原、重启。之后每次点选只改 `micabar\*.yaml`，调用
`PUT /providers/rules/<名字>` 刷新，再 `DELETE /connections/<id>` 断开受影响的连接让程序重连。
规则本身存在 `%LOCALAPPDATA%\MicaBar\rules.json`。覆写内容和当前版本不一致时提示「更新」。

## 泄漏守卫

JS 覆写把 3 个守卫规则集（`micabar-guard-domain` / `-kw` / `-ip` → `MicaBar`）插在订阅里
**最后一条按目的地分到代理组的规则之后、下一条直连规则之前**（目标是 DIRECT，或默认选中 DIRECT 的分组；
NETWORK / DST-PORT / PROCESS 之类不按目的地分流的规则不算）。例如插在 `geolocation-!cn` 之后、
`cn_domain → 🇨🇳 国内服务` 之前，于是 Google、Netflix、AI 等按服务分好的组照旧，只有会落进直连的才被截住。

检测：经过 mihomo 且链路为 `DIRECT`（MicaBar 自己的规则除外）= 规则漏网；mihomo 在运行、目的地是公网、
连接却没经过它 = 绕过代理。同一程序 + 目的地 10 分钟内只通知一次；全屏时不弹通知。

禁止直连：`netsh advfirewall` 出站阻止规则，`program=` 该程序，`remoteip=` 公网地址段（不含回环、私网、链路本地、
CGNAT、组播），再用 `SetTcpEntry` 重置它现有的 IPv4 连接。TUN 模式下不提供（程序的流量也走虚拟网卡，会彻底断网）。

通知：未打包的 exe 要先在 `HKCU\Software\Classes\AppUserModelId\com.aile.micabar` 登记 DisplayName / IconUri。

## 窗口与交互

- 窗口是固定大小的透明置顶窗口；胶囊 / 面板由页面绘制，位置用可动画的 CSS 变量（`@property --ix/--iy`）。
- 后端每 15 ms 读光标位置，按页面报告的胶囊 / 面板矩形（物理像素）决定是否接收鼠标（`set_ignore_cursor_events`）。
  悬停要停留一段时间才展开；按着鼠标键时不展开；离开或在外面点击就收起；点击模式下由页面报告按下事件。
- 前台窗口铺满显示器 = 全屏（游戏）：不自动展开、点击穿透、磨砂截屏暂停。
- 页面的 devicePixelRatio 包含 Windows「文本大小」，窗口尺寸由页面按它计算。

## 磨砂玻璃

DXGI 桌面复制（本窗口用 `SetWindowCompositionAttribute(WCA_EXCLUDED_FROM_DDA)` 排除，截图仍然能看到它），
GPU 生成 mip 取 1/4 尺寸，CPU 三遍盒式模糊 + 轻微提高饱和度，原始字节通过 Tauri Channel 发给页面画到 canvas；
只在岛背后的区域有变化时出帧。着色随背景亮度调整。

## 发布

```powershell
.\tools\build-release.ps1 -Notes "更新说明"
# -> dist\release-<版本>\  MicaBar_<v>_x64-setup.exe (+ .sig)、MicaBar_<v>_portable.exe、latest.json、SHA256SUMS.txt
```

把这些文件上传到 Release `v<版本>`。应用内更新读 `releases/latest/download/latest.json`，校验签名后静默安装。
签名私钥 `keys\micabar.key` 不进仓库；公钥在 `src-tauri\tauri.conf.json`。

## 测试虚拟机

`tools\vm-test.ps1` 通过 SSH 操作一台 Windows 测试机（环境变量 `MICABAR_VM=user@host`，可选
`MICABAR_VM_KEY`）：复制程序、用计划任务 `/IT` 启动到已登录的桌面、截图 / 点击 / 拖动 / 按键 / 滚轮、
修改设置并重启、读日志。`tools\vm\fullscreen.ps1` 模拟全屏游戏窗口。

## 分组出口

JS 覆写给每个 select / url-test / fallback / load-balance 分组加一个 `listeners` 入站：`mixed`、只听 `127.0.0.1`、
端口 `47800 + 序号`、`proxy: <分组名>`（进来的流量跳过规则直接交给该分组）。MicaBar 从运行中的配置文件读出这些入站，
经各端口查出口 IP，并沿 `/proxies/<分组>` 的 `now` 一路找到实际节点。自动模式 = MATCH 规则的兜底分组 + 按连接流量
打分（半衰期约 23 分钟）最高的分组；新分组开始有流量时最多每分钟补测一次。

## AI 额度

| 工具 | 凭据 | 查询 |
| --- | --- | --- |
| Claude Code | `~/.claude/.credentials.json` 的 `claudeAiOauth.accessToken` | `GET api.anthropic.com/api/oauth/usage`（`anthropic-beta: oauth-2025-04-20`）：`five_hour` / `seven_day` 的 `utilization`、`resets_at` |
| Codex | `~/.codex/auth.json`（ChatGPT 登录） | `GET chatgpt.com/backend-api/wham/usage`；没有时读会话日志里最近的 `rate_limits`；自定义服务商（API）没有订阅额度 |
| Antigravity | 运行中的 `language_server` 命令行里的 `--csrf_token` | 本机端口 `…/LanguageServerService/RetrieveUserQuotaSummary` 或 `GetUserStatus`（自签名证书，只连回环） |
| Grok | `~/.grok/auth.json`（grok CLI 登录） | `GET cli-chat-proxy.grok.com/v1/billing?format=credits` |

凭据每次查询时读取，只发给该工具自己的服务，不保存、不写日志、不续期（过期时提示打开该工具用一下）。
查询失败时保留上次的数字并标明时间。Antigravity 和 Grok 的接口是按公开的社区资料实现的，还没有在真机上验证过。

## 正在播放与桌面窗口

正在播放：`GlobalSystemMediaTransportControlsSessionManager`（MTA 线程每 1.5 秒轮询；换歌时才读封面）。
桌面窗口 `manager.html` 与岛共用 `app.js`（`body[data-mode=manager]`），在事件循环线程上创建：在命令线程里创建的
WebView 窗口只有收到鼠标输入后才重绘。两个窗口的 WebView2 启动参数必须一致（`additionalBrowserArgs`）。
