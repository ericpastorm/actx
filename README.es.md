# actx

**El contexto de tus agentes, en todos tus PCs.** Mantiene tus conversaciones de
[Claude Code](https://claude.com/claude-code), [opencode](https://opencode.ai) y
[Codex](https://github.com/openai/codex) al día entre tus PCs, agrupadas en proyectos, a través de
cualquier carpeta compartida: un NAS, Syncthing, Dropbox…

[Read in English](README.md)

Trabajas en un sobremesa y un portátil. `git pull` te trae el código, pero la conversación con tu
agente (todo lo que aprendió sobre la tarea) se queda atrás. actx se la lleva:

```
sobremesa$ actx mk tienda       # creas un proyecto y eliges qué conversaciones van en él
portátil$  actx sync            # las bajas, y subes lo que hayas seguido aquí
portátil$  claude --resume …    # sigues donde lo dejaste
```

## Qué hace

- **Proyectos con nombre, conversaciones que eliges tú.** Solo viaja lo que metes en un proyecto.
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
- **Un binario pequeño.** Escrito en Rust; solo necesita git y fzf. Linux y macOS. Interfaz en inglés y
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
| `actx mk <proyecto>` | Crea un proyecto (o abre uno) y eliges qué conversaciones guardar. Salen primero las que parecen suyas (misma carpeta, el nombre en el título). |
| `actx sync [proyecto]` | Sube lo que has seguido aquí y baja lo nuevo. Eliges los proyectos, o `--all`. |
| `actx` / `actx ls [proyecto]` | Proyectos y estado de cada conversación: `↑ subir`, `↓ bajar`, `↓ nueva`, `⚠ conflicto`, `✓ al día`. |
| `actx cv [proyecto]` | Copia una conversación a otro programa (Claude Code, opencode, Codex, Antigravity). |
| `actx rm <proyecto>` | Quita conversaciones de un proyecto (en tus PCs no se borra nada). |
| `actx auto [on\|off]` | Sincronización automática: hooks en Claude Code y Codex, y un temporizador. `actx auto` a secas dice cómo va. |
| `actx restore` | Recupera una conversación de una copia de seguridad. |
| `actx doctor` | Comprueba la carpeta compartida, los programas y la sincronización automática. |
| `actx update` | Instala la última versión de actx que haya en la carpeta compartida (si lo copiaste a mano). |

Selectores: escribe para filtrar, <kbd>Tab</kbd> marca varias, <kbd>Ctrl</kbd>+<kbd>A</kbd> todas, <kbd>Enter</kbd> confirma.

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

### Sincronización automática

`actx auto on` instala en este PC:

- **hooks en Claude Code** (`~/.claude/settings.json`) **y en Codex** (`~/.codex/hooks.json`), junto a
  los que ya tengas. Al empezar una conversación, actx sincroniza en segundo plano; si la que retomas
  tiene mensajes más nuevos de otro PC, Claude Code o Codex te lo avisan (sal, ejecuta `actx sync` y
  vuelve a abrirla). Al terminar, se sube. Codex te pide revisar los hooks nuevos una vez: acéptalos en
  `/hooks`.
- **un temporizador** cada 10 minutos (un timer de usuario de systemd en Linux, un LaunchAgent en macOS).
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

`actx cv` crea una copia en el programa que elijas. Se conservan tus mensajes y las respuestas; las
herramientas pasan a texto (qué se ejecutó y qué salió, recortado a 1500 caracteres por salida); el
razonamiento interno y las imágenes se pierden. Si la conversación se compactó, solo se convierte el
resumen y lo que vino después (que es lo que el agente tenía en contexto). Es una aproximación, pero
basta para seguir trabajando. En opencode, la copia usa el último modelo que hayas usado allí; en Codex y
Antigravity, el que tengas por defecto. Antigravity no tiene forma de importar: actx escribe la
conversación en su base de datos (los pasos, el índice y el título), y `agy --conversation <id>` la retoma.

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
- Linux y macOS (los dos probados en la CI). Windows no está soportado.

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

## Licencia

[MIT](LICENSE). actx es un proyecto independiente, sin relación con Anthropic, OpenAI ni el equipo de opencode.
