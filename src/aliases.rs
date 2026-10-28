//! Alias y archivo de configuración ~/.fyshrc.
//! Cada línea de .fyshrc se ejecuta al abrir fysh (alias, set, cd...). `alias` y `unalias`
//! actualizan ese archivo solos; `config` lo abre en el editor y lo recarga al cerrarlo.

use crate::table::{self, Align};
use crate::{parser, Shell, GRAY, GREEN, RESET};
use std::fs;
use std::io;
use std::path::PathBuf;

const TEMPLATE: &str = "# Configuración de fysh. Cada línea se ejecuta al abrir fysh; # = comentario.
# Ejemplos:
# alias gs = git status
# alias crm = cd ~\\Documents\\autospot-CRM
# set EDITOR=code
";

pub fn rc_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".fyshrc"))
}

/// Ejecuta ~/.fyshrc al abrir fysh. Los alias definidos aquí no se vuelven a escribir en el archivo.
pub fn load(shell: &mut Shell) {
    let Some(text) = rc_path().and_then(|p| fs::read_to_string(p).ok()) else { return };
    shell.loading_rc = true;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        crate::execute(shell, line);
    }
    shell.loading_rc = false;
}

/// Si la primera palabra es un alias, la reemplaza por su comando (una sola vez, sin recursión).
pub fn expand(shell: &Shell, args: Vec<String>) -> Vec<String> {
    match shell.aliases.get(&args[0]) {
        Some(value) => {
            let mut expanded = parser::parse(value, shell);
            expanded.extend_from_slice(&args[1..]);
            expanded
        }
        None => args,
    }
}

/// "gs = git status" → ("gs", "git status"). También acepta "gs=git status".
fn parse_definition(text: &str) -> Option<(String, String)> {
    let (name, value) = text.split_once('=')?;
    let (name, value) = (name.trim(), value.trim());
    (!name.is_empty() && !name.contains(char::is_whitespace) && !value.is_empty()).then(|| (name.into(), value.into()))
}

/// `alias` lista; `alias nombre` muestra uno; `alias nombre = comando` crea o cambia uno.
/// Recibe el texto tal cual se escribió después de "alias" (para no perder comillas).
pub fn alias_cmd(shell: &mut Shell, raw: &str) -> i32 {
    let raw = raw.trim();
    if raw.is_empty() {
        if shell.aliases.is_empty() {
            println!("{GRAY}No hay alias. Ejemplo: alias gs = git status{RESET}");
        } else {
            let rows: Vec<Vec<String>> = shell.aliases.iter().map(|(k, v)| vec![k.clone(), v.clone()]).collect();
            table::print(&["Alias", "Comando"], &[Align::Left, Align::Left], &rows);
        }
        return 0;
    }
    if !raw.contains('=') {
        return match shell.aliases.get(raw) {
            Some(v) => {
                println!("{raw} = {v}");
                0
            }
            None => {
                eprintln!("alias: {raw} no existe");
                1
            }
        };
    }
    let Some((name, value)) = parse_definition(raw) else {
        eprintln!("{GRAY}uso: alias nombre = comando{RESET}");
        return 1;
    };
    shell.aliases.insert(name.clone(), value.clone());
    if shell.loading_rc {
        return 0;
    }
    match save(&name, Some(&value)) {
        Ok(()) => {
            println!("{GREEN}✓{RESET} {name} = {value} {GRAY}(guardado en ~/.fyshrc){RESET}");
            0
        }
        Err(e) => {
            eprintln!("alias: no se pudo guardar en ~/.fyshrc: {e}");
            1
        }
    }
}

/// `unalias nombre`: borra el alias de la sesión y de ~/.fyshrc.
pub fn unalias_cmd(shell: &mut Shell, args: &[String]) -> i32 {
    let Some(name) = args.first() else {
        eprintln!("{GRAY}uso: unalias nombre{RESET}");
        return 1;
    };
    if shell.aliases.remove(name).is_none() {
        eprintln!("unalias: {name} no existe");
        return 1;
    }
    match save(name, None) {
        Ok(()) => {
            println!("{GREEN}✓{RESET} alias {name} eliminado");
            0
        }
        Err(e) => {
            eprintln!("unalias: no se pudo actualizar ~/.fyshrc: {e}");
            1
        }
    }
}

