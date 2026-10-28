//! `history`: los comandos usados, sin repetir, para elegir uno y volver a usarlo.

use crate::picker;
use crate::table::{self, truncate, Align};
use crate::{GRAY, RESET};
use std::collections::HashMap;
use std::io::{self, IsTerminal};

/// Agrupa el historial (del más viejo al más nuevo) en comandos únicos con cuántas veces se usaron,
/// el usado más recientemente primero. Omite líneas vacías y el propio `history`.
pub fn group<'a>(entries: impl DoubleEndedIterator<Item = &'a str>, filter: Option<&str>) -> Vec<(String, usize)> {
    let filter = filter.map(str::to_lowercase);
    let mut order: Vec<String> = Vec::new();
    let mut count: HashMap<String, usize> = HashMap::new();
    for entry in entries.rev() {
        let cmd = entry.trim();
        if cmd.is_empty() || cmd == "history" || cmd.starts_with("history ") {
            continue;
        }
        if filter.as_ref().is_some_and(|f| !cmd.to_lowercase().contains(f.as_str())) {
            continue;
        }
        let n = count.entry(cmd.to_string()).or_insert(0);
        if *n == 0 {
            order.push(cmd.to_string());
        }
        *n += 1;
    }
    order.into_iter().map(|c| { let n = count[&c]; (c, n) }).collect()
}

/// Muestra el selector y devuelve el comando elegido para dejarlo escrito en la línea.
pub fn pick<'a>(entries: impl DoubleEndedIterator<Item = &'a str>, filter: Option<&str>) -> Option<String> {
    let commands = group(entries, filter);
    if commands.is_empty() {
        match filter {
            Some(f) => println!("{GRAY}No hay comandos en el historial que contengan \"{f}\".{RESET}"),
            None => println!("{GRAY}El historial está vacío.{RESET}"),
        }
        return None;
    }
    let rows: Vec<Vec<String>> = commands
        .iter()
        .map(|(cmd, n)| vec![truncate(cmd, 70), format!("{GRAY}{n}{RESET}")])
        .collect();

    if !io::stdin().is_terminal() {
        table::print(&["Comando", "Veces"], &[Align::Left, Align::Right], &rows);
        return None;
    }
    println!("{GRAY}{} comandos distintos · el elegido queda escrito en la línea para editarlo o darle Enter{RESET}", commands.len());
    let i = picker::select_one(&["Comando", "Veces"], &rows)?;
    Some(commands[i].0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agrupa_sin_repetir_y_cuenta() {
        let entries = ["git add .", "dir", "git add .", "  dir  ", "history", "git status", "", "git add ."];
        let grouped = group(entries.iter().copied(), None);
        assert_eq!(
            grouped,
            [("git add .".to_string(), 3), ("git status".to_string(), 1), ("dir".to_string(), 2)]
        );
    }

    #[test]
    fn filtra_sin_distinguir_mayusculas() {
        let entries = ["git add .", "dir", "GIT status", "history git"];
        let grouped = group(entries.iter().copied(), Some("git"));
        assert_eq!(grouped.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["GIT status", "git add ."]);
    }
}
