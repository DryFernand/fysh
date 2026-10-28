//! Git en fysh: rama en el prompt, tablas (status, log, branch) y selectores con flechas
//! (switch/checkout, add, show, commit). Todo lo demás pasa directo al git real.

use crate::picker;
use crate::table::{self, truncate, Align};
use crate::{BLUE, BOLD, GRAY, GREEN, RED, RESET, YELLOW};
use std::io::{self, Write};
use std::process::Command;

/// `git <args>`: si fysh tiene una versión propia del comando la usa; si no, devuelve None
/// y se ejecuta el git real. También devuelve None si git falla (fuera de un repo, por ejemplo),
/// para que el git real muestre su propio mensaje de error.
pub fn run(args: &[String]) -> Option<i32> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["status"] => status_table(),
        ["log"] => log_table(20),
        ["log", n] if n.starts_with('-') && n[1..].parse::<usize>().is_ok() => log_table(n[1..].parse().ok()?),
        ["log", "-n", n] => log_table(n.parse().ok()?),
        ["branch"] => branch_table(),
        ["switch"] | ["checkout"] => switch_picker(),
        ["add"] => add_picker(),
        ["show"] => show_picker(),
        ["commit"] => commit(),
        _ => None,
    }
}

/// Ejecuta git y devuelve su salida, o None si falla.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------- estado del repositorio ----------

struct Status {
    branch: String,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    entries: Vec<Entry>,
}

struct Entry {
    staged: char,   // cambio preparado ('.' = ninguno)
    unstaged: char, // cambio sin preparar ('.' = ninguno)
    kind: EntryKind,
    path: String,
    from: Option<String>, // nombre anterior si se renombró
}

#[derive(PartialEq)]
enum EntryKind {
    Tracked,
    Untracked,
    Conflict,
}

/// ¿Estamos dentro de un repositorio? Busca .git subiendo carpetas (sin lanzar git).
fn in_repo() -> bool {
    let Ok(cwd) = std::env::current_dir() else { return false };
    cwd.ancestors().any(|d| d.join(".git").exists())
}

fn status() -> Option<Status> {
    let out = git(&["status", "--porcelain=v2", "--branch", "-z"])?;
    Some(parse_status(&out))
}

/// Interpreta `git status --porcelain=v2 --branch -z` (campos separados por NUL).
fn parse_status(out: &str) -> Status {
    let mut st = Status { branch: String::new(), upstream: None, ahead: 0, behind: 0, entries: Vec::new() };
    let mut oid = String::new();
    let mut fields = out.split('\0');
    while let Some(f) = fields.next() {
        if let Some(h) = f.strip_prefix("# branch.head ") {
            st.branch = h.to_string();
        } else if let Some(o) = f.strip_prefix("# branch.oid ") {
            oid = o.chars().take(7).collect();
        } else if let Some(u) = f.strip_prefix("# branch.upstream ") {
            st.upstream = Some(u.to_string());
        } else if let Some(ab) = f.strip_prefix("# branch.ab ") {
            let mut it = ab.split(' ').map(|n| n.trim_start_matches(['+', '-']).parse().unwrap_or(0));
            st.ahead = it.next().unwrap_or(0);
            st.behind = it.next().unwrap_or(0);
        } else if let Some(p) = f.strip_prefix("? ") {
            st.entries.push(Entry { staged: '.', unstaged: '?', kind: EntryKind::Untracked, path: p.into(), from: None });
        } else if f.starts_with("1 ") || f.starts_with("2 ") || f.starts_with("u ") {
            // 1: cambio normal (8 campos + ruta), 2: renombrado (9 + ruta, y la ruta vieja en el siguiente campo),
            // u: conflicto (10 + ruta). La ruta va al final y puede tener espacios.
            let n = match &f[..1] { "1" => 9, "2" => 10, _ => 11 };
            let parts: Vec<&str> = f.splitn(n, ' ').collect();
            let xy: Vec<char> = parts.get(1).unwrap_or(&"..").chars().collect();
            let path = parts.last().unwrap_or(&"").to_string();
            let from = if f.starts_with('2') { fields.next().map(str::to_owned) } else { None };
            let kind = if f.starts_with('u') { EntryKind::Conflict } else { EntryKind::Tracked };
            st.entries.push(Entry { staged: xy[0], unstaged: xy[1], kind, path, from });
        }
    }
    if st.branch == "(detached)" {
        st.branch = format!("({oid})");
    }
    st
}

