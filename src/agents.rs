use crate::picker;
use crate::GRAY;
use crate::RESET;
use chrono::{DateTime, Local};
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal};
use std::path::{Path, PathBuf};

/// Una conversación guardada de un agente (Claude Code, Antigravity...).
struct Session {
    id: String,
    title: String,
    modified: DateTime<Local>,
}

/// `claude` sin argumentos: muestra las conversaciones de esta carpeta para reanudar una,
/// o abre una nueva. Con argumentos, fysh llama a claude directo (no pasa por aquí).
pub fn claude_picker() -> i32 {
    pick("claude", "--resume", list_claude_sessions())
}

/// `agy` (CLI de Antigravity) sin argumentos: lo mismo con sus conversaciones.
pub fn agy_picker() -> i32 {
    pick("agy", "--conversation", list_agy_sessions())
}

// ---------- renombrar y borrar conversaciones (claude / agy / opencode -rename / -delete) ----------
// Sin abrir el chat de ningún agente:
// - Claude Code: el título es la última línea "custom-title" del .jsonl (lo mismo que hace su /rename);
//   borrar = mandar a la papelera el .jsonl, su carpeta y sus datos en file-history y session-env.
// - OpenCode: sus funciones oficiales (`opencode api PATCH /api/session/<id>` y `opencode session delete`).
// - agy: su índice real es jetbox_summaries_proto.pb (la base conversation_summaries.db es una copia
//   que agy reconstruye desde ahí), así que se cambian los dos. Solo con agy cerrado (si no, al
//   cerrarse escribiría encima) y con copia de respaldo en ~/.fysh/respaldos antes de tocar nada.

/// `<agente> -rename` o `<agente> -delete`.
pub fn manage(agent: &str, flag: &str) -> i32 {
    let (sessions, label) = match agent {
        "claude" => (list_claude_sessions(), "Claude Code"),
        "opencode" => (list_opencode_sessions(), "OpenCode"),
        _ => (list_agy_sessions(), "Antigravity"),
    };
    let rename = flag.trim_start_matches('-') == "rename";
    if sessions.is_empty() {
        println!("{GRAY}No hay conversaciones de {label} en esta carpeta.{RESET}");
        return 0;
    }
    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|s| vec![short(&s.title), format!("{GRAY}{}{RESET}", s.modified.format("%Y-%m-%d %H:%M"))])
        .collect();
    println!("{GRAY}Elige la conversación de {label} a {}:{RESET}", if rename { "renombrar" } else { "borrar" });
    let Some(i) = picker::select_one(&["Conversación", "Última actividad"], &rows) else { return 0 };
    let s = &sessions[i];

    let result = if rename {
        let Some(name) = crate::ask::ask("Nuevo nombre: ", &s.title) else { return 0 };
        if name == s.title {
            println!("{GRAY}Sin cambios.{RESET}");
            return 0;
        }
        match agent {
            "claude" => claude_rename(s, &name),
            "opencode" => opencode_rename(s, &name),
            _ => agy_rename(s, &name),
        }
        .map(|()| format!("Conversación renombrada: {name}"))
    } else {
        if !crate::ask::confirm(&format!("¿Borrar la conversación \"{}\"?", short(&s.title))) {
            return 0;
        }
        match agent {
            "claude" => claude_delete(s),
            "opencode" => opencode_delete(s),
            _ => agy_delete(s),
        }
        .map(|()| format!("Conversación borrada: {}", short(&s.title)))
    };
    match result {
        Ok(msg) => {
            println!("{}✓{RESET} {msg}", crate::GREEN);
            0
        }
        Err(e) => {
            eprintln!("{}No se pudo:{RESET} {e}", crate::RED);
            1
        }
    }
}

