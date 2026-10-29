//! `renombrar <texto> <nuevo>`: cambia el nombre de muchos archivos de la carpeta a la vez,
//! con vista previa antes de tocar nada y `renombrar -deshacer` para volver atrás.

use crate::ask::confirm;
use crate::builtins::error_msg;
use crate::table::{self, Align};
use crate::{home, GRAY, GREEN, RED, RESET};
use std::path::{Path, PathBuf};

/// Filas de la vista previa; el resto se resume.
const SHOWN: usize = 20;

fn undo_path() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("renombrar-deshacer.tsv"))
}

/// Posición (en bytes) de `pat` dentro de `name`, sin importar mayúsculas.
fn find_ci(name: &str, pat: &str) -> Option<(usize, usize)> {
    let pat = pat.to_lowercase();
    let n = pat.chars().count();
    name.char_indices().find_map(|(start, _)| {
        let end = name[start..].char_indices().nth(n).map_or(name.len(), |(i, _)| start + i);
        (name[start..end].to_lowercase() == pat).then_some((start, end))
    })
}

/// Reemplaza todas las veces que aparece `pat` (sin importar mayúsculas).
fn replace_ci(name: &str, pat: &str, new: &str) -> String {
    let mut out = String::new();
    let mut rest = name;
    while let Some((s, e)) = find_ci(rest, pat) {
        out += &rest[..s];
        out += new;
        rest = &rest[e..];
    }
    out + rest
}

/// Nombres nuevos para `names` (ya ordenados). Con {n} en `new`, el nombre entero pasa a ser
/// `new` con el número (y la extensión de antes si `new` no trae una); si no, se reemplaza el texto.
fn plan(names: &[String], pat: &str, new: &str) -> Vec<(String, String)> {
    let matched: Vec<&String> = names.iter().filter(|n| find_ci(n, pat).is_some()).collect();
    let width = matched.len().to_string().len();
    matched
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let renamed = if new.contains("{n}") {
                let base = new.replace("{n}", &format!("{:0width$}", i + 1));
                match Path::new(name.as_str()).extension() {
                    Some(ext) if Path::new(&base).extension().is_none() => format!("{base}.{}", ext.to_string_lossy()),
                    _ => base,
                }
            } else {
                replace_ci(name, pat, new)
            };
            (name.to_string(), renamed)
        })
        .filter(|(a, b)| a != b)
        .collect()
}

/// Por qué no se puede aplicar el plan (nombres repetidos o que ya existen), si es el caso.
fn conflict(dir: &Path, changes: &[(String, String)]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    for (_, new) in changes {
        if new.is_empty() || new.contains(['/', '\\']) {
            return Some(format!("nombre no válido: \"{new}\""));
        }
        if !seen.insert(new.to_lowercase()) {
            return Some(format!("dos archivos terminarían llamándose \"{new}\""));
        }
        let renamed_away = changes.iter().any(|(old, _)| old.eq_ignore_ascii_case(new));
        if dir.join(new).exists() && !renamed_away {
            return Some(format!("ya existe \"{new}\""));
        }
    }
    None
}

/// Aplica los cambios en dos pasos (primero a nombres temporales) para que a→b y b→a no choquen.
fn apply(dir: &Path, changes: &[(String, String)]) -> Result<(), String> {
    let tmp = |i: usize| dir.join(format!(".fysh-renombrar-{}-{i}", std::process::id()));
    for (i, (old, _)) in changes.iter().enumerate() {
        std::fs::rename(dir.join(old), tmp(i)).map_err(|e| format!("{old}: {}", error_msg(&e)))?;
    }
    for (i, (_, new)) in changes.iter().enumerate() {
        std::fs::rename(tmp(i), dir.join(new)).map_err(|e| format!("{new}: {}", error_msg(&e)))?;
    }
    Ok(())
}

fn preview(changes: &[(String, String)]) {
    let rows: Vec<Vec<String>> = changes.iter().take(SHOWN).map(|(a, b)| vec![a.clone(), "→".into(), format!("{GREEN}{b}{RESET}")]).collect();
    table::print(&["Antes", "", "Después"], &[Align::Left, Align::Left, Align::Left], &rows);
    if changes.len() > SHOWN {
        println!("{GRAY}… y {} más{RESET}", changes.len() - SHOWN);
    }
}

