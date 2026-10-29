//! `extraer` y `comprimir`: zip, tar.gz, 7z… sin recordar las letras de tar.
//! Usa el `tar` del sistema (en Windows y Mac entiende zip; en Linux el zip va con unzip/zip).

use crate::builtins::{error_msg, human_size};
use crate::{GRAY, GREEN, RESET};
use std::path::{Path, PathBuf};
use std::process::Command;

const EXTENSIONS: &[&str] = &[".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tgz", ".tbz2", ".txz", ".tar", ".zip", ".7z", ".rar"];

/// "fotos.tar.gz" → "fotos". None si no es un comprimido conocido.
fn stem(name: &str) -> Option<&str> {
    let lower = name.to_lowercase();
    EXTENSIONS.iter().find(|e| lower.ends_with(*e) && lower.len() > e.len()).map(|e| &name[..name.len() - e.len()])
}

fn is_zip(path: &Path) -> bool {
    path.to_string_lossy().to_lowercase().ends_with(".zip")
}

/// En Windows, el tar de System32: el de Git (si va antes en el PATH) no entiende zip ni rutas C:\.
fn tar() -> Command {
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        Command::new(Path::new(&root).join("System32").join("tar.exe"))
    } else {
        Command::new("tar")
    }
}

/// Linux: tar (GNU) no maneja zip; para eso unzip y zip.
fn zip_tools() -> bool {
    cfg!(target_os = "linux")
}

/// "fotos.zip" si no existe; si no, "fotos (2).zip", "fotos (3).zip"…
pub fn unique(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let base_len = stem(&name).map(str::len).or_else(|| name.rfind('.').filter(|&i| i > 0)).unwrap_or(name.len());
    let (base, ext) = (name[..base_len].to_string(), name[base_len..].to_string());
    (2..).map(|n| path.with_file_name(format!("{base} ({n}){ext}"))).find(|p| !p.exists()).unwrap()
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("{what} no pudo terminar")),
        Err(e) => Err(format!("{what}: {}", error_msg(&e))),
    }
}

/// Si lo extraído es una sola carpeta (fotos/fotos/…), la sube un nivel.
fn flatten(dest: &Path) -> std::io::Result<()> {
    let entries: Vec<_> = std::fs::read_dir(dest)?.flatten().collect();
    if let [only] = entries.as_slice() {
        if only.file_type()?.is_dir() {
            let tmp = unique(dest.with_file_name(format!("{}.fysh-tmp", dest.file_name().unwrap_or_default().to_string_lossy())));
            std::fs::rename(dest, &tmp)?;
            std::fs::rename(tmp.join(only.file_name()), dest)?;
            std::fs::remove_dir(&tmp)?;
        }
    }
    Ok(())
}

