//! `z`: saltar a carpetas que visitas seguido. fysh anota cada carpeta donde entras
//! (~/.fysh/carpetas.tsv) y las ordena por visitas y por qué tan reciente fue la última.

use crate::picker;
use crate::table::{self, Align};
use crate::{home, tilde, GRAY, RESET};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// Máximo de carpetas recordadas; las de menor puntaje se olvidan.
const MAX_DIRS: usize = 500;

#[derive(Debug, Clone)]
struct Dir {
    path: PathBuf,
    visits: u32,
    last: i64, // segundos desde 1970
}

fn data_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("carpetas.tsv"))
}

fn load() -> Vec<Dir> {
    let Some(text) = data_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some(Dir { visits: p.next()?.parse().ok()?, last: p.next()?.parse().ok()?, path: PathBuf::from(p.next()?) })
        })
        .collect()
}

fn save(dirs: &[Dir]) {
    let Some(file) = data_file() else { return };
    let text: String = dirs.iter().map(|d| format!("{}\t{}\t{}\n", d.visits, d.last, d.path.display())).collect();
    let _ = file.parent().map(std::fs::create_dir_all);
    let _ = std::fs::write(file, text);
}

fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// Más visitas y más reciente = más puntaje (como zoxide).
fn score(d: &Dir, now: i64) -> f64 {
    let age = now - d.last;
    let weight = match age {
        ..=3_600 => 4.0,
        ..=86_400 => 2.0,
        ..=604_800 => 0.5,
        _ => 0.25,
    };
    d.visits as f64 * weight
}

/// Anota una visita a `path`.
pub fn visit(path: &Path) {
    let now = chrono::Local::now().timestamp();
    let mut dirs = load();
    match dirs.iter_mut().find(|d| same(&d.path, path)) {
        Some(d) => {
            d.visits += 1;
            d.last = now;
        }
        None => dirs.push(Dir { path: path.to_path_buf(), visits: 1, last: now }),
    }
    if dirs.len() > MAX_DIRS {
        dirs.sort_by(|a, b| score(b, now).total_cmp(&score(a, now)));
        dirs.truncate(MAX_DIRS);
    }
    save(&dirs);
}

/// ¿La ruta contiene todas las palabras? La última debe estar en el nombre final de la carpeta,
/// así `z crm` prefiere ...\autospot-CRM antes que ...\autospot-CRM\frontend.
fn matches(path: &Path, words: &[String]) -> bool {
    let full = path.to_string_lossy().to_lowercase();
    let last = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    words.iter().all(|w| full.contains(&w.to_lowercase()))
        && words.last().is_some_and(|w| last.contains(&w.to_lowercase()))
}

/// Carpetas que todavía existen, de mayor a menor puntaje.
fn ranked() -> Vec<Dir> {
    let now = chrono::Local::now().timestamp();
    let mut dirs: Vec<Dir> = load().into_iter().filter(|d| d.path.is_dir()).collect();
    dirs.sort_by(|a, b| score(b, now).total_cmp(&score(a, now)));
    dirs
}

fn go(path: &Path) -> i32 {
    match std::env::set_current_dir(path) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("z: {}: {}", path.display(), crate::builtins::error_msg(&e));
            1
        }
    }
}

/// `z palabra...` salta a la mejor carpeta que coincida; `z` solo abre el selector.
pub fn z_cmd(args: &[String]) -> i32 {
    let dirs = ranked();
    if args.is_empty() {
        if dirs.is_empty() {
            println!("{GRAY}Todavía no hay carpetas recordadas. Muévete con cd y z las irá aprendiendo.{RESET}");
            return 0;
        }
        let rows: Vec<Vec<String>> = dirs
            .iter()
            .take(50)
            .map(|d| {
                let when = chrono::DateTime::from_timestamp(d.last, 0)
                    .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                vec![tilde(&d.path), format!("{GRAY}{}{RESET}", d.visits), format!("{GRAY}{when}{RESET}")]
            })
            .collect();
        if !std::io::stdin().is_terminal() {
            table::print(&["Carpeta", "Visitas", "Última vez"], &[Align::Left, Align::Right, Align::Left], &rows);
            return 0;
        }
        return match picker::select_one(&["Carpeta", "Visitas", "Última vez"], &rows) {
            Some(i) => go(&dirs[i].path),
            None => 0,
        };
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    match dirs.iter().find(|d| matches(&d.path, args) && !same(&d.path, &cwd)) {
        Some(d) => go(&d.path),
        None => {
            eprintln!("z: no conozco ninguna carpeta que coincida con \"{}\".", args.join(" "));
            eprintln!("{GRAY}Entra una vez con cd y después z la recordará.{RESET}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn coincide_con_la_ultima_carpeta() {
        // Rutas armadas con join: valen en Windows, Linux y Mac.
        let crm = std::env::temp_dir().join("Documents").join("autospot-CRM");
        let front = crm.join("frontend");
        assert!(matches(&crm, &words(&["crm"])));
        assert!(!matches(&front, &words(&["crm"])));
        assert!(matches(&front, &words(&["crm", "front"])));
        assert!(!matches(&crm, &words(&["fysh"])));
    }

    #[test]
    fn lo_reciente_pesa_mas() {
        let now = 10_000_000;
        let reciente = Dir { path: "a".into(), visits: 3, last: now - 60 };
        let vieja = Dir { path: "b".into(), visits: 10, last: now - 30 * 86_400 };
        assert!(score(&reciente, now) > score(&vieja, now));
    }
}
