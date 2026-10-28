use crate::table::{self, Align};
use crate::{BOLD, GRAY, RED, RESET};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{cursor, queue};
use chrono::{DateTime, Local};
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Cuántas filas muestra el selector a la vez; con más, hace scroll.
const VISIBLE: usize = 10;

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

/// Muestra el selector y abre el agente: nueva conversación o `<program> <resume_flag> <id>`.
fn pick(program: &str, resume_flag: &str, sessions: Vec<Session>) -> i32 {
    // Sin conversaciones previas, o sin teclado (entrada por pipe): abrir el agente directo.
    if sessions.is_empty() || !io::stdin().is_terminal() {
        return crate::run_external(&[program.into()]);
    }

    let mut options = vec![("+ Nueva conversación".to_string(), String::new())];
    options.extend(sessions.iter().map(|s| (short(&s.title), s.modified.format("%Y-%m-%d %H:%M").to_string())));

    match select(&options) {
        Some(0) => crate::run_external(&[program.into()]),
        Some(i) => crate::run_external(&[program.into(), resume_flag.into(), sessions[i - 1].id.clone()]),
        None => 0,
    }
}

/// Selector con flechas: ↑↓ mover, Enter elegir, Esc/q/Ctrl+C cancelar.
/// Devuelve el índice elegido o None si se canceló.
fn select(options: &[(String, String)]) -> Option<usize> {
    let mut out = io::stdout();
    terminal::enable_raw_mode().ok()?;
    let (mut selected, mut top, mut drawn) = (0usize, 0usize, 0u16);

    let result = loop {
        // Mantener la opción elegida dentro de la ventana visible.
        top = top.min(selected).max((selected + 1).saturating_sub(VISIBLE));
        let frame = render(options, selected, top);
        if drawn > 0 {
            queue!(out, cursor::MoveUp(drawn)).ok();
        }
        queue!(out, cursor::MoveToColumn(0), Clear(ClearType::FromCursorDown)).ok();
        // En modo raw "\n" no vuelve al inicio de la línea: hace falta "\r\n".
        write!(out, "{}", frame.replace('\n', "\r\n")).ok();
        out.flush().ok();
        drawn = frame.lines().count() as u16;

        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Up if selected > 0 => selected -= 1,
                KeyCode::Down if selected + 1 < options.len() => selected += 1,
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = options.len() - 1,
                KeyCode::Enter => break Some(selected),
                KeyCode::Esc | KeyCode::Char('q') => break None,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break None,
                _ => {}
            },
            Err(_) => break None,
            _ => {}
        }
    };

    // Borrar el selector para que claude arranque en pantalla limpia.
    queue!(out, cursor::MoveUp(drawn), cursor::MoveToColumn(0), Clear(ClearType::FromCursorDown)).ok();
    out.flush().ok();
    terminal::disable_raw_mode().ok();
    result
}

/// Dibuja la tabla del selector: solo las filas visibles, la elegida marcada con ❯ en rojo.
fn render(options: &[(String, String)], selected: usize, top: usize) -> String {
    let rows: Vec<Vec<String>> = options
        .iter()
        .enumerate()
        .skip(top)
        .take(VISIBLE)
        .map(|(i, (title, when))| {
            if i == selected {
                vec![format!("{RED}❯{RESET}"), format!("{BOLD}{RED}{title}{RESET}"), when.clone()]
            } else {
                vec![" ".into(), title.clone(), format!("{GRAY}{when}{RESET}")]
            }
        })
        .collect();
    let mut frame = table::render(&[" ", "Conversación", "Última actividad"], &[Align::Left, Align::Left, Align::Left], &rows);
    let position = if options.len() > VISIBLE { format!(" · {}/{}", selected + 1, options.len()) } else { String::new() };
    frame += &format!("{GRAY}↑↓ moverse · Enter abrir · Esc cancelar{position}{RESET}\n");
    frame
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
    fn selector_marca_la_opcion_y_hace_scroll() {
        let options: Vec<(String, String)> = (0..15).map(|i| (format!("conv {i}"), "fecha".into())).collect();
        let frame = render(&options, 12, 5);
        let plain: String = frame.lines().map(|l| format!("{}\n", strip(l))).collect();
        assert!(plain.contains("❯ │ conv 12"), "{plain}");
        assert!(!plain.contains("conv 4 ") && plain.contains("conv 14"), "{plain}");
        assert!(plain.contains("13/15"));
        assert_eq!(frame.lines().count(), VISIBLE + 5); // 4 bordes/encabezado + filas + ayuda
    }

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            if esc {
                esc = c != 'm';
            } else if c == '\x1b' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn convierte_uris_de_antigravity() {
        let p = uri_to_path("file:///c%3A/Users/daryf/Documents/Organiza%20tu%20desarrollo").unwrap();
        assert!(same_dir(&p, Path::new(r"C:\Users\daryf\Documents\Organiza tu desarrollo")));
        let p = uri_to_path("file:///c:/Users/daryf/Documents/autospot-CRM").unwrap();
        assert!(same_dir(&p, Path::new(r"C:\Users\daryf\Documents\autospot-CRM\")));
        assert!(!same_dir(&p, Path::new(r"C:\Users\daryf\Documents\autospot")));
        assert_eq!(uri_to_path("file:///home/dary/app").unwrap(), PathBuf::from("/home/dary/app"));
        assert!(uri_to_path("https://x").is_none());
    }

    #[test]
    fn recorta_textos_largos() {
        assert_eq!(short("hola\nmundo"), "hola");
        assert_eq!(short(&"a".repeat(80)).chars().count(), 60);
    }
}
