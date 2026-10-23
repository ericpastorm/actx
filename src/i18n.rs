//! English source text, Spanish and Simplified Chinese catalogs. `tr!` picks the language.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Language {
    En,
    Es,
    ZhCn,
}
static LANGUAGE: AtomicU8 = AtomicU8::new(0);
pub fn parse(code: &str) -> Language {
    let code = code.to_lowercase().replace('_', "-");
    let code = code
        .split('.')
        .next()
        .unwrap_or_default()
        .split('@')
        .next()
        .unwrap_or_default();
    if code == "es" || code.starts_with("es-") {
        Language::Es
    } else if code == "zh" || code == "zh-cn" || code == "zh-sg" || code.starts_with("zh-hans") {
        Language::ZhCn
    } else {
        Language::En
    }
}
pub fn set_language(code: &str) {
    LANGUAGE.store(parse(code) as u8, Ordering::Relaxed);
}
pub fn language() -> Language {
    match LANGUAGE.load(Ordering::Relaxed) {
        1 => Language::Es,
        2 => Language::ZhCn,
        _ => Language::En,
    }
}
pub fn code() -> &'static str {
    match language() {
        Language::En => "en",
        Language::Es => "es",
        Language::ZhCn => "zh-CN",
    }
}
pub static ZH_CN: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| serde_json::from_str(include_str!("zh_cn.json")).expect("valid Chinese catalog"));

