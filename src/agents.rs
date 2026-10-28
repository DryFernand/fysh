use crate::table::{self, Align};
use crate::{BOLD, GRAY, RED, RESET};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{cursor, queue};
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Cuántas filas muestra el selector a la vez; con más, hace scroll.
const VISIBLE: usize = 10;

/// Una conversación guardada de Claude Code.
struct Session {
    id: String,
    title: String,
    modified: SystemTime,
}

/// `claude` sin argumentos: muestra las conversaciones de esta carpeta para reanudar una,
/// o abre una nueva. Con argumentos, fysh llama a claude directo (no pasa por aquí).
pub fn claude_picker() -> i32 {
    let sessions = list_sessions();
    // Sin conversaciones previas, o sin teclado (entrada por pipe): abrir claude directo.
    if sessions.is_empty() || !io::stdin().is_terminal() {
        return crate::run_external(&["claude".into()]);
    }

    let mut options = vec![("+ Nueva conversación".to_string(), String::new())];
    options.extend(sessions.iter().map(|s| {
        let when = chrono::DateTime::<chrono::Local>::from(s.modified).format("%Y-%m-%d %H:%M");
        (short(&s.title), when.to_string())
    }));

    match select(&options) {
        Some(0) => crate::run_external(&["claude".into()]),
        Some(i) => crate::run_external(&["claude".into(), "--resume".into(), sessions[i - 1].id.clone()]),
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

/// Conversaciones de la carpeta actual, la más reciente primero.
fn list_sessions() -> Vec<Session> {
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
                modified: fs::metadata(&p).and_then(|m| m.modified()).ok()?,
            })
        })
        .collect();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    sessions
}

/// Carpeta donde Claude Code guarda las conversaciones de la carpeta actual:
/// ~/.claude/projects/<ruta con todo lo que no es letra o número cambiado por '-'>.
fn project_dir() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok()?;
    let projects = Path::new(&home).join(".claude").join("projects");
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
    fn recorta_textos_largos() {
        assert_eq!(short("hola\nmundo"), "hola");
        assert_eq!(short(&"a".repeat(80)).chars().count(), 60);
    }
}
