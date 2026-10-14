# actx

**El contexto de tus agentes, en todos tus PCs.** Mantiene tus conversaciones de
[Claude Code](https://claude.com/claude-code) y [opencode](https://opencode.ai) al día entre tus
PCs, agrupadas en proyectos, a través de cualquier carpeta compartida: un NAS, Syncthing, Dropbox…

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
- **Claude Code ⇄ opencode.** Copia una conversación de un programa al otro y sigue allí.
- **Seguro.** Copias de seguridad antes de sobrescribir, escrituras atómicas en la carpeta compartida
  y avisos de git cuando el código no cuadra (cambios sin commitear, otro commit).
- **Un solo archivo, sin dependencias.** Python 3.12+, git y fzf. Interfaz en inglés y español.

## Instalación

**Arch Linux (AUR):**

```
yay -S actx-git
```

**En cualquier otro sitio:** es un único script.

```
curl -fsSLo ~/.local/bin/actx https://raw.githubusercontent.com/ericpastorm/actx/main/actx
chmod +x ~/.local/bin/actx
```

Necesita Python 3.12+ (mejor 3.14+: las conversaciones se comprimen con zstd; si no, con gzip),
`git` y [`fzf`](https://github.com/junegunn/fzf). Todos tus PCs deberían usar la misma compresión:
o todos con 3.14+, o todos por debajo.

## Primeros pasos

En cada PC, una vez:

```
actx init            # eliges la carpeta compartida (te sugiere unidades de red y carpetas sincronizadas)
```

Y luego:

| Comando | Qué hace |
|---|---|
| `actx mk <proyecto>` | Crea un proyecto (o abre uno) y eliges qué conversaciones guardar. Salen primero las que parecen suyas (misma carpeta, el nombre en el título). |
| `actx sync [proyecto]` | Sube lo que has seguido aquí y baja lo nuevo. Eliges los proyectos, o `--all`. |
| `actx` / `actx ls [proyecto]` | Proyectos y estado de cada conversación: `↑ subir`, `↓ bajar`, `↓ nueva`, `⚠ conflicto`, `✓ al día`. |
| `actx cv [proyecto]` | Copia una conversación al otro programa (Claude Code ⇄ opencode). |
| `actx rm <proyecto>` | Quita conversaciones de un proyecto (en tus PCs no se borra nada). |
| `actx update` | Instala la última versión de actx que haya en la carpeta compartida (si lo instalaste como script). |

Selectores: escribe para filtrar, <kbd>Tab</kbd> marca varias, <kbd>Ctrl</kbd>+<kbd>A</kbd> todas, <kbd>Enter</kbd> confirma.

**Truco:** lanza `actx sync` dentro del proyecto (p. ej. en la terminal de tu IDE): ese proyecto sale
el primero, y las conversaciones nuevas de ese repo van directamente ahí.

## Cómo funciona

### Dónde va cada conversación

Cuando una conversación llega por primera vez a un PC, actx busca:

1. una carpeta que ya elegiste antes para ese repo;
2. la misma ruta (relativa a tu home);
3. la carpeta en la que estás, si es el mismo repo;
4. cualquier clon o worktree del repo dentro de tu home (`ACTX_SCAN`), identificado por su **primer
   commit**, así que da igual la ruta o el nombre de la carpeta;
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

### Convertir entre programas

`actx cv` crea una copia en el otro programa. Se conservan tus mensajes y las respuestas; las
herramientas pasan a texto (qué se ejecutó y qué salió, recortado a 1500 caracteres por salida); el
razonamiento interno y las imágenes se pierden. Si la conversación se compactó, solo se convierte el
resumen y lo que vino después (que es lo que el agente tenía en contexto). Es una aproximación, pero
basta para seguir trabajando. En opencode, la copia usa el último modelo que hayas usado allí.

### La carpeta compartida

```
<carpeta compartida>/
  .actx                         marca
  <proyecto>/
    claude/<id de sesión>/      meta.json · preview.txt · session.jsonl.zst · extra.tar.zst
    opencode/<id de sesión>/    meta.json · preview.txt · session.json.zst · child-*.json.zst
  _bin/actx                     una copia de actx, para `actx update` si lo instalaste como script
```

Vale cualquier carpeta a la que lleguen todos tus PCs: una unidad SMB/NFS, una carpeta de Syncthing o
Dropbox… Si tu herramienta de sincronización no es en tiempo real, evita tocar el mismo proyecto en
dos PCs a la vez.

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
| `ACTX_OPENCODE_ARGS` | Opciones extra para `opencode session …`, p. ej. `--standalone`. |
| `CLAUDE_CONFIG_DIR`, `XDG_*` | Se respetan, igual que hacen Claude Code y opencode. |

## Privacidad

Las conversaciones contienen tus mensajes, las respuestas del agente y **salidas de comandos**, que
pueden incluir secretos (claves de API, tokens, el contenido de un `.env`). actx las guarda tal cual
en la carpeta compartida: que sea privada, y no uses un servicio de sincronización público en el que
no confiarías tu código.

## Limitaciones

- actx lee **formatos internos y sin documentar** de Claude Code (`~/.claude/projects/*.jsonl`) y de
  opencode (su base de datos SQLite y `opencode session export/import`). Pueden cambiar sin aviso.
  Probado con Claude Code 2.1 y opencode 2.0; los tests se ejecutan contra opencode de verdad para
  detectar cambios de formato cuanto antes. Si algo se rompe, abre un issue.
- La conversión entre programas pierde detalle (ver arriba). Sincronizar dentro del mismo programa, no.
- opencode se maneja con su CLI: mientras importas una sesión, no la tengas abierta en opencode.
- Pensado para Linux. En macOS probablemente funcione; Windows no está soportado.

## Desarrollo

```
pip install pytest
pytest                # los tests de opencode se ejecutan si está instalado y se saltan si no
```

Los tests montan PCs simulados (cada uno con su home y sus datos de Claude Code y opencode) que
comparten una carpeta, y hacen sincronizaciones, mezclas y conversiones reales entre ellos.

## Licencia

[MIT](LICENSE). actx es un proyecto independiente, sin relación con Anthropic ni con el equipo de opencode.
