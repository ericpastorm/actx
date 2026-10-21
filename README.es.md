# actx

**El contexto de tus agentes, en todos tus PCs.** Mantiene tus conversaciones de
[Claude Code](https://claude.com/claude-code), [opencode](https://opencode.ai) y
[Codex](https://github.com/openai/codex) al día entre tus PCs, agrupadas en proyectos, a través de
cualquier carpeta compartida: un NAS, Syncthing, Dropbox…

[Read in English](README.md)

Trabajas en un sobremesa y un portátil. `git pull` te trae el código, pero la conversación con tu
agente (todo lo que aprendió sobre la tarea) se queda atrás. actx se la lleva:

```
sobremesa$ actx mk tienda       # creas un proyecto vacío
sobremesa$ actx add tienda      # eliges qué conversaciones guardar en él
portátil$  actx sync            # las bajas, y subes lo que hayas seguido aquí
portátil$  claude --resume …    # sigues donde lo dejaste
```

## Qué hace

- **Proyectos con nombre, conversaciones que eliges tú.** Solo viajan las conversaciones que añades, a un proyecto o a General.
- **No depende de las rutas.** El repo puede estar en otra ruta en cada PC (otro clon, un worktree de
  tu IDE…): actx lo encuentra por su primer commit y cambia las rutas dentro de la conversación.
- **Sincroniza en los dos sentidos y gestiona conflictos.** Recuerda la última sincronización, así
  que sabe qué subir, qué bajar y cuándo han cambiado los dos lados. Entonces puedes **mezclar**: te
  quedas con la versión de este PC más un resumen de lo hecho en el otro, y la otra versión se
  guarda aparte como una rama.
- **Automático, si quieres.** `actx auto on`, y las conversaciones se sincronizan al abrirlas o
  cerrarlas en Claude Code o Codex, y cada 10 minutos. Nunca reemplaza una conversación abierta en un agente.
- **Claude Code ⇄ opencode ⇄ Codex ⇄ Antigravity.** Copia una conversación a otro programa y sigue allí.
- **Seguro.** Copias de seguridad antes de sobrescribir (`actx restore`), escrituras atómicas y un
  bloqueo en la carpeta compartida, y avisos de git cuando el código no cuadra (cambios sin commitear,
  otro commit).
- **Un binario pequeño.** Escrito en Rust; solo necesita git y fzf. Linux, macOS y Windows. Interfaz en inglés y
  español.

## Instalación

Con [Rust](https://rustup.rs) instalado:

```
cargo install --git https://github.com/ericpastorm/actx
```

**Arch Linux:** el paquete de AUR está en camino. Mientras tanto puedes construirlo con el PKGBUILD de este repo:

```
git clone https://github.com/ericpastorm/actx
cd actx/packaging/aur && makepkg -si
```

**Windows:** vale el mismo `cargo install` (con las
[Build Tools de Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/), que Rust pide de
todos modos). `git` y `fzf` salen de winget: `winget install Git.Git junegunn.fzf`. Las conversaciones viajan
entre Windows, Linux y macOS: las rutas de dentro se cambian entre `C:\Users\yo\tienda` y `/home/yo/tienda`.

Para funcionar necesita `git` y [`fzf`](https://github.com/junegunn/fzf). En los PCs sin Rust, copia el
binario desde uno que lo tenga: actx deja una copia en la carpeta compartida (`_bin/`), y `actx update` la
mantiene al día (los PCs con la versión antigua en Python también se pasan a esta: ejecuta `actx update` dos veces).

Autocompletado: `actx completions bash|zsh|fish` (el PKGBUILD lo instala), p. ej.
`actx completions bash > ~/.local/share/bash-completion/completions/actx`.

Programas soportados: **Claude Code**, **opencode** 1.x y 2.x (cada PC puede tener cualquiera: una
conversación guardada con una se convierte al llegar a la otra) y **Codex**. **Antigravity CLI** (`agy`)
se convierte desde y hacia los demás, pero sus conversaciones aún no se sincronizan.

## Primeros pasos

En cada PC, una vez:

```
actx init            # eliges la carpeta compartida (te sugiere unidades de red y carpetas sincronizadas)
actx auto on         # opcional: que se sincronice solo (ver «Sincronización automática»)
```

Y luego:

| Comando | Qué hace |
|---|---|
| `actx mk <proyecto>` | Crea un proyecto vacío. |
| `actx add [proyecto]` | Elige conversaciones locales para guardarlas en un proyecto existente. Si lo omites, se guardan en General. |
| `actx delete [proyecto]` | Elige copias compartidas para borrar, de todos los proyectos y General o solo del proyecto indicado. Conserva originales locales y proyectos, aunque queden vacíos. |
| `actx sync --dry-run [proyecto]` | Previsualiza operaciones y decisiones pendientes sin modificar datos; admite `--all` y `--general`. |
| `actx clean [advanced]` | Sugiere conversaciones locales sin usar (subagentes, pruebas, aperturas repetidas, muy cortas) y mueve las elegidas a una cuarentena recuperable. |
| `actx sync [proyecto]` | Sube lo que has seguido aquí y baja lo nuevo. Eliges los proyectos, o `--all`. |
| `actx` / `actx browse [proyecto]` | Navegador: una fila por conversación con sus harnesses. Enter abre, Alt-H configura y Supr quita. |
| `actx ls [proyecto]` | Lista compacta sin interacción, con los estados de las copias. |
| `actx resume [proyecto]` | Sincroniza la conversación elegida y abre su copia en el harness seleccionado. |
| `actx setup [carpeta]` | Asistente completo de carpeta compartida, harnesses, retención y preferencia al abrir. |
| `actx profile` / `actx profile setup` | Resumen del perfil o asistente para modificarlo. `--json` conserva la salida estructurada. |
| `actx cv [proyecto]` | Copia una conversación a otro programa (Claude Code, opencode, Codex, Antigravity). |
| `actx rm <proyecto>` | Borra el proyecto completo y sus copias compartidas, previa confirmación. Conserva los originales locales en todos los equipos. |
| `actx auto [on\|off]` | Sincronización automática: hooks en Claude Code y Codex, y un temporizador. `actx auto` a secas dice cómo va. |
| `actx restore` | Recupera una conversación de una copia de seguridad. |
| `actx doctor` | Comprueba la carpeta compartida, los programas y la sincronización automática. |
| `actx update` | Instala la última versión de actx que haya en la carpeta compartida (si lo copiaste a mano). |

Selectores: escribe para filtrar, <kbd>Tab</kbd> marca varias, <kbd>Ctrl</kbd>+<kbd>A</kbd> todas, <kbd>Enter</kbd> confirma.

`mk` ahora solo crea proyectos: para guardar conversaciones, usa `add <proyecto>`.
`delete` quita conversaciones concretas; `rm` borra el proyecto completo.
Los proyectos que ya tenías guardados siguen funcionando sin migración.

### Conversaciones generales

`actx add` permite guardar conversaciones sin crear un proyecto de actx ni un repositorio Git.
Al sincronizar en otro equipo, actx pregunta la carpeta de destino de cada conversación nueva de
General. Puedes elegir una carpeta existente o **otra carpeta** e introducir cualquier ruta.
El destino se recuerda por conversación y equipo, aunque varias procedan de la misma carpeta.
Si cancelas, queda pendiente; si la carpeta recordada desaparece, vuelve a preguntar.

```sh
actx add                     # guarda conversaciones seleccionadas en General
actx sync --general          # descarga General y pregunta los destinos que falten
actx ls --general            # muestra su estado
actx delete --general        # borra copias compartidas; conserva originales locales
```

También puedes usar `cv --general` para convertir solo conversaciones guardadas en General.
La sincronización automática deja pendientes las que necesitan destino y lo indica en el log;
ejecuta una sincronización interactiva para elegirlo. Las conversaciones que ya están en el equipo
conservan su carpeta local. Añadir una conversación a otra colección mueve su copia compartida.
Todos los equipos deben usar una versión con soporte para General para sincronizar esas conversaciones;
las versiones anteriores siguen viendo los proyectos existentes.

**Truco:** lanza `actx sync` dentro del proyecto (p. ej. en la terminal de tu IDE): ese proyecto sale
el primero, y las conversaciones nuevas de ese repo van directamente ahí.

## Cómo funciona

### Dónde va cada conversación

Cuando una conversación llega por primera vez a un PC, actx busca:

1. una carpeta que ya elegiste antes para ese repo;
2. la misma ruta (relativa a tu home);
3. la carpeta en la que estás, si es el mismo repo;
4. cualquier clon o worktree del repo dentro de tu home (`ACTX_SCAN`), o junto a carpetas con
   conversaciones (proyectos en otro disco, por ejemplo), identificado por su **primer commit**,
   así que da igual la ruta o el nombre de la carpeta;
5. si no, te pregunta una vez (con *aquí* como primera opción) y se acuerda.

Las rutas dentro de la conversación se cambian de las del otro PC a las de este.

### Sincronización y conflictos

Cada PC recuerda cómo estaba cada conversación en su última sincronización. Si solo ha cambiado aquí,
se sube. Si solo ha cambiado en la carpeta compartida, se baja. Si ha cambiado en los dos, hay
conflicto y eliges: **mezclar**, quedarte con la de aquí, con la del otro, o saltarla.

*Mezclar* se queda con la versión de este PC y le añade un mensaje con el resumen de lo hecho en el
otro (y una respuesta del agente confirmándolo), así el agente sabe lo que pasó en los dos. La otra
versión también se guarda, entera, como *«título (rama de portátil)»*. Antes de sobrescribir nada,
se guarda una copia en `~/.local/state/actx/backups/`.

Cierra la conversación en su agente antes de mezclar. Si actx detecta que está abierta, la mezcla
queda pendiente hasta que la cierres y vuelvas a ejecutar `actx sync`.

### Vista previa antes de sincronizar

```sh
actx sync --dry-run            # elige proyectos; todos si no hay terminal
actx sync --dry-run --all      # todos los proyectos y General
actx sync --dry-run miproyecto # un proyecto
actx sync --dry-run --general  # solo General
actx sync --dry-run --auto     # consulta el modo automático sin ejecutarlo
```

Muestra subidas, descargas, conversaciones nuevas, conflictos, copias entre harnesses y retención,
con los destinos conocidos y las decisiones pendientes. No escribe conversaciones, estado, backups,
logs, bloqueos compartidos ni binarios de actx. Consulta SQLite mediante copias temporales privadas
y no ejecuta la CLI de opencode. Si una operación depende de descargar o resolver un conflicto,
el plan indica que tendrá que comprobarse después.

Es una vista del estado actual, no un plan guardado para ejecutar. Si detecta cambios mientras la
prepara, pide repetirla. Un `sync` real recalcula las operaciones y conserva sus bloqueos y
comprobaciones de seguridad.

### Limpieza de conversaciones antiguas

`actx clean` revisa conversaciones locales y sugiere las que probablemente ya no sirven:
subagentes (hilos creados por otro agente), pruebas de conexión y peticiones de «responde solo OK»,
aperturas repetidas y conversaciones muy cortas. No hay nada seleccionado por defecto y no se
borra nada mientras revisas.

```sh
actx clean                  # sugerencias generales, 7 días de inactividad
actx clean --dry-run        # lista las candidatas sin abrir la revisión
actx clean advanced         # pregunta categorías, antigüedad, harnesses, longitud y texto
actx clean quarantine       # lista las conversaciones recuperables
actx clean restore          # recupera una, sin sobrescribir otra existente
```

Las copias compartidas y vinculadas, las conversaciones abiertas o activas hace poco y los padres
de subagentes abiertos están protegidos. Las elegidas pasan a una cuarentena comprimida en
`~/.local/state/actx/quarantine/`; no se borran para siempre. Nada se elimina automáticamente por
antigüedad.

Filtros: `--only agents,repeated,tests,short,old`, `--older-than DÍAS`, `--max-messages N`,
`--max-chars N`, `--min-similar N`, `--harnesses claude,codex,opencode,antigravity`,
`--folder RUTA` y `--contains TEXTO`.

### Sincronización automática

`actx auto on` instala en este PC:

- **hooks en Claude Code** (`~/.claude/settings.json`) **y en Codex** (`~/.codex/hooks.json`), junto a
  los que ya tengas. Al empezar una conversación, actx sincroniza en segundo plano; si la que retomas
  tiene mensajes más nuevos de otro PC, Claude Code o Codex te lo avisan (sal, ejecuta `actx sync` y
  vuelve a abrirla). Al terminar, se sube. Codex te pide revisar los hooks nuevos una vez: acéptalos en
  `/hooks`.
- **un temporizador** cada 10 minutos (un timer de usuario de systemd en Linux, un LaunchAgent en macOS, una
  tarea del Programador de tareas en Windows, sin ventana de consola).
  Es lo que mantiene opencode al día, y recoge lo que se les escape a los hooks.

Cada ejecución es `actx sync --auto`: sube lo que ha cambiado y baja lo nuevo, pero como no pregunta,
solo hace lo seguro. No baja encima de una conversación abierta en un agente (espera a que la cierres), y
deja los conflictos, y las conversaciones nuevas que no sabe dónde poner, para `actx sync`. Lo que hace
queda en `~/.local/state/actx/auto.log`; `actx auto` enseña las últimas líneas. `actx auto off` lo quita
todo.

### Copias de seguridad

Antes de reemplazar una conversación en este PC (al bajar, mezclar o restaurar), actx guarda una copia en
`~/.local/state/actx/backups/` durante 30 días. `actx restore` las lista y recupera la que elijas (antes
guarda una copia de la actual).

### Convertir entre programas

`actx cv` crea una copia en el programa que elijas. Funciona con cualquier conversación local,
incluso sin haber ejecutado `actx init`, sin proyecto de actx y fuera de cualquier repositorio Git.
Si la conversación no tenía carpeta o la carpeta original ya no existe en este PC, actx te deja
elegir una carpeta de trabajo existente y cambia las rutas de la carpeta antigua a la nueva. La
conversación original no se toca, y las conversaciones locales que no pertenecen a un proyecto ni a
General no se suben al convertirlas.

Se conservan tus mensajes y las respuestas; las
herramientas pasan a texto (qué se ejecutó y qué salió, recortado a 1500 caracteres por salida); el
razonamiento interno y las imágenes se pierden. Si la conversación se compactó, solo se convierte el
resumen y lo que vino después (que es lo que el agente tenía en contexto). Es una aproximación, pero
basta para seguir trabajando. En opencode, la copia usa el último modelo que hayas usado allí; en Codex y
Antigravity, el que tengas por defecto. Antigravity no tiene forma de importar: actx escribe la
conversación en su base de datos (los pasos, el índice y el título), y `agy --conversation <id>` la retoma.

Cuando Codex está instalado, actx también registra las conversaciones convertidas y descargadas
mediante su app-server local para que aparezcan en el selector de sesiones. Solo lee la conversación
guardada; no inicia un turno del agente ni solicita una respuesta al modelo.

### La carpeta compartida

```
<carpeta compartida>/
  .actx                         marca
  .lock                         quién está escribiendo ahora (los PCs se turnan)
  <proyecto>/
    claude/<id de sesión>/      meta.json · preview.txt · session.jsonl.zst · extra.tar.zst
    opencode/<id de sesión>/    meta.json · preview.txt · session.json.zst · child-*.json.zst
    codex/<id de sesión>/       meta.json · preview.txt · rollout.jsonl.zst
  _bin/actx-<arq>-<so>          una copia de actx, para `actx update` si lo copiaste a mano
  _bin/actx                     para equipos con la versión antigua en Python: los pasa a la de arriba
```

Vale cualquier carpeta a la que lleguen todos tus PCs: una unidad SMB/NFS, una carpeta de Syncthing o
Dropbox… En una unidad de red, los PCs se turnan para escribir (el fichero `.lock` dice quién está en
ello; si se queda uno de una ejecución que murió, caduca a los 5 minutos), y un sync que tuvo que esperar
se salta lo que se haya subido mientras tanto. Una herramienta de sincronización que no es en tiempo
real no puede ofrecer eso: evita sincronizar el mismo proyecto en dos PCs a la vez.

## Configuración

`~/.config/actx/config.json`, lo escribe `actx init`:

```json
{"root": "/mnt/nas/actx", "lang": "es"}
```

| Variable | Para qué |
|---|---|
| `ACTX_ROOT` | Carpeta compartida (manda sobre la configuración). |
| `ACTX_LANG` | `en` o `es` (por defecto: el idioma de tu sistema). |
| `ACTX_SCAN` | Dónde buscar repos, separados por `:` (por defecto: tu home). |
| `ACTX_OPENCODE_ARGS` | Opciones extra para exportar/importar/borrar en opencode, p. ej. `--standalone` (2.x). |
| `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_*` | Se respetan, igual que hacen Claude Code, Codex y opencode. |

## Privacidad

Las conversaciones contienen tus mensajes, las respuestas del agente y **salidas de comandos**, que
pueden incluir secretos (claves de API, tokens, el contenido de un `.env`). actx las guarda tal cual
en la carpeta compartida: que sea privada, y no uses un servicio de sincronización público en el que
no confiarías tu código.

## Limitaciones

- actx lee **formatos internos y sin documentar** de Claude Code (`~/.claude/projects/*.jsonl`), de
  opencode (su base de datos SQLite y sus comandos de exportar/importar), de Codex (los rollouts de
  `~/.codex/sessions`) y de Antigravity CLI (`~/.gemini/antigravity-cli`: una base de datos SQLite de
  protocol buffers por conversación). Pueden cambiar sin aviso. Probado con Claude Code 2.1, opencode
  1.19 y 2.0, Codex 0.160 y Antigravity CLI 1.4; los tests se ejecutan contra opencode y Codex de verdad (también cada semana) para
  detectar cambios de formato cuanto antes. Si algo se rompe, abre un issue.
- La conversión entre programas pierde detalle (ver arriba). Sincronizar dentro del mismo programa, no,
  salvo entre opencode 1.x y 2.x, que pasa por la misma conversión. Para mezclar hace falta la misma
  versión de opencode.
- opencode se maneja con su CLI: mientras importas una sesión, no la tengas abierta en opencode.
- Linux, macOS y Windows (los tres probados en la CI). En Windows, actx no puede ver en qué carpeta corre
  otro programa, así que mientras Codex u opencode estén abiertos, no baja encima de sus conversaciones
  (Claude Code dice exactamente cuál tiene abierta, así que a él no le afecta). Los hooks se ejecutan en el
  shell que usan allí Claude Code y Codex (Git Bash).
- Las apps de sincronización de Windows con archivos bajo demanda (Synology Drive, OneDrive…) dejan los
  ficheros solo en línea, y actx falla con `os error 395`. Pon la carpeta compartida en «Mantener siempre
  en este dispositivo» (o `attrib +P "<carpeta>" /S /D`).

## Desarrollo

```
cargo test            # los tests de opencode y Codex se ejecutan si están instalados y se saltan si no
```

Los tests de extremo a extremo montan PCs simulados (cada uno con su home y sus datos de Claude Code,
opencode y Codex) que comparten una carpeta, y ejecutan el binario de verdad en ellos: sincronizaciones,
mezclas y conversiones. Lo manejan con `ACTX_HOST` (el nombre del PC), `ACTX_PICK` (qué eligen los
selectores), `ACTX_ANSWER` (la respuesta a cada pregunta), `ACTX_LOCK_WAIT` (segundos de espera por el
bloqueo de la carpeta compartida) y `ACTX_HOOK_WAIT` (los hooks esperan al sync que lanzan). La CI los
ejecuta en Linux y macOS, y además pasa `cargo fmt --check` y `cargo clippy -- -D warnings`.

actx 0.3 es una reescritura en Rust del script en Python de las versiones 0.1–0.2. Todas las versiones
leen y escriben la misma carpeta compartida y el mismo estado local, así que puedes mezclarlas mientras
actualizas tus PCs (la 0.4 añade el bloqueo, que las anteriores ignoran).

### Navegador de conversaciones

`actx` abre el navegador cuando se ejecuta en una terminal; sin interacción muestra la lista compacta.
Las copias se agrupan por vínculos e identificadores guardados, nunca por coincidencia de título.
Una fila muestra el título, proyecto, harnesses con copia existente y la actividad más reciente. La
vista previa detalla el estado de cada copia y los harnesses configurados que aún están pendientes.
Las conversiones nuevas guardan su identidad común, también en la metadata compartida. Las copias
antiguas sin una identidad o vínculo conocido permanecen separadas.

- **Enter:** elige el harness, sincroniza solo esa conversación y abre su copia local. Ctrl-R en el
  selector recuerda la elección. Los conflictos o cambios pendientes impiden abrir una copia obsoleta.
- **Alt-H:** configura los harnesses de las conversaciones marcadas, usa el perfil del equipo o deja
  de mantener copias. Las copias locales existentes se conservan.
- **Supr:** permite cambiar el perfil, borrar copias compartidas o elegir un proyecto para borrarlo.
  Los borrados requieren confirmación y conservan los originales locales.
- **Tab / Ctrl-A:** marca varias / todas para configurar o quitar. Para abrir, selecciona una sola.

`actx resume --print` sincroniza e imprime el comando sin iniciar el agente. `add`, `cv`, `delete` y
el selector de perfiles usan la misma agrupación. Si `cv` ya tiene una copia en el destino, la reutiliza.
Un perfil puede adoptar una conversión existente identificada si los historiales coinciden; si difieren,
conserva ambos sin crear una tercera copia ni sobrescribir las continuaciones.

### Perfil de harnesses y retención

`actx setup` configura la carpeta compartida y el perfil de **este equipo** en un asistente.
`actx init` elige solo la carpeta; `actx profile setup` pregunta por los harnesses, la retención y
el harness preferido al abrir, sin cambiar la carpeta. `actx profile` muestra un resumen legible:

```bash
actx setup                                        # configuración completa
actx profile setup                                # configurar solo el perfil
actx profile                                      # ver preferencias
actx profile --json                               # salida para scripts
actx profile --harnesses claude,codex,opencode
actx profile --retention 3m
actx profile --conversation --harnesses claude,codex # seleccionar conversación
actx profile --conversation claude:ID --harnesses claude,codex
actx profile --harnesses none                     # desactivar copias vinculadas
actx profile --retention never                    # no borrar automáticamente
```

Para configurar sin preguntas: `actx setup /ruta/compartida --harnesses claude,codex --retention 3m`.
Los flags de perfil en `init` se conservan por compatibilidad. Los asistentes muestran la selección
actual; Ctrl-D desactiva las copias vinculadas en la pregunta de harnesses.
Los perfiles son locales: cada PC puede usar harnesses distintos. Las preferencias por conversación
sustituyen la lista del perfil general. Los valores de retención son `never` (por defecto), `1m`, `3m`,
`6m`, `1y` y `2y`: 30, 90, 180, 365 y 730 días de inactividad, respectivamente.

Al añadir o sincronizar una conversación guardada, actx crea las copias del perfil que falten y recuerda
sus identificadores. Las siguientes sincronizaciones añaden las continuaciones completas a esas
mismas sesiones. Una continuación de una copia vuelve a la conversación base y viaja a los otros PCs
mediante el sync normal. Los originales conservan sus herramientas nativas; los intercambios que
cruzan de harness pasan por el formato de conversión a texto. Las copias vinculadas no se ofrecen de
nuevo en `add`, para evitar guardarlas como conversaciones independientes.

La base compartida sigue siendo de Claude, Codex u opencode; Antigravity puede ser una copia vinculada
local de ida y vuelta. Para partir de una conversación de Antigravity, conviértela primero con `cv` y
usa `add` sobre la copia. Las copias requieren una carpeta existente y el harness de destino instalado;
opencode necesita un modelo configurado. Se posponen conversaciones abiertas o con actividad reciente.
Si ambas ramas cambiaron, se editaron mensajes anteriores o se compactó el historial, actx conserva
ambas y avisa del conflicto; esta primera entrega no resuelve esas diferencias automáticamente.

La retención se aplica en cada `sync`, incluidos los automáticos que tengas activados con `actx auto`.
Solo elimina copias de la carpeta compartida; conserva proyectos, originales y copias locales.
Se omiten conversaciones con cambios locales pendientes, conflictos o actividad detectada.
Elige `never` para desactivarla. No se instala un temporizador adicional.

Esta entrega tiene comprobaciones básicas; la validación amplia de conflictos, interrupciones y de
la matriz de harnesses queda para la siguiente sesión de depuración.

## Licencia

[MIT](LICENSE). actx es un proyecto independiente, sin relación con Anthropic, OpenAI ni el equipo de opencode.
