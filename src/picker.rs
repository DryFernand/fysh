//! Selector con flechas sobre una tabla. Lo usan los agentes (conversaciones) y git (ramas, archivos, commits).

use crate::table::{self, Align};
use crate::{BOLD, GRAY, RED, RESET};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{cursor, queue};
use std::io::{self, IsTerminal, Write};

/// Cuántas filas muestra el selector a la vez; con más, hace scroll.
const VISIBLE: usize = 10;

/// Elegir una fila: ↑↓ mover, Enter elegir, Esc/q/Ctrl+C cancelar.
/// Devuelve None si se canceló o si no hay teclado (entrada por pipe).
pub fn select_one(headers: &[&str], rows: &[Vec<String>]) -> Option<usize> {
    run(headers, rows, false).map(|v| v[0])
}

/// Marcar varias filas: Espacio marca/desmarca, `a` marca o desmarca todas, Enter confirma.
/// Si no se marcó ninguna, Enter devuelve la fila donde está el cursor.
pub fn select_many(headers: &[&str], rows: &[Vec<String>]) -> Option<Vec<usize>> {
    run(headers, rows, true)
}

fn run(headers: &[&str], rows: &[Vec<String>], multi: bool) -> Option<Vec<usize>> {
    if rows.is_empty() || !io::stdin().is_terminal() {
        return None;
    }
    let mut out = io::stdout();
    terminal::enable_raw_mode().ok()?;
    let (mut cur, mut top, mut drawn) = (0usize, 0usize, 0u16);
    let mut checked = vec![false; rows.len()];

    let result = loop {
        // Mantener el cursor dentro de la ventana visible.
        top = top.min(cur).max((cur + 1).saturating_sub(VISIBLE));
        let frame = render(headers, rows, cur, top, multi.then_some(&checked[..]));
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
                KeyCode::Up if cur > 0 => cur -= 1,
                KeyCode::Down if cur + 1 < rows.len() => cur += 1,
                KeyCode::Home => cur = 0,
                KeyCode::End => cur = rows.len() - 1,
                KeyCode::Char(' ') if multi => checked[cur] = !checked[cur],
                KeyCode::Char('a') if multi => {
                    let all = checked.iter().all(|&c| c);
                    checked.iter_mut().for_each(|c| *c = !all);
                }
                KeyCode::Enter => {
                    let picked: Vec<usize> = (0..rows.len()).filter(|&i| checked[i]).collect();
                    break Some(if picked.is_empty() { vec![cur] } else { picked });
                }
                KeyCode::Esc | KeyCode::Char('q') => break None,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break None,
                _ => {}
            },
            Err(_) => break None,
            _ => {}
        }
    };

    // Borrar el selector para que lo que sigue arranque en pantalla limpia.
    queue!(out, cursor::MoveUp(drawn), cursor::MoveToColumn(0), Clear(ClearType::FromCursorDown)).ok();
    out.flush().ok();
    terminal::disable_raw_mode().ok();
    result
}

/// Dibuja la tabla: solo las filas visibles, la del cursor marcada con ❯ y su primera columna en rojo.
fn render(headers: &[&str], rows: &[Vec<String>], cur: usize, top: usize, checked: Option<&[bool]>) -> String {
    let mut all_headers = vec![" "];
    if checked.is_some() {
        all_headers.push(" ");
    }
    all_headers.extend_from_slice(headers);

    let visible: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .skip(top)
        .take(VISIBLE)
        .map(|(i, row)| {
            let mut line = vec![if i == cur { format!("{RED}❯{RESET}") } else { " ".into() }];
            if let Some(checked) = checked {
                line.push(if checked[i] { format!("{RED}■{RESET}") } else { format!("{GRAY}□{RESET}") });
            }
            for (j, cell) in row.iter().enumerate() {
                line.push(if i == cur && j == 0 { format!("{BOLD}{RED}{cell}{RESET}") } else { cell.clone() });
            }
            line
        })
        .collect();

    let align = vec![Align::Left; all_headers.len()];
    let mut frame = table::render(&all_headers, &align, &visible);
    let position = if rows.len() > VISIBLE { format!(" · {}/{}", cur + 1, rows.len()) } else { String::new() };
    let keys = if checked.is_some() {
        "↑↓ moverse · Espacio marcar · a todos · Enter confirmar · Esc cancelar"
    } else {
        "↑↓ moverse · Enter elegir · Esc cancelar"
    };
    frame += &format!("{GRAY}{keys}{position}{RESET}\n");
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn marca_el_cursor_y_hace_scroll() {
        let rows: Vec<Vec<String>> = (0..15).map(|i| vec![format!("conv {i}"), "fecha".into()]).collect();
        let frame = strip(&render(&["Nombre", "Fecha"], &rows, 12, 5, None));
        assert!(frame.contains("❯ │ conv 12"), "{frame}");
        assert!(!frame.contains("conv 4 ") && frame.contains("conv 14"), "{frame}");
        assert!(frame.contains("13/15"));
        assert_eq!(frame.lines().count(), VISIBLE + 5); // 4 bordes/encabezado + filas + ayuda
    }

    #[test]
    fn muestra_casillas_en_seleccion_multiple() {
        let rows: Vec<Vec<String>> = vec![vec!["a.txt".into()], vec!["b.txt".into()]];
        let frame = strip(&render(&["Archivo"], &rows, 0, 0, Some(&[false, true])));
        assert!(frame.contains("❯ │ □ │ a.txt"), "{frame}");
        assert!(frame.contains("  │ ■ │ b.txt"), "{frame}");
        assert!(frame.contains("Espacio marcar"));
    }
}
