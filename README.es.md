<p align="center">
  <img src="assets/readme-hero.svg" alt="actX — el contexto de tus agentes, en todos tus equipos. Continúa tus conversaciones a través de una carpeta compartida." width="100%">
</p>

<p align="center">
  <a href="https://github.com/ericpastorm/actx/actions/workflows/test.yml"><img src="https://github.com/ericpastorm/actx/actions/workflows/test.yml/badge.svg?branch=main" alt="Pruebas de la CLI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-252525?style=flat-square" alt="Licencia MIT"></a>
  <img src="https://img.shields.io/badge/CLI-Linux%20%7C%20macOS%20%7C%20Windows-252525?style=flat-square" alt="CLI para Linux, macOS y Windows">
  <img src="https://img.shields.io/badge/idiomas-EN%20%7C%20ES%20%7C%20ZH-252525?style=flat-square" alt="Inglés, español y chino simplificado">
</p>

<p align="center">
  <a href="README.md">English</a> · <strong>Español</strong> · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="#instalación">Instalación</a> · <a href="#del-sobremesa-al-portátil">Primeros pasos</a> · <a href="docs/desktop.md">Guía de escritorio</a> · <a href="docs/cli.es.md">Guía de la CLI</a> · <a href="docs/json-api.md">API JSON</a>
</p>

# El contexto de tus agentes, en todos tus equipos

**actX sincroniza y convierte conversaciones de agentes de programación.** Empieza con Claude Code
en el sobremesa, continúa con Codex en el portátil o abre el mismo trabajo en opencode. Conserva el
contexto que ya has construido, con una interfaz de escritorio nativa o un navegador de terminal compacto.

Tú eliges las conversaciones que compartes y el almacenamiento: un NAS, una unidad de red,
Syncthing, Dropbox u otra carpeta compartida. Cada equipo conserva sus propias rutas y preferencias de agentes.

## Qué puedes hacer

| Quieres… | actX se encarga de… |
| --- | --- |
| **Continuar en otro equipo** | Sincronizar en ambos sentidos, identificar el repositorio y adaptar las rutas cuando el proyecto está en otra carpeta. |
| **Cambiar de agente** | Convertir entre Claude Code, Codex, opencode y Antigravity CLI, también con conversaciones locales sin proyecto. |
| **Elegir qué compartes** | Organizar proyectos con nombre y conversaciones en General. La selección es explícita; puedes activar reglas por proyecto con exclusiones. |
| **Usar una GUI o la terminal** | Buscar, filtrar proyectos, leer mensajes, consultar estados y abrir la conversación en el agente elegido. |
| **Mantener copias en varios agentes** | Configurar perfiles por equipo, vincular copias y propagar continuaciones completas, reutilizando las sesiones existentes. |
| **Revisar un conflicto** | Comparar historiales, elegir cómo resolverlo y conservar la alternativa. Recuperar backups y publicaciones interrumpidas. |
| **Limpiar el historial local** | Revisar conversaciones antiguas, cortas, repetidas o de subagentes y mover las elegidas a cuarentena recuperable. |
| **Automatizar lo habitual** | Activar hooks de Claude Code/Codex, un temporizador y reglas de incorporación por proyecto. Las decisiones pendientes esperan tu revisión. |
| **Crear tu propio flujo** | Usar una API JSON con versiones para consultar, previsualizar, sincronizar, convertir, limpiar y recuperar. |

## Una interfaz de escritorio cuando la necesitas

![Escritorio de actX con proyectos, estados, logos de agentes y vista previa de mensajes](assets/screenshots/desktop.png)

*Interfaz nativa de Linux con conversaciones de ejemplo. La misma instalación incluye la CLI.*

1. **Elige un proyecto o General.** Consulta las conversaciones mantenidas o muestra las locales para añadirlas.
2. **Elige una conversación y un agente.** Revisa los mensajes, las copias y la carpeta de destino.
3. **Revisa y aplica.** Sincroniza, convierte, retoma, resuelve un conflicto o recupera una versión anterior.

La GUI incluye limpieza, reglas de incorporación automática, ajustes y diagnóstico. Primero muestra
el catálogo local y lo comprueba en segundo plano; los mensajes se cargan al seleccionar una conversación.
La marca Pixel 5×7 tiene una X roja animada, con opciones de aspecto locales en Configuración.

## Instalación