/// Segmento del prompt: " (main ●3 ↑1 ↓2)". Vacío fuera de un repositorio.
pub fn prompt_segment() -> String {
    if !in_repo() {
        return String::new();
    }
    let Some(st) = status() else { return String::new() };
    let mut s = format!(" {GRAY}({RESET}{BLUE}{}{RESET}", st.branch);
    if !st.entries.is_empty() {
        s += &format!(" {YELLOW}●{}{RESET}", st.entries.len());
    }
    if st.ahead > 0 {
        s += &format!(" {GREEN}↑{}{RESET}", st.ahead);
    }
    if st.behind > 0 {
        s += &format!(" {RED}↓{}{RESET}", st.behind);
    }
    s + &format!("{GRAY}){RESET}")
}

fn change_name(c: char) -> &'static str {
    match c {
        'M' => "modificado",
        'A' => "nuevo",
        'D' => "eliminado",
        'R' => "renombrado",
        'C' => "copiado",
        'T' => "cambió de tipo",
        _ => "cambiado",
    }
}

fn entry_path(e: &Entry) -> String {
    match &e.from {
        Some(from) => format!("{from} → {}", e.path),
        None => e.path.clone(),
    }
}

// ---------- tablas ----------

fn status_table() -> Option<i32> {
    let st = status()?;
    let mut line = format!("{GRAY}Rama:{RESET} {BOLD}{BLUE}{}{RESET}", st.branch);
    if let Some(up) = &st.upstream {
        line += &format!(" {GRAY}→ {up}{RESET}");
        match (st.ahead, st.behind) {
            (0, 0) => line += &format!(" {GREEN}al día{RESET}"),
            (a, b) => {
                if a > 0 {
                    line += &format!(" {GREEN}↑{a} por subir{RESET}");
                }
                if b > 0 {
                    line += &format!(" {RED}↓{b} por bajar{RESET}");
                }
            }
        }
    } else {
        line += &format!(" {GRAY}(sin rama remota){RESET}");
    }
    println!("{line}");

    if st.entries.is_empty() {
        println!("{GREEN}Sin cambios: todo está guardado en el último commit.{RESET}");
        return Some(0);
    }

    // Orden: preparados, sin preparar, conflictos, sin seguimiento.
    let mut rows: Vec<(u8, Vec<String>)> = Vec::new();
    for e in &st.entries {
        let path = entry_path(e);
        match e.kind {
            EntryKind::Conflict => rows.push((2, vec![format!("{RED}conflicto{RESET}"), "por resolver".into(), path])),
            EntryKind::Untracked => rows.push((3, vec![format!("{GRAY}sin seguimiento{RESET}"), "nuevo".into(), path])),
            EntryKind::Tracked => {
                if e.staged != '.' {
                    rows.push((0, vec![format!("{GREEN}preparado{RESET}"), change_name(e.staged).into(), path.clone()]));
                }
                if e.unstaged != '.' {
                    rows.push((1, vec![format!("{YELLOW}sin preparar{RESET}"), change_name(e.unstaged).into(), path]));
                }
            }
        }
    }
    rows.sort_by_key(|(order, _)| *order);
    let rows: Vec<Vec<String>> = rows.into_iter().map(|(_, r)| r).collect();
    table::print(&["Estado", "Cambio", "Archivo"], &[Align::Left, Align::Left, Align::Left], &rows);
    println!("{GRAY}git add: elegir archivos para preparar · git commit: guardar lo preparado{RESET}");
    Some(0)
}

