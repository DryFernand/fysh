//! Editor de línea de fysh: autocompletado con Tab, sugerencias del historial, colores mientras
//! escribes (comando verde si existe, rojo si no) y prompt con colores.

use crate::git;
use crate::{BLUE, GREEN, RED, RESET, YELLOW};
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;

/// Comandos propios de fysh y agentes, para completar la primera palabra.
const COMMANDS: &[&str] = &[
    "cd", "cls", "clear", "dir", "ls", "pwd", "mkdir", "touch", "cat", "cp", "mv", "rm", "open", "echo", "set", "env",
    "path", "history", "alias", "unalias", "config", "help", "exit", "z", "ssh", "tunel", "db", "run", "puertos", "procesos", "docker", "agent", "explica", "ins:", "actualizar", "update", "git", "claude",
    "agy", "antigravity", "opencode",
];

/// Catálogo de subcomandos de git.
const GIT_SUBCOMMANDS: &[&str] = &[
    "add", "am", "archive", "bisect", "blame", "branch", "bundle", "checkout", "cherry-pick", "clean", "clone",
    "commit", "config", "describe", "diff", "fetch", "format-patch", "gc", "grep", "init", "log", "merge", "mv",
    "notes", "pull", "push", "range-diff", "rebase", "reflog", "remote", "reset", "restore", "revert", "rm",
    "shortlog", "show", "sparse-checkout", "stash", "status", "submodule", "switch", "tag", "worktree",
];

/// Subcomandos de git cuyo siguiente argumento suele ser una rama.
const GIT_BRANCH_ARGS: &[&str] =
    &["switch", "checkout", "merge", "rebase", "branch", "push", "pull", "cherry-pick", "log", "diff", "reset"];

/// Subcomandos de git cuyo siguiente argumento suele ser un archivo con cambios.
const GIT_FILE_ARGS: &[&str] = &["add", "restore", "rm", "diff", "checkout", "stash"];

pub struct FyshHelper {
    /// Prompt con colores. rustyline mide el ancho con la versión sin colores
    /// y muestra esta, así el cursor queda en su lugar.
    pub colored_prompt: RefCell<String>,
    /// Nombres de los alias, para completarlos como comandos.
    pub aliases: RefCell<Vec<String>>,
    files: FilenameCompleter,
    hinter: HistoryHinter,
    /// Programas del PATH (en minúsculas, sin extensión) y el PATH con el que se calcularon.
    path_cache: RefCell<(String, HashSet<String>)>,
}

impl FyshHelper {
    pub fn new() -> Self {
        Self {
            colored_prompt: RefCell::new(String::new()),
            aliases: RefCell::new(Vec::new()),
            files: FilenameCompleter::new(),
            hinter: HistoryHinter::new(),
            path_cache: RefCell::new((String::new(), HashSet::new())),
        }
    }

    /// ¿Existe este comando? Propio de fysh, alias, ruta a un archivo o programa del PATH.
    fn is_command(&self, name: &str) -> bool {
        if COMMANDS.contains(&name) || name.starts_with("ins:") || self.aliases.borrow().iter().any(|a| a == name) {
            return true;
        }
        if name.contains(['/', '\\']) {
            return std::path::Path::new(name).is_file();
        }
        let path = std::env::var("PATH").unwrap_or_default();
        let mut cache = self.path_cache.borrow_mut();
        if cache.0 != path {
            *cache = (path.clone(), scan_path(&path));
        }
        cache.1.contains(&name.to_lowercase())
    }
}

/// Nombres de los programas del PATH. En Windows solo .exe/.cmd/.bat/.com, guardados sin extensión
/// y también con ella (para que "notepad" y "notepad.exe" cuenten).
fn scan_path(path: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for dir in std::env::split_paths(path) {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if cfg!(windows) {
                if let Some((stem, ext)) = name.rsplit_once('.') {
                    if ["exe", "cmd", "bat", "com"].contains(&ext) {
                        names.insert(stem.to_string());
                        names.insert(name);
                    }
                }
            } else {
                names.insert(name);
            }
        }
    }
    names
}