/// `extraer <archivo> [carpeta]`: lo descomprime en una carpeta con su nombre, al lado.
pub fn extraer_cmd(args: &[String]) -> i32 {
    let Some(file) = args.first() else {
        eprintln!("{GRAY}uso: extraer <archivo.zip|.tar.gz|.7z|…> [carpeta]{RESET}");
        return 1;
    };
    let path = Path::new(file);
    if !path.is_file() {
        eprintln!("extraer: {file}: no existe");
        return 1;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let dest = match args.get(1) {
        Some(d) => PathBuf::from(d),
        None => unique(path.with_file_name(stem(&name).unwrap_or(&name))),
    };
    if let Err(e) = std::fs::create_dir_all(&dest) {
        eprintln!("extraer: {}: {}", dest.display(), error_msg(&e));
        return 1;
    }
    let result = if is_zip(path) && zip_tools() {
        let mut cmd = Command::new("unzip");
        cmd.arg("-q").arg(path).arg("-d").arg(&dest);
        run(cmd, "unzip (instálalo con: sudo apt install unzip)")
    } else {
        let mut cmd = tar();
        cmd.arg("-xf").arg(path).arg("-C").arg(&dest);
        run(cmd, "tar")
    };
    if let Err(e) = result {
        let _ = std::fs::remove_dir(&dest); // solo si quedó vacía
        eprintln!("extraer: {e}");
        return 1;
    }
    if args.get(1).is_none() {
        let _ = flatten(&dest);
    }
    let count = ignore::WalkBuilder::new(&dest).standard_filters(false).build().filter(|e| e.as_ref().is_ok_and(|e| e.depth() > 0)).count();
    println!("{GREEN}✓{RESET} {} {GRAY}({count} elementos){RESET}", dest.display());
    0
}

/// `comprimir <ruta>... [-o nombre.zip|.tar.gz]`: por defecto crea <primera>.zip al lado.
pub fn comprimir_cmd(args: &[String]) -> i32 {
    let mut out: Option<PathBuf> = None;
    let mut items: Vec<PathBuf> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => out = it.next().map(PathBuf::from),
            _ => items.push(PathBuf::from(a)),
        }
    }
    if items.is_empty() {
        eprintln!("{GRAY}uso: comprimir <archivo o carpeta>... [-o nombre.zip|.tar.gz]{RESET}");
        return 1;
    }
    for i in &items {
        if !i.exists() {
            eprintln!("comprimir: {}: no existe", i.display());
            return 1;
        }
    }
    // Rutas completas: así cada cosa entra con su nombre y no con la ruta desde aquí.
    let items: Vec<PathBuf> = items.iter().filter_map(|i| std::path::absolute(i).ok()).collect();
    let out = match out {
        Some(o) if stem(&o.to_string_lossy()).is_some() => o,
        Some(o) => PathBuf::from(format!("{}.zip", o.display())),
        None => {
            let first = items[0].file_name().map_or("archivo".into(), |n| n.to_string_lossy().to_string());
            unique(PathBuf::from(format!("{first}.zip")))
        }
    };
    if out.exists() {
        eprintln!("comprimir: {} ya existe; elige otro nombre con -o", out.display());
        return 1;
    }
    let result = if is_zip(&out) && zip_tools() {
        let out_abs = std::path::absolute(&out).unwrap_or(out.clone());
        items.iter().try_for_each(|i| {
            let mut cmd = Command::new("zip");
            cmd.arg("-rq").arg(&out_abs).arg(i.file_name().unwrap_or_default());
            cmd.current_dir(i.parent().unwrap_or(Path::new(".")));
            run(cmd, "zip (instálalo con: sudo apt install zip)")
        })
    } else {
        let mut cmd = tar();
        cmd.arg("-a").arg("-cf").arg(&out);
        for i in &items {
            cmd.arg("-C").arg(i.parent().unwrap_or(Path::new("."))).arg(i.file_name().unwrap_or_default());
        }
        run(cmd, "tar")
    };
    match result {
        Ok(()) => {
            let size = std::fs::metadata(&out).map_or(0, |m| m.len());
            println!("{GREEN}✓{RESET} {} {GRAY}({}){RESET}", out.display(), human_size(size));
            0
        }
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            eprintln!("comprimir: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombres_de_comprimidos() {
        assert_eq!(stem("fotos.zip"), Some("fotos"));
        assert_eq!(stem("Backup.TAR.GZ"), Some("Backup"));
        assert_eq!(stem("v1.2.tgz"), Some("v1.2"));
        assert_eq!(stem("notas.txt"), None);
        assert_eq!(stem(".zip"), None);
    }

    #[test]
    fn comprime_y_extrae() {
        let base = std::env::temp_dir().join(format!("fysh-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("fotos").join("sub")).unwrap();
        std::fs::write(base.join("fotos").join("sub").join("a.txt"), "hola").unwrap();
        let s = |p: PathBuf| p.display().to_string();
        let ext = if zip_tools() { "tar.gz" } else { "zip" }; // en Linux puede no haber zip instalado
        let out = base.join(format!("fotos.{ext}"));
        assert_eq!(comprimir_cmd(&[s(base.join("fotos")), "-o".into(), s(out.clone())]), 0);
        std::fs::remove_dir_all(base.join("fotos")).unwrap();
        assert_eq!(extraer_cmd(&[s(out)]), 0);
        // Sin carpeta repetida: fotos/sub/a.txt, no fotos/fotos/sub/a.txt
        assert_eq!(std::fs::read_to_string(base.join("fotos").join("sub").join("a.txt")).unwrap(), "hola");
        std::fs::remove_dir_all(&base).unwrap();
    }
}
