//! `espacio [carpeta]`: qué ocupa más lugar, de mayor a menor.

use crate::builtins::human_size;
use crate::script::INTERRUPTED;
use crate::table::{self, Align};
use crate::{BLUE, GRAY, RESET, YELLOW};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Cuántas filas se muestran; el resto se resume en una línea.
const SHOWN: usize = 20;

/// Carpetas que se regeneran solas (dependencias, compilados, cachés): se pueden borrar.
const REGENERABLE: &[&str] = &["node_modules", "target", "__pycache__", ".next", ".nuxt", ".cache", ".gradle", ".venv", "venv", "dist", "build"];

struct Item {
    name: String,
    is_dir: bool,
    size: u64,
}

/// Suma lo que ocupa cada cosa que hay directamente dentro de `root` (las carpetas con todo su contenido).
fn measure(root: &Path, progress: &(dyn Fn(u64) + Sync)) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut items = Vec::new();
    let mut index: HashMap<OsString, usize> = HashMap::new();
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let size = if t.is_file() { e.metadata().map_or(0, |m| m.len()) } else { 0 };
        if t.is_dir() {
            index.insert(e.file_name(), items.len());
        }
        items.push(Item { name: e.file_name().to_string_lossy().to_string(), is_dir: t.is_dir(), size });
    }
    // Cada carpeta se recorre en paralelo y todo lo de adentro suma a su total.
    let sizes: Vec<AtomicU64> = items.iter().map(|_| AtomicU64::new(0)).collect();
    let files = AtomicU64::new(0);
    for (name, &i) in &index {
        ignore::WalkBuilder::new(root.join(name))
            .standard_filters(false)
            .build_parallel()
            .run(|| {
                Box::new(|entry| {
                    if INTERRUPTED.load(Ordering::Relaxed) {
                        return ignore::WalkState::Quit;
                    }
                    if let Ok(e) = entry {
                        if e.file_type().is_some_and(|t| t.is_file()) {
                            sizes[i].fetch_add(e.metadata().map_or(0, |m| m.len()), Ordering::Relaxed);
                            let n = files.fetch_add(1, Ordering::Relaxed) + 1;
                            if n % 5000 == 0 {
                                progress(n);
                            }
                        }
                    }
                    ignore::WalkState::Continue
                })
            });
    }
    for (item, size) in items.iter_mut().zip(sizes) {
        item.size += size.into_inner();
    }
    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    items
}

/// Barra de 20 casillas para el porcentaje.
fn bar(part: u64, total: u64) -> String {
    let filled = if total == 0 { 0 } else { (part as f64 / total as f64 * 20.0).round() as usize };
    format!("{}{GRAY}{}{RESET}", "█".repeat(filled), "░".repeat(20 - filled))
}

pub fn espacio_cmd(args: &[String]) -> i32 {
    let target = args.first().map(String::as_str).unwrap_or(".");
    let root = match std::fs::canonicalize(target) {
        Ok(p) if p.is_dir() => p,
        _ => {
            eprintln!("espacio: {target}: no es una carpeta");
            return 1;
        }
    };
    let shown_root = crate::tilde(Path::new(root.display().to_string().trim_start_matches(r"\\?\")));
    let started = std::time::Instant::now();
    let live = std::io::stderr().is_terminal();
    if live {
        eprint!("{GRAY}Midiendo {shown_root}…{RESET}");
    }
    let items = measure(&root, &|n| {
        if live {
            eprint!("\r\x1b[2K{GRAY}Midiendo {shown_root}… {n} archivos{RESET}");
        }
    });
    if live {
        eprint!("\r\x1b[2K");
    }
    if INTERRUPTED.load(Ordering::Relaxed) {
        eprintln!("{GRAY}Cancelado{RESET}");
        return 130;
    }
    if items.is_empty() {
        println!("{GRAY}{shown_root} está vacía{RESET}");
        return 0;
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    let rows: Vec<Vec<String>> = items
        .iter()
        .take(SHOWN)
        .map(|i| {
            let name = if i.is_dir { format!("{BLUE}{}{}{RESET}", i.name, std::path::MAIN_SEPARATOR) } else { i.name.clone() };
            let note = if i.is_dir && REGENERABLE.contains(&i.name.as_str()) { format!(" {YELLOW}se puede borrar{RESET}") } else { String::new() };
            let pct = if total == 0 { 0.0 } else { i.size as f64 * 100.0 / total as f64 };
            vec![name, human_size(i.size), format!("{pct:.0}%"), bar(i.size, total) + &note]
        })
        .collect();
    table::print(&["Nombre", "Tamaño", "%", ""], &[Align::Left, Align::Right, Align::Right, Align::Left], &rows);
    if items.len() > SHOWN {
        let rest = &items[SHOWN..];
        println!("{GRAY}… y {} más ({}){RESET}", rest.len(), human_size(rest.iter().map(|i| i.size).sum()));
    }
    println!(
        "{GRAY}Total {} en {shown_root} ({}) · espacio <carpeta> para ver dentro{RESET}",
        human_size(total),
        crate::format_duration(started.elapsed())
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn suma_carpetas_completas_y_ordena() {
        let base = std::env::temp_dir().join(format!("fysh-espacio-{}", std::process::id()));
        fs::create_dir_all(base.join("grande").join("sub")).unwrap();
        fs::create_dir_all(base.join("vacia")).unwrap();
        fs::write(base.join("grande").join("a.bin"), vec![0u8; 3000]).unwrap();
        fs::write(base.join("grande").join("sub").join("b.bin"), vec![0u8; 2000]).unwrap();
        fs::write(base.join("chico.txt"), vec![0u8; 100]).unwrap();
        let items = measure(&base, &|_| {});
        let got: Vec<(&str, u64, bool)> = items.iter().map(|i| (i.name.as_str(), i.size, i.is_dir)).collect();
        assert_eq!(got, [("grande", 5000, true), ("chico.txt", 100, false), ("vacia", 0, true)]);
        fs::remove_dir_all(&base).unwrap();
    }
}
