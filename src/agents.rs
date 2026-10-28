use crate::table::{self, Align};
use crate::{GRAY, RED, RESET};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    if sessions.is_empty() {
        return crate::run_external(&["claude".into()]);
    }

    let rows: Vec<Vec<String>> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let when = chrono::DateTime::<chrono::Local>::from(s.modified).format("%Y-%m-%d %H:%M");
            vec![(i + 1).to_string(), s.title.clone(), when.to_string()]
        })
        .collect();
    table::print(&["#", "Conversación", "Última actividad"], &[Align::Right, Align::Left, Align::Left], &rows);
    print!("{GRAY}Número para reanudar · Enter = nueva conversación · q = cancelar:{RESET} {RED}❯{RESET} ");
    io::stdout().flush().ok();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
        println!();
        return 0; // Ctrl+C o fin de entrada: cancelar
    }
    match answer.trim() {
        "" => crate::run_external(&["claude".into()]),
        "q" | "Q" => 0,
        n => match n.parse::<usize>().ok().and_then(|n| sessions.get(n.wrapping_sub(1))) {
            Some(s) => crate::run_external(&["claude".into(), "--resume".into(), s.id.clone()]),
            None => {
                eprintln!("claude: opción no válida: {n}");
                1
            }
        },
    }
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
    fn recorta_textos_largos() {
        assert_eq!(short("hola\nmundo"), "hola");
        assert_eq!(short(&"a".repeat(80)).chars().count(), 60);
    }
}