/// `text` in the user's language, with `{name}` placeholders filled in.
pub fn translate(text: &str, args: &[(&str, String)]) -> String {
    let mut out = match language() {
        Language::En => text,
        Language::Es => ES.get(text).copied().unwrap_or(text),
        Language::ZhCn => ZH_CN
            .get(if text == HELP { "__help__" } else { text })
            .map(String::as_str)
            .unwrap_or(text),
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

pub const HELP: &str = "actx (agent context): keep your Claude Code, opencode and Codex conversations in sync across\nmachines, grouped into projects, through any shared folder (a NAS, Syncthing, ...).\n\n  actx setup [folder]  guided setup: shared folder, harnesses and retention\n  actx init [folder]   choose only the shared folder\n  actx profile        show your profile (use profile setup to configure it)\n  actx browse         compact conversation browser (also: plain actx)\n  actx resume         sync a conversation and choose where to open it\n  actx mk <project>    bind this project folder and choose conversations\n  actx add [project]   save conversations from this project (or General)\n  actx delete [project] delete selected saved conversations; keep local originals\n  actx sync [project]  push what you continued here and pull what's new (pick projects; --all)\n  actx sync --dry-run  preview operations without changing data\n  actx ls [project]    compact conversation list and harness states\n  actx rm <project>    delete a project and its saved copies; keep local originals\n  actx mv [target]     move saved conversations; --project OLD NEW renames a project\n  actx cv [project]    convert conversations from this folder/project (--all: all folders)\n  actx clean           review unused local conversations and quarantine them safely\n  actx clean purge     permanently delete chosen quarantine entries\n  actx auto [on|off]   sync by itself: when a conversation starts or ends, and every 10 min\n  actx restore         put back a conversation from a backup\n  actx doctor          check the shared folder, the tools and automatic sync\n  actx update          install the newest actx found in the shared folder\n\nConversations don't depend on folder paths. When pulling one, actx looks for its repo on this\nmachine by the first commit (other paths, clones and worktrees all work); otherwise it tries\nthe same path; otherwise it asks once and remembers. Run it inside the project and it goes\nthere. Paths inside the conversation are rewritten to this machine's.\n\nGeneral conversations ask for a destination folder on each machine and remember it per conversation.\nUse --general with sync, ls, cv or delete to select only General.\n\nIf you continued a conversation on two machines, sync lets you merge them: you keep this one\nplus a summary of what was done on the other, and the other version is kept separately.\n\nPickers: type to filter, TAB marks several, Ctrl-A all, Enter confirms, Esc quits.\nLanguage: English, Spanish or Simplified Chinese, from your locale; set ACTX_LANG=en|es|zh-CN or `actx init --lang`.\n";

const HELP_ES: &str = "actx (agent context): mantiene tus conversaciones de Claude Code, opencode y Codex al día\nentre tus PCs, agrupadas en proyectos, a través de cualquier carpeta compartida (un NAS, Syncthing...).\n\n  actx setup [carpeta]  asistente: carpeta compartida, harnesses y retención\n  actx init [carpeta]   elige solo la carpeta compartida\n  actx profile         muestra tu perfil (profile setup para configurarlo)\n  actx browse          explorador compacto (también: actx a secas)\n  actx resume          sincroniza una conversación y elige dónde abrirla\n  actx mk <proyecto>    vincula esta carpeta y elige conversaciones\n  actx add [proyecto]   guarda conversaciones del proyecto actual (o en General)\n  actx delete [proyecto] borra conversaciones guardadas; conserva originales locales\n  actx sync [proyecto]  sube lo que has seguido aquí y baja lo nuevo (eliges proyectos; --all)\n  actx sync --dry-run   previsualiza las operaciones sin modificar datos\n  actx ls [proyecto]    lista compacta de conversaciones y sus harnesses\n  actx rm <proyecto>    borra un proyecto y sus copias guardadas; conserva originales locales\n  actx mv [destino]     mueve conversaciones guardadas; --project OLD NEW renombra un proyecto\n  actx cv [proyecto]    convierte conversaciones de esta carpeta/proyecto (--all: todas)\n  actx clean            revisa conversaciones locales sin usar y las pone a salvo en cuarentena\n  actx clean purge      borra definitivamente las entradas elegidas de la cuarentena\n  actx auto [on|off]    sincroniza solo: al abrir o cerrar una conversación y cada 10 min\n  actx restore          recupera una conversación de una copia de seguridad\n  actx doctor           comprueba la carpeta compartida, los programas y el modo auto\n  actx update           instala la última versión de actx que haya en la carpeta compartida\n\nLas conversaciones no dependen de la carpeta. Al bajar una, actx busca su repo en este PC por\nel primer commit (vale con otras rutas, clones y worktrees); si no, prueba la misma ruta; y si\ntampoco, te pregunta una vez y se acuerda. Si lo lanzas dentro del proyecto, va ahí. Las rutas\ndentro de la conversación se cambian a las de este PC.\n\nLas conversaciones de General preguntan la carpeta de destino en cada PC y la recuerdan por conversación.\nUsa --general con sync, ls, cv o delete para seleccionar solo General.\n\nSi seguiste una conversación en dos PCs, sync te deja mezclarlas: sigues con la de aquí más un\nresumen de lo hecho en el otro, y la otra versión queda aparte.\n\nSelectores: escribe para filtrar, TAB marca varias, Ctrl-A todas, Enter confirma, Esc sale.\nIdioma: inglés, español o chino simplificado, según tu sistema; o ACTX_LANG=en|es|zh-CN, o `actx init --lang`.\n";

pub static ES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("Wordmark style", "Estilo del wordmark"),
        ("Animate the X", "Animar la X"),
        ("Shared publication", "Publicación compartida"),
        ("Interrupted publications: {n}", "Publicaciones interrumpidas: {n}"),
        (
            "Review shared recovery with actx recoveries",
            "Revisa la recuperación compartida con actx recoveries",
        ),
        (
            "Cleanup stopped; completed moves remain recoverable",
            "Limpieza detenida; los movimientos completados se pueden recuperar",
        ),
        (
            "The current shared version will be preserved as another recovery entry",
            "La versión compartida actual se conservará como otra entrada de recuperación",
        ),
        (
            "Shared recovery unavailable; local backups remain available",
            "La recuperación compartida no está disponible; las copias locales siguen disponibles",
        ),
        ("Interrupted publication", "Publicación interrumpida"),
        (
            "A shared-folder writer is active; recovery is deferred",
            "Hay una escritura activa en la carpeta compartida; la recuperación queda pendiente",
        ),
        (
            "Incomplete publication; its remaining files are preserved",
            "Publicación incompleta; se conservan los archivos que quedan",
        ),
        (
            "Review the shared recovery; local histories stay unchanged",
            "Revisa la recuperación compartida; los historiales locales se conservan",
        ),
        ("Restore shared publication", "Recuperar publicación compartida"),
        ("Previous shared version", "Versión compartida anterior"),
        ("Staged shared version", "Versión compartida preparada"),
        ("Shared writer active", "Escritura compartida activa"),
        ("Incomplete publication", "Publicación incompleta"),
        ("Source", "Origen"),
        ("Recovery preview", "Vista previa de recuperación"),
        ("Clean local conversations", "Limpiar conversaciones locales"),
        ("Analyze local conversations", "Analizar conversaciones locales"),
        ("Move selected to quarantine", "Mover las seleccionadas a cuarentena"),
        (
            "Stopping after the current conversation",
            "Se detendrá al terminar la conversación actual",
        ),
        (
            "Selected local conversations move to recoverable quarantine",
            "Las conversaciones locales seleccionadas se moverán a una cuarentena recuperable",
        ),
        ("Operation cancelled", "Operación cancelada"),
        (
            "Project data changed; refreshing the catalog",
            "Los datos del proyecto han cambiado; actualizando el catálogo",
        ),
        (
            "Catalog check failed; showing the last known data",
            "No se pudo comprobar el catálogo; se muestran los últimos datos conocidos",
        ),
        (
            "Checking conversations in the background",
            "Comprobando conversaciones en segundo plano",
        ),
        (
            "Last known data; verification pending",
            "Últimos datos conocidos; pendientes de comprobar",
        ),
        ("Last checked {when}", "Última comprobación: {when}"),
        (
            "Configure automatic additions for a project",
            "Configurar la incorporación automática de un proyecto",
        ),
        ("Choose a project", "Elige un proyecto"),
        (
            "More than one project matches; choose manually",
            "Coincide con varios proyectos; elige manualmente",
        ),
        (
            "Add a new conversation from the enabled project rule",
            "Añadir una conversación nueva según la regla del proyecto",
        ),
        (
            "Finding new conversations in enabled projects",
            "Buscando conversaciones nuevas en proyectos habilitados",
        ),
        ("Sync project scope", "Sincronizar proyectos seleccionados"),
        ("Automatic additions", "Incorporación automática"),
        (
            "Manual by default. This shared rule applies on each computer with a resolved project folder.",
            "Manual por defecto. Esta regla compartida se aplica en cada PC que tenga localizada la carpeta del proyecto.",
        ),
        (
            "Add new conversations on project sync",
            "Añadir conversaciones nuevas al sincronizar el proyecto",
        ),
        ("Include subfolders", "Incluir subcarpetas"),
        (
            "Include other clones of this repository",
            "Incluir otros clones de este repositorio",
        ),
        (
            "Excluded relative folders, one pattern per line (scratch/**)",
            "Carpetas relativas excluidas, un patrón por línea (scratch/**)",
        ),
        (
            "Excluded titles, one pattern per line (*private*)",
            "Títulos excluidos, un patrón por línea (*privado*)",
        ),
        (
            "Disabling keeps existing synchronized conversations. Deleted conversations stay excluded until added manually.",
            "Desactivar conserva las conversaciones ya sincronizadas. Las borradas quedan excluidas hasta que las añadas manualmente.",
        ),
        (
            "The rule takes effect on the next project sync. Previewed candidates can change.",
            "La regla se aplica en la próxima sincronización del proyecto. Los candidatos pueden cambiar.",
        ),
        ("Delete project", "Borrar proyecto"),
        ("Recovery", "Recuperación"),
        ("Compare histories", "Comparar historiales"),
        ("Enable automatic sync", "Activar sincronización automática"),
        ("Disable automatic sync", "Desactivar sincronización automática"),
        ("Diagnostics", "Diagnóstico"),
        (
            "Review both versions; the alternative is preserved when resolving",
            "Revisa ambas versiones; al resolver se conserva la alternativa",
        ),
        (
            "This conversation contains a compacted history",
            "Esta conversación contiene un historial compactado",
        ),
        (
            "Preview shortened; the complete original is preserved",
            "Vista previa recortada; se conserva el original completo",
        ),
        ("Merge preview", "Vista previa de la mezcla"),
        (
            "Choose a backup or quarantined conversation to review its recovery",
            "Elige una copia de seguridad o conversación en cuarentena para revisar su recuperación",
        ),
        ("No recoverable conversations", "No hay conversaciones recuperables"),
        ("Backup", "Copia de seguridad"),
        ("Quarantine", "Cuarentena"),
        ("Shared folder available", "Carpeta compartida disponible"),
        ("Linked copy", "Copia vinculada"),
        ("Timer running", "Temporizador activo"),
        ("Timer stopped", "Temporizador detenido"),
        ("Saved copies", "Copias guardadas"),
        ("Keep local", "Conservar local"),
        ("Keep shared", "Conservar compartida"),
        ("Merge both", "Mezclar ambas"),
        ("Repair linked copy", "Reparar copia vinculada"),
        ("Choose what to do", "Elige qué hacer"),
        (
            "Compare conversation histories",
            "Comparar historiales de una conversación",
        ),
        (
            "Resolve a conversation conflict",
            "Resolver un conflicto de conversación",
        ),
        ("Browse recoverable conversations", "Ver conversaciones recuperables"),
        (
            "Finish the current agent turn before synchronizing",
            "Termina el turno actual del agente antes de sincronizar",
        ),
        (
            "Compacted history repaired; the previous native copy was preserved",
            "Historial compactado reparado; la copia nativa anterior se ha conservado",
        ),
        (
            "Enable the target harness in Settings before resuming",
            "Activa el harness de destino en Configuración antes de retomar",
        ),
        ("Add conversations", "Añadir conversaciones"),
        (
            "ACTX_ROOT overrides the shared folder in settings",
            "ACTX_ROOT tiene prioridad sobre la carpeta configurada",
        ),
        ("Action finished", "Acción terminada"),
        ("Action needs attention", "La acción necesita atención"),
        ("Add", "Añadir"),
        ("All projects", "Todos los proyectos"),
        ("Apply", "Aplicar"),
        ("Cancel", "Cancelar"),
        ("Choose folder", "Elegir carpeta"),
        (
            "Choose the destination and review changes before applying",
            "Elige el destino y revisa los cambios antes de aplicarlos",
        ),
        ("Confirm changes", "Confirmar cambios"),
        ("Convert", "Convertir"),
        ("Create project", "Crear proyecto"),
        (
            "Create the project, then select its conversations from the local list",
            "Crea el proyecto y después elige sus conversaciones en la lista local",
        ),
        ("Delete", "Borrar"),
        ("Destination project", "Proyecto de destino"),
        ("Language", "Idioma"),
        ("Local", "Local"),
        ("Name", "Nombre"),
        ("No changes were made", "No se han hecho cambios"),
        ("No pending operations", "No hay operaciones pendientes"),
        (
            "Only saved copies are deleted",
            "Solo se borran las copias compartidas guardadas",
        ),
        (
            "Only selected conversations are shared",
            "Solo se comparten las conversaciones que eliges",
        ),
        ("Open the desktop interface", "Abrir la interfaz de escritorio"),
        ("Operation in progress", "Operación en curso"),
        ("Preview", "Vista previa"),
        (
            "Read one structured request from stdin",
            "Leer una petición estructurada de la entrada estándar",
        ),
        ("Refresh", "Actualizar"),
        ("Results", "Resultados"),
        ("Resume", "Retomar"),
        ("Save settings", "Guardar configuración"),
        ("Search conversations", "Buscar conversaciones"),
        ("Select a conversation", "Selecciona una conversación"),
        ("Settings", "Configuración"),
        ("Shared", "Compartida"),
        ("Show local conversations", "Mostrar conversaciones locales"),
        (
            "Showing the latest messages; open the agent for the full conversation",
            "Se muestran los últimos mensajes; abre el agente para ver toda la conversación",
        ),
        ("Sync", "Sincronizar"),
        (
            "Use Convert without shared storage",
            "Puedes convertir sin configurar una carpeta compartida",
        ),
        ("Using existing copy", "Reutilizar la copia existente"),
        (
            "Wait for the current operation before closing",
            "Espera a que termine la operación antes de cerrar",
        ),
        ("Work folder", "Carpeta de trabajo"),
        (
            "Your conversations, across tools and computers",
            "Tus conversaciones, entre herramientas y equipos",
        ),
        (
            "Another operation is running. Try again shortly.",
            "Hay otra operación en curso. Vuelve a intentarlo en un momento.",
        ),
        (
            "Shared folder unavailable. Check its location or connection.",
            "La carpeta compartida no está disponible. Comprueba su ubicación o la conexión.",
        ),
        ("Details", "Detalles"),
        (
            "Preparing synchronization preview",
            "Preparando la vista previa de sincronización",
        ),
        (
            "Output versioned JSON without prompts",
            "Mostrar JSON versionado sin preguntas",
        ),
        ("Deleting {title}", "Borrando {title}"),
        (
            "Updating the shared actx binary",
            "Actualizando el ejecutable compartido de actx",
        ),
        ("Checking running agents", "Comprobando los agentes abiertos"),
        ("Finding local conversations", "Buscando conversaciones locales"),
        ("Loading synced conversations", "Cargando conversaciones sincronizadas"),
        (
            "Finding the project on this machine",
            "Buscando el proyecto en este equipo",
        ),
        (
            "Connecting to the shared folder",
            "Conectando con la carpeta compartida",
        ),
        ("Saving {title}", "Guardando {title}"),
        (
            "Reading the conversation from opencode",
            "Leyendo la conversación de opencode",
        ),
        (
            "Installing the conversation in opencode",
            "Instalando la conversación en opencode",
        ),
        (
            "Updating the Codex conversation index",
            "Actualizando el índice de conversaciones de Codex",
        ),
        ("Installing {title}", "Instalando {title}"),
        ("Syncing linked harnesses", "Sincronizando los harnesses vinculados"),
        ("Syncing {title} to {tool}", "Sincronizando {title} con {tool}"),
        ("Reviewing local conversations", "Revisando conversaciones locales"),
        ("Saving the recovery copy", "Guardando la copia de recuperación"),
        ("Restoring the conversation", "Restaurando la conversación"),
        ("Work folder: {path}", "Carpeta de trabajo: {path}"),
        (
            "This project belongs to another repository; run this command in its work folder",
            "Este proyecto pertenece a otro repositorio; ejecuta este comando en su carpeta de trabajo",
        ),
        (
            "No conversations found in this project folder; use actx add {name} --all to choose from other folders",
            "No hay conversaciones en la carpeta de este proyecto; usa actx add {name} --all para elegir las de otras carpetas",
        ),
        (
            "create a project here and choose conversations",
            "crea un proyecto aquí y elige conversaciones",
        ),
        ("Working", "Procesando"),
        (
            "A linked continuation changes the source; recheck propagation and conflicts between harnesses during sync",
            "Una continuación vinculada cambia el origen; volver a comprobar la propagación y los conflictos entre harnesses al sincronizar",
        ),
        (
            "Preview synchronization without changing any data",
            "Previsualizar la sincronización sin modificar datos",
        ),
        (
            "A reachable shared folder is required for a preview; no folder was created",
            "La vista previa necesita una carpeta compartida accesible; no se ha creado ninguna carpeta",
        ),
        (
            "Synchronization preview — no changes",
            "Vista previa de sincronización — sin cambios",
        ),
        (
            "Preview which projects? TAB marks several · Enter confirms · Esc cancels",
            "¿Qué proyectos quieres previsualizar? TAB marca varios · Enter confirma · Esc cancela",
        ),
        (
            "No shared conversations in the selected scope",
            "No hay conversaciones compartidas en la selección",
        ),
        ("Upload", "Subir"),
        ("Download", "Descargar"),
        ("New local conversation", "Nueva conversación local"),
        ("Conflict", "Conflicto"),
        ("Deferred", "Aplazado"),
        ("Up to date", "Al día"),
        ("Create linked copy", "Crear copia vinculada"),
        (
            "Import linked continuation and upload",
            "Importar continuación vinculada y subirla",
        ),
        ("Update linked copy", "Actualizar copia vinculada"),
        ("Link existing copy", "Vincular copia existente"),
        ("Delete expired shared copy", "Borrar copia compartida caducada"),
        ("Recheck after synchronization", "Volver a comprobar tras sincronizar"),
        (
            "Local changes since the last sync",
            "Cambios locales desde la última sincronización",
        ),
        (
            "Shared changes since the last sync",
            "Cambios compartidos desde la última sincronización",
        ),
        (
            "The shared conversation is not on this machine",
            "La conversación compartida no está en este equipo",
        ),
        (
            "Both copies changed; choose merge, local, remote or defer during sync",
            "Ambas copias han cambiado; al sincronizar podrás mezclar, elegir local o remota, o aplazar",
        ),
        (
            "Local and shared copies match",
            "Las copias local y compartida coinciden",
        ),
        (
            "Conversation open or active; automatic sync defers it, manual sync requires confirmation",
            "Conversación abierta o activa; el modo automático la aplaza y el manual pide confirmación",
        ),
        (
            "Destination folder needs a decision; automatic sync defers it",
            "Falta elegir la carpeta de destino; el modo automático lo aplaza",
        ),
        (
            "opencode CLI is required to import this conversation",
            "Se necesita la CLI de opencode para importar esta conversación",
        ),
        ("Destination", "Destino"),
        (
            "Data changed while preparing the preview; discard this plan and run it again",
            "Los datos han cambiado al preparar la vista previa; descarta este plan y vuelve a ejecutarla",
        ),
        ("Incomplete preview: {error}", "Vista previa incompleta: {error}"),
        (
            "Snapshot only: sync recalculates and rechecks the plan before writing. No data or actx binaries were changed.",
            "Esta vista refleja el estado actual: sync recalcula y vuelve a comprobar antes de escribir. No se han modificado datos ni binarios de actx.",
        ),
        (
            "Cannot read linked state: {error}",
            "No se puede leer el estado de las copias vinculadas: {error}",
        ),
        (
            "Create or update linked copies after downloading the source and choosing its folder",
            "Crear o actualizar las copias vinculadas después de descargar el origen y elegir su carpeta",
        ),
        (
            "Recheck linked copies after downloading or resolving the shared conflict",
            "Volver a comprobar las copias vinculadas después de descargar o resolver el conflicto compartido",
        ),
        (
            "Source open or active in the last 2 minutes; linked copies are deferred",
            "Origen abierto o activo en los últimos 2 minutos; se aplazan las copias vinculadas",
        ),
        (
            "An existing work folder is required for linked copies",
            "Las copias vinculadas necesitan una carpeta de trabajo existente",
        ),
        (
            "No conversation turns to convert",
            "No hay mensajes de conversación que convertir",
        ),
        (
            "Linked copy missing; restore it or change the conversation profile",
            "Falta la copia vinculada; restáurala o cambia el perfil de la conversación",
        ),
        (
            "Linked histories match their saved baselines",
            "Los historiales vinculados coinciden con su estado guardado",
        ),
        (
            "Linked history edited or compacted; both copies are kept",
            "Historial vinculado editado o compactado; se conservan ambas copias",
        ),
        (
            "Both harnesses continued independently; both copies are kept",
            "Ambos harnesses han continuado por separado; se conservan ambas copias",
        ),
        (
            "Same continuation in both harnesses; update linked baselines",
            "Misma continuación en ambos harnesses; actualizar el estado de los vínculos",
        ),
        (
            "Incomplete continuation; finish the agent turn before syncing",
            "Continuación incompleta; termina el turno del agente antes de sincronizar",
        ),
        (
            "Destination conversation open or recently active; update deferred",
            "Conversación de destino abierta o activa recientemente; actualización aplazada",
        ),
        (
            "Append complete new turns; preserve conversation identifiers and existing history",
            "Añadir los turnos nuevos completos conservando los identificadores y el historial existente",
        ),
        (
            "Target harness is not installed",
            "El harness de destino no está instalado",
        ),
        (
            "Several identified copies exist in this harness; choose one before linking",
            "Hay varias copias identificadas en este harness; elige una antes de vincular",
        ),
        (
            "Existing harness copy has different history; no duplicate will be created",
            "La copia existente tiene otro historial; no se creará un duplicado",
        ),
        (
            "Adopt the existing matching harness copy without creating a duplicate",
            "Vincular la copia existente que coincide, sin crear un duplicado",
        ),
        (
            "Convert the source into a new linked conversation",
            "Convertir el origen en una nueva conversación vinculada",
        ),
        (
            "Cannot inspect linked history: {error}",
            "No se puede consultar el historial vinculado: {error}",
        ),
        (
            "Older than {days} days; delete only the shared copy, keeping local originals",
            "Más de {days} días de inactividad; borrar solo la copia compartida y conservar los originales locales",
        ),
        (
            "Retention deferred until pending changes and linked copies are resolved",
            "Retención pendiente de resolver los cambios y las copias vinculadas",
        ),
        (
            "Retention deferred: a linked copy is active, changed or unreadable",
            "Retención aplazada: una copia vinculada está activa, ha cambiado o no se puede leer",
        ),
        ("local copy (not maintained)", "copia local (sin seguimiento)"),
        (
            "Using the existing {tool} copy of {title}",
            "Se usa la copia existente de {title} en {tool}",
        ),
        ("Removed {n} shared copy/copies", "Borradas {n} copia(s) compartida(s)"),
        ("STATE", "ESTADO"),
        ("CONVERSATION", "CONVERSACIÓN"),
        ("PROJECT", "PROYECTO"),
        ("HARNESSES · ACTIVITY", "HARNESSES · ACTIVIDAD"),
        (" (shared only)", " (solo compartida)"),
        (
            "Browse conversations in one compact view",
            "Explorar conversaciones en una vista compacta",
        ),
        (
            "CONVERSATION                               PROJECT           HARNESSES · ACTIVITY",
            "CONVERSACIÓN                               PROYECTO          HARNESSES · ACTIVIDAD",
        ),
        ("Change harness preferences", "Cambiar los harnesses de la conversación"),
        ("Choose a conversation to open", "Elige una conversación para abrir"),
        (
            "Choose an existing work folder before opening this conversation",
            "Elige una carpeta de trabajo existente antes de abrir esta conversación",
        ),
        (
            "Choose harnesses for this conversation",
            "Elegir los harnesses de esta conversación",
        ),
        (
            "Choose the physical copy to open",
            "Elige la copia concreta que quieres abrir",
        ),
        ("Choose the project to delete", "Elige el proyecto que quieres borrar"),
        (
            "Conversation no longer available",
            "La conversación ya no está disponible",
        ),
        (
            "Conversations · TAB select · Enter open · Alt-H harnesses · Delete remove · Esc back",
            "Conversaciones · TAB marca · Enter abre · Alt-H harnesses · Supr quita · Esc vuelve",
        ),
        ("Convert from which harness?", "¿Desde qué harness quieres convertir?"),
        ("Delete a whole project", "Borrar un proyecto completo"),
        ("Delete shared copies", "Borrar las copias compartidas"),
        (
            "Existing local copies are kept",
            "Se conservan las copias locales existentes",
        ),
        (
            "Future machine profile changes will also apply to this conversation",
            "Los cambios futuros en el perfil del equipo también se aplicarán a esta conversación",
        ),
        (
            "Harness preferences saved; new copies will be created during sync",
            "Preferencias guardadas; las copias que falten se crearán al sincronizar",
        ),
        (
            "Harness preferences · Esc cancels",
            "Preferencias de harnesses · Esc cancela",
        ),
        (
            "Harnesses · TAB toggles · Enter saves · Esc cancels · existing copies are kept",
            "Harnesses · TAB marca o desmarca · Enter guarda · Esc cancela · se conservan las copias existentes",
        ),
        (
            "Keep local originals and projects",
            "Conservar los originales locales y los proyectos",
        ),
        ("No conversations in this view", "No hay conversaciones en esta vista"),
        ("No linked harnesses", "No mantener copias vinculadas"),
        (
            "No local copy in this harness yet; finish syncing first",
            "Aún no hay una copia local en este harness; termina de sincronizar primero",
        ),
        (
            "Open in which harness? Enter opens · Ctrl-R remembers and opens",
            "¿En qué harness quieres abrirla? Enter abre · Ctrl-R recuerda la elección y abre",
        ),
        (
            "Print the resume command without launching the agent",
            "Mostrar el comando para retomar sin iniciar el agente",
        ),
        (
            "Removes every saved conversation in the chosen project; local originals are kept",
            "Borra todas las conversaciones guardadas del proyecto elegido; conserva los originales locales",
        ),
        (
            "Select one conversation to open; multiple selection is available for harnesses and removal",
            "Selecciona una conversación para abrir; puedes marcar varias para configurar harnesses o quitar copias",
        ),
        (
            "Select the harnesses where you want to maintain a copy; missing copies will be created during sync",
            "Elige los harnesses donde quieres mantener una copia; las que falten se crearán al sincronizar",
        ),
        (
            "Shared folder unavailable. Open the local copy without syncing?",
            "La carpeta compartida no está disponible. ¿Abrir la copia local sin sincronizar?",
        ),
        (
            "Stop maintaining a harness copy; keep all existing copies",
            "Dejar de mantener una copia vinculada; conservar todas las copias existentes",
        ),
        (
            "Sync a conversation and open it in a harness",
            "Sincronizar una conversación y abrirla en un harness",
        ),
        (
            "The selected harness is not installed",
            "El harness elegido no está instalado",
        ),
        (
            "This conversation is already open; close its agent first",
            "Esta conversación ya está abierta; cierra su agente primero",
        ),
        (
            "This conversation still has pending changes or a conflict; sync or resolve it before opening",
            "Esta conversación aún tiene cambios pendientes o un conflicto; sincronízala o resuélvelo antes de abrir",
        ),
        (
            "This harness copy is shared only: sync it first",
            "Esta copia solo está en la carpeta compartida: sincronízala primero",
        ),
        ("Use the machine profile", "Usar el perfil del equipo"),
        (
            "What do you want to remove? Esc cancels",
            "¿Qué quieres quitar? Esc cancela",
        ),
        ("copy missing", "falta la copia"),
        ("local copy", "copia local"),
        ("pending creation", "pendiente de crear"),
        ("⚠ state unknown", "⚠ estado desconocido"),
        (
            "Set up the shared folder and your harness profile",
            "Configurar la carpeta compartida y tu perfil de harnesses",
        ),
        (
            "Configure harnesses, retention and opening preferences interactively",
            "Configurar harnesses, retención y preferencias al abrir con un asistente",
        ),
        ("Show the profile as JSON", "Mostrar el perfil en formato JSON"),
        (
            "Use an interactive terminal, or configure the profile with --harnesses and --retention",
            "Usa una terminal interactiva o configura el perfil con --harnesses y --retention",
        ),
        (
            "Ask which harness to use each time",
            "Preguntar cada vez qué harness usar",
        ),
        (
            "When opening a conversation, which harness should be preferred?",
            "Al abrir una conversación, ¿qué harness prefieres usar?",
        ),
        (
            "Which harnesses do you use? Enter toggles · Next continues · Ctrl-D disables linked copies · Esc keeps preferences",
            "¿Qué harnesses usas? Enter marca o desmarca · Siguiente continúa · Ctrl-D desactiva las copias vinculadas · Esc conserva las preferencias",
        ),
        ("Next →", "Siguiente →"),
        (
            "{title}: linked copies deferred; close the source agent and retry (recent activity may take up to 2 minutes)",
            "{title}: copias entre harnesses aplazadas; cierra el agente de origen y reintenta (la actividad reciente puede tardar hasta 2 minutos)",
        ),
        ("Ask each time", "Preguntar cada vez"),
        ("Your actx profile", "Tu perfil de actx"),
        ("Harnesses", "Harnesses"),
        ("No harnesses configured", "Sin harnesses configurados"),
        ("Keep shared copies", "Conservar copias"),
        ("When opening", "Al abrir"),
        ("Conversation overrides", "Por conversación"),
        ("None", "Ninguna"),
        ("{n} configured", "{n} configuradas"),
        (
            "Local originals are always kept.",
            "Los originales locales siempre se conservan.",
        ),
        ("Configure profile:", "Configurar el perfil:"),
        ("Full setup:", "Configuración completa:"),
        (
            "Choose a conversation profile",
            "Elige una conversación para configurar su perfil",
        ),
        (
            "Linked harness copies; comma-separated, or none",
            "Copias vinculadas por harness, separadas por comas; none para desactivar",
        ),
        (
            "Delete shared copies after inactivity; local originals are kept",
            "Borrar copias compartidas por inactividad; conservar originales locales",
        ),
        (
            "Show or change harnesses and retention",
            "Ver o cambiar los harnesses y la retención",
        ),
        (
            "Which harnesses do you use? TAB selects several; Esc keeps current preferences",
            "¿Qué harnesses usas? TAB marca varios; Esc conserva las preferencias actuales",
        ),
        (
            "Delete shared copies after how much inactivity? Esc keeps current preferences",
            "¿Tras cuánta inactividad se borran las copias compartidas? Esc conserva las preferencias",
        ),
        (
            "Only saved shared copies are deleted; local originals are kept.",
            "Solo se borran las copias compartidas guardadas; se conservan los originales locales.",
        ),
        ("Never delete automatically", "No borrar nunca automáticamente"),
        ("1 month", "1 mes"),
        ("3 months", "3 meses"),
        ("6 months", "6 meses"),
        ("1 year", "1 año"),
        ("2 years", "2 años"),
        (HELP, HELP_ES),
        (
            "{name} · TAB marks several · Enter saves",
            "{name} · TAB marca varias · Enter guarda",
        ),
        (
            "No General conversations yet. Save some with: actx add",
            "Aún no hay conversaciones en General. Guárdalas con: actx add",
        ),
        (
            "Note: tool calls become text, and reasoning, images, and messages before a compaction are left out.",
            "Nota: las herramientas pasan a texto, y el razonamiento, las imágenes y lo anterior a una compactación no se incluyen.",
        ),
        ("(no folder)", "(sin carpeta)"),
        ("no project", "sin proyecto"),
        (
            "Where should «{title}» work? Pick a folder",
            "¿En qué carpeta quieres continuar «{title}»? Elige una carpeta",
        ),
        (
            "Where should «{title}» go? ({path} doesn't exist here)",
            "¿Dónde va «{title}»? ({path} no existe aquí)",
        ),
        ("General (no project)", "General (sin proyecto)"),
        ("Project {name} already exists", "El proyecto {name} ya existe"),
        (
            "«{title}» needs a destination folder: run actx sync --general",
            "«{title}» necesita una carpeta de destino: ejecuta actx sync --general",
        ),
        (
            "Delete project {name} and all its saved conversations ({n})? Local originals are kept.",
            "¿Borrar el proyecto {name} y todas sus conversaciones guardadas ({n})? Se conservan los originales locales.",
        ),
        (
            "Project {name} changed meanwhile: run actx rm again",
            "El proyecto {name} ha cambiado mientras tanto: ejecuta actx rm de nuevo",
        ),
        (
            "No saved conversations to delete",
            "No hay conversaciones guardadas que borrar",
        ),
        (
            "Delete saved conversations · local originals are kept · TAB marks several",
            "Borrar conversaciones guardadas · se conservan los originales locales · TAB marca varias",
        ),
        (
            "Delete {n} saved conversation(s)? Local originals and projects are kept.",
            "¿Borrar {n} conversación(es) guardada(s)? Se conservan los originales locales y los proyectos.",
        ),
        (
            "only General conversations (no project)",
            "solo las conversaciones de General (sin proyecto)",
        ),
        ("create an empty project", "crear un proyecto vacío"),
        (
            "save conversations in a project, or in General when omitted",
            "guardar conversaciones en un proyecto, o en General si se omite",
        ),
        (
            "delete selected shared copies; keep local originals and projects",
            "borrar las copias compartidas seleccionadas; conservar originales locales y proyectos",
        ),
        (
            "all projects and General, without the project picker",
            "todos los proyectos y General, sin el selector de proyectos",
        ),
        (
            "delete a project and its shared copies; keep local originals",
            "borrar un proyecto y sus copias compartidas; conservar los originales locales",
        ),
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
            "Codex couldn't index {sid}: {err}; the conversation file was kept",
            "Codex no pudo indexar {sid}: {err}; se ha conservado el archivo de la conversación",
        ),
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
            "{err}. The shared folder keeps its files online only: set it to «Always keep on this device» (or run: attrib +P \"<folder>\" /S /D)",
            "{err}. La carpeta compartida solo guarda sus ficheros en línea: ponla en «Mantener siempre en este dispositivo» (o ejecuta: attrib +P \"<carpeta>\" /S /D)",
        ),
        (
            "actx needs fzf (e.g. winget install fzf)",
            "actx necesita fzf (p. ej. winget install fzf)",
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
            "No conversations found in {path}; use actx cv --all to choose from all folders",
            "No hay conversaciones en {path}; usa actx cv --all para elegir entre todas las carpetas",
        ),
        (
            "Folder: {path} · use actx cv --all for other folders",
            "Carpeta: {path} · usa actx cv --all para ver otras carpetas",
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
            "also show local conversations actx is not syncing yet",
            "mostrar también conversaciones locales que actx aún no sincroniza",
        ),
        (
            "{n} local conversations actx is not syncing are hidden; use --all to see them",
            "{n} conversaciones locales que actx no sincroniza están ocultas; usa --all para verlas",
        ),
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
            "By default, show conversations from the current folder/project. Use --all to search all folders.",
            "Por defecto, muestra las conversaciones de la carpeta/proyecto actual. Usa --all para buscar en todas las carpetas.",
        ),
        (
            "show conversations from all folders",
            "mostrar conversaciones de todas las carpetas",
        ),
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
        (
            "Conversation changed while being inspected",
            "La conversación cambió mientras se inspeccionaba",
        ),
        (
            "Subagent confirmed by native metadata",
            "Subagente confirmado por los metadatos nativos",
        ),
        (
            "Possible subagent: its opening prompt assigns an agent role",
            "Posible subagente: su mensaje inicial le asigna un rol de agente",
        ),
        (
            "Short connection test or reply-only prompt",
            "Prueba de conexión corta o petición de responder solo algo",
        ),
        (
            "Short conversation: {n} messages, {chars} characters",
            "Conversación corta: {n} mensajes, {chars} caracteres",
        ),
        (
            "Similar opening in {n} inactive conversations in the same folder; not necessarily duplicates",
            "Apertura similar en {n} conversaciones inactivas de la misma carpeta; no necesariamente duplicadas",
        ),
        (
            "Older than the selected inactivity threshold",
            "Más antigua que el umbral de inactividad elegido",
        ),
        (
            "Protected or recent conversations skipped: {n}",
            "Conversaciones protegidas o recientes omitidas: {n}",
        ),
        (
            "Subagents and agent-role prompts",
            "Subagentes y prompts que asignan un rol de agente",
        ),
        ("Repeated opening prompts", "Prompts iniciales repetidos"),
        (
            "Connection tests and reply-only prompts",
            "Pruebas de conexión y peticiones de responder solo algo",
        ),
        ("Very short conversations", "Conversaciones muy cortas"),
        (
            "Any conversation older than the threshold",
            "Cualquier conversación más antigua que el umbral",
        ),
        (
            "1. Which kinds of conversations should clean suggest? Enter toggles · Next continues",
            "1. ¿Qué tipos de conversación debe sugerir clean? Enter marca · Siguiente continúa",
        ),
        ("{n} days of inactivity", "{n} días de inactividad"),
        (
            "2. Minimum inactivity? Open and recently active conversations are always protected",
            "2. ¿Inactividad mínima? Las conversaciones abiertas o activas hace poco siempre están protegidas",
        ),
        (
            "3. Which harnesses? Enter toggles · Next continues",
            "3. ¿Qué harnesses? Enter marca · Siguiente continúa",
        ),
        ("At most {n} messages", "Como máximo {n} mensajes"),
        (
            "4. How short should the conversation be?",
            "4. ¿Cómo de corta debe ser la conversación?",
        ),
        ("At most {n} characters", "Como máximo {n} caracteres"),
        (
            "5. Maximum conversation text length?",
            "5. ¿Longitud máxima del texto de la conversación?",
        ),
        (
            "At least {n} similar conversations",
            "Al menos {n} conversaciones similares",
        ),
        (
            "6. How many similar openings should form a group?",
            "6. ¿Cuántas aperturas similares deben formar un grupo?",
        ),
        (
            "Optional text to match in the title or opening prompt (Enter keeps the current filter):",
            "Texto opcional que debe aparecer en el título o el mensaje inicial (Enter conserva el filtro actual):",
        ),
        (
            "Advanced clean needs an interactive terminal; use clean flags for a scripted preview",
            "clean advanced necesita una terminal interactiva; usa las opciones de clean para una vista previa desde un script",
        ),
        (
            "Inactivity threshold is too large",
            "El umbral de inactividad es demasiado grande",
        ),
        (
            "Clean suggests local conversations only; shared, linked and open conversations are protected.",
            "clean solo sugiere conversaciones locales; las compartidas, vinculadas y abiertas están protegidas.",
        ),
        (
            "No cleanup candidates match these criteria",
            "Ninguna conversación coincide con estos criterios de limpieza",
        ),
        (
            "Found {n} candidates; nothing has been removed",
            "Encontradas {n} candidatas; no se ha borrado nada",
        ),
        (
            "Review cleanup candidates · Enter toggles · Next continues · Esc cancels",
            "Revisa las candidatas · Enter marca · Siguiente continúa · Esc cancela",
        ),
        ("Select all", "Seleccionar todas"),
        ("Next → ({n})", "Siguiente → ({n})"),
        ("Analyzing conversations", "Analizando conversaciones"),
        (
            "Moving conversations to quarantine",
            "Moviendo conversaciones a la cuarentena",
        ),
        (
            "Move {n} selected local conversations to compressed quarantine? Shared copies are kept.",
            "¿Mover {n} conversaciones locales seleccionadas a la cuarentena comprimida? Las copias compartidas se conservan.",
        ),
        (
            "Show cleanup candidates without moving anything",
            "Mostrar candidatas sin mover nada",
        ),
        (
            "Minimum inactivity in days (default: 7)",
            "Inactividad mínima en días (por defecto: 7)",
        ),
        (
            "Suggestion categories; old enables age as the only reason",
            "Categorías de sugerencia; old permite usar la antigüedad como único motivo",
        ),
        (
            "Literal text required in the title or opening prompt",
            "Texto literal obligatorio en el título o el mensaje inicial",
        ),
        (
            "Review unused local conversations and move selected ones to quarantine",
            "Revisar conversaciones locales sin usar y mover las elegidas a cuarentena",
        ),
        (
            "Answer questions to refine cleanup suggestions",
            "Responder preguntas para afinar las sugerencias de limpieza",
        ),
        (
            "List recoverable quarantined conversations",
            "Listar conversaciones recuperables en cuarentena",
        ),
        (
            "Recover a quarantined conversation without overwriting anything",
            "Recuperar una conversación en cuarentena sin sobrescribir nada",
        ),
        (
            "Permanently delete quarantined conversations",
            "Borrar definitivamente conversaciones en cuarentena",
        ),
        (
            "quarantine id; if omitted, choose from a list",
            "id de cuarentena; si se omite, se elige de una lista",
        ),
        (
            "Purge needs an interactive terminal, or pass the quarantine id",
            "El borrado definitivo necesita una terminal interactiva o el id de cuarentena",
        ),
        (
            "Delete quarantined conversations permanently · Enter toggles · Next continues · Esc cancels",
            "Borrar definitivamente conversaciones en cuarentena · Enter marca · Siguiente continúa · Esc cancela",
        ),
        (
            "No quarantined conversations selected",
            "No se seleccionó ninguna conversación en cuarentena",
        ),
        (
            "Permanently delete {title} from quarantine? This cannot be undone.",
            "¿Borrar definitivamente {title} de la cuarentena? Esto no se puede deshacer.",
        ),
        (
            "Permanently delete {n} quarantined conversations? This cannot be undone.",
            "¿Borrar definitivamente {n} conversaciones en cuarentena? Esto no se puede deshacer.",
        ),
        (
            "Permanently deleted {n} quarantined conversation(s)",
            "Borradas definitivamente {n} conversación(es) en cuarentena",
        ),
        (
            "Some quarantine entries changed and were not deleted: {n}",
            "Algunas entradas de la cuarentena cambiaron y no se borraron: {n}",
        ),
        (
            "Use at least 2 similar conversations and positive message and character limits",
            "Usa al menos 2 conversaciones similares y límites positivos de mensajes y caracteres",
        ),
        (
            "Maximum messages in a short conversation",
            "Máximo de mensajes en una conversación corta",
        ),
        (
            "Maximum characters in a short conversation",
            "Máximo de caracteres en una conversación corta",
        ),
        (
            "Minimum similar openings to form a group",
            "Mínimo de aperturas similares para formar un grupo",
        ),
        ("Only these harnesses", "Solo estos harnesses"),
        (
            "Only conversations whose work folder is inside this path",
            "Solo conversaciones cuya carpeta de trabajo esté dentro de esta ruta",
        ),
        (
            "Conversation is open or recently active; cleanup deferred",
            "La conversación está abierta o activa hace poco; limpieza aplazada",
        ),
        (
            "Conversation changed since selection; cleanup deferred",
            "La conversación cambió desde la selección; limpieza aplazada",
        ),
        (
            "Conversation became shared, linked or owned by an active parent; cleanup deferred",
            "La conversación pasó a estar compartida, vinculada o bajo un padre activo; limpieza aplazada",
        ),
        (
            "Conversation has child sessions; clean the children individually",
            "La conversación tiene sesiones hijas; limpia las hijas por separado",
        ),
        (
            "Archived conversation changed; its native archive was kept",
            "La conversación archivada cambió; se conservó su archivo nativo",
        ),
        (
            "Codex CLI is required to update its conversation index; the original was kept",
            "Se necesita la CLI de Codex para actualizar su índice de conversaciones; se conservó el original",
        ),
        ("Quarantined: {title}", "En cuarentena: {title}"),
        (
            "{n} local conversations quarantined. Recover with actx clean restore. Shared copies were kept.",
            "{n} conversaciones locales en cuarentena. Recupéralas con actx clean restore. Las copias compartidas se conservaron.",
        ),
        ("Quarantine is empty", "La cuarentena está vacía"),
        (
            "Destination already exists; nothing was overwritten",
            "El destino ya existe; no se sobrescribió nada",
        ),
        (
            "Choose a quarantined conversation to recover",
            "Elige una conversación en cuarentena para recuperar",
        ),
        ("Quarantine entry not found", "No se encontró la entrada de cuarentena"),
        (
            "Recover {title} to its original harness? Existing conversations will not be overwritten.",
            "¿Recuperar {title} en su harness original? No se sobrescribirán conversaciones existentes.",
        ),
        (
            "Conversation was already recovered",
            "La conversación ya se había recuperado",
        ),
        (
            "Quarantine payload failed its integrity check",
            "Los datos en cuarentena no superaron la comprobación de integridad",
        ),
        (
            "The original work folder is missing; restore it before recovering this conversation",
            "Falta la carpeta de trabajo original; restáurala antes de recuperar esta conversación",
        ),
        ("Recovered: {title}", "Recuperada: {title}"),
        (
            "A shared copy already exists at {path}; nothing was moved",
            "Ya existe una copia compartida en {path}; no se ha movido nada",
        ),
        (
            "Cannot move a shared entry that is a symbolic link",
            "No se puede mover una entrada compartida que es un enlace simbólico",
        ),
        (
            "Choose from all saved conversations, beyond the current project",
            "Elegir entre todas las conversaciones guardadas, fuera del proyecto actual",
        ),
        (
            "Destination project; otherwise choose it after selecting conversations",
            "Proyecto de destino; si se omite, se elige después de seleccionar conversaciones",
        ),
        (
            "Move conversations from {name} · --all shows other projects · TAB marks several",
            "Mover conversaciones de {name} · --all muestra otros proyectos · TAB marca varias",
        ),
        (
            "Move interrupted: {err}. Copies remain at: {paths}",
            "Movimiento interrumpido: {err}. Las copias permanecen en: {paths}",
        ),
        (
            "Move saved conversations or rename an actx project",
            "Mover conversaciones guardadas o renombrar un proyecto de actx",
        ),
        (
            "Move saved conversations · all linked harness copies move together · TAB marks several",
            "Mover conversaciones guardadas · se mueven juntas las copias vinculadas de cada harness · TAB marca varias",
        ),
        (
            "Move the selected conversations to General",
            "Mover las conversaciones seleccionadas a General",
        ),
        (
            "Move to which project or General?",
            "¿A qué proyecto o a General las muevo?",
        ),
        (
            "Move {n} conversation(s) to {name}? Native histories and work folders are kept.",
            "¿Mover {n} conversación(es) a {name}? Se conservan los historiales nativos y las carpetas de trabajo.",
        ),
        (
            "Moved {n} conversation(s) to {name}",
            "Movidas {n} conversación(es) a {name}",
        ),
        ("Moving saved conversations", "Moviendo conversaciones guardadas"),
        ("New project name:", "Nombre del nuevo proyecto:"),
        (
            "No saved conversations in this view; use actx mv --all to choose from other projects",
            "No hay conversaciones guardadas en esta vista; usa actx mv --all para elegir de otros proyectos",
        ),
        ("Project renamed: {old} → {new}", "Proyecto renombrado: {old} → {new}"),
        (
            "Rename actx project {old} → {new}? Work folders and histories are kept.",
            "¿Renombrar el proyecto de actx {old} → {new}? Se conservan las carpetas de trabajo y los historiales.",
        ),
        (
            "Rename an actx project; keep its work folder",
            "Renombrar un proyecto de actx; conservar su carpeta de trabajo",
        ),
        ("Renaming project", "Renombrando proyecto"),
        ("Preparing move", "Preparando movimiento"),
        (
            "The selected conversations are already in {name}",
            "Las conversaciones seleccionadas ya están en {name}",
        ),
        (
            "The shared folder changed or its lock was lost; run actx mv again",
            "La carpeta compartida ha cambiado o se ha perdido su bloqueo; ejecuta actx mv de nuevo",
        ),
        (
            "The shared folder changed while confirming; run actx mv again",
            "La carpeta compartida ha cambiado durante la confirmación; ejecuta actx mv de nuevo",
        ),
    ])
});

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chinese_catalog_preserves_all_placeholders_and_locale_variants() {
        let placeholder = regex::Regex::new(r"\{(\w+)\}").unwrap();
        let names = |s: &str| {
            placeholder
                .captures_iter(s)
                .map(|c| c[1].to_string())
                .collect::<Vec<_>>()
        };
        for (key, _) in ES.iter().filter(|(k, _)| **k != HELP) {
            let zh = ZH_CN.get(*key).unwrap_or_else(|| panic!("missing Chinese: {key}"));
            let mut a = names(key);
            let mut b = names(zh);
            a.sort();
            b.sort();
            assert_eq!(a, b, "Chinese placeholders: {key}");
            assert!(!zh.is_empty());
        }
        for locale in ["zh", "zh_CN.UTF-8", "zh-CN", "zh_Hans_CN", "zh_SG"] {
            assert_eq!(parse(locale), Language::ZhCn);
        }
        assert_eq!(parse("zh_TW.UTF-8"), Language::En);
        assert_eq!(parse("es_ES.UTF-8"), Language::Es);
    }

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
        let mut files = vec![src];
        while let Some(file) = files.pop() {
            if file.is_dir() {
                files.extend(std::fs::read_dir(file).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if file.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(file).unwrap();
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
