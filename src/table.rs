use crate::{BOLD, GRAY, RED, RESET};

/// Alineación de una columna.
#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Right,
}

/// Imprime una tabla con bordes. Las celdas pueden traer colores ANSI.
pub fn print(headers: &[&str], align: &[Align], rows: &[Vec<String>]) {
    print!("{}", render(headers, align, rows));
}

pub fn render(headers: &[&str], align: &[Align], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(cell));
        }
    }

    let colored: Vec<String> = headers.iter().map(|h| format!("{BOLD}{RED}{h}{RESET}")).collect();
    let mut out = border(&widths, "╭", "┬", "╮");
    out += &line(&widths, align, &colored);
    out += &border(&widths, "├", "┼", "┤");
    for row in rows {
        out += &line(&widths, align, row);
    }
    out += &border(&widths, "╰", "┴", "╯");
    out
}

fn border(widths: &[usize], left: &str, mid: &str, right: &str) -> String {
    let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
    format!("{GRAY}{left}{}{right}{RESET}\n", parts.join(mid))
}

fn line(widths: &[usize], align: &[Align], cells: &[String]) -> String {
    let mut out = format!("{GRAY}│{RESET}");
    for (i, cell) in cells.iter().enumerate() {
        let pad = " ".repeat(widths[i] - width(cell));
        let cell = match align[i] {
            Align::Left => format!("{cell}{pad}"),
            Align::Right => format!("{pad}{cell}"),
        };
        out += &format!(" {cell} {GRAY}│{RESET}");
    }
    out + "\n"
}

/// Recorta el texto a `max` caracteres visibles, terminando en "…" si se cortó.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    } else {
        text.to_string()
    }
}

/// Ancho visible del texto: ignora los códigos de color ANSI (\x1b[...m).
pub fn width(s: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            in_escape = c != 'm';
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todas_las_lineas_miden_igual() {
        let rows = vec![
            vec![format!("{RED}rojo{RESET}"), "1".into()],
            vec!["texto largo".into(), "12345".into()],
        ];
        let out = render(&["Nombre", "N"], &[Align::Left, Align::Right], &rows);
        let widths: Vec<usize> = out.lines().map(width).collect();
        assert_eq!(widths.len(), 6);
        assert!(widths.iter().all(|&w| w == widths[0]), "{out}");
        assert_eq!(width("\x1b[1m\x1b[38;2;1;2;3mhola\x1b[0m"), 4);
    }
}
