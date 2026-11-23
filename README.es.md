# MicaBar

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [Русский](README.ru.md) · **Español** · [Português (Brasil)](README.pt-BR.md) · [Français](README.fr.md) · [Deutsch](README.de.md)

Un monitor de red para Windows al estilo «isla dinámica». Una cápsula de cristal esmerilado arriba de la pantalla
muestra tu IP de salida y el número de conexiones; deja el ratón encima para ver qué programa usa qué puerto, adónde va
cada conexión y por qué nodo de proxy, y decide ahí mismo si va por el proxy o en directo.

## Funciones

- **Salidas por grupo**: la IP de salida y el nodo de cada grupo de tu suscripción (proxy predeterminado, IA,
  streaming…) más la salida directa, con país y proveedor / ASN, y el tipo de IP (doméstica / empresa / hosting /
  móvil, con una marca aparte si figura como proxy) y su propietario (operador y provincia para IP de China continental).
- **Cápsula rotativa**: salidas por grupo, la canción que suena (carátula + título), cuota de IA, velocidad y latencia
  del nodo se turnan; qué rota y a qué ritmo es configurable.
- **Cuota de IA**: uso de la suscripción (ventanas de 5 horas / semanal…) y hora de restablecimiento de Claude Code,
  Codex, Antigravity y Grok, cada anillo con la marca de su herramienta.
- **Tareas de IA en curso**: mientras Claude Code trabaja, el inicio de la cápsula muestra el proyecto y el tiempo; se
  vuelve naranja cuando espera tu aprobación. También arriba de la isla abierta; un clic trae su terminal al frente.
  Interrumpir con Esc o cerrar el terminal lo quita.
- **Diagnóstico de red**: un sondeo ligero cada 10 s; dos fallos seguidos lanzan una comprobación completa de la cadena
  (adaptador → router → DNS → internet → app de proxy → nodo). La cápsula dice qué eslabón falló («DNS averiado»,
  «Nodo caído», «Línea caída»…) y el panel ofrece una solución: vaciar la caché DNS, cambiar a un nodo que funcione,
  reiniciar la app de proxy… Al volver la conexión indica cuánto duró la caída. Si otro programa desactiva o cambia el
  proxy del sistema, avisa con restauración en un clic.
- **Primer acceso a la red**: una tarjeta en la isla cuando un programa se conecta a internet por primera vez —
  «Permitir» o «Bloquear» (una regla del Firewall de Windows; con el control de salida activo también se rechaza su
  tráfico por el proxy del sistema). La primera vez aprende tus programas habituales durante 10 minutos; las carpetas
  de versión se agrupan, así que las actualizaciones no cuentan como programas nuevos.
- **Notificaciones**: mensajes nuevos de WeChat, Telegram, correo y otras apps, y Claude Code / Codex al terminar una
  tarea o esperar aprobación, aparecen en la isla — un icono pequeño al final de la cápsula, una línea dentro de la
  cápsula o una tarjeta, a tu elección; la página Notificaciones los agrupa por app. El texto del mensaje se oculta por
  defecto (solo app + remitente); no molestar, horas de silencio, silencio por app y control de las ventanas propias
  de las apps.
- **Conexiones por programa**: icono, número de procesos, velocidad de subida / bajada en directo, primero los que van
  por proxy; para cada conexión, puerto local → dominio / IP, país, proveedor, nodo y la regla que coincidió.
- **Control de salida** (Clash Party / Mihomo Party): por programa, dominio, dominio principal o «solo este programa
  hacia este dominio» — proxy / directo / bloquear, aplicado al instante sin reiniciar el núcleo; cambia y prueba el
  nodo usado para «Proxy» desde la isla.
- **Alertas de fuga**: cuando un sitio que debería ir por el proxy va en directo, la cápsula se pone roja y aparece una
  notificación de Windows. Prevención: los fallos de regla se fuerzan por el proxy; a los programas que evitan el proxy
  se les puede bloquear el acceso directo con un clic (Firewall de Windows).