Instala [Rust](https://rustup.rs), Git y [fzf](https://github.com/junegunn/fzf), además de los agentes
que quieras utilizar. fzf permite los selectores interactivos de la terminal.

**Con interfaz de escritorio:**

```sh
cargo install --git https://github.com/ericpastorm/actx --features gui --locked
actx gui
```

Se instalan `actx` y el lanzador `actx-gui`. Abre `actx-gui` directamente para usarlo sin terminal;
si copias la instalación a mano, mantén ambos ejecutables juntos.

**Solo CLI:**

```sh
cargo install --git https://github.com/ericpastorm/actx --locked
actx setup
```

**CI:** pruebas de CLI, compilación de GUI, Clippy y pruebas de estado/renderizado en Linux,
macOS y Windows. Linux también abre una ventana real bajo Xvfb. Las ventanas, los selectores de
carpetas y el lanzamiento de terminales en Windows/macOS siguen pendientes de validación manual.
Consulta los [requisitos de instalación](docs/cli.es.md#instalación)
y la [guía de escritorio](docs/desktop.md).

## Del sobremesa al portátil

En el primer equipo:

```sh
actx setup                  # elige almacenamiento y agentes para este equipo
cd ~/trabajo/tienda
actx mk tienda              # vincula la carpeta y elige conversaciones
actx sync tienda            # publica los cambios y actualiza las copias vinculadas
```

En el otro equipo:

```sh
actx setup                  # misma carpeta compartida; preferencias propias de agentes
actx sync tienda            # encuentra el repositorio o elige su carpeta local
actx resume tienda          # selecciona una conversación y continúa en un agente
```

El proyecto puede estar en `~/trabajo/tienda` en un equipo y en `D:\Proyectos\tienda` en otro.
actX identifica los repositorios, adapta las rutas y recuerda el destino que elijas.
**El código viaja con Git o tu flujo habitual; actX lleva las conversaciones.**

Ejecuta `actx` para abrir el navegador compacto. La vista normal muestra las conversaciones que
mantienes; `actx --all` incluye el resto del historial local.

## Conversaciones sin proyecto

Guarda las conversaciones que elijas en **General** y selecciona sus carpetas de destino en el otro equipo:

```sh
actx add --general
actx sync --general
```

Cada conversación recuerda su propio destino en cada equipo. El conversor también funciona de forma
local sin almacenamiento compartido, sin proyecto de actX y fuera de un repositorio Git:

```sh
actx cv        # conversaciones de la carpeta/proyecto actual
actx cv --all  # todas las carpetas, incluidas las conversaciones sin carpeta
```

## Agentes compatibles

| Agente | Sincronización nativa compartida | Conversión | Copias locales vinculadas |
| --- | :---: | :---: | :---: |
| Claude Code | ✓ | ✓ | ✓ |
| Codex | ✓ | ✓ | ✓ |
| opencode 1.x / 2.x | ✓ | ✓ | ✓ |
| Antigravity CLI (`agy`) | — | ✓ | ✓ |

Para compartir una conversación que empiece en Antigravity, conviértela primero a uno de los tres
agentes con sincronización nativa. Antigravity participa mediante conversión y copias locales vinculadas.

La conversión conserva tus mensajes y las respuestas. Las llamadas a herramientas pasan a texto;
las imágenes, el razonamiento interno y el historial anterior a una compactación no se trasladan
por completo. Cambiar de versión de opencode también puede requerir conversión.
Consulta los [detalles de conversión](docs/cli.es.md#convertir-entre-programas).

## Comandos de un vistazo

| Comando | Para qué sirve |
| --- | --- |
| `actx gui` | Abrir la interfaz de escritorio opcional. |
| `actx` / `actx ls` | Navegar por las conversaciones mantenidas / imprimir sus estados. |
| `actx mk <proyecto>` / `actx rm <proyecto>` | Crear o quitar un proyecto de actX. |
| `actx add` / `actx delete` | Elegir conversaciones que compartir o quitar del almacenamiento compartido. Conserva los originales locales. |
| `actx sync` / `actx sync --dry-run` | Sincronizar o consultar las operaciones propuestas. |
| `actx resume` / `actx cv --all` | Retomar una conversación / convertirla a otro agente. |
| `actx compare` / `actx resolve` | Comparar historiales y revisar la resolución de un conflicto. |
| `actx recoveries` / `actx restore` | Consultar versiones recuperables / restaurar un backup. |
| `actx clean` / `actx clean advanced` | Revisar candidatas a limpieza y poner las elegidas en cuarentena. |
| `actx auto on` / `actx auto off` | Activar o desactivar hooks y sincronización periódica. |
| `actx auto-add <proyecto>` | Configurar reglas de incorporación y exclusiones. |
| `actx profile setup` / `actx doctor` | Configurar las preferencias del equipo / diagnosticar la instalación. |

La [guía de la CLI](docs/cli.es.md) detalla los ámbitos, la retención, el autocompletado,
las actualizaciones y las opciones de cada comando.

## Revisión, recuperación y almacenamiento

actX recuerda las sincronizaciones anteriores para distinguir subidas, descargas y conflictos.
Comprueba la identidad y actividad de cada conversación antes de escribir, guarda backups al
sustituir historiales y utiliza bloqueos y publicación atómica en la carpeta compartida.
Las ediciones independientes quedan pendientes de resolución; las ampliaciones verificadas de
una respuesta sincronizada pueden continuar sin duplicar la pregunta original.

La limpieza mueve a cuarentena lo que seleccionas y protege copias compartidas, vinculadas y
conversaciones cuya actividad detecta. Recuperación también muestra restos reconocidos de publicaciones
interrumpidas. La retención opcional elimina copias **compartidas** que cumplan sus condiciones y
conserva los originales locales y los proyectos.

La carpeta compartida contiene conversaciones, incluidos prompts y salidas de comandos. Mantenla
privada: esos historiales pueden contener credenciales u otros datos sensibles. Con herramientas de
sincronización diferida, evita escribir a la vez sobre el mismo proyecto;
consulta los [detalles de almacenamiento](docs/cli.es.md#la-carpeta-compartida).

## Documentación

| Guía | Qué encontrarás |
| --- | --- |
| [Guía de la CLI](docs/cli.es.md) · [English guide](docs/cli.md) | Instalación, comandos, perfiles, General, automatización, conversión y almacenamiento. |
| [Guía de escritorio](docs/desktop.md) | Flujos de la GUI, catálogo local, recuperación y comprobaciones por plataforma. |
| [API JSON](docs/json-api.md) | Acciones, vistas previas, revisiones, confirmación, estados y códigos de salida. |
| [Historial de cambios](CHANGELOG.md) | Funciones incorporadas y límites de compatibilidad. |
| [Desarrollo](docs/cli.md#development) | Comprobaciones locales, entornos de integración y matriz de CI. |

actX lee formatos nativos que pueden cambiar con las versiones de cada agente.
La [guía de la CLI](docs/cli.es.md#limitaciones) recoge sus límites de formato y plataforma.

[MIT](LICENSE). Proyecto independiente. Los logos pertenecen a sus respectivos propietarios;
consulta la [procedencia de los recursos](assets/harnesses/README.md).
