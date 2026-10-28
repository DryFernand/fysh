//! Editor de línea de fysh: autocompletado con Tab, sugerencias del historial y prompt con colores.

use crate::git;
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use std::borrow::Cow;
use std::cell::RefCell;

/// Comandos propios de fysh y agentes, para completar la primera palabra.
const COMMANDS: &[&str] = &[
    "cd", "cls", "clear", "dir", "ls", "pwd", "echo", "set", "help", "exit", "git", "claude", "agy", "antigravity",
    "opencode",
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
    files: FilenameCompleter,
    hinter: HistoryHinter,
}

impl FyshHelper {
    pub fn new() -> Self {
        Self { colored_prompt: RefCell::new(String::new()), files: FilenameCompleter::new(), hinter: HistoryHinter::new() }
    }
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
fn candidates(words: &[&str], current: &str) -> Option<Vec<Pair>> {
    let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match words {
        [] => Some(matching(current, owned(COMMANDS))),
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
        let words: Vec<&str> = before[..start].split_whitespace().collect();
        match candidates(&words, current) {
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
    fn completa_comandos_y_subcomandos_de_git() {
        assert_eq!(names(candidates(&[], "cl")), ["cls", "clear", "claude"]);
        assert_eq!(names(candidates(&["git"], "che")), ["checkout", "cherry-pick"]);
        assert_eq!(names(candidates(&["git"], "sta")), ["stash", "status"]);
        assert!(candidates(&["notepad"], "a").is_none()); // archivos
    }
}