- **Velocidad / tráfico / nodos**: gráfico de velocidad en directo, tráfico proxy frente a directo por programa (hoy,
  7 o 30 días), latencia y pérdida de los nodos.
- **Monitor del sistema**: CPU / GPU / memoria por programa, finalizar procesos; activa y desactiva las apps de inicio
  como en el Administrador de tareas.
- **Ventana de escritorio**: normalmente solo la isla y un icono en la bandeja; la ventana de escritorio tiene un
  resumen (salidas, velocidad, tráfico, latencia, alertas, cuota de IA, programas más activos) y todos los ajustes.
- **Aspecto y sensación**: cristal esmerilado en vivo (claro / oscuro, según el sistema) que mantiene el texto legible
  sobre cualquier fondo, colores predefinidos y temas personalizados; formas de cápsula / muesca / barra indicadora
  ancladas a cualquier borde de la pantalla; abrir al pasar el ratón o con clic, modo atravesar, atajo personalizado,
  navegación con teclado; no estorba en los juegos a pantalla completa; animaciones fluidas que siguen el ajuste de
  animaciones de Windows.
- **10 idiomas de interfaz**: 简体中文, 繁體中文, English, 日本語, 한국어, Русский, Español, Português (Brasil),
  Français, Deutsch — sigue el idioma de pantalla de Windows por defecto.
- **Historial del portapapeles**: el texto y las imágenes que copias se guardan en la página «Portapapeles» de la isla; un clic las copia de nuevo. Búsqueda, fijar, limpieza automática por antigüedad y cantidad; no se guarda lo que un gestor de contraseñas marca como privado.