fn claude_rename(s: &Session, name: &str) -> Result<(), String> {
    use std::io::Write;
    let file = project_dir().ok_or("no encontré la carpeta de conversaciones")?.join(format!("{}.jsonl", s.id));
    let line = serde_json::json!({"type": "custom-title", "customTitle": name, "sessionId": s.id});
    let mut f = fs::OpenOptions::new().append(true).open(&file).map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// Todo lo de la conversación va a la papelera (se puede recuperar).
fn claude_delete(s: &Session) -> Result<(), String> {
    let project = project_dir().ok_or("no encontré la carpeta de conversaciones")?;
    let claude = home().ok_or("sin carpeta de usuario")?.join(".claude");
    let mut paths = vec![project.join(format!("{}.jsonl", s.id)), project.join(&s.id)];
    paths.extend(["file-history", "session-env"].iter().map(|d| claude.join(d).join(&s.id)));
    if let Ok(todos) = fs::read_dir(claude.join("todos")) {
        paths.extend(todos.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&s.id))));
    }
    let existing: Vec<PathBuf> = paths.into_iter().filter(|p| p.exists()).collect();
    trash::delete_all(&existing).map_err(|e| e.to_string())
}

/// Ejecuta el CLI de OpenCode y devuelve el error que diga si falla.
fn opencode_run(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(opencode_program()).args(args).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let out_text = String::from_utf8_lossy(&out.stdout);
    Err(err.lines().chain(out_text.lines()).find(|l| !l.trim().is_empty()).unwrap_or("OpenCode no respondió").trim().to_string())
}

fn opencode_rename(s: &Session, name: &str) -> Result<(), String> {
    let body = serde_json::json!({"title": name}).to_string();
    opencode_run(&["api", "PATCH", &format!("/api/session/{}", s.id), "-d", &body])
}

fn opencode_delete(s: &Session) -> Result<(), String> {
    opencode_run(&["session", "delete", &s.id])
}

fn agy_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".gemini").join("antigravity-cli"))
}

/// agy no puede estar abierto mientras se cambia su índice.
fn agy_running() -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
    sys.processes().values().any(|p| p.name().to_string_lossy().to_lowercase().trim_end_matches(".exe") == "agy")
}

/// Copia el índice y la base de agy a ~/.fysh/respaldos/agy-<fecha> antes de cambiarlos.
fn agy_backup(dir: &Path) -> Result<(), String> {
    let dest = home().ok_or("sin carpeta de usuario")?.join(".fysh").join("respaldos").join(format!("agy-{}", Local::now().format("%Y%m%d-%H%M%S")));
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for f in ["jetbox_summaries_proto.pb", "conversation_summaries.db"] {
        if dir.join(f).exists() {
            fs::copy(dir.join(f), dest.join(f)).map_err(|e| format!("respaldo de {f}: {e}"))?;
        }
    }
    Ok(())
}

fn agy_check() -> Result<PathBuf, String> {
    if agy_running() {
        return Err("agy está abierto: ciérralo primero (si usas su control remoto: agy remote-control stop).".into());
    }
    let dir = agy_dir().ok_or("sin carpeta de usuario")?;
    agy_backup(&dir)?;
    Ok(dir)
}

/// annotations/<id>.pbtxt (texto, una propiedad por línea): ahí agy guarda el título que manda;
/// lo vuelve a copiar a sus índices cada vez que la conversación avanza. Se cambia solo `title:`.
fn set_annotation_title(text: &str, name: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let mut lines: Vec<String> = text.lines().filter(|l| !l.trim_start().starts_with("title:")).map(str::to_owned).collect();
    lines.insert(0, format!("title:\"{escaped}\""));
    lines.join("\n") + "\n"
}

fn agy_rename(s: &Session, name: &str) -> Result<(), String> {
    let dir = agy_check()?;
    let annotation = dir.join("annotations").join(format!("{}.pbtxt", s.id));
    let old = fs::read_to_string(&annotation).unwrap_or_default();
    fs::create_dir_all(dir.join("annotations")).map_err(|e| e.to_string())?;
    fs::write(&annotation, set_annotation_title(&old, name)).map_err(|e| e.to_string())?;
    let index = dir.join("jetbox_summaries_proto.pb");
    let bytes = fs::read(&index).map_err(|e| e.to_string())?;
    let new = crate::protobuf::rename_entry(&bytes, &s.id, name).ok_or("no encontré la conversación en el índice de agy (no se cambió nada)")?;
    // Verificar antes de guardar: el archivo nuevo se lee bien y tiene el título nuevo.
    if crate::protobuf::title_of(&new, &s.id).as_deref() != Some(name) {
        return Err("el índice de agy no quedó bien al cambiarlo (no se guardó nada)".into());
    }
    fs::write(&index, new).map_err(|e| e.to_string())?;
    let db = rusqlite::Connection::open(dir.join("conversation_summaries.db")).map_err(|e| e.to_string())?;
    db.execute("UPDATE conversation_summaries SET title = ?1 WHERE conversation_id = ?2", (name, &s.id)).map_err(|e| e.to_string())?;
    Ok(())
}

