# MicaBar

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [Русский](README.ru.md) · [Español](README.es.md) · **Português (Brasil)** · [Français](README.fr.md) · [Deutsch](README.de.md)

Um monitor de rede para Windows no estilo «ilha dinâmica». Uma cápsula de vidro fosco no topo da tela mostra o seu IP
de saída e o número de conexões; deixe o mouse em cima para ver qual programa usa qual porta, para onde vai cada conexão
e por qual nó de proxy — e decidir ali mesmo se vai pelo proxy ou direto.

## Recursos

- **Saídas por grupo**: o IP de saída e o nó de cada grupo da sua assinatura (proxy padrão, IA, streaming…) mais a saída
  direta, com país e provedor / ASN, e o tipo do IP (residencial / empresarial / hospedagem / móvel, com uma marca à parte
  quando consta como proxy) e o dono (operadora e província para IPs da China continental).
- **Cápsula rotativa**: saídas por grupo, a música tocando (capa + título), cota de IA, velocidade e latência do nó se
  revezam; o que gira e em que ritmo é configurável.
- **Cota de IA**: uso da assinatura (janelas de 5 horas / semanal…) e horário de redefinição de Claude Code, Codex,
  Antigravity e Grok, cada anel com a marca da ferramenta.
- **Tarefas de IA rodando**: enquanto o Claude Code trabalha, o início da cápsula mostra o projeto e o tempo; fica laranja
  quando ele aguarda a sua aprovação. Também no topo da ilha aberta; um clique traz o terminal para a frente. Interromper
  com Esc ou fechar o terminal o remove.
- **Diagnóstico de rede**: uma sondagem leve a cada 10 s; duas falhas seguidas disparam uma verificação completa da cadeia
  (adaptador → roteador → DNS → internet → app de proxy → nó). A cápsula diz qual elo caiu («DNS com defeito», «Nó caiu»,
  «Banda larga caiu»…) e o painel oferece uma correção: limpar o cache DNS, trocar para um nó que funcione, reiniciar o
  app de proxy… Na volta mostra quanto tempo ficou fora. Se outro programa desligar ou mudar o proxy do sistema, avisa
  com restauração em um clique.
- **Primeiro acesso à rede**: um cartão na ilha quando um programa acessa a internet pela primeira vez — «Permitir» ou
  «Bloquear» (uma regra do Firewall do Windows; com o controle de saída ativo, o tráfego dele pelo proxy do sistema também
  é rejeitado). Na primeira vez aprende seus programas de sempre por 10 minutos; pastas de versão são agrupadas, então
  atualizações não contam como programas novos.
- **Notificações**: mensagens novas do WeChat, Telegram, e-mail e outros apps, e o Claude Code / Codex terminando uma
  tarefa ou aguardando aprovação, aparecem na ilha — um ícone pequeno no fim da cápsula, uma linha dentro da cápsula ou um
  cartão, à sua escolha; a página Notificações agrupa por app. O texto da mensagem fica oculto por padrão (só app +
  remetente); não perturbe, horário silencioso, silêncio por app e controle dos pop-ups próprios dos apps.
- **Conexões por programa**: ícone, número de processos, velocidade de upload / download ao vivo, os que vão pelo proxy
  primeiro; para cada conexão, porta local → domínio / IP, país, provedor, nó e a regra que correspondeu.
- **Controle de saída** (Clash Party / Mihomo Party): por programa, domínio, domínio principal ou «só este programa para
  este domínio» — proxy / direto / bloquear, aplicado na hora sem reiniciar o núcleo; troque e teste o nó usado para
  «Proxy» pela ilha.
- **Alertas de vazamento**: quando um site que deveria ir pelo proxy vai direto, a cápsula fica vermelha e aparece uma
  notificação do Windows. Prevenção: falhas de regra são forçadas pelo proxy; programas que evitam o proxy podem ser
  bloqueados de ir direto com um clique (Firewall do Windows).
- **Velocidade / tráfego / nós**: gráfico de velocidade ao vivo, tráfego proxy x direto por programa (hoje, 7 ou 30 dias),
  latência e perda dos nós.
- **Monitor do sistema**: CPU / GPU / memória por programa, encerrar processos; ligue e desligue apps de inicialização
  como no Gerenciador de Tarefas.