struct Commit {
    hash: String,
    date: String,
    author: String,
    subject: String,
    refs: String,
}

fn commits(n: usize) -> Option<Vec<Commit>> {
    let out = git(&[
        "log",
        &format!("-{n}"),
        "--date=format:%Y-%m-%d %H:%M",
        "--format=%h%x1f%ad%x1f%an%x1f%s%x1f%D",
    ])?;
    Some(
        out.lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.split('\x1f').collect();
                Some(Commit {
                    hash: p.first()?.to_string(),
                    date: p.get(1)?.to_string(),
                    author: p.get(2)?.to_string(),
                    subject: p.get(3)?.to_string(),
                    refs: p.get(4).unwrap_or(&"").to_string(),
                })
            })
            .collect(),
    )
}

/// Fila de un commit. El mensaje (con sus ramas/etiquetas) se limita a 60 caracteres en total
/// para que la tabla quepa en la ventana.
fn commit_row(c: &Commit) -> Vec<String> {
    let refs = if c.refs.is_empty() { String::new() } else { format!("({}) ", truncate(&c.refs, 24)) };
    let room = 60usize.saturating_sub(refs.chars().count()).max(20);
    vec![
        format!("{YELLOW}{}{RESET}", c.hash),
        c.date.clone(),
        truncate(&c.author, 20),
        format!("{BLUE}{refs}{RESET}{}", truncate(&c.subject, room)),
    ]
}

fn log_table(n: usize) -> Option<i32> {
    let list = commits(n)?;
    if list.is_empty() {
        println!("{GRAY}Este repositorio todavía no tiene commits.{RESET}");
        return Some(0);
    }
    let rows: Vec<Vec<String>> = list.iter().map(commit_row).collect();
    table::print(&["Commit", "Fecha", "Autor", "Mensaje"], &[Align::Left; 4], &rows);
    println!("{GRAY}Últimos {} commits · git log -50 para ver más · git show para abrir uno{RESET}", list.len());
    Some(0)
}

struct Branch {
    current: bool,
    name: String,
    date: String,
    upstream: String,
    track: String,
    subject: String,
}

fn local_branches() -> Option<Vec<Branch>> {
    let out = git(&[
        "for-each-ref",
        "--sort=-committerdate",
        "refs/heads",
        "--format=%(HEAD)%1f%(refname:short)%1f%(committerdate:format:%Y-%m-%d %H:%M)%1f%(upstream:short)%1f%(upstream:track)%1f%(contents:subject)",
    ])?;
    Some(
        out.lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.split('\x1f').collect();
                Some(Branch {
                    current: *p.first()? == "*",
                    name: p.get(1)?.to_string(),
                    date: p.get(2)?.to_string(),
                    upstream: p.get(3)?.to_string(),
                    track: p.get(4)?.to_string(),
                    subject: p.get(5).unwrap_or(&"").to_string(),
                })
            })
            .collect(),
    )
}

/// "[ahead 1, behind 2]" → "↑1 ↓2"; "[gone]" → "borrada en el remoto".
fn track_text(track: &str) -> String {
    if track == "[gone]" {
        return format!("{RED}borrada en el remoto{RESET}");
    }
    let mut s = Vec::new();
    for part in track.trim_matches(['[', ']']).split(", ").filter(|p| !p.is_empty()) {
        if let Some(n) = part.strip_prefix("ahead ") {
            s.push(format!("{GREEN}↑{n}{RESET}"));
        } else if let Some(n) = part.strip_prefix("behind ") {
            s.push(format!("{RED}↓{n}{RESET}"));
        }
    }
    s.join(" ")
}