fn agy_delete(s: &Session) -> Result<(), String> {
    let dir = agy_check()?;
    let db = rusqlite::Connection::open(dir.join("conversation_summaries.db")).map_err(|e| e.to_string())?;
    // La conversación y sus subconversaciones (agentes que lanzó).
    let mut ids = vec![s.id.clone()];
    if let Ok(mut stmt) = db.prepare("SELECT conversation_id FROM conversation_summaries WHERE parent_conversation_id = ?1") {
        if let Ok(rows) = stmt.query_map([&s.id], |r| r.get::<_, String>(0)) {
            ids.extend(rows.filter_map(Result::ok));
        }
    }
    let index = dir.join("jetbox_summaries_proto.pb");
    let bytes = fs::read(&index).map_err(|e| e.to_string())?;
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (new, _) = crate::protobuf::remove_entries(&bytes, &refs).ok_or("el índice de agy no tiene la forma esperada (no se cambió nada)")?;
    fs::write(&index, new).map_err(|e| e.to_string())?;
    for id in &ids {
        db.execute("DELETE FROM conversation_summaries WHERE conversation_id = ?1", [id]).map_err(|e| e.to_string())?;
    }
    let mut files = Vec::new();
    for id in &ids {
        files.extend([
            dir.join("conversations").join(format!("{id}.db")),
            dir.join("brain").join(id),
            dir.join("annotations").join(format!("{id}.pbtxt")),
            dir.join("presence").join(format!("{id}.lock")),
        ]);
    }
    let existing: Vec<PathBuf> = files.into_iter().filter(|p| p.exists()).collect();
    trash::delete_all(&existing).map_err(|e| e.to_string())
}

/// `opencode`: sin argumentos, el selector; con argumentos, el CLI de OpenCode directo.
pub fn opencode(args: &[String]) -> i32 {
    let program = opencode_program();
    if args.is_empty() {
        return pick(&program, "--session", list_opencode_sessions());
    }
    let mut cmd = vec![program];
    cmd.extend_from_slice(args);
    crate::run_external(&cmd)
}

/// La app de escritorio de OpenCode pone en el PATH un `opencode` que abre la ventana de la app,
/// no la terminal. Si está instalada, usar el CLI que trae dentro; si no, el `opencode` normal.
pub fn opencode_program() -> String {
    let desktop_cli = std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join(r"Programs\@opencode-aidesktop\resources\opencode-cli.exe"));
    match desktop_cli {
        Some(p) if p.is_file() => p.to_string_lossy().into_owned(),
        _ => "opencode".into(),
    }
}

