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

/// Elegir una conversación de agy de esta carpeta (para renombrarla o borrarla).
fn pick_agy_session(action: &str) -> Option<Session> {
    let sessions = list_agy_sessions();
    if sessions.is_empty() {
        println!("{GRAY}No hay conversaciones de agy en esta carpeta.{RESET}");
        return None;
    }
    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|s| vec![short(&s.title), format!("{GRAY}{}{RESET}", s.modified.format("%Y-%m-%d %H:%M"))])
        .collect();
    println!("{GRAY}Elige la conversación a {action}:{RESET}");
    let i = picker::select_one(&["Conversación", "Última actividad"], &rows)?;
    sessions.into_iter().nth(i)
}

// agy guarda el nombre real de cada conversación en un archivo interno propio: si se cambia por
// fuera, agy lo deshace (o queda una conversación rota). Por eso renombrar y borrar se hacen con
// sus propios comandos /rename y /delete, que solo existen en su pantalla interactiva: fysh abre
// agy en esa conversación con el comando ya puesto (`-i`), y se sale con Ctrl+C dos veces o /exit.

/// `agy -rename`: elegir una conversación y ponerle otro nombre.
pub fn agy_rename() -> i32 {
    let Some(s) = pick_agy_session("renombrar") else { return 0 };
    let Some(name) = crate::ask::ask("Nuevo nombre: ", &s.title) else { return 0 };
    if name == s.title {
        println!("{GRAY}Sin cambios.{RESET}");
        return 0;
    }
    println!("{GRAY}Abriendo agy para renombrarla. Cuando diga que se renombró, sal con Ctrl+C dos veces (o /exit).{RESET}");
    crate::run_external(&["agy".into(), "--conversation".into(), s.id, "-i".into(), format!("/rename {name}")])
}

/// `agy -delete`: elegir una conversación y borrarla (agy pide confirmar también).
pub fn agy_delete() -> i32 {
    let Some(s) = pick_agy_session("borrar") else { return 0 };
    if !crate::ask::confirm(&format!("¿Borrar la conversación \"{}\"? No se puede deshacer.", short(&s.title))) {
        return 0;
    }
    println!("{GRAY}Abriendo agy para borrarla: confirma ahí. Después sal con Ctrl+C dos veces (o /exit).{RESET}");
    crate::run_external(&["agy".into(), "--conversation".into(), s.id, "-i".into(), "/delete".into()])
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