/// Cambia (Some) o quita (None) la línea `alias name = ...` de ~/.fyshrc, sin tocar las demás.
fn save(name: &str, value: Option<&str>) -> io::Result<()> {
    let path = rc_path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no se encontró la carpeta de usuario"))?;
    let text = fs::read_to_string(&path).unwrap_or_else(|_| TEMPLATE.to_string());
    fs::write(&path, update_rc(&text, name, value))
}

fn update_rc(text: &str, name: &str, value: Option<&str>) -> String {
    let is_this_alias = |line: &str| {
        line.trim().strip_prefix("alias ").and_then(parse_definition).is_some_and(|(n, _)| n == name)
    };
    let new_line = value.map(|v| format!("alias {name} = {v}"));
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        if is_this_alias(line) {
            if let (Some(l), false) = (&new_line, replaced) {
                lines.push(l.clone()); // reemplazar en el mismo lugar
                replaced = true;
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if let (Some(l), false) = (new_line, replaced) {
        lines.push(l);
    }
    lines.join("\n") + "\n"
}

/// `config`: abre ~/.fyshrc en el editor ($EDITOR, o el Bloc de notas) y lo recarga al cerrarlo.
pub fn config_cmd(shell: &mut Shell) -> i32 {
    let Some(path) = rc_path() else { return 1 };
    if !path.exists() {
        if let Err(e) = fs::write(&path, TEMPLATE) {
            eprintln!("config: no se pudo crear {}: {e}", path.display());
            return 1;
        }
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "nano".into() });
    println!("{GRAY}Abriendo {} con {editor}. Guarda y ciérralo para aplicar los cambios.{RESET}", path.display());
    // `code` (VS Code) vuelve enseguida; --wait hace que espere a que cierres el archivo.
    let mut cmd = vec![editor.clone()];
    if editor == "code" {
        cmd.push("--wait".into());
    }
    cmd.push(path.display().to_string());
    let code = crate::run_external(&cmd);
    if code == 0 {
        shell.aliases.clear();
        load(shell);
        println!("{GREEN}✓{RESET} configuración recargada");
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_definiciones_de_alias() {
        assert_eq!(parse_definition("gs = git status"), Some(("gs".into(), "git status".into())));
        assert_eq!(parse_definition("ll=dir"), Some(("ll".into(), "dir".into())));
        assert_eq!(parse_definition(r#"e = echo "a = b""#), Some(("e".into(), r#"echo "a = b""#.into())));
        assert_eq!(parse_definition("mal nombre = x"), None);
        assert_eq!(parse_definition("vacio ="), None);
    }

    #[test]
    fn actualiza_fyshrc_sin_tocar_lo_demas() {
        let rc = "# mi config\nset EDITOR=code\nalias gs = git status\nalias ll = dir\n";
        assert_eq!(
            update_rc(rc, "gs", Some("git status -s")),
            "# mi config\nset EDITOR=code\nalias gs = git status -s\nalias ll = dir\n"
        );
        assert_eq!(update_rc(rc, "nuevo", Some("cls")), format!("{rc}alias nuevo = cls\n"));
        assert_eq!(update_rc(rc, "ll", None), "# mi config\nset EDITOR=code\nalias gs = git status\n");
    }

    #[test]
    fn expande_alias_con_argumentos() {
        let mut shell = Shell::new();
        shell.aliases.insert("gl".into(), "git log --oneline".into());
        let args: Vec<String> = ["gl", "-5"].iter().map(|s| s.to_string()).collect();
        assert_eq!(expand(&shell, args), ["git", "log", "--oneline", "-5"]);
        let other = vec!["dir".to_string()];
        assert_eq!(expand(&shell, other), ["dir"]);
    }
}