/// Muestra el selector y abre el agente: nueva conversación o `<program> <resume_flag> <id>`.
fn pick(program: &str, resume_flag: &str, sessions: Vec<Session>) -> i32 {
    // Sin conversaciones previas, o sin teclado (entrada por pipe): abrir el agente directo.
    if sessions.is_empty() || !io::stdin().is_terminal() {
        return crate::run_external(&[program.into()]);
    }

    let mut rows = vec![vec!["+ Nueva conversación".to_string(), String::new()]];
    rows.extend(sessions.iter().map(|s| {
        vec![short(&s.title), format!("{GRAY}{}{RESET}", s.modified.format("%Y-%m-%d %H:%M"))]
    }));

    match picker::select_one(&["Conversación", "Última actividad"], &rows) {
        Some(0) => crate::run_external(&[program.into()]),
        Some(i) => crate::run_external(&[program.into(), resume_flag.into(), sessions[i - 1].id.clone()]),
        None => 0,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// Conversaciones de Claude Code de la carpeta actual, la más reciente primero.
fn list_claude_sessions() -> Vec<Session> {
    let Some(dir) = project_dir() else { return Vec::new() };
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut sessions: Vec<Session> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| {
            let title = read_title(&p)?;
            Some(Session {
                id: p.file_stem()?.to_string_lossy().into_owned(),
                title,
                modified: fs::metadata(&p).and_then(|m| m.modified()).ok()?.into(),
            })
        })
        .collect();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    sessions
}

/// Conversaciones del CLI de Antigravity (agy) de la carpeta actual, la más reciente primero.
/// agy guarda un resumen de cada una en ~/.gemini/antigravity-cli/conversation_summaries.db.
/// Solo las del CLI (las de la app de escritorio viven aparte) y sin subagentes (nesting_depth 0).
fn list_agy_sessions() -> Vec<Session> {
    use rusqlite::{Connection, OpenFlags};
    let (Some(home), Ok(cwd)) = (home(), std::env::current_dir()) else { return Vec::new() };
    let db = home.join(".gemini").join("antigravity-cli").join("conversation_summaries.db");
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT conversation_id, title, preview, last_modified_time, workspace_uris
         FROM conversation_summaries
         WHERE nesting_depth = 0 AND app_data_dir = 'antigravity-cli'
         ORDER BY last_modified_time DESC",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
    });
    let Ok(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok)
        .filter(|(.., workspaces)| {
            let uris: Vec<String> = serde_json::from_str(workspaces).unwrap_or_default();
            uris.iter().filter_map(|u| uri_to_path(u)).any(|p| same_dir(&p, &cwd))
        })
        .filter_map(|(id, title, preview, modified, _)| {
            // Formato: "2026-09-30 17:20:13.9133621+00:00"
            let modified = DateTime::parse_from_str(&modified, "%Y-%m-%d %H:%M:%S%.f%:z").ok()?.with_timezone(&Local);
            // Sin título ni vista previa = conversación abierta y nunca usada: no se muestra.
            let title = if !title.is_empty() { title } else if !preview.is_empty() { preview } else { return None };
            Some(Session { id, title, modified })
        })
        .collect()
}

/// Sesiones de OpenCode (2.x) de la carpeta actual, la más reciente primero.
/// Están en ~/.local/share/opencode/opencode.db, tabla session_v2. Sin subagentes ni archivadas.
fn list_opencode_sessions() -> Vec<Session> {
    use rusqlite::{Connection, OpenFlags};
    let (Some(home), Ok(cwd)) = (home(), std::env::current_dir()) else { return Vec::new() };
    let db = home.join(".local").join("share").join("opencode").join("opencode.db");
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, COALESCE(NULLIF(title, ''), slug), time_updated, directory
         FROM session_v2
         WHERE parent_id IS NULL AND time_archived IS NULL
         ORDER BY time_updated DESC",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    });
    let Ok(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok)
        .filter(|(.., dir)| same_dir(Path::new(dir), &cwd))
        .filter_map(|(id, title, millis, _)| {
            let modified = DateTime::from_timestamp_millis(millis)?.with_timezone(&Local);
            Some(Session { id, title, modified })
        })
        .collect()
}

/// "file:///c%3A/Users/x/Mi%20App" → "c:/Users/x/Mi App" (Windows) o "/home/x" (Linux/Mac).
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::new();
    let mut it = rest.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // En Windows viene como "/c:/...": quitar la barra inicial.
    let path = match path.as_bytes() {
        [b'/', _, b':', ..] => &path[1..],
        _ => &path[..],
    };
    Some(PathBuf::from(path))
}

/// Compara carpetas sin importar mayúsculas (Windows) ni el tipo de barra.
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) {
        norm(a).eq_ignore_ascii_case(&norm(b))
    } else {
        norm(a) == norm(b)
    }
}

/// Carpeta donde Claude Code guarda las conversaciones de la carpeta actual:
/// ~/.claude/projects/<ruta con todo lo que no es letra o número cambiado por '-'>.
fn project_dir() -> Option<PathBuf> {
    let projects = home()?.join(".claude").join("projects");
    let cwd = std::env::current_dir().ok()?;
    let name = encode_path(&cwd.to_string_lossy());
    // En Windows la letra de la unidad puede venir en mayúscula o minúscula: comparar sin distinguir.
    fs::read_dir(&projects)
        .ok()?
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&name))
        .map(|e| e.path())
}

