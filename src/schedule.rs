//! `programar "<comando>" <cuándo>`: correr algo solo, a una hora fija, sin schtasks ni cron a mano.
//! Cada tarea es un script en ~/.fysh/programadas/<nombre>.fysh (entra a la carpeta donde se creó y
//! corre el comando); en Windows la dispara el Programador de tareas (carpeta "fysh"), en Linux/Mac cron.

use crate::table::{self, Align};
use crate::{home, picker, GRAY, GREEN, RED, RESET};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const DAYS: [(&str, &str); 7] = [("lunes", "MON"), ("martes", "TUE"), ("miercoles", "WED"), ("jueves", "THU"), ("viernes", "FRI"), ("sabado", "SAT"), ("domingo", "SUN")];

#[derive(Debug, PartialEq)]
enum When {
    /// Días de la semana (0 = lunes) a una hora.
    Days(Vec<usize>, u32, u32),
    Every(u32),
}

const USAGE: &str = "uso: programar \"<comando>\" <cuándo>   ·   programar (lista)   ·   programar -delete [nombre]
cuándo: diario 9:00 · laborables 8:30 · lunes,jueves 18:00 · cada 15 minutos · cada 2 horas";

fn dir() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("programadas"))
}

fn parse_time(t: &str) -> Option<(u32, u32)> {
    let (h, m) = t.split_once(':').unwrap_or((t, "0"));
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

fn parse_when(words: &[&str]) -> Option<When> {
    let plain = |w: &str| w.to_lowercase().replace('é', "e").replace('á', "a");
    match words {
        ["cada", n, unit] => {
            let n: u32 = n.parse().ok().filter(|&n| n > 0)?;
            match plain(unit).as_str() {
                "minuto" | "minutos" | "min" if n < 1440 => Some(When::Every(n)),
                "hora" | "horas" | "h" if n <= 24 => Some(When::Every(n * 60)),
                _ => None,
            }
        }
        [days, time] => {
            let (h, m) = parse_time(time)?;
            let days: Vec<usize> = match plain(days).as_str() {
                "diario" | "todos" => (0..7).collect(),
                "laborables" => (0..5).collect(),
                list => list.split(',').map(|d| DAYS.iter().position(|(name, _)| *name == d)).collect::<Option<_>>()?,
            };
            Some(When::Days(days, h, m))
        }
        _ => None,
    }
}

/// Argumentos de schtasks para el disparador.
fn schtasks_trigger(when: &When) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match when {
        When::Every(m) if m % 60 == 0 => s(&["/SC", "HOURLY", "/MO", &(m / 60).to_string()]),
        When::Every(m) => s(&["/SC", "MINUTE", "/MO", &m.to_string()]),
        When::Days(days, h, m) if days.len() == 7 => s(&["/SC", "DAILY", "/ST", &format!("{h:02}:{m:02}")]),
        When::Days(days, h, m) => {
            let d: Vec<&str> = days.iter().map(|&i| DAYS[i].1).collect();
            s(&["/SC", "WEEKLY", "/D", &d.join(","), "/ST", &format!("{h:02}:{m:02}")])
        }
    }
}

/// Los cinco campos de cron.
fn cron_fields(when: &When) -> String {
    match when {
        When::Every(m) if m % 60 == 0 => format!("0 */{} * * *", m / 60),
        When::Every(m) => format!("*/{m} * * * *"),
        When::Days(days, h, m) if days.len() == 7 => format!("{m} {h} * * *"),
        When::Days(days, h, m) => {
            let d: Vec<String> = days.iter().map(|&i| ((i + 1) % 7).to_string()).collect(); // cron: 0 = domingo
            format!("{m} {h} * * {}", d.join(","))
        }
    }
}

/// "git pull origin" → "git-pull-origin"
fn slug(command: &str) -> String {
    let s: String = command.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.chars().take(30).collect();
    if s.is_empty() { "tarea".into() } else { s.trim_end_matches('-').to_string() }
}

struct Task {
    name: String,
    command: String,
    when: String,
}

