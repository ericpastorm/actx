//! Interface text: English is the source, Spanish translations below. `tr!` picks the language.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

static SPANISH: AtomicBool = AtomicBool::new(false);

pub fn set_spanish(on: bool) {
    SPANISH.store(on, Ordering::Relaxed)
}

pub fn spanish() -> bool {
    SPANISH.load(Ordering::Relaxed)
}

/// `text` in the user's language, with `{name}` placeholders filled in.
pub fn translate(text: &str, args: &[(&str, String)]) -> String {
    let mut out = if spanish() {
        ES.get(text).copied().unwrap_or(text)
    } else {
        text
    }
    .to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// `tr!("{n} min ago", n = 5)`: interface text in the user's language.
#[macro_export]
macro_rules! tr {
    ($text:literal) => {
        $crate::i18n::translate($text, &[])
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::translate($text, &[$((stringify!($name), $value.to_string())),+])
    };
}

pub const HELP: &str = "actx (agent context): keep your Claude Code, opencode and Codex conversations in sync across\nmachines, grouped into projects, through any shared folder (a NAS, Syncthing, ...).\n\n  actx init [folder]   choose the shared folder (once per machine)\n  actx mk <project>    create a project (or open one) and pick conversations to save in it\n  actx sync [project]  push what you continued here and pull what's new (pick projects; --all)\n  actx ls [project]    projects and the state of each conversation (also: plain actx)\n  actx rm <project>    remove conversations from a project (remove them all to delete it)\n  actx cv [project]    copy a conversation into another tool (Claude Code, opencode, Codex, Antigravity)\n  actx auto [on|off]   sync by itself: when a conversation starts or ends, and every 10 min\n  actx restore         put back a conversation from a backup\n  actx doctor          check the shared folder, the tools and automatic sync\n  actx update          install the newest actx found in the shared folder\n\nConversations don't depend on folder paths. When pulling one, actx looks for its repo on this\nmachine by the first commit (other paths, clones and worktrees all work); otherwise it tries\nthe same path; otherwise it asks once and remembers. Run it inside the project and it goes\nthere. Paths inside the conversation are rewritten to this machine's.\n\nIf you continued a conversation on two machines, sync lets you merge them: you keep this one\nplus a summary of what was done on the other, and the other version is kept separately.\n\nPickers: type to filter, TAB marks several, Ctrl-A all, Enter confirms, Esc quits.\nLanguage: English or Spanish, from your locale; set ACTX_LANG=en|es or `actx init --lang`.\n";

const HELP_ES: &str = "actx (agent context): mantiene tus conversaciones de Claude Code, opencode y Codex al día\nentre tus PCs, agrupadas en proyectos, a través de cualquier carpeta compartida (un NAS, Syncthing...).\n\n  actx init [carpeta]   elige la carpeta compartida (una vez en cada PC)\n  actx mk <proyecto>    crea un proyecto (o abre uno) y eliges qué conversaciones guardar\n  actx sync [proyecto]  sube lo que has seguido aquí y baja lo nuevo (eliges proyectos; --all)\n  actx ls [proyecto]    proyectos y estado de cada conversación (también: actx a secas)\n  actx rm <proyecto>    quita conversaciones de un proyecto (si quitas todas, lo borra)\n  actx cv [proyecto]    copia una conversación a otro programa (Claude Code, opencode, Codex, Antigravity)\n  actx auto [on|off]    sincroniza solo: al abrir o cerrar una conversación y cada 10 min\n  actx restore          recupera una conversación de una copia de seguridad\n  actx doctor           comprueba la carpeta compartida, los programas y el modo auto\n  actx update           instala la última versión de actx que haya en la carpeta compartida\n\nLas conversaciones no dependen de la carpeta. Al bajar una, actx busca su repo en este PC por\nel primer commit (vale con otras rutas, clones y worktrees); si no, prueba la misma ruta; y si\ntampoco, te pregunta una vez y se acuerda. Si lo lanzas dentro del proyecto, va ahí. Las rutas\ndentro de la conversación se cambian a las de este PC.\n\nSi seguiste una conversación en dos PCs, sync te deja mezclarlas: sigues con la de aquí más un\nresumen de lo hecho en el otro, y la otra versión queda aparte.\n\nSelectores: escribe para filtrar, TAB marca varias, Ctrl-A todas, Enter confirma, Esc sale.\nIdioma: inglés o español, según tu sistema; o ACTX_LANG=en|es, o `actx init --lang`.\n";

pub static ES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        (HELP, HELP_ES),
        ("[y/N]", "[s/N]"),
        ("now", "ahora"),
        ("{n} min ago", "hace {n} min"),
        ("{n} h ago", "hace {n} h"),
        ("{n} d ago", "hace {n} d"),
        ("↓ new", "↓ nueva"),
        ("✓ synced", "✓ al día"),
        ("↑ push", "↑ subir"),
        ("↓ pull", "↓ bajar"),
        ("⚠ conflict", "⚠ conflicto"),
        ("(no text messages)", "(sin mensajes de texto)"),
        ("you", "tú"),
        ("(untitled)", "(sin título)"),
        (
            "Can't read the opencode database ({err}); skipping opencode",
            "No puedo leer la base de datos de opencode ({err}); me salto opencode",
        ),
        (
            "opencode is not installed on this machine",
            "opencode no está instalado en este PC",
        ),
        (
            "opencode couldn't export {sid}: {err}",
            "opencode no pudo exportar {sid}: {err}",
        ),
        (
            "opencode didn't accept the conversation: {err}",
            "opencode no aceptó la conversación: {err}",
        ),
        (
            "opencode couldn't delete {sid}: {err}",
            "opencode no pudo borrar {sid}: {err}",
        ),
        ("… ({n} more characters)", "… ({n} caracteres más)"),
        (
            "[Summary of the earlier part of the conversation]",
            "[Resumen de la parte anterior de la conversación]",
        ),
        (
            "[Conversation imported from {source}{model}. Tool calls appear as text.]",
            "[Conversación importada de {source}{model}. Las herramientas que usó salen como texto.]",
        ),
        (
            "[The first {n} messages were left out because they didn't fit.]",
            "[Se han omitido los {n} primeros mensajes porque no cabían.]",
        ),
        (
            "I don't know which model you use in opencode: open opencode once and try again",
            "no sé qué modelo usas en opencode: abre opencode una vez y vuelve a probar",
        ),
        (
            "I don't know which folder it goes in on this machine",
            "no sé en qué carpeta va en este PC",
        ),
        (
            "it has no text messages to convert",
            "no tiene mensajes de texto que convertir",
        ),
        ("{path} doesn't exist on this machine", "{path} no existe en este PC"),
        ("{title} (from {source})", "{title} (de {source})"),
        ("You", "Tú"),
        ("Agent", "Agente"),
        (
            "[Meanwhile, this conversation continued on {host}. This is what happened there:]",
            "[Mientras tanto, esta conversación siguió en {host}. Esto es lo que se hizo allí:]",
        ),
        (
            "(no new messages on {host}: keeping this one)",
            "(en {host} no había mensajes nuevos: me quedo con la de aquí)",
        ),
        ("{title} (branch from {host})", "{title} (rama de {host})"),
        (
            "Got it: I'm taking into account what was done on {host}, and we continue from here.",
            "Entendido: tengo en cuenta lo que se hizo en {host} y seguimos desde aquí.",
        ),
        (
            "+ summary of what was done on {host}; its version is kept as «{title}»",
            "+ resumen de lo hecho en {host}; su versión queda como «{title}»",
        ),
        ("uncommitted changes", "cambios sin commitear"),
        ("{n} unpushed commit(s)", "{n} commit(s) sin push"),
        ("● here: ", "● aquí: "),
        ("✎ other folder (type the path)", "✎ otra carpeta (escribir la ruta)"),
        (
            "Where does «{title}» go? On {host} it was in {path}",
            "¿Dónde va «{title}»? En {host} estaba en {path}",
        ),
        ("Path:", "Ruta:"),
        ("{path} doesn't exist", "{path} no existe"),
        (
            "When it was pushed, {host} had uncommitted changes: they're not here.",
            "Al subirla, {host} tenía cambios sin commitear: aquí no están.",
        ),
        (
            "When it was pushed, {host} had {n} unpushed commit(s).",
            "Al subirla, {host} tenía {n} commit(s) sin push.",
        ),
        (
            "{repo}: {host} was on {b1} @ {h1}, you're on {b2} @ {h2}. Sync git before continuing.",
            "{repo}: {host} estaba en {b1} @ {h1}, aquí estás en {b2} @ {h2}. Sincroniza git antes de seguir.",
        ),
        (
            "{name} is zstd-compressed and this Python ({version}) can't read it (needs 3.14+)",
            "{name} está en zstd y este Python ({version}) no puede leerlo (necesita 3.14+)",
        ),
        (
            "No shared folder configured yet. Run: {cmd}",
            "Aún no hay carpeta compartida. Ejecuta: {cmd}",
        ),
        (
            "Can't reach the shared folder ({err}): using only what's on this machine",
            "No llego a la carpeta compartida ({err}): solo uso lo que hay en este PC",
        ),
        (
            "Can't reach the shared folder ({root}): {err}.",
            "No llego a la carpeta compartida ({root}): {err}.",
        ),
        (
            "If it's a network share, check that it's mounted (away from home you may need a VPN).",
            "Si es una carpeta de red, comprueba que esté montada (fuera de casa puede que necesites una VPN).",
        ),
        (
            "There's a newer actx in the shared folder: {cmd}",
            "Hay una versión más nueva de actx en la carpeta compartida: {cmd}",
        ),
        (
            "actx needs fzf (e.g. sudo pacman -S fzf)",
            "actx necesita fzf (p. ej. sudo pacman -S fzf)",
        ),
        (
            "in {project} · pushed from {host} {ago}",
            "en {project} · subida desde {host} {ago}",
        ),
        ("folder there: {path}", "carpeta allí: {path}"),
        ("git when pushed: ", "git al subir: "),
        ("(no preview)", "(sin vista previa)"),
        ("{path} · last activity {ago}", "{path} · última actividad {ago}"),
        (
            "{repo}: {state}. The conversation travels, the code doesn't: commit and push.",
            "{repo}: {state}. La conversación viaja, el código no: haz commit y push.",
        ),
        ("Usage: {cmd}", "Uso: {cmd}"),
        ("✓ already used by actx", "✓ ya la usa actx"),
        (
            "Where should actx keep your conversations? A folder all your machines can reach",
            "¿Dónde guarda actx tus conversaciones? Una carpeta a la que lleguen todos tus PCs",
        ),
        ("Can't create {path}: {err}", "No puedo crear {path}: {err}"),
        ("Shared folder: {path}", "Carpeta compartida: {path}"),
        (
            "It already has {n} project(s): {names}",
            "Ya tiene {n} proyecto(s): {names}",
        ),
        (
            "Conversations include command output and may contain secrets (API keys, tokens). Keep this folder private.",
            "Las conversaciones incluyen salidas de comandos y pueden llevar secretos (claves de API, tokens). Que esta carpeta sea privada.",
        ),
        ("Next: {cmd}", "Ahora: {cmd}"),
        ("Project name [{default}]:", "Nombre del proyecto [{default}]:"),
        (
            "Invalid name: use letters, numbers, '-', '_' or '.'",
            "Nombre no válido: usa letras, números, '-', '_' o '.'",
        ),
        ("Project {name} created", "Proyecto {name} creado"),
        (
            "No Claude Code, opencode or Codex conversations found on this machine",
            "No encuentro conversaciones de Claude Code, opencode ni Codex en este PC",
        ),
        ("in {project}", "en {project}"),
        ("open now", "abierta ahora"),
        (
            "{name} · ★ = looks like this project · TAB marks several · Enter saves",
            "{name} · ★ = parecen de este proyecto · TAB marca varias · Enter guarda",
        ),
        (
            "«{title}» has changes in the shared folder (from {host}) that aren't here. Overwrite them?",
            "«{title}» tiene cambios en la carpeta compartida (de {host}) que aquí no están. ¿Sobrescribirlos?",
        ),
        ("(moved from {project})", "(movida desde {project})"),
        ("On your other machines: {cmd}", "En tus otros PCs: {cmd}"),
        ("{project}: nothing to sync", "{project}: nada que sincronizar"),
        (
            "Sync · ● = where you are · TAB marks several · Ctrl-A all · Enter confirms",
            "Sincronizar · ● = donde estás · TAB marca varios · Ctrl-A todos · Enter confirma",
        ),
        (
            "No conversations in the shared folder yet. Start with: {cmd}",
            "Aún no hay conversaciones en la carpeta compartida. Empieza con: {cmd}",
        ),
        ("All in sync ({n} conversations)", "Todo al día ({n} conversaciones)"),
        (
            "{project} · «{title}» changed here and on {host} since the last sync.",
            "{project} · «{title}» ha cambiado aquí y en {host} desde la última vez.",
        ),
        (
            "merge = keep this one + a summary of what was done on {host}; {host}'s version is kept separately",
            "mezclar = sigues con la de aquí + un resumen de lo hecho en {host}; la de {host} queda aparte",
        ),
        (
            "What now? [m]erge / [l]ocal / [r]emote ({host}) / [s]kip:",
            "¿Qué hago? [m]ezclar / [a]quí / [o]tro ({host}) / [s]altar:",
        ),
        ("skipped", "saltada"),
        (
            "{project} · «{title}»: I don't know which folder it goes in on this machine; skipping",
            "{project} · «{title}»: no sé en qué carpeta va en este PC; la salto",
        ),
        (
            "{title}: couldn't merge ({err}); nothing else was touched",
            "{title}: no pude mezclarla ({err}); no se ha tocado nada más",
        ),
        ("{n} in sync", "{n} al día"),
        (
            "{n} pending in projects you didn't pick",
            "{n} pendiente(s) en proyectos que no has elegido",
        ),
        ("To continue:", "Para seguir:"),
        (
            "Project {name} doesn't exist. Create it with: {cmd}",
            "No existe el proyecto {name}. Créalo con: {cmd}",
        ),
        (
            "No projects yet. Create one with: {cmd}",
            "Aún no hay proyectos. Crea uno con: {cmd}",
        ),
        ("{n} conversation(s)", "{n} conversación(es)"),
        ("(empty)", "(vacío)"),
        ("{path} on {host}", "{path} en {host}"),
        ("There are pending changes: {cmd}", "Hay cambios pendientes: {cmd}"),
        ("Project {name} doesn't exist", "No existe el proyecto {name}"),
        ("{name} is empty. Delete it?", "{name} está vacío. ¿Borrarlo?"),
        ("Project {name} deleted", "Proyecto {name} borrado"),
        (
            "Remove from {name} (nothing is deleted on your machines) · TAB marks several",
            "Quitar de {name} (en tus PCs no se borra nada) · TAB marca varias",
        ),
        ("Delete the whole project {name}?", "¿Borrar el proyecto {name} entero?"),
        ("Remove {n} from {name}?", "¿Quitar {n} de {name}?"),
        ("Removed {n}", "Quitadas {n}"),
        (
            "No conversations in project {name}",
            "No hay conversaciones en el proyecto {name}",
        ),
        (
            "No Claude Code, opencode, Codex or Antigravity conversations found",
            "No encuentro conversaciones de Claude Code, opencode, Codex ni Antigravity",
        ),
        (
            "There's no other tool to convert to: install Claude Code, opencode, Codex or Antigravity",
            "No hay otro programa al que convertirla: instala Claude Code, opencode, Codex o Antigravity",
        ),
        (
            "Antigravity conversations can be converted, but they don't sync yet",
            "Las conversaciones de Antigravity se pueden convertir, pero aún no se sincronizan",
        ),
        ("Convert to which tool?", "¿A qué programa la convierto?"),
        ("converts, doesn't sync yet", "se convierte, aún no se sincroniza"),
        (
            "{title}: it's already a {tool} conversation",
            "{title}: ya es una conversación de {tool}",
        ),
        (
            "can't merge between opencode 1.x and 2.x: pick this one or the other one",
            "no se pueden mezclar opencode 1.x y 2.x: elige la de aquí o la del otro",
        ),
        (
            "«{title}» comes from opencode {a}.x and here you have opencode {b}.x: converted (tool calls become text)",
            "«{title}» viene de opencode {a}.x y aquí tienes opencode {b}.x: convertida (las herramientas pasan a texto)",
        ),
        (
            "Convert · creates a copy in another tool, the original is left untouched · TAB marks several",
            "Convertir · se crea una copia en otro programa, la original no se toca · TAB marca varias",
        ),
        ("Reading «{title}» ({size})…", "Leyendo «{title}» ({size})…"),
        ("(also saved in {project})", "(también guardada en {project})"),
        (
            "(couldn't save it in {project}: {err})",
            "(no pude guardarla en {project}: {err})",
        ),
        (
            "actx was installed with your package manager: update it there (e.g. {cmd}).",
            "actx está instalado con tu gestor de paquetes: actualízalo con él (p. ej. {cmd}).",
        ),
        (
            "This is a git checkout: update it with {cmd}.",
            "Esto es una copia de git: actualízala con {cmd}.",
        ),
        (
            "There was no actx in the shared folder: uploaded this machine's copy",
            "No había actx en la carpeta compartida: he subido el de este PC",
        ),
        ("You already have the latest version", "Ya tienes la última versión"),
        (
            "This machine's version is newer than the shared one: uploaded it",
            "La versión de este PC es más nueva que la compartida: la he subido",
        ),
        (
            "The shared copy is broken ({err}); not installing it",
            "La copia compartida está rota ({err}); no la instalo",
        ),
        ("Can't write {path}: {err}", "No puedo escribir {path}: {err}"),
        (
            "actx updated from the shared folder (saved {ago})",
            "actx actualizado desde la carpeta compartida (guardada {ago})",
        ),
        ("cancelled", "cancelado"),
        ("project", "proyecto"),
        ("folder", "carpeta"),
        ("choose the shared folder", "elegir la carpeta compartida"),
        (
            "the shared folder (otherwise a picker shows up)",
            "la carpeta compartida (sin esto, sale un selector)",
        ),
        ("interface language", "idioma de la interfaz"),
        (
            "create a project / save conversations in it",
            "crear un proyecto / guardar conversaciones en él",
        ),
        (
            "push what you continued here and pull what's new",
            "subir lo seguido aquí y bajar lo nuevo",
        ),
        (
            "only this project (otherwise a picker shows up)",
            "solo este proyecto (sin esto, sale un selector)",
        ),
        ("all projects, no questions", "todos los proyectos, sin preguntar"),
        ("show projects and their state", "ver los proyectos y su estado"),
        (
            "remove conversations from a project",
            "quitar conversaciones de un proyecto",
        ),
        (
            "copy a conversation into another tool",
            "copiar una conversación a otro programa",
        ),
        ("only this project's", "solo las de este proyecto"),
        (
            "install the newest actx from the shared folder",
            "instalar la última versión de actx de la carpeta compartida",
        ),
        ("show the version", "ver la versión"),
        (
            "actx: «{title}» has newer messages from {host} ({ago}). To get them, exit, run actx sync and resume it again.",
            "actx: «{title}» tiene mensajes más nuevos de {host} ({ago}). Para tenerlos, sal, ejecuta actx sync y vuelve a abrirla.",
        ),
        (
            "actx: «{title}» also changed on {host} ({ago}). Exit and run actx sync to merge both versions.",
            "actx: «{title}» también ha cambiado en {host} ({ago}). Sal y ejecuta actx sync para mezclar las dos versiones.",
        ),
        (
            "{path} isn't valid JSON ({err}): not touching it",
            "{path} no es un JSON válido ({err}): no lo toco",
        ),
        (
            "{path} isn't a JSON object: not touching it",
            "{path} no es un objeto JSON: no lo toco",
        ),
        (
            "This is a development build: the hooks will run {path} until you turn this on again from an installed actx.",
            "Esto es una build de desarrollo: los hooks ejecutarán {path} hasta que lo vuelvas a activar desde un actx instalado.",
        ),
        (
            "Codex asks you to review new hooks once: accept them in /hooks",
            "Codex te pide revisar los hooks nuevos una vez: acéptalos en /hooks",
        ),
        ("{tool}: hooks in {path}", "{tool}: hooks en {path}"),
        (
            "Every {n} min ({name}): this covers opencode too",
            "Cada {n} min ({name}): esto cubre también opencode",
        ),
        (
            "Couldn't start the timer ({name}). Without it, opencode only syncs when you run actx sync. With cron: {cmd}",
            "No he podido poner en marcha el temporizador ({name}). Sin él, opencode solo se sincroniza cuando ejecutas actx sync. Con cron: {cmd}",
        ),
        ("The timer: {err}", "El temporizador: {err}"),
        (
            "Automatic sync is on: when a conversation starts or ends, and every {n} minutes, actx pushes what changed and pulls what's new. It never pulls over a conversation that's open in an agent, and leaves conflicts and new folders for {cmd}.",
            "Sincronización automática activada: al abrir o cerrar una conversación, y cada {n} minutos, actx sube lo que ha cambiado y baja lo nuevo. Nunca baja encima de una conversación abierta en un agente, y deja los conflictos y las carpetas nuevas para {cmd}.",
        ),
        ("Log: {path}", "Registro: {path}"),
        ("{tool}: hooks removed", "{tool}: hooks quitados"),
        ("Timer removed ({name})", "Temporizador quitado ({name})"),
        (
            "Automatic sync wasn't on",
            "La sincronización automática no estaba activada",
        ),
        ("{tool} hooks", "hooks de {tool}"),
        ("timer ({name})", "temporizador ({name})"),
        ("Automatic sync: {state}", "Sincronización automática: {state}"),
        ("on", "activada"),
        ("off", "desactivada"),
        ("Last runs ({path}):", "Últimas ejecuciones ({path}):"),
        ("Turn it on with {cmd}", "Actívala con {cmd}"),
        (
            "{host} is writing to the shared folder (since {time}): waiting…",
            "{host} está escribiendo en la carpeta compartida (desde las {time}): esperando…",
        ),
        (
            "The shared folder is still busy ({host}, since {time}). Try again in a while; if nothing is running there, delete {path}",
            "La carpeta compartida sigue ocupada ({host}, desde las {time}). Prueba en un rato; si allí no se está ejecutando nada, borra {path}",
        ),
        (
            "Can't write to the shared folder: {err}",
            "No puedo escribir en la carpeta compartida: {err}",
        ),
        (
            "«{title}» changed in the shared folder meanwhile: next time",
            "«{title}» ha cambiado en la carpeta compartida mientras tanto: la próxima vez",
        ),
        (
            "«{title}» changed in the shared folder meanwhile: run actx sync again",
            "«{title}» ha cambiado en la carpeta compartida mientras tanto: vuelve a ejecutar actx sync",
        ),
        (
            "«{title}» is open in {tool} right now: what you do there would be lost. Replace it anyway?",
            "«{title}» está abierta ahora mismo en {tool}: lo que hagas allí se perdería. ¿La reemplazo igualmente?",
        ),
        (
            "{project} · «{title}» changed here and on {host}: run actx sync to decide",
            "{project} · «{title}» ha cambiado aquí y en {host}: ejecuta actx sync para decidir",
        ),
        (
            "«{title}» is open in {tool}: it'll be pulled later",
            "«{title}» está abierta en {tool}: se bajará más tarde",
        ),
        (
            "it's open in {tool} right now: close it first",
            "está abierta ahora mismo en {tool}: ciérrala antes",
        ),
        (
            "No backups yet: actx makes one before replacing a conversation on this machine.",
            "Aún no hay copias de seguridad: actx hace una antes de reemplazar una conversación en este PC.",
        ),
        ("backed up {ago}", "copia de {ago}"),
        (
            "Backups, newest first (kept {n} days) · Enter restores one",
            "Copias de seguridad, las más nuevas primero (se guardan {n} días) · Enter restaura una",
        ),
        (
            "Replace «{title}» on this machine with the copy from {when}? (the current one is backed up first)",
            "¿Reemplazo «{title}» en este PC por la copia del {when}? (antes se guarda una copia de la actual)",
        ),
        ("Restore «{title}» from {when}?", "¿Restauro «{title}» del {when}?"),
        ("Restored «{title}»", "«{title}» restaurada"),
        (
            "If it's in a project, the next sync pushes it (or asks, if it changed elsewhere too).",
            "Si está en un proyecto, el próximo sync la sube (o te pregunta, si también ha cambiado en otro sitio).",
        ),
        ("shared folder", "carpeta compartida"),
        ("not configured: run {cmd}", "sin configurar: ejecuta {cmd}"),
        (
            "{path}: can't reach it (if it's a network share: is it mounted?)",
            "{path}: no llego (si es una unidad de red: ¿está montada?)",
        ),
        (
            "{path} · {p} project(s), {n} conversation(s)",
            "{path} · {p} proyecto(s), {n} conversación(es)",
        ),
        ("can't write to it", "no puedo escribir en ella"),
        ("lock", "bloqueo"),
        (
            "{host} is writing to it (since {ago}); if nothing is running there, delete {path}",
            "{host} está escribiendo en ella ({ago}); si allí no se está ejecutando nada, borra {path}",
        ),
        ("config: {path}", "configuración: {path}"),
        ("Tools", "Programas"),
        ("not found: actx needs it", "no está: actx lo necesita"),
        ("not installed", "no instalado"),
        ("{version} · {n} conversation(s)", "{version} · {n} conversación(es)"),
        (
            "the CLI is {a}.x but its database is from {b}.x",
            "el CLI es {a}.x pero su base de datos es de {b}.x",
        ),
        ("Automatic sync", "Sincronización automática"),
        ("last: {line}", "última: {line}"),
        ("turn it on with actx auto on", "actívala con actx auto on"),
        ("agents running now: {list}", "agentes abiertos ahora: {list}"),
        ("This machine", "Este PC"),
        ("state", "estado"),
        (
            "{n} conversation(s) tracked · {path}",
            "{n} conversación(es) seguidas · {path}",
        ),
        ("backups", "copias"),
        (
            "{n} · {size} · kept {days} days (actx restore)",
            "{n} · {size} · se guardan {days} días (actx restore)",
        ),
        ("All good", "Todo bien"),
        ("{n} problem(s) above", "{n} problema(s) arriba"),
        (
            "what automatic sync runs: no questions, only what's safe",
            "lo que ejecuta el modo auto: sin preguntas, solo lo seguro",
        ),
        (
            "automatic sync: hooks in Claude Code and Codex, and a timer",
            "sincronización automática: hooks en Claude Code y Codex, y un temporizador",
        ),
        (
            "turn it on or off (otherwise: how it's doing)",
            "activarla o desactivarla (si no: cómo va)",
        ),
        (
            "put back a conversation from a backup",
            "recuperar una conversación de una copia de seguridad",
        ),
        ("check that everything is in place", "comprobar que todo está en orden"),
        (
            "shell completions (e.g. actx completions bash > ~/.local/share/bash-completion/completions/actx)",
            "autocompletado para tu shell (p. ej. actx completions bash > ~/.local/share/bash-completion/completions/actx)",
        ),
    ])
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tr!("...") in the source has a Spanish translation, with the same placeholders.
    #[test]
    fn every_interface_string_is_translated() {
        let call = regex::Regex::new(r#"\btr!\(\s*"((?:[^"\\]|\\.)*)""#).unwrap();
        let placeholder = regex::Regex::new(r"\{(\w+)\}").unwrap();
        let names = |s: &str| {
            placeholder
                .captures_iter(s)
                .map(|c| c[1].to_string())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = 0;
        for file in std::fs::read_dir(src).unwrap() {
            let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
            let text: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for cap in call.captures_iter(&text) {
                let english: String = serde_json::from_str(&format!("\"{}\"", &cap[1])).unwrap();
                let spanish = ES
                    .get(english.as_str())
                    .unwrap_or_else(|| panic!("untranslated: {english}"));
                assert_eq!(names(&english), names(spanish), "placeholders differ: {english}");
                seen += 1;
            }
        }
        assert!(seen > 100);
        assert_eq!(names(HELP), names(HELP_ES));
    }
}
