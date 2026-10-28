//! Buscador difuso: escribes y la lista se filtra al instante, los más parecidos arriba
//! ("dk up" encuentra "docker compose up -d backend"). Lo usan Ctrl+R (historial) y Ctrl+T (archivos).

use crate::table::truncate;
use crate::{BOLD, GRAY, RED, RESET};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{cursor, queue};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use std::io::Write;

/// Cuántos resultados se muestran a la vez.
const SHOWN: usize = 10;

/// `items` ordenados por parecido con `query` (los más parecidos primero). Sin texto, el orden original.
pub fn rank<'a>(query: &str, items: &'a [String], matcher: &mut Matcher) -> Vec<&'a String> {
    if query.trim().is_empty() {
        return items.iter().collect();
    }
    Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart)
        .match_list(items.iter(), matcher)
        .into_iter()
        .map(|(item, _score)| item)
        .collect()
}

/// Buscador debajo de la línea actual. Devuelve el elegido o None (Esc / Ctrl+C).
/// `in_editor`: se llama desde el editor de línea, que ya tiene la terminal en modo raw
/// (no hay que activarlo ni desactivarlo, o el editor quedaría roto).
pub fn search(title: &str, items: &[String], initial: &str, in_editor: bool) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    if !in_editor {
        terminal::enable_raw_mode().ok()?;
    }
    let mut out = std::io::stdout();
    let width = terminal::size().map_or(100, |(w, _)| w as usize).max(30);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut query = initial.to_string();
    let (mut cur, mut drawn) = (0usize, 0u16);
    write!(out, "\r\n").ok();

    let result = loop {
        let ranked = rank(&query, items, &mut matcher);
        cur = cur.min(ranked.len().saturating_sub(1));
        // Líneas sin pasar del ancho de la ventana: si una se partiera en dos, el redibujo fallaría.
        let mut lines = vec![format!("{GRAY}{title}{RESET} {RED}❯{RESET} {}", truncate(&query, width - title.len() - 4))];
        for (i, item) in ranked.iter().take(SHOWN).enumerate() {
            let text = truncate(item, width - 4);
            lines.push(if i == cur { format!(" {RED}❯{RESET} {BOLD}{RED}{text}{RESET}") } else { format!("   {text}") });
        }
        lines.push(format!("{GRAY}{}/{} · escribe para filtrar · ↑↓ · Enter elegir · Esc cancelar{RESET}", ranked.len(), items.len()));

        if drawn > 0 {
            queue!(out, cursor::MoveUp(drawn)).ok();
        }
        queue!(out, cursor::MoveToColumn(0), Clear(ClearType::FromCursorDown)).ok();
        write!(out, "{}", lines.join("\r\n")).ok();
        out.flush().ok();
        drawn = (lines.len() - 1) as u16;

        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Enter => break ranked.get(cur).map(|s| s.to_string()),
                KeyCode::Esc => break None,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break None,
                KeyCode::Up if cur > 0 => cur -= 1,
                KeyCode::Down if cur + 1 < ranked.len().min(SHOWN) => cur += 1,
                KeyCode::Backspace => {
                    query.pop();
                    cur = 0;
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    query.push(c);
                    cur = 0;
                }
                _ => {}
            },
            Err(_) => break None,
            _ => {}
        }
    };

    // Borrar el buscador y volver a la línea donde se estaba escribiendo.
    queue!(out, cursor::MoveUp(drawn), cursor::MoveToColumn(0), Clear(ClearType::FromCursorDown), cursor::MoveUp(1)).ok();
    out.flush().ok();
    if !in_editor {
        terminal::disable_raw_mode().ok();
    }
    result
}

/// Archivos de la carpeta (sin lo ignorado por .gitignore ni carpetas ocultas), como mucho `max`.
pub fn project_files(max: usize) -> Vec<String> {
    let Ok(cwd) = std::env::current_dir() else { return Vec::new() };
    ignore::WalkBuilder::new(&cwd)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(&cwd).ok().map(|p| p.display().to_string()))
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encuentra_aunque_no_sea_exacto() {
        let items: Vec<String> = ["git status", "docker compose up -d backend", "npm run dev", "cd ~/Documents/autospot-CRM"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut m = Matcher::new(Config::DEFAULT);
        assert_eq!(rank("dk up", &items, &mut m)[0], "docker compose up -d backend");
        assert_eq!(rank("autocrm", &items, &mut m)[0], "cd ~/Documents/autospot-CRM");
        assert_eq!(rank("NPM", &items, &mut m)[0], "npm run dev", "sin importar mayúsculas");
        assert!(rank("zzzz", &items, &mut m).is_empty());
        assert_eq!(rank("", &items, &mut m).len(), 4, "sin texto: todos, en el orden original");
    }

    #[test]
    fn lista_archivos_respetando_gitignore() {
        let files = project_files(10_000);
        assert!(files.iter().any(|f| f.ends_with("main.rs")), "{:?}", &files[..files.len().min(5)]);
        assert!(!files.iter().any(|f| f.starts_with("target")), "target/ está en .gitignore");
    }
}