- **Janela da área de trabalho**: normalmente só a ilha e um ícone na bandeja; a janela da área de trabalho tem uma visão
  geral (saídas, velocidade, tráfego, latência, alertas, cota de IA, programas mais ativos) e todas as configurações.
- **Visual e uso**: vidro fosco ao vivo (claro / escuro, segue o sistema) que mantém o texto legível sobre qualquer fundo,
  paletas prontas e temas personalizados; formatos cápsula / entalhe / barra indicadora encaixados em qualquer borda da
  tela; abrir ao passar o mouse ou com clique, modo atravessar, atalho personalizado, navegação por teclado; não atrapalha
  jogos em tela cheia; animações fluidas que seguem a configuração de animação do Windows.
- **10 idiomas de interface**: 简体中文, 繁體中文, English, 日本語, 한국어, Русский, Español, Português (Brasil),
  Français, Deutsch — segue o idioma de exibição do Windows por padrão.

> O visual e a interação se inspiram no [fly-island](https://github.com/flyl1u/fly-island), algumas ideias de recursos no
> [BrimDeck](https://github.com/MoFeng2223/BrimDeck); todo o código foi escrito de forma independente.

## Instalação

Baixe em [Releases](https://github.com/aileboeth/MicaBar/releases):

- `MicaBar_<versão>_x64-setup.exe` — instalador (`C:\Program Files\MicaBar`), com atualizações pelo app;
- `MicaBar_<versão>_portable.exe` — um único exe portátil, sem atualização automática.

Requer Windows 10 2004+ / Windows 11 (x64) e o runtime WebView2 (já vem no Windows 11; o instalador adiciona se faltar).
O MicaBar roda como administrador (a velocidade por programa precisa de eventos ETW do kernel), então pede o UAC ao
iniciar; com «Iniciar com o Windows» ativo ele inicia por uma tarefa agendada, sem aviso.

## Como usar

Na primeira inicialização, um tutorial de meio minuto na ilha mostra o básico descrito abaixo; reveja quando quiser em
Configurações → Sobre → «Rever tutorial».

### Expandir e recolher

| Ação | Resultado |
| --- | --- |
| Deixar o mouse sobre a cápsula por cerca de 0,5 s | Expande no painel (modo padrão «Ao passar o mouse»; o tempo fica em Configurações → Geral → Interação) |
| Tirar o mouse do painel | Recolhe |
| Clicar na cápsula / clicar fora | Expandir / recolher no modo «Ao clicar» (troque em Configurações → Geral → Interação → «Como expandir») |
| `Ctrl+Alt+N` | Expandir / recolher a qualquer momento (para mudar, clique no campo do atalho em Configurações e pressione a nova combinação) |
| `Esc` | Recolhe o painel; antes fecha um painel inferior ou menu aberto |

- No modo «Ao passar o mouse» a cápsula recolhida nunca bloqueia cliques: enquanto o mouse está em cima ela fica
  transparente para você ver a aba que ela cobre, e o clique vai para a janela de baixo.
- **Modo atravessar**: o mouse não toca a ilha de jeito nenhum e só o atalho a abre — bom quando você não quer acertá-la
  sem querer.
- Enquanto um app em tela cheia (jogo, vídeo) está na frente, a cápsula continua visível, mas nunca expande sozinha nem
  bloqueia cliques; também dá para escondê-la totalmente.
- Opcional «Virar barra indicadora com uma janela maximizada»: enquanto uma janela maximizada está na frente, a cápsula
  vira uma linha fina e volta quando você troca de janela.

### Arrastar, jogar e encaixar

A ilha se encaixa na borda de cima, de baixo, da esquerda ou da direita da tela, no início, no meio ou no fim dessa borda.

1. **Pegar**: com o painel expandido, **pressione e segure parado por cerca de 0,26 s** em qualquer ponto do painel (o
   tempo fica em Configurações → Geral → Interação → «Pressão longa para pegar a ilha»). Recolhida, **segure `Alt` e
   pressione longamente a cápsula** (com o Alt pressionado a cápsula não fica transparente, então você pega a cápsula e
   não a janela de baixo).
2. **Jogar ao longo da borda**: depois de pegar, dê um impulso para a esquerda ou a direita e solte — a ilha desliza para
   o início, o meio ou o fim dessa borda.
3. **Mudar de borda**: depois de pegar, puxe para o meio da tela e ela segue o mouse; solte perto de outra borda e ela se
   encaixa lá. Nas bordas esquerda / direita a cápsula vira um entalhe vertical.

Se preferir não arrastar, escolha a borda e a posição nela em Configurações → Aparência recolhida.

### Ajustar o tamanho

- Clique direito no ícone da bandeja → «Ajustar o tamanho da ilha…», o botão «Ajustar o tamanho da ilha» no canto inferior
  esquerdo da janela da área de trabalho, ou Configurações → Aparência recolhida / Aparência expandida → «Arraste na tela
  para ajustar»: a cápsula e o painel na tela ganham bordas arrastáveis; arraste-as para mudar largura e altura e clique em
  «Concluído» (`Esc` cancela).
- Ou use os controles deslizantes de Configurações → Aparência recolhida / Aparência expandida: altura da cápsula, largura
  (0 = cresce com o conteúdo), largura do painel, altura máxima e escala da interface.

### Teclado

Com o painel expandido: `Tab` / `Shift+Tab` movem o foco em ordem; com o foco nos ícones de página no topo ou nas abas
de Rede, `←` / `→` trocam; `↑` / `↓` andam entre linhas e botões da lista; `Enter` / `Espaço` expandem o programa atual;
`Esc` recolhe. Listas que dá para reordenar em Configurações também aceitam `Alt+↑` / `Alt+↓` no item selecionado.

### As páginas

Os ícones no topo do painel trocam de página (Rede, Sistema, Música, Cota de IA, Notificações); quais páginas aparecem e
em que ordem fica em Configurações → Aparência expandida, e a ilha lembra em que página você estava.

**Rede**

- No topo ficam os **cartões de saída**. À esquerda as saídas do proxy: uma linha por IP de saída com a etiqueta de tipo
  (Residencial / Empresarial / Hospedagem / Móvel / Marcado proxy), na segunda linha o nó e o dono, e à direita os grupos
  que usam esse IP; o que não cabe vira «+N» — clique para ver todos e em «Recolher» para dobrar de novo. À direita a saída
  direta (operadora, adaptador, DNS reverso, última verificação). Deixe o mouse sobre um IP ou etiqueta para ver o dono
  registrado, o provedor, o ASN e a fonte dos dados. ↻ no canto superior direito verifica as saídas de novo; o ícone de
  pulso ao lado abre o **diagnóstico de rede**.
- A linha abaixo dos cartões mostra o app de proxy, o proxy do sistema, o TUN, a porta local e o controlador.
- Abas: **Conex. / Escuta / Saída / Alertas / Velocidade / Tráfego / Nós** (role com a roda do mouse quando não couberem;
  reordene em Configurações). A busca à direita filtra por programa, domínio, IP ou porta.
  - **Conexões**: uma linha por programa conectado (os do proxy primeiro). Clique numa linha para abrir as conexões; o
    botão «Saída» à direita escolhe Proxy / Direto / Bloquear para o programa inteiro, e o «⋯» ao lado de uma conexão só
    para ela (este domínio, este IP, ou «só este programa para ele»). Clicar de novo no botão selecionado = voltar às regras.
  - **Escuta**: portas em escuta neste PC; as que escutam em todos os adaptadores (acessíveis pela LAN) são marcadas.
  - **Saída**: ligar / desligar o controle de saída, ver e excluir regras de saída, trocar e testar o nó usado para «Proxy».
  - **Alertas**: vazamentos de proxy e primeiro acesso à rede; bloquear acesso direto, ignorar um domínio, permitir de novo.
  - **Velocidade**: o gráfico de velocidade ao vivo. Mova o mouse para a esquerda / direita e ele se ajusta a cada segundo
    enquanto os números grandes acompanham; clique num programa abaixo do gráfico para subir um painel com o gráfico e os
    números dele. Arraste o painel para cima / baixo e jogue para baixo para fechar; ele tem «Ver as conexões» e «Ver só
    este no gráfico grande».
  - **Tráfego**: tráfego proxy / direto por programa para hoje / 7 dias / 30 dias; clique numa barra para ver aquele dia.
  - **Nós**: latência e perda de cada nó de saída; «Testar agora» roda uma rodada na hora.
- **Diagnóstico de rede** (o ícone de pulso, ou o alerta vermelho na cápsula quando você está sem conexão): verifica do
  adaptador ao nó, diz qual elo caiu e oferece botões de correção abaixo (limpar o cache DNS, restaurar o proxy do
  sistema, abrir / reiniciar o app de proxy, trocar para um nó que funcione…).

**Sistema**: uso de cada núcleo de CPU / GPU, memória e VRAM; abaixo troque entre «Apps que mais usam» (por CPU ou
memória; ✕ encerra um processo depois de perguntar — processos essenciais do Windows e o próprio MicaBar nunca são
encerrados) e «Apps de inicialização» (o mesmo botão de «Aplicativos de inicialização» do Gerenciador de Tarefas;
desligar um não apaga nada).

**Música**: a música tocando (NetEase Cloud Music, QQ Music, Spotify, vídeos no navegador…) com capa, título, barras no
ritmo da música e anterior / tocar-pausar / próxima.

**Cota de IA**: um cartão por ferramenta com quanto de cada janela de cota foi usado e quando redefine; «Atualizar agora»
consulta na hora (as APIs oficiais limitam bastante; Claude no máximo a cada 10 minutos).

**Notificações**: agrupadas por app; clique numa para abrir o app, «Marcar tudo como lido», «Limpar tudo»; **clique
direito** num grupo para silenciar o app ou limpar o grupo.

### O que a cápsula mostra

- O que a cápsula mostra (IP de saída, velocidade, anéis de cota de IA, relógio, CPU / memória…) é marcado e reordenado em
  Configurações → Aparência recolhida → «Conteúdo»; o que não cabe se junta no último espaço e se reveza.
- Quando você está sem conexão, o proxy do sistema foi alterado, ou o Claude Code está trabalhando ou aguardando sua
  aprovação, o alerta correspondente aparece no início da cápsula; clique para abrir o diagnóstico ou ir para aquele terminal.
- Mudanças de IP de saída, um nó sem resposta ou lento e picos de tráfego aparecem rapidamente por 5 segundos na cápsula
  (botões e limites em Configurações → Rede → «Alertas rápidos»).
- A cápsula fica vermelha quando há vazamentos; notificações não lidas mostram o ícone do app e um ponto vermelho à direita.

### Janela da área de trabalho, configurações e bandeja

- **Clique esquerdo no ícone da bandeja** ou na **engrenagem** no canto superior direito do painel para abrir a janela:
  Visão geral, Conexões, Portas em escuta, Controle de saída, Alertas de vazamento, Velocidade, Tráfego, Nós, Sistema,
  Cota de IA, Notificações e Configurações.
- **Clique direito no ícone da bandeja**: Abrir o MicaBar, Configurações, Ajustar o tamanho da ilha, Verificar IP de saída
  de novo, Aparência (seguir o sistema / fosco claro / fosco escuro), Abrir log, Sair.
- As configurações ficam agrupadas na barra lateral (Básico: Geral, Aparência recolhida, Aparência expandida, Tema do
  painel; Saídas e alertas; Cota e anéis; Tocando agora; Notificações; Sobre) e qualquer recurso pode ser desligado;
  formatos e temas são escolhidos em cartões de pré-visualização, e listas com ⠿ são reordenadas arrastando.
  «Redefinir todas as configurações» fica no fim.
- **Idioma da interface**: Configurações → Geral → «Idioma»; o padrão «Seguir o sistema» usa o idioma de exibição do
  Windows. Depois da troca todas as janelas recarregam e o menu da bandeja também muda.

## Saídas por grupo

Com o controle de saída ativo, a substituição abre para cada grupo de proxy uma porta que só escuta neste PC (a partir
de `127.0.0.1:47800`), cujo tráfego vai direto para aquele grupo. O MicaBar verifica o IP de saída por cada porta, então
sabe por onde saem «padrão», «IA» ou «YouTube». Por padrão mostra o grupo de reserva mais os grupos com tráfego recente
(até 4); os grupos também podem ser escolhidos à mão.

## Controle de saída

Quando o Clash Party / Mihomo Party é detectado, a ilha oferece ativar o controle de saída. Ativar faz backup da lista de
substituições do Clash Party, adiciona uma substituição global «MicaBar 出口控制» e reinicia o Clash Party uma vez
(cerca de 3 s sem conexão); se o núcleo não conseguir carregá-la, tudo é restaurado. Depois escolha um destino em «Saída»
na linha de um programa ou em «⋯» numa conexão. Regras mais específicas vencem, então «este programa direto» e «só este
programa para *.example.com pelo proxy» funcionam juntas. A substituição é removida no fim da aba «Saída».

No modo proxy do sistema só dá para controlar os programas que respeitam o proxy do sistema; ative o TUN para cobrir todos.

## Clientes de proxy compatíveis

| Cliente | Saída do proxy / porta local | Nós, grupos, regras correspondentes | Controle de saída / prevenção de vazamentos |
|---|---|---|---|
| Clash Party / Mihomo Party | ✓ | ✓ | ✓ |
| Clash Verge Rev | ✓ | ✓ (pelo controlador dele) | — |
| v2rayN, Throne (NekoRay), Hiddify, FlClash, Karing | ✓ (porta do proxy do sistema ou a porta local de costume) | — | — |

O proxy do sistema, o adaptador TUN e as conexões por programa aparecem com qualquer cliente.

## Notificações

As notificações vêm de três lugares, todos lidos localmente:

- **Piscar da barra de tarefas**: o WeChat 4.x avisa de uma mensagem nova só piscando o botão na barra de tarefas (sem
  notificação do Windows nem janela legível), então a ilha só pode dizer «WeChat · N mensagens novas»; o clique abre o
  WeChat. Outros apps que piscam funcionam do mesmo jeito.
- **Pop-ups dos próprios apps / notificações do Windows**: com as notificações do Windows desligadas, o Telegram mostra as
  próprias janelinhas no canto; quando uma aparece, o MicaBar lê uma vez a lista de conversas do Telegram (Windows UI
  Automation) e escolhe as conversas cujo número de mensagens novas subiu. Apps que usam notificações do Windows são lidos
  do banco de dados de notificações do sistema. «Assumir os pop-ups» esconde o pop-up do app (ou do Windows) para que só
  a ilha o mostre.
- Apps com notificações do Windows (WhatsApp, Teams, Slack, Discord, Signal, Zoom, LINE…) aparecem com o próprio nome e
  ícone; uma mensagem que pisca na barra de tarefas e também gera notificação conta uma vez só.
- Abrir o app marca as notificações dele como lidas e tira o ponto vermelho; uma que chega enquanto você usa o app não é anunciada.
- **Avisos de IA ao terminar**: «Conectar» em Configurações → Notificações adiciona um hook ao `~/.claude/settings.json` do
  Claude Code ou ao `~/.codex/config.toml` do Codex (com backup antes, removível com um clique); quando uma tarefa termina ou
  precisa de aprovação, ele avisa `127.0.0.1:47900` com `curl` (com um token aleatório).

## Alertas de vazamento

A biblioteca «deveria ir pelo proxy» = uma lista embutida de serviços comuns (~140 domínios principais) + a lista gfw
(atualizada diariamente) + suas próprias entradas / listas importadas (só domínios, formatos de regra v2fly e Clash).
Dois tipos de vazamento:

- **Falha de regra**: a conexão passou pelo app de proxy, mas uma regra a mandou direto. Com «Agir antes» ligado, esses
  domínios vão para o proxy antes de chegar às regras diretas da assinatura; os grupos por serviço da assinatura não são
  mexidos.
- **Fora do proxy**: o programa se conectou direto sem passar pelo app de proxy (ignora o proxy do sistema, ou a conexão é
  anterior ao TUN). O cartão de alerta pode bloquear o acesso direto do programa (uma regra do Firewall do Windows só para
  endereços públicos; o programa continua alcançando a porta local do proxy) e desfazer depois.

## Privacidade: com o que o MicaBar se conecta

Todas as estatísticas (conexões, tráfego, regras, alertas) ficam neste PC em `%LOCALAPPDATA%\MicaBar\`. As únicas
requisições que o MicaBar faz por conta própria:

| Finalidade | Endereço | Quando |
| --- | --- | --- |
| IP de saída do proxy | ipinfo.io → api.ipify.org → ifconfig.me (pela porta local do proxy; saídas por grupo pela porta loopback de cada grupo) | ao iniciar, ao mudar a rede / o proxy, a cada 5 min |
| IP de saída direta | 4.ipw.cn → myip.ipip.net → ip.3322.net (ligado ao adaptador físico; resolvido por 223.5.5.5 / 119.29.29.29) | idem |
| Tipo e dono do IP | proxycheck.io (HTTPS) e ip-api.com (só HTTP no plano gratuito), com cada IP de saída (o direto também) como consulta; pela porta local do cliente de proxy | quando aparece um IP de saída novo; cache de 24 h por IP; pode ser desligado em Configurações → Rede |
| Latência dos nós | medida pelo núcleo mihomo contra `www.gstatic.com/generate_204` | a cada 15–60 s (perfil de recursos) |
| Lista gfw | cdn.jsdelivr.net / raw.githubusercontent.com (Loyalsoldier/v2ray-rules-dat) | diariamente, se ativada |
| Procurar atualizações | latest.json de github.com/aileboeth/MicaBar | diariamente, se ativado |
| Cota de IA | api.anthropic.com (Claude Code), chatgpt.com (Codex), cli-chat-proxy.grok.com (Grok), o servidor de linguagem local do Antigravity | a cada 10 min (configurável), se ativada; sempre pela porta local do cliente de proxy, nunca direto (sem porta, sem consulta) |
| Diagnóstico de rede | direto: as APIs HTTP de resolução de 119.29.29.29 / 223.5.5.5 (consultando qq.com); pelo proxy: `www.gstatic.com/generate_204`; DNS: uma consulta nova de www.baidu.com pelo DNS do sistema; a verificação completa também faz ping no roteador | se ativado: uma requisição pequena a cada 10 s (direto), a cada 30 s pelo proxy e ao DNS; a verificação completa só quando falham |

País / ASN vêm dos bancos de dados offline que acompanham o cliente de proxy (`country.mmdb`, `ASN.mmdb`); os IPs das
conexões nunca são enviados a lugar nenhum. A cota de IA usa o login que cada ferramenta guarda neste PC e o envia só ao
serviço daquela ferramenta; ele nunca é salvo, registrado ou renovado. «Tocando agora» lê os controles de mídia do Windows
sem acessar a rede. As notificações também não acessam a rede: as do dia ficam só na memória, o texto das mensagens nunca
é gravado em disco nem no log (e por padrão não aparece na ilha); a porta loopback `127.0.0.1:47900` só aceita as chamadas
dos hooks de IA com o token. O segredo do controlador do mihomo é lido da configuração dele em tempo de execução e nunca é
salvo nem enviado.

## Compilar a partir do código

Precisa de Rust (rustup), Visual Studio C++ Build Tools, Node.js + pnpm.

```powershell
pnpm install
pnpm build            # um exe: src-tauri\target\release\micabar.exe
pnpm release          # instalador + portátil + manifesto de atualização -> dist\release-<versão>\ (precisa de keys\micabar.key)
```

Estrutura: `src-tauri\src\` (backend em Rust: tabelas de sockets, ETW, controlador do mihomo, verificação de saídas,
proteção contra vazamentos, comportamento da janela, captura para o vidro fosco…), `ui\` (HTML / CSS / JS puro, sem
empacotador; `ui\i18n\` guarda as traduções, com o texto chinês do código como chave), `tools\` (script de publicação,
driver da máquina virtual de teste).
Notas de implementação (em chinês): [docs/internals.zh-CN.md](docs/internals.zh-CN.md).

## Licença

[MIT](LICENSE). Ícones de bandeiras do [flag-icons](https://github.com/lipis/flag-icons) (MIT, veja `ui/flags/LICENSE`). Marcas das ferramentas de IA do [lobe-icons](https://github.com/lobehub/lobe-icons) (MIT, veja `ui/brands/LICENSE`); as marcas pertencem aos respectivos donos.

## Aviso legal

O MicaBar é software livre e de código aberto feito pelo autor no tempo livre, fornecido «no estado em que se encontra»,
sem garantia de qualquer tipo. Ele lê conexões de rede, processos, notificações do sistema e o estado de login das
ferramentas de IA neste PC; esses dados são usados só neste PC. O controle de saída e a proteção contra vazamentos alteram
a substituição do cliente de proxy, adicionam regras de firewall e podem reiniciar o cliente de proxy. O autor não se
responsabiliza por nenhuma perda decorrente do uso ou da impossibilidade de uso do software (perda de dados, quedas de
rede, contas restritas…). Use respeitando as leis locais e os termos de cada serviço. O MicaBar não tem vínculo com Clash,
mihomo, Anthropic, OpenAI, Google, xAI nem com qualquer outro projeto ou empresa citados; seus nomes e marcas pertencem
aos respectivos donos.