> El aspecto y la interacción se inspiran en [fly-island](https://github.com/flyl1u/fly-island), algunas ideas de
> funciones en [BrimDeck](https://github.com/MoFeng2223/BrimDeck); todo el código está escrito de forma independiente.

## Instalación

Descarga desde [Releases](https://github.com/aileboeth/MicaBar/releases):

- `MicaBar_<versión>_x64-setup.exe` — instalador (`C:\Program Files\MicaBar`), con actualizaciones desde la app;
- `MicaBar_<versión>_portable.exe` — un único exe portátil, sin actualización automática.

Requiere Windows 10 2004+ / Windows 11 (x64) y el runtime WebView2 (incluido en Windows 11; el instalador lo añade si
falta). MicaBar se ejecuta como administrador (la velocidad por programa necesita eventos ETW del núcleo), así que pide
UAC al iniciar; con «Iniciar con Windows» activado arranca desde una tarea programada sin aviso.

## Cómo se usa

En el primer inicio, un tutorial de medio minuto en la isla te guía por lo básico que se explica abajo; repítelo cuando
quieras en Ajustes → Acerca de → «Repetir tutorial».

### Expandir y contraer

| Acción | Resultado |
| --- | --- |
| Dejar el ratón sobre la cápsula unos 0,5 s | Se expande en el panel (modo predeterminado «Al pasar el ratón»; la espera está en Ajustes → General → Interacción) |
| Sacar el ratón del panel | Se contrae |
| Clic en la cápsula / clic fuera | Expandir / contraer en el modo «Al hacer clic» (se cambia en Ajustes → General → Interacción → «Cómo expandir») |
| `Ctrl+Alt+N` | Expandir / contraer en cualquier momento (para cambiarlo, haz clic en el campo del atajo en Ajustes y pulsa la nueva combinación) |
| `Esc` | Contrae el panel; antes cierra un panel inferior o un menú abierto |

- En el modo «Al pasar el ratón» la cápsula contraída nunca bloquea los clics: mientras el ratón está encima se vuelve
  transparente para que veas la pestaña que tapa, y el clic va a la ventana de debajo.
- **Modo atravesar**: el ratón no toca la isla en absoluto y solo el atajo la abre; útil si nunca quieres darle sin querer.
- Mientras una app a pantalla completa (juego, vídeo) está delante, la cápsula sigue visible pero nunca se expande sola
  ni bloquea clics; también se puede ocultar por completo.
- Opcional «Reducir a la barra indicadora con una ventana maximizada»: mientras una ventana maximizada está delante,
  la cápsula se vuelve una línea fina y vuelve al cambiar de ventana.

### Arrastrar, lanzar y anclar

La isla se ancla al borde superior, inferior, izquierdo o derecho de la pantalla, al inicio, en medio o al final de ese borde.

1. **Agarrarla**: con el panel expandido, **mantén pulsado sin moverte unos 0,26 s** en cualquier punto del panel (el
   tiempo está en Ajustes → General → Interacción → «Pulsación larga para agarrar la isla»). Contraída, **mantén `Alt`
   y pulsa largo la cápsula** (con Alt pulsado la cápsula deja de ser transparente, así que agarras la cápsula y no la
   ventana de debajo).
2. **Lanzar a lo largo del borde**: una vez agarrada, dale un golpe a la izquierda o a la derecha y suelta: la isla se
   desliza al inicio, al medio o al final de ese borde.
3. **Llevarla a otro borde**: una vez agarrada, tira hacia el centro de la pantalla y seguirá al ratón; suéltala cerca de
   otro borde y se anclará ahí. En los bordes izquierdo / derecho la cápsula se vuelve una muesca vertical.

Si prefieres no arrastrar, elige el borde y la posición en él en Ajustes → Aspecto contraído.

### Cambiar el tamaño

- Clic derecho en el icono de la bandeja → «Cambiar tamaño de la isla…», el botón «Cambiar tamaño de la isla» abajo a
  la izquierda de la ventana de escritorio, o Ajustes → Aspecto contraído / Aspecto expandido → «Arrastra en la pantalla
  para ajustar»: la cápsula y el panel en pantalla muestran bordes arrastrables; arrástralos para cambiar ancho y alto y
  pulsa «Listo» (`Esc` cancela).
- O usa los deslizadores de Ajustes → Aspecto contraído / Aspecto expandido: alto de la cápsula, ancho (0 = crece con el
  contenido), ancho del panel, alto máximo y escala de la interfaz.

### Teclado

Con el panel expandido: `Tab` / `Shift+Tab` mueven el foco en orden; con el foco en los iconos de página de arriba o en
las pestañas de Red, `←` / `→` cambian; `↑` / `↓` se mueven entre filas y botones de la lista; `Enter` / `Espacio`
expanden el programa actual; `Esc` contrae. Las listas que se pueden reordenar en Ajustes también aceptan `Alt+↑` /
`Alt+↓` sobre el elemento seleccionado.

### Las páginas

Los iconos de la parte superior del panel cambian de página (Red, Sistema, Música, Cuota de IA, Notificaciones); qué
páginas se ven y en qué orden está en Ajustes → Aspecto expandido, y la isla recuerda en qué página estabas.

**Red**

- Arriba están las **tarjetas de salida**. A la izquierda las salidas por proxy: una fila por IP de salida con su
  etiqueta de tipo (Doméstica / Empresa / Hosting / Móvil / Marcada proxy), en la segunda línea el nodo y el
  propietario, y a la derecha los grupos que usan esa IP; lo que no cabe se pliega en «+N»: haz clic para verlos todos y
  en «Contraer» para plegarlos. A la derecha la salida directa (operador, adaptador, DNS inverso, última comprobación).
  Deja el ratón sobre una IP o etiqueta para ver el propietario registrado, el proveedor, el ASN y la fuente de datos.
  ↻ arriba a la derecha vuelve a comprobar las salidas; el icono de pulso al lado abre el **diagnóstico de red**.
- La línea bajo las tarjetas muestra la app de proxy, el proxy del sistema, TUN, el puerto local y el controlador.
- Pestañas: **Conex. / Escucha / Salida / Alertas / Velocidad / Tráfico / Nodos** (desplázalas con la rueda del ratón si
  no caben; reordénalas en Ajustes). El buscador de la derecha filtra por programa, dominio, IP o puerto.
  - **Conexiones**: una fila por programa conectado (primero los que van por proxy). Haz clic en una fila para ver sus
    conexiones; el botón «Salida» de la derecha elige Proxy / Directo / Bloquear para todo el programa, y el «⋯» junto a
    una conexión solo para esa conexión (este dominio, esta IP, o «solo este programa hacia él»). Volver a pulsar el botón
    seleccionado = volver a las reglas.
  - **Escucha**: puertos en escucha en este PC; los que escuchan en todos los adaptadores (accesibles desde la LAN) se marcan.
  - **Salida**: activar / desactivar el control de salida, ver y borrar reglas de salida, cambiar y probar el nodo de «Proxy».
  - **Alertas**: fugas de proxy y primer acceso a la red; bloquear el acceso directo, ignorar un dominio, volver a permitir.
  - **Velocidad**: el gráfico de velocidad en directo. Mueve el ratón a izquierda / derecha y se ajusta a cada segundo
    mientras los números grandes lo siguen; haz clic en un programa bajo el gráfico para subir un panel con su propio
    gráfico y cifras. Arrastra el panel arriba / abajo y lánzalo hacia abajo para cerrarlo; tiene «Ver sus conexiones» y
    «Ver solo esta en el gráfico grande».
  - **Tráfico**: tráfico proxy / directo por programa para hoy / 7 días / 30 días; haz clic en una barra para ver ese día.
  - **Nodos**: latencia y pérdida de cada nodo de salida; «Probar ahora» lanza una ronda al momento.
- **Diagnóstico de red** (el icono de pulso, o la alerta roja de la cápsula cuando no hay conexión): comprueba del
  adaptador al nodo, te dice qué eslabón falló y ofrece botones de reparación debajo (vaciar la caché DNS, restaurar el
  proxy del sistema, abrir / reiniciar la app de proxy, cambiar a un nodo que funcione…).

**Sistema**: uso de cada núcleo de CPU / GPU, memoria y VRAM; debajo cambia entre «Apps que más usan» (ordenadas por CPU
o memoria; ✕ finaliza un proceso tras confirmar — los procesos básicos de Windows y el propio MicaBar nunca se
finalizan) y «Apps de inicio» (el mismo interruptor que «Aplicaciones de inicio» del Administrador de tareas;
desactivar una no borra nada).

**Música**: la canción que suena (NetEase Cloud Music, QQ Music, Spotify, vídeos en el navegador…) con carátula, título,
barras al ritmo de la música y anterior / reproducir-pausar / siguiente.

**Cuota de IA**: una tarjeta por herramienta con cuánto se ha usado de cada ventana de cuota y cuándo se restablece;
«Actualizar ahora» consulta al momento (las API oficiales limitan mucho; Claude como mucho cada 10 minutos).

**Notificaciones**: agrupadas por app; haz clic en una para abrir su app, «Marcar todo como leído», «Borrar todo»;
**clic derecho** en un grupo para silenciar esa app o borrar el grupo.

### Lo que te dice la cápsula

- Lo que muestra la cápsula (IP de salida, velocidad, anillos de cuota de IA, reloj, CPU / memoria…) se marca y reordena
  en Ajustes → Aspecto contraído → «Contenido»; lo que no cabe se une en la última casilla y se turna.
- Si no hay conexión, cambió el proxy del sistema, o Claude Code está trabajando o esperando tu aprobación, el aviso
  correspondiente aparece al inicio de la cápsula; haz clic para abrir el diagnóstico o ir a ese terminal.
- Los cambios de IP de salida, un nodo que no responde o se vuelve lento y los picos de tráfico aparecen brevemente 5
  segundos en la cápsula (interruptores y umbrales en Ajustes → Red → «Avisos breves»).
- La cápsula se pone roja si hay fugas; las notificaciones sin leer muestran el icono de su app y un punto rojo a la derecha.

### Ventana de escritorio, ajustes y bandeja

- **Clic izquierdo en el icono de la bandeja** o en el **engranaje** de arriba a la derecha del panel para abrir la
  ventana de escritorio: Resumen, Conexiones, Puertos en escucha, Control de salida, Alertas de fuga, Velocidad, Tráfico,
  Nodos, Sistema, Cuota de IA, Notificaciones y Ajustes.
- **Clic derecho en el icono de la bandeja**: Abrir MicaBar, Ajustes, Cambiar tamaño de la isla, Volver a comprobar la IP
  de salida, Aspecto (según el sistema / esmerilado claro / esmerilado oscuro), Abrir registro, Salir.
- Los ajustes se agrupan en la barra lateral (Básico: General, Aspecto contraído, Aspecto expandido, Tema del panel;
  Salidas y avisos; Cuota y anillos; Reproduciendo; Notificaciones; Acerca de) y cualquier función se puede desactivar;
  las formas y los temas se eligen en tarjetas de vista previa, y las listas con ⠿ se reordenan arrastrando.
  «Restablecer todos los ajustes» está abajo del todo.
- **Idioma de la interfaz**: Ajustes → General → «Idioma»; el predeterminado «Según el sistema» usa el idioma de pantalla
  de Windows. Al cambiarlo se recargan todas las ventanas y el menú de la bandeja también cambia.

## Salidas por grupo

Con el control de salida activado, la anulación abre por cada grupo de proxy un puerto que solo escucha en este PC (desde
`127.0.0.1:47800`) cuyo tráfico va directo a ese grupo. MicaBar comprueba la IP de salida por cada puerto, así sabe por
dónde salen «predeterminado», «IA» o «YouTube». Por defecto muestra el grupo de respaldo más los grupos con tráfico
reciente (hasta 4); también se pueden elegir a mano.

## Control de salida

Cuando se detecta Clash Party / Mihomo Party, la isla ofrece activar el control de salida. Al activarlo se hace copia de
la lista de anulaciones de Clash Party, se añade una anulación global «MicaBar 出口控制» y se reinicia Clash Party una
vez (unos 3 s sin conexión); si el núcleo no la carga, todo se restaura. Después elige un destino en «Salida» en la fila
de un programa o en «⋯» en una conexión. Las reglas más específicas ganan, así que «este programa en directo» y «solo
este programa hacia *.example.com por proxy» funcionan juntas. La anulación se quita al final de la pestaña «Salida».

En modo proxy del sistema solo se pueden controlar los programas que respetan el proxy del sistema; activa TUN para
cubrirlos todos.

## Clientes de proxy compatibles

| Cliente | Salida por proxy / puerto local | Nodos, grupos, reglas coincidentes | Control de salida / prevención de fugas |
|---|---|---|---|
| Clash Party / Mihomo Party | ✓ | ✓ | ✓ |
| Clash Verge Rev | ✓ | ✓ (a través de su controlador) | — |
| v2rayN, Throne (NekoRay), Hiddify, FlClash, Karing | ✓ (puerto del proxy del sistema o su puerto local habitual) | — | — |

El proxy del sistema, el adaptador TUN y las conexiones por programa se muestran con cualquier cliente.

## Notificaciones

Las notificaciones vienen de tres sitios, todos leídos en local:

- **Parpadeo de la barra de tareas**: WeChat 4.x avisa de un mensaje nuevo solo haciendo parpadear su botón de la barra
  de tareas (sin notificación de Windows ni ventana legible), así que la isla solo puede decir «WeChat · N mensajes
  nuevos»; el clic abre WeChat. Otras apps que parpadean funcionan igual.
- **Ventanas propias de las apps / notificaciones de Windows**: con las notificaciones de Windows desactivadas, Telegram
  muestra sus propias ventanitas en la esquina; cuando aparece una, MicaBar lee una vez la lista de chats de Telegram
  (Windows UI Automation) y elige los chats cuyo número de mensajes nuevos subió. Las apps que usan notificaciones de
  Windows se leen de la base de datos de notificaciones del sistema. «Tomar el control de las ventanas» oculta la
  ventana propia de la app (o de Windows) para que solo la muestre la isla.
- Las apps con notificaciones de Windows (WhatsApp, Teams, Slack, Discord, Signal, Zoom, LINE…) aparecen con su nombre e
  icono; un mensaje que parpadea en la barra de tareas y además lanza una notificación cuenta una sola vez.
- Abrir la app marca sus notificaciones como leídas y quita el punto rojo; una que llega mientras usas esa app no se anuncia.
- **Avisos de IA al terminar**: «Conectar» en Ajustes → Notificaciones añade un hook en `~/.claude/settings.json` de
  Claude Code o `~/.codex/config.toml` de Codex (con copia previa, se quita con un clic); cuando una tarea termina o
  necesita aprobación, avisa a `127.0.0.1:47900` con `curl` (con un token aleatorio).

## Historial del portapapeles

- El texto y las imágenes copiadas (capturas, etc.) aparecen por orden de tiempo en la página «Portapapeles» del panel, con las fijadas arriba; copiar otra vez lo mismo solo lo sube arriba.
- Al hacer clic en una entrada solo vuelve al portapapeles: pegas tú con Ctrl+V. Pasa el ratón por una entrada para fijarla o borrarla; arriba puedes buscar.
- Ajustes → Portapapeles: guardar también imágenes, guardar el historial en el disco de este PC (cifrado con la clave de la cuenta de Windows actual, sigue tras reiniciar) o solo en memoria, cuántos días y cuántas entradas conservar. Las fijadas nunca se borran automáticamente.
- Los gestores de contraseñas como Bitwarden, KeePass y 1Password marcan las contraseñas copiadas como «no guardar»; ese contenido se omite por defecto.
- Un icono de portapapeles al final de la cápsula salta con cada copia nueva (se desactiva en Ajustes → Aspecto contraído).

## Alertas de fuga

La biblioteca «debería ir por proxy» = una lista integrada de servicios comunes (~140 dominios principales) + la lista
gfw (actualizada a diario) + tus propias entradas / listas importadas (dominios sin más, formatos de reglas v2fly y
Clash). Dos tipos de fuga:

- **Fallo de regla**: la conexión pasó por la app de proxy pero una regla la envió en directo. Con «Adelantarse»
  activado, esos dominios se envían al proxy antes de llegar a las reglas directas de la suscripción; los grupos por
  servicio de la suscripción no se tocan.
- **Sin pasar por el proxy**: el programa se conectó directamente sin pasar por la app de proxy (ignora el proxy del
  sistema, o la conexión es anterior a TUN). La tarjeta de alerta puede bloquear el acceso directo del programa (una
  regla del Firewall de Windows solo para direcciones públicas; el programa sigue llegando al puerto local del proxy) y
  deshacerlo después.

## Privacidad: a qué se conecta MicaBar

Todas las estadísticas (conexiones, tráfico, reglas, alertas) se quedan en este PC en `%LOCALAPPDATA%\MicaBar\`. Las
únicas peticiones que hace MicaBar por su cuenta:

| Propósito | Dirección | Cuándo |
| --- | --- | --- |
| IP de salida por proxy | ipinfo.io → api.ipify.org → ifconfig.me (por el puerto local del proxy; las salidas por grupo por el puerto loopback de cada grupo) | al iniciar, al cambiar la red / el proxy, cada 5 min |
| IP de salida directa | 4.ipw.cn → myip.ipip.net → ip.3322.net (ligada al adaptador físico; resuelto con 223.5.5.5 / 119.29.29.29) | igual |
| Tipo y propietario de la IP | proxycheck.io (HTTPS) e ip-api.com (solo HTTP en el plan gratuito), con cada IP de salida (también la directa) como consulta; por el puerto local del cliente de proxy | cuando aparece una IP de salida nueva; caché de 24 h por IP; se desactiva en Ajustes → Red |
| Latencia de nodos | la mide el núcleo mihomo contra `www.gstatic.com/generate_204` | cada 15–60 s (perfil de recursos) |
| Lista gfw | cdn.jsdelivr.net / raw.githubusercontent.com (Loyalsoldier/v2ray-rules-dat) | a diario, si está activada |
| Buscar actualizaciones | latest.json de github.com/aileboeth/MicaBar | a diario, si está activado |
| Cuota de IA | api.anthropic.com (Claude Code), chatgpt.com (Codex), cli-chat-proxy.grok.com (Grok), el servidor de lenguaje local de Antigravity | cada 10 min (configurable), si está activada; siempre por el puerto local del cliente de proxy, nunca en directo (sin puerto no hay consulta) |
| Diagnóstico de red | directo: las API HTTP de resolución de 119.29.29.29 / 223.5.5.5 (consultando qq.com); por el proxy: `www.gstatic.com/generate_204`; DNS: una consulta nueva de www.baidu.com por el DNS del sistema; la comprobación completa también hace ping al router | si está activado: una petición pequeña cada 10 s (directo), cada 30 s por el proxy y al DNS; la comprobación completa solo cuando fallan |

País / ASN se consultan en las bases de datos sin conexión que trae el cliente de proxy (`country.mmdb`, `ASN.mmdb`); las
IP de las conexiones nunca se envían a ningún sitio. La cuota de IA usa la sesión que cada herramienta guarda en este PC y
la envía solo al servicio de esa herramienta; nunca se guarda, se registra ni se renueva. «Reproduciendo» lee los
controles multimedia de Windows sin conectarse a la red. Las notificaciones tampoco se conectan: las del día se guardan
solo en memoria, el texto de los mensajes nunca se escribe en disco ni en el registro (y por defecto no se muestra en la
isla); el puerto loopback `127.0.0.1:47900` solo acepta las llamadas de los hooks de IA con el token. El secreto del
controlador de mihomo se lee de su configuración en tiempo de ejecución y nunca se guarda ni se envía.

El historial del portapapeles nunca se conecta a internet: las entradas se quedan en este PC (`%LOCALAPPDATA%\MicaBar\clipboard`, cifrado) o solo en memoria, y nunca en el registro.

## Compilar desde el código

Necesita Rust (rustup), Visual Studio C++ Build Tools, Node.js + pnpm.

```powershell
pnpm install
pnpm build            # un exe: src-tauri\target\release\micabar.exe
pnpm release          # instalador + portátil + manifiesto de actualización -> dist\release-<versión>\ (necesita keys\micabar.key)
```

Estructura: `src-tauri\src\` (backend en Rust: tablas de sockets, ETW, controlador de mihomo, comprobación de salidas,
protección contra fugas, comportamiento de la ventana, captura para el esmerilado…), `ui\` (HTML / CSS / JS sin
empaquetador; `ui\i18n\` contiene las traducciones, con el texto chino del código como clave), `tools\` (script de
publicación, driver de la máquina virtual de pruebas).
Notas de implementación (en chino): [docs/internals.zh-CN.md](docs/internals.zh-CN.md).

## Licencia

[MIT](LICENSE). Iconos de banderas de [flag-icons](https://github.com/lipis/flag-icons) (MIT, ver `ui/flags/LICENSE`). Marcas de herramientas de IA de [lobe-icons](https://github.com/lobehub/lobe-icons) (MIT, ver `ui/brands/LICENSE`); las marcas pertenecen a sus propietarios.

## Descargo de responsabilidad

MicaBar es software libre y de código abierto que su autor desarrolla en su tiempo libre, proporcionado «tal cual», sin
garantía de ningún tipo. Lee las conexiones de red, procesos, notificaciones del sistema y el estado de sesión de las
herramientas de IA en este PC; estos datos solo se usan en este PC. El control de salida y la protección contra fugas
modifican la anulación del cliente de proxy, añaden reglas de firewall y pueden reiniciar el cliente de proxy. El autor
no se hace responsable de ninguna pérdida derivada del uso o la imposibilidad de uso del software (pérdida de datos,
cortes de red, cuentas restringidas…). Úsalo respetando las leyes locales y las condiciones de cada servicio. MicaBar no
está afiliado a Clash, mihomo, Anthropic, OpenAI, Google, xAI ni a ningún otro proyecto o empresa mencionados; sus
nombres y marcas pertenecen a sus propietarios.