fn branch_table() -> Option<i32> {
    let list = local_branches()?;
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|b| {
            let name = if b.current { format!("{GREEN}{BOLD}* {}{RESET}", b.name) } else { format!("  {}", b.name) };
            let remote = if b.upstream.is_empty() {
                format!("{GRAY}sin remota{RESET}")
            } else {
                format!("{} {}", b.upstream, track_text(&b.track)).trim_end().to_string()
            };
            vec![name, b.date.clone(), remote, truncate(&b.subject, 50)]
        })
        .collect();
    table::print(&["Rama", "Último commit", "Remota", "Mensaje"], &[Align::Left; 4], &rows);
    println!("{GRAY}* = rama actual · git switch para cambiar de rama{RESET}");
    Some(0)
}

// ---------- selectores ----------

/// `git switch` / `git checkout` sin argumentos: elegir rama (locales y remotas sin copia local).
fn switch_picker() -> Option<i32> {
    let locals = local_branches()?;
    let mut names: Vec<String> = locals.iter().map(|b| b.name.clone()).collect();
    let mut rows: Vec<Vec<String>> = locals
        .iter()
        .map(|b| {
            let kind = if b.current { format!("{GREEN}actual{RESET}") } else { "local".into() };
            vec![b.name.clone(), kind, format!("{GRAY}{}{RESET}", b.date)]
        })
        .collect();

    // Ramas remotas que no tienen copia local: git switch <nombre> la crea y la conecta.
    let remotes = git(&["for-each-ref", "--sort=-committerdate", "refs/remotes", "--format=%(refname:short)%1f%(committerdate:format:%Y-%m-%d %H:%M)"]).unwrap_or_default();
    for l in remotes.lines() {
        let Some((full, date)) = l.split_once('\x1f') else { continue };
        let Some((_, short)) = full.split_once('/') else { continue };
        if short == "HEAD" || names.iter().any(|n| n == short) {
            continue;
        }
        names.push(short.to_string());
        rows.push(vec![short.to_string(), format!("{BLUE}remota ({full}){RESET}"), format!("{GRAY}{date}{RESET}")]);
    }

    let i = picker::select_one(&["Rama", "Tipo", "Último commit"], &rows)?;
    if locals.get(i).is_some_and(|b| b.current) {
        println!("{GRAY}Ya estás en {}.{RESET}", names[i]);
        return Some(0);
    }
    Some(crate::run_external(&["git".into(), "switch".into(), names[i].clone()]))
}

/// `git add` sin argumentos: marcar qué archivos preparar.
fn add_picker() -> Option<i32> {
    let st = status()?;
    let pending: Vec<&Entry> = st.entries.iter().filter(|e| e.unstaged != '.').collect();
    if pending.is_empty() {
        println!("{GREEN}No hay cambios sin preparar.{RESET}");
        return Some(0);
    }
    let rows: Vec<Vec<String>> = pending
        .iter()
        .map(|e| {
            let change = match e.kind {
                EntryKind::Untracked => format!("{GRAY}nuevo (sin seguimiento){RESET}"),
                EntryKind::Conflict => format!("{RED}conflicto resuelto{RESET}"),
                EntryKind::Tracked => change_name(e.unstaged).into(),
            };
            vec![e.path.clone(), change]
        })
        .collect();
    let picked = picker::select_many(&["Archivo", "Cambio"], &rows)?;
    let mut cmd: Vec<String> = vec!["git".into(), "add".into(), "--".into()];
    cmd.extend(picked.iter().map(|&i| pending[i].path.clone()));
    let code = crate::run_external(&cmd);
    if code == 0 {
        println!("{GREEN}✓ {} archivo(s) preparado(s).{RESET} {GRAY}git commit para guardarlos.{RESET}", picked.len());
    }
    Some(code)
}