fn encode_path(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Nombre de la conversación: el último título (custom-title) o, si no hay, su primer mensaje.
/// Devuelve None si la conversación está vacía.
fn read_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut title = None;
    let mut first_prompt = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Revisar el texto antes de parsear: los archivos pueden ser grandes.
        if line.contains("\"custom-title\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                title = v["customTitle"].as_str().map(str::to_owned).or(title);
            }
        } else if first_prompt.is_none() && line.contains("\"type\":\"user\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                first_prompt = v["message"]["content"].as_str().map(short);
            }
        }
    }
    title.or(first_prompt)
}

/// Primera línea del texto, recortada a 60 caracteres.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(59).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cambia_el_titulo_en_las_anotaciones_de_agy() {
        let old = "title:\"Viejo\"\nlast_user_view_time:{seconds:1 nanos:2}\n";
        assert_eq!(set_annotation_title(old, "Nuevo \"x\""), "title:\"Nuevo \\\"x\\\"\"\nlast_user_view_time:{seconds:1 nanos:2}\n");
        assert_eq!(set_annotation_title("", "Ñandú"), "title:\"Ñandú\"\n");
    }

    /// Prueba real (cambia datos): FYSH_PRUEBA_AGENTE=agy|claude|opencode, FYSH_PRUEBA_ID=<id>,
    /// FYSH_PRUEBA_ACCION=rename|delete, FYSH_PRUEBA_NOMBRE=<nombre>. Solo con conversaciones de prueba.
    #[test]
    #[ignore]
    fn prueba_real_renombrar_borrar() {
        let var = |k: &str| std::env::var(k).unwrap();
        if let Ok(dir) = std::env::var("FYSH_PRUEBA_CARPETA") {
            std::env::set_current_dir(dir).unwrap(); // Claude busca sus conversaciones por carpeta
        }
        let s = Session { id: var("FYSH_PRUEBA_ID"), title: String::new(), modified: Local::now() };
        let rename = var("FYSH_PRUEBA_ACCION") == "rename";
        let name = std::env::var("FYSH_PRUEBA_NOMBRE").unwrap_or_default();
        let result = match (var("FYSH_PRUEBA_AGENTE").as_str(), rename) {
            ("agy", true) => agy_rename(&s, &name),
            ("agy", false) => agy_delete(&s),
            ("claude", true) => claude_rename(&s, &name),
            ("claude", false) => claude_delete(&s),
            ("opencode", true) => opencode_rename(&s, &name),
            (_, false) => opencode_delete(&s),
            _ => unreachable!(),
        };
        println!("RESULTADO: {result:?}");
        result.unwrap();
    }

    #[test]
    fn codifica_rutas_como_claude_code() {
        assert_eq!(encode_path(r"C:\Users\daryf\Documents\FyDry Task"), "C--Users-daryf-Documents-FyDry-Task");
        assert_eq!(encode_path("/home/dary/my.app"), "-home-dary-my-app");
    }

    #[test]
    fn convierte_uris_de_antigravity() {
        let p = uri_to_path("file:///home/dary/Mi%20App").unwrap();
        assert_eq!(p, PathBuf::from("/home/dary/Mi App"));
        assert!(same_dir(&p, Path::new("/home/dary/Mi App/")));
        assert!(!same_dir(&p, Path::new("/home/dary/Mi")));
        assert!(uri_to_path("https://x").is_none());
    }

    /// En Windows la unidad viene en minúscula ("c%3A") y no importan mayúsculas ni el tipo de barra.
    #[cfg(windows)]
    #[test]
    fn convierte_uris_de_antigravity_en_windows() {
        let p = uri_to_path("file:///c%3A/Users/daryf/Documents/Organiza%20tu%20desarrollo").unwrap();
        assert!(same_dir(&p, Path::new(r"C:\Users\daryf\Documents\Organiza tu desarrollo")));
        let p = uri_to_path("file:///c:/Users/daryf/Documents/autospot-CRM").unwrap();
        assert!(same_dir(&p, Path::new(r"C:\Users\daryf\Documents\autospot-CRM\")));
        assert!(!same_dir(&p, Path::new(r"C:\Users\daryf\Documents\autospot")));
    }

    #[test]
    fn recorta_textos_largos() {
        assert_eq!(short("hola\nmundo"), "hola");
        assert_eq!(short(&"a".repeat(80)).chars().count(), 60);
    }
}