fn load() -> Vec<Task> {
    let Some(d) = dir() else { return Vec::new() };
    let mut tasks: Vec<Task> = std::fs::read_dir(d)
        .map(|r| r.flatten().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().strip_suffix(".fysh")?.to_string();
            let text = std::fs::read_to_string(e.path()).ok()?;
            let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("# {k}: "))).unwrap_or("").to_string();
            Some(Task { name, command: field("comando"), when: field("cuando") })
        })
        .collect();
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    tasks
}

fn exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "fysh".into())
}

fn crontab_read() -> String {
    Command::new("crontab").arg("-l").stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

fn crontab_write(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("crontab").arg("-").stdin(Stdio::piped()).spawn().map_err(|e| format!("crontab: {e}"))?;
    child.stdin.take().unwrap().write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    child.wait().ok().filter(|s| s.success()).map(|_| ()).ok_or_else(|| "crontab no aceptó el cambio".into())
}

/// Quita la línea de la tarea del crontab (las de fysh terminan en "# fysh:<nombre>").
fn crontab_without(text: &str, name: &str) -> String {
    text.lines().filter(|l| !l.ends_with(&format!("# fysh:{name}"))).map(|l| format!("{l}\n")).collect()
}

fn register(name: &str, script: &std::path::Path, when: &When) -> Result<(), String> {
    if cfg!(windows) {
        let out = Command::new("schtasks")
            .args(["/Create", "/F", "/TN", &format!("fysh\\{name}"), "/TR", &format!("\"{}\" \"{}\"", exe(), script.display())])
            .args(schtasks_trigger(when))
            .output()
            .map_err(|e| format!("schtasks: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    } else {
        let log = script.with_extension("log");
        let line = format!("{} \"{}\" \"{}\" >> \"{}\" 2>&1 # fysh:{name}\n", cron_fields(when), exe(), script.display(), log.display());
        crontab_write(&(crontab_without(&crontab_read(), name) + &line))
    }
}

fn unregister(name: &str) {
    if cfg!(windows) {
        let _ = Command::new("schtasks").args(["/Delete", "/F", "/TN", &format!("fysh\\{name}")]).output();
    } else {
        let _ = crontab_write(&crontab_without(&crontab_read(), name));
    }
}

/// Windows: próxima vez, última vez y su resultado, según el Programador de tareas.
fn status(name: &str) -> (String, String, String) {
    let out = Command::new("schtasks").args(["/Query", "/TN", &format!("fysh\\{name}"), "/FO", "CSV", "/V", "/NH"]).output();
    let Ok(out) = out else { return Default::default() };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().find(|l| !l.trim().is_empty()) else {
        return ("—".into(), "—".into(), format!("{RED}no registrada{RESET}"));
    };
    let f: Vec<&str> = line.trim_matches('"').split("\",\"").collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
    let result = match get(6).as_str() {
        "267011" => format!("{GRAY}aún no corrió{RESET}"),
        "0" => format!("{GREEN}✓{RESET}"),
        code => format!("{RED}✗ código {code}{RESET}"),
    };
    (get(2), get(5), result)
}

fn list() -> i32 {
    let tasks = load();
    if tasks.is_empty() {
        println!("{GRAY}No hay tareas programadas. Ejemplo: programar \"git pull\" diario 9:00{RESET}");
        return 0;
    }
    if cfg!(windows) {
        let rows: Vec<Vec<String>> = tasks
            .iter()
            .map(|t| {
                let (next, last, result) = status(&t.name);
                vec![t.name.clone(), t.command.clone(), t.when.clone(), next, last, result]
            })
            .collect();
        table::print(&["Nombre", "Comando", "Cuándo", "Próxima", "Última", "Resultado"], &[Align::Left; 6], &rows);
    } else {
        let rows: Vec<Vec<String>> = tasks.iter().map(|t| vec![t.name.clone(), t.command.clone(), t.when.clone()]).collect();
        table::print(&["Nombre", "Comando", "Cuándo"], &[Align::Left; 3], &rows);
        println!("{GRAY}Lo que imprimen queda en ~/.fysh/programadas/<nombre>.log{RESET}");
    }
    println!("{GRAY}programar -delete <nombre> para quitar una{RESET}");
    0
}

fn delete(name: Option<&String>) -> i32 {
    let tasks = load();
    let name = match name {
        Some(n) => n.clone(),
        None => {
            let rows: Vec<Vec<String>> = tasks.iter().map(|t| vec![t.name.clone(), t.command.clone(), t.when.clone()]).collect();
            match picker::select_one(&["Nombre", "Comando", "Cuándo"], &rows) {
                Some(i) => tasks[i].name.clone(),
                None => return 1,
            }
        }
    };
    if !tasks.iter().any(|t| t.name == name) {
        eprintln!("programar: no existe la tarea {name} (mira la lista con programar)");
        return 1;
    }
    unregister(&name);
    if let Some(d) = dir() {
        let _ = std::fs::remove_file(d.join(format!("{name}.fysh")));
        let _ = std::fs::remove_file(d.join(format!("{name}.log")));
    }
    println!("{GREEN}✓{RESET} Quitada: {name}");
    0
}

pub fn programar_cmd(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None => return list(),
        Some("-delete") => return delete(args.get(1)),
        _ => {}
    }
    let command = &args[0];
    let words: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let Some(when) = parse_when(&words) else {
        eprintln!("{GRAY}{USAGE}{RESET}");
        return 1;
    };
    let Some(d) = dir() else { return 1 };
    let base = slug(command);
    let name = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|n| !d.join(format!("{n}.fysh")).exists()).unwrap();
    let cwd = std::env::current_dir().unwrap_or_default();
    let script = d.join(format!("{name}.fysh"));
    let text = format!("# Tarea de fysh (programar)\n# comando: {command}\n# cuando: {}\ncd \"{}\"\n{command}\n", words.join(" "), cwd.display());
    if let Err(e) = std::fs::create_dir_all(&d).and_then(|_| std::fs::write(&script, text)) {
        eprintln!("programar: {}", crate::builtins::error_msg(&e));
        return 1;
    }
    if let Err(e) = register(&name, &script, &when) {
        let _ = std::fs::remove_file(&script);
        eprintln!("programar: {e}");
        return 1;
    }
    println!("{GREEN}✓{RESET} {name}: {command} {GRAY}({}, en {}){RESET}", words.join(" "), crate::tilde(&cwd));
    if cfg!(windows) {
        println!("{GRAY}Corre aunque fysh esté cerrado, mientras tu sesión de Windows esté abierta.{RESET}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entiende_cuando() {
        assert_eq!(parse_when(&["diario", "9:00"]), Some(When::Days((0..7).collect(), 9, 0)));
        assert_eq!(parse_when(&["laborables", "8:30"]), Some(When::Days(vec![0, 1, 2, 3, 4], 8, 30)));
        assert_eq!(parse_when(&["Lunes,miércoles", "18"]), Some(When::Days(vec![0, 2], 18, 0)));
        assert_eq!(parse_when(&["cada", "15", "minutos"]), Some(When::Every(15)));
        assert_eq!(parse_when(&["cada", "2", "horas"]), Some(When::Every(120)));
        assert_eq!(parse_when(&["diario", "25:00"]), None);
        assert_eq!(parse_when(&["jueve", "9:00"]), None);
        assert_eq!(parse_when(&["cada", "0", "minutos"]), None);
    }

    #[test]
    fn arma_schtasks_y_cron() {
        let w = |words: &[&str]| parse_when(words).unwrap();
        assert_eq!(schtasks_trigger(&w(&["diario", "9:05"])), ["/SC", "DAILY", "/ST", "09:05"]);
        assert_eq!(schtasks_trigger(&w(&["lunes,viernes", "18:00"])), ["/SC", "WEEKLY", "/D", "MON,FRI", "/ST", "18:00"]);
        assert_eq!(schtasks_trigger(&w(&["cada", "2", "horas"])), ["/SC", "HOURLY", "/MO", "2"]);
        assert_eq!(cron_fields(&w(&["laborables", "8:30"])), "30 8 * * 1,2,3,4,5");
        assert_eq!(cron_fields(&w(&["domingo", "10:00"])), "0 10 * * 0");
        assert_eq!(cron_fields(&w(&["cada", "15", "min"])), "*/15 * * * *");
        assert_eq!(slug("git pull && npm test"), "git-pull-npm-test");
        assert_eq!(crontab_without("a\nx # fysh:uno\nb\n", "uno"), "a\nb\n");
    }
}