/// `git show` sin argumentos: elegir un commit de los últimos 50 y abrirlo.
fn show_picker() -> Option<i32> {
    let list = commits(50)?;
    let rows: Vec<Vec<String>> = list.iter().map(commit_row).collect();
    let i = picker::select_one(&["Commit", "Fecha", "Autor", "Mensaje"], &rows)?;
    Some(crate::run_external(&["git".into(), "show".into(), list[i].hash.clone()]))
}

/// `git commit` sin argumentos: si no hay nada preparado ofrece elegir archivos,
/// y pide el mensaje en la misma terminal (en vez de abrir un editor).
fn commit() -> Option<i32> {
    let st = status()?;
    let staged = |st: &Status| st.entries.iter().any(|e| e.kind == EntryKind::Tracked && e.staged != '.');
    if !staged(&st) {
        if st.entries.is_empty() {
            println!("{GREEN}No hay cambios para guardar.{RESET}");
            return Some(0);
        }
        println!("{GRAY}No hay archivos preparados. Elige cuáles incluir en el commit:{RESET}");
        if add_picker()? != 0 || !staged(&status()?) {
            println!("{GRAY}Commit cancelado.{RESET}");
            return Some(1);
        }
    }
    print!("{BOLD}Mensaje del commit:{RESET} ");
    io::stdout().flush().ok();
    let mut msg = String::new();
    io::stdin().read_line(&mut msg).ok()?;
    let msg = msg.trim();
    if msg.is_empty() {
        println!("{GRAY}Commit cancelado (mensaje vacío).{RESET}");
        return Some(1);
    }
    Some(crate::run_external(&["git".into(), "commit".into(), "-m".into(), msg.into()]))
}

/// Ramas locales y remotas, para el autocompletado.
pub fn branch_names() -> Vec<String> {
    if !in_repo() {
        return Vec::new();
    }
    let out = git(&["for-each-ref", "--format=%(refname:short)", "refs/heads", "refs/remotes"]).unwrap_or_default();
    out.lines().filter(|l| !l.ends_with("/HEAD")).map(str::to_owned).collect()
}

/// Archivos con cambios, para el autocompletado de git add/restore/diff.
pub fn changed_files() -> Vec<String> {
    if !in_repo() {
        return Vec::new();
    }
    status().map(|st| st.entries.into_iter().map(|e| e.path).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreta_status_porcelain_v2() {
        let out = [
            "# branch.oid 0123456789abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 M. N... 100644 100644 100644 aaa bbb src/main.rs",
            "1 .M N... 100644 100644 100644 aaa bbb mi archivo.txt",
            "2 R. N... 100644 100644 100644 aaa bbb R100 nuevo.rs",
            "viejo.rs",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflicto.rs",
            "? suelto.txt",
            "",
        ]
        .join("\0");
        let st = parse_status(&out);
        assert_eq!((st.branch.as_str(), st.ahead, st.behind), ("main", 2, 1));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!(st.entries.len(), 5);
        assert_eq!((st.entries[0].staged, st.entries[0].unstaged), ('M', '.'));
        assert_eq!(st.entries[1].path, "mi archivo.txt");
        assert_eq!(entry_path(&st.entries[2]), "viejo.rs → nuevo.rs");
        assert!(st.entries[3].kind == EntryKind::Conflict && st.entries[3].path == "conflicto.rs");
        assert!(st.entries[4].kind == EntryKind::Untracked);
    }

    #[test]
    fn rama_desconectada_muestra_el_commit() {
        let st = parse_status("# branch.oid 0123456789abcdef\0# branch.head (detached)\0");
        assert_eq!(st.branch, "(0123456)");
    }

    #[test]
    fn traduce_seguimiento_de_rama() {
        let strip = |s: String| s.replace(GREEN, "").replace(RED, "").replace(RESET, "");
        assert_eq!(strip(track_text("[ahead 1, behind 2]")), "↑1 ↓2");
        assert_eq!(strip(track_text("")), "");
        assert!(track_text("[gone]").contains("borrada"));
    }
}