pub fn renombrar_cmd(args: &[String]) -> i32 {
    if args.first().is_some_and(|a| a == "-deshacer") {
        return undo();
    }
    let [pat, new] = args else {
        eprintln!("{GRAY}uso: renombrar <texto> <nuevo>   (renombrar IMG_ foto- · renombrar IMG_ vacaciones-{{n}}){RESET}");
        eprintln!("{GRAY}     renombrar -deshacer          vuelve atrás el último cambio{RESET}");
        return 1;
    };
    let Ok(dir) = std::env::current_dir() else { return 1 };
    let mut names: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect(),
        Err(e) => {
            eprintln!("renombrar: {}", error_msg(&e));
            return 1;
        }
    };
    names.sort_by_key(|n| n.to_lowercase());
    let changes = plan(&names, pat, new);
    if changes.is_empty() {
        println!("{GRAY}Nada que renombrar: ningún nombre contiene \"{pat}\"{RESET}");
        return 1;
    }
    preview(&changes);
    if let Some(why) = conflict(&dir, &changes) {
        eprintln!("{RED}✗{RESET} No se cambió nada: {why}");
        return 1;
    }
    if !confirm(&format!("¿Renombrar {}?", if changes.len() == 1 { "1 elemento".into() } else { format!("{} elementos", changes.len()) })) {
        return 1;
    }
    if let Err(e) = apply(&dir, &changes) {
        eprintln!("renombrar: {e}");
        return 1;
    }
    // Para deshacer: carpeta y cada cambio al revés.
    if let Some(p) = undo_path() {
        let text: String = changes.iter().map(|(a, b)| format!("{}\t{b}\t{a}\n", dir.display())).collect();
        let _ = std::fs::create_dir_all(p.parent().unwrap()).and_then(|_| std::fs::write(&p, text));
    }
    println!("{GREEN}✓{RESET} {} renombrados {GRAY}· renombrar -deshacer para volver atrás{RESET}", changes.len());
    0
}

fn undo() -> i32 {
    let text = undo_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let mut dir = PathBuf::new();
    let changes: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            dir = PathBuf::from(f.next()?);
            Some((f.next()?.to_string(), f.next()?.to_string()))
        })
        .collect();
    if changes.is_empty() {
        println!("{GRAY}No hay nada que deshacer{RESET}");
        return 1;
    }
    println!("{GRAY}En {}{RESET}", dir.display());
    preview(&changes);
    if !confirm("¿Deshacer?") {
        return 1;
    }
    if let Err(e) = apply(&dir, &changes) {
        eprintln!("renombrar: {e}");
        return 1;
    }
    let _ = undo_path().map(std::fs::remove_file);
    println!("{GREEN}✓{RESET} Deshecho");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arma_los_nombres_nuevos() {
        let files = names(&["IMG_001.jpg", "img_002.JPG", "notas.txt"]);
        assert_eq!(plan(&files, "img_", "foto-"), [("IMG_001.jpg".into(), "foto-001.jpg".into()), ("img_002.JPG".into(), "foto-002.JPG".into())]);
        assert_eq!(
            plan(&files, "IMG", "playa-{n}"),
            [("IMG_001.jpg".into(), "playa-1.jpg".into()), ("img_002.JPG".into(), "playa-2.JPG".into())],
            "con {{n}} se numera y se mantiene la extensión"
        );
        let many: Vec<String> = (1..=12).map(|i| format!("a{i:02}.png")).collect();
        assert_eq!(plan(&many, "a", "b{n}")[0].1, "b01.png", "relleno con ceros según cuántos hay");
        assert!(plan(&files, "zzz", "x").is_empty());
        assert_eq!(replace_ci("Ñandú ÑANDÚ", "ñandú", "x"), "x x");
    }

    #[test]
    fn detecta_choques_y_aplica() {
        let dir = std::env::temp_dir().join(format!("fysh-renombrar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        let swap = vec![("a.txt".to_string(), "b.txt".to_string()), ("b.txt".to_string(), "a.txt".to_string())];
        assert!(conflict(&dir, &swap).is_none(), "intercambiar nombres está permitido");
        apply(&dir, &swap).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "b.txt");
        assert!(conflict(&dir, &[("a.txt".into(), "c.txt".into())]).is_some(), "c.txt ya existe");
        assert!(conflict(&dir, &[("a.txt".into(), "x".into()), ("b.txt".into(), "X".into())]).is_some(), "repetidos");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