/// Pinta la línea: cada comando (al inicio y después de | && || ;) verde si existe o rojo si no,
/// texto entre comillas amarillo y $VARIABLES azul. Solo agrega colores: no cambia el ancho.
fn colorize(line: &str, is_command: impl Fn(&str) -> bool) -> String {
    let chars: Vec<char> = line.chars().collect();
    let is_op = |c: char| matches!(c, '|' | ';' | '&' | '<' | '>');
    let mut out = String::new();
    let mut at_command = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        if c == '|' || c == ';' || (c == '&' && chars.get(i + 1) == Some(&'&')) {
            at_command = true;
            while i < chars.len() && matches!(chars[i], '|' | ';' | '&') {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if at_command && c != '"' && c != '\'' && !is_op(c) {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() && !is_op(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let color = if is_command(&word) { GREEN } else { RED };
            out += &format!("{color}{word}{RESET}");
            at_command = false;
            continue;
        }
        if c == '"' || c == '\'' {
            let end = chars[i + 1..].iter().position(|&x| x == c).map_or(chars.len(), |p| i + 1 + p + 1);
            let text: String = chars[i..end].iter().collect();
            out += &format!("{YELLOW}{text}{RESET}");
            i = end;
        } else if c == '$' {
            let mut end = i + 1;
            if chars.get(end) == Some(&'?') {
                end += 1;
            } else {
                while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                    end += 1;
                }
            }
            let text: String = chars[i..end].iter().collect();
            out += &format!("{BLUE}{text}{RESET}");
            i = end;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Candidatos que empiezan con `prefix`, sin repetir.
fn matching(prefix: &str, options: impl IntoIterator<Item = String>) -> Vec<Pair> {
    let mut out: Vec<Pair> = Vec::new();
    for o in options {
        if o.starts_with(prefix) && !out.iter().any(|p| p.replacement == o) {
            out.push(Pair { display: o.clone(), replacement: o });
        }
    }
    out
}

/// Qué completar para las palabras ya escritas (sin la que se está escribiendo).
/// None = completar nombres de archivo.
fn candidates(words: &[&str], current: &str, aliases: &[String]) -> Option<Vec<Pair>> {
    let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match words {
        [] => Some(matching(current, owned(COMMANDS).into_iter().chain(aliases.iter().cloned()))),
        ["unalias"] => Some(matching(current, aliases.iter().cloned())),
        ["ssh"] => Some(matching(current, crate::ssh::host_names())),
        ["tunel"] => Some(matching(current, crate::tunnel::names())),
        ["db"] => Some(matching(current, crate::db::names())),
        ["git"] => Some(matching(current, owned(GIT_SUBCOMMANDS))),
        ["git", sub, ..] => {
            let mut opts = Vec::new();
            if GIT_FILE_ARGS.contains(sub) {
                opts.extend(git::changed_files());
            }
            if GIT_BRANCH_ARGS.contains(sub) {
                opts.extend(git::branch_names());
            }
            let found = matching(current, opts);
            // Si no hay ramas ni archivos con cambios que coincidan, completar archivos normales.
            (!found.is_empty()).then_some(found)
        }
        _ => None,
    }
}

impl Completer for FyshHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        let start = before.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let current = &before[start..];
        // Solo el comando que se está escribiendo: lo que va después del último | && || ;
        let cut = before[..start].rfind(['|', ';', '&']).map_or(0, |i| i + 1);
        let words: Vec<&str> = before[cut..start].split_whitespace().collect();
        match candidates(&words, current, &self.aliases.borrow()) {
            Some(list) => Ok((start, list)),
            None => self.files.complete(line, pos, ctx),
        }
    }
}

impl Hinter for FyshHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Highlighter for FyshHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(&'s self, prompt: &'p str, default: bool) -> Cow<'b, str> {
        if default {
            Cow::Owned(self.colored_prompt.borrow().clone())
        } else {
            Cow::Borrowed(prompt)
        }
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("{}{hint}{}", crate::GRAY, crate::RESET))
    }

    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Owned(colorize(line, |w| self.is_command(w)))
    }

    /// Volver a pintar con cada tecla (el color del comando cambia mientras se escribe).
    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        true
    }
}

impl Validator for FyshHelper {}
impl Helper for FyshHelper {}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: Option<Vec<Pair>>) -> Vec<String> {
        p.unwrap_or_default().into_iter().map(|p| p.replacement).collect()
    }

    #[test]
    fn pinta_comandos_comillas_y_variables() {
        let known = |w: &str| w == "git";
        let out = colorize("git commit -m \"hola mundo\" $USER $?", known);
        assert!(out.starts_with(&format!("{GREEN}git{RESET}")));
        assert!(out.contains(&format!("{YELLOW}\"hola mundo\"{RESET}")));
        assert!(out.contains(&format!("{BLUE}$USER{RESET}")) && out.contains(&format!("{BLUE}$?{RESET}")));
        assert!(colorize("  gti status", known).starts_with(&format!("  {RED}gti{RESET}")));
        // Los colores no cambian el texto visible.
        let line = "echo \"sin cerrar $X";
        assert_eq!(crate::table::strip_ansi(&colorize(line, known)), line);
        // Después de | && ; también hay un comando que pintar; "git" como argumento no.
        let piped = colorize("gti log|git x && git echo git > git.txt", known);
        assert!(piped.starts_with(&format!("{RED}gti{RESET}")));
        assert_eq!(piped.matches(&format!("{GREEN}git{RESET}")).count(), 2, "{piped}");
        assert_eq!(crate::table::strip_ansi(&piped), "gti log|git x && git echo git > git.txt");
    }

    #[test]
    fn completa_comandos_y_subcomandos_de_git() {
        let aliases = vec!["crm".to_string(), "gs".to_string()];
        assert_eq!(names(candidates(&[], "cl", &[])), ["cls", "clear", "claude"]);
        assert_eq!(names(candidates(&[], "cr", &aliases)), ["crm"]);
        assert_eq!(names(candidates(&["unalias"], "g", &aliases)), ["gs"]);
        assert_eq!(names(candidates(&["git"], "che", &[])), ["checkout", "cherry-pick"]);
        assert_eq!(names(candidates(&["git"], "sta", &[])), ["stash", "status"]);
        assert!(candidates(&["notepad"], "a", &[]).is_none()); // archivos
    }
}
