//! Comandos de archivos: mkdir, rm, cp, mv, cat, touch y open.
//! rm no borra para siempre: manda a la papelera. cp y mv preguntan antes de reemplazar,
//! y lo reemplazado también va a la papelera, así nada se pierde por un error.

use crate::ask::confirm;
use crate::builtins::error_msg;
use crate::{GRAY, GREEN, RESET};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn usage(text: &str) -> i32 {
    eprintln!("{GRAY}uso: {text}{RESET}");
    1
}

fn fail(cmd: &str, path: &str, e: &io::Error) -> i32 {
    eprintln!("{cmd}: {path}: {}", error_msg(e));
    1
}

fn done(text: &str) {
    println!("{GREEN}✓{RESET} {text}");
}

/// `mkdir carpeta...`: crea las carpetas (y las intermedias que falten).
pub fn mkdir(args: &[String]) -> i32 {
    if args.is_empty() {
        return usage("mkdir <carpeta>...");
    }
    let mut code = 0;
    for a in args {
        match fs::create_dir_all(a) {
            Ok(()) => done(a),
            Err(e) => code = fail("mkdir", a, &e),
        }
    }
    code
}

/// `rm archivo...`: manda archivos y carpetas a la papelera. Con carpetas pide confirmación.
pub fn rm(args: &[String]) -> i32 {
    if args.is_empty() {
        return usage("rm <archivo o carpeta>...   (van a la papelera, se pueden recuperar)");
    }
    let mut code = 0;
    for a in args {
        let path = Path::new(a);
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                code = fail("rm", a, &e);
                continue;
            }
        };
        if meta.is_dir() && !confirm(&format!("¿Mover la carpeta {a} ({} elementos) a la papelera?", count(path))) {
            continue;
        }
        match trash::delete(path) {
            Ok(()) => done(&format!("{a} {GRAY}→ papelera{RESET}")),
            Err(e) => {
                eprintln!("rm: {a}: {e}");
                code = 1;
            }
        }
    }
    code
}

/// Cuántos archivos y carpetas hay dentro de una carpeta (para avisar antes de borrarla).
fn count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries.filter_map(Result::ok).map(|e| if e.path().is_dir() { 1 + count(&e.path()) } else { 1 }).sum()
        })
        .unwrap_or(0)
}

/// `cp origen... destino`
pub fn cp(args: &[String]) -> i32 {
    transfer("cp", args, false)
}

/// `mv origen... destino` (también sirve para renombrar)
pub fn mv(args: &[String]) -> i32 {
    transfer("mv", args, true)
}

fn transfer(cmd: &str, args: &[String], moving: bool) -> i32 {
    if args.len() < 2 {
        return usage(&format!("{cmd} <origen>... <destino>"));
    }
    let (sources, dest) = args.split_at(args.len() - 1);
    let dest = Path::new(&dest[0]);
    if sources.len() > 1 && !dest.is_dir() {
        eprintln!("{cmd}: con varios orígenes el destino debe ser una carpeta que exista");
        return 1;
    }
    let mut code = 0;
    for s in sources {
        let src = Path::new(s);
        if let Err(e) = fs::symlink_metadata(src) {
            code = fail(cmd, s, &e);
            continue;
        }
        let target: PathBuf = if dest.is_dir() { dest.join(src.file_name().unwrap_or_default()) } else { dest.to_path_buf() };
        if same_path(src, &target) {
            eprintln!("{cmd}: {s}: origen y destino son el mismo");
            code = 1;
            continue;
        }
        if target.exists() {
            if !confirm(&format!("{} ya existe. ¿Reemplazarlo? (lo anterior va a la papelera)", target.display())) {
                continue;
            }
            if let Err(e) = trash::delete(&target) {
                eprintln!("{cmd}: no se pudo mover {} a la papelera: {e}", target.display());
                code = 1;
                continue;
            }
        }
        let result = if moving { move_path(src, &target) } else { copy_path(src, &target) };
        match result {
            Ok(()) => done(&format!("{s} {GRAY}→ {}{RESET}", target.display())),
            Err(e) => code = fail(cmd, s, &e),
        }
    }
    code
}

fn same_path(a: &Path, b: &Path) -> bool {
    matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

fn copy_path(src: &Path, dst: &Path) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_path(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

/// Renombrar es instantáneo; si el destino está en otro disco, copia y después borra el origen.
fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_path(src, dst)?;
    if src.is_dir() {
        fs::remove_dir_all(src)
    } else {
        fs::remove_file(src)
    }
}

/// `cat archivo...`: muestra el contenido.
pub fn cat(args: &[String]) -> i32 {
    use std::io::IsTerminal;
    if args.is_empty() {
        // Sin archivos, en un pipe: copia lo que llega (ej: `git log | cat > log.txt`).
        if !io::stdin().is_terminal() {
            return io::copy(&mut io::stdin().lock(), &mut io::stdout().lock()).map_or(1, |_| 0);
        }
        return usage("cat <archivo>...");
    }
    let mut code = 0;
    for a in args {
        if Path::new(a).is_dir() {
            eprintln!("cat: {a}: es una carpeta (usa dir {a})");
            code = 1;
            continue;
        }
        match fs::read(a) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                print!("{text}");
                if !text.is_empty() && !text.ends_with('\n') {
                    println!();
                }
            }
            Err(e) => code = fail("cat", a, &e),
        }
    }
    code
}

/// `touch archivo...`: crea el archivo vacío o, si existe, actualiza su fecha.
pub fn touch(args: &[String]) -> i32 {
    if args.is_empty() {
        return usage("touch <archivo>...");
    }
    let mut code = 0;
    for a in args {
        let result = if Path::new(a).exists() {
            fs::File::options().write(true).open(a).and_then(|f| f.set_modified(std::time::SystemTime::now()))
        } else {
            fs::File::create(a).map(|_| ())
        };
        match result {
            Ok(()) => done(a),
            Err(e) => code = fail("touch", a, &e),
        }
    }
    code
}

/// `open [ruta]`: abre la carpeta (o archivo, o URL) con la aplicación del sistema.
pub fn open(args: &[String]) -> i32 {
    let target = args.first().map(String::as_str).unwrap_or(".");
    // Rutas relativas a absolutas: el Explorador no siempre entiende "." o "..".
    let target = match std::env::current_dir() {
        Ok(cwd) if Path::new(target).exists() => cwd.join(target).display().to_string(),
        _ => target.to_string(),
    };
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // explorer.exe devuelve 1 aunque funcione: solo cuenta si no se pudo lanzar.
    match Command::new(program).arg(&target).status() {
        Ok(_) => 0,
        Err(e) => fail("open", &target, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copia_y_mueve_carpetas_completas() {
        let base = std::env::temp_dir().join(format!("fysh-files-{}", std::process::id()));
        let src = base.join("origen");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "hola").unwrap();
        fs::write(src.join("sub").join("b.txt"), "chao").unwrap();

        copy_path(&src, &base.join("copia")).unwrap();
        assert_eq!(fs::read_to_string(base.join("copia").join("sub").join("b.txt")).unwrap(), "chao");
        assert!(src.exists(), "copiar no borra el origen");
        assert_eq!(count(&src), 3); // a.txt, sub, sub/b.txt

        move_path(&src, &base.join("movida")).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(base.join("movida").join("a.txt")).unwrap(), "hola");
        assert!(same_path(&base.join("movida"), &base.join("movida").join("..").join("movida")));

        fs::remove_dir_all(&base).unwrap();
    }
}
