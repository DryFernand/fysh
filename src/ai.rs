//! Asistente de IA dentro de fysh.
//! `agent` elige qué IA instalada usar (Claude Code, Antigravity u OpenCode).
//! `explica` le pasa el último comando que falló y lo que se vio en pantalla, y explica el error.
//! `ins: <tarea>` pide el comando para una tarea. En ambos, el comando sugerido queda escrito
//! en la línea para revisarlo y darle Enter: nunca se ejecuta solo.

use crate::picker;
use crate::{home, Shell, BOLD, GRAY, GREEN, RED, RESET};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Agent {
    Claude,
    Agy,
    OpenCode,
}

const ALL: [Agent; 3] = [Agent::Claude, Agent::Agy, Agent::OpenCode];

impl Agent {
    fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Agy => "agy",
            Agent::OpenCode => "opencode",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude (Claude Code)",
            Agent::Agy => "Antigravity (agy)",
            Agent::OpenCode => "OpenCode",
        }
    }

    /// Programa a ejecutar, si está instalado.
    fn program(self) -> Option<PathBuf> {
        match self {
            Agent::OpenCode => {
                let p = PathBuf::from(crate::agents::opencode_program());
                if p.is_absolute() { Some(p) } else { crate::which("opencode") }
            }
            other => crate::which(other.id()),
        }
    }

    /// Argumentos para mandar una pregunta y recibir solo la respuesta impresa.
    fn ask_args(self, prompt: &str) -> Vec<String> {
        match self {
            Agent::Claude | Agent::Agy => vec!["-p".into(), prompt.into()],
            Agent::OpenCode => vec!["run".into(), prompt.into()],
        }
    }

    /// Argumentos para abrir una sesión interactiva que empieza con `prompt`.
    fn interactive_args(self, prompt: &str) -> Vec<String> {
        match self {
            Agent::Claude => vec![prompt.into()],
            Agent::Agy => vec!["-i".into(), prompt.into()],
            Agent::OpenCode => vec!["--prompt".into(), prompt.into()],
        }
    }
}

/// Abre el asistente en tu PC, ya instruido para trabajar en el servidor `name` por ssh.
/// No hace falta instalar nada en el servidor: el agente ejecuta `ssh name '<comando>'`.
pub fn agent_for_server(name: &str, target: &str) -> i32 {
    let Some(agent) = current() else {
        eprintln!("No hay ninguna IA instalada. Instala Claude Code: irm https://claude.ai/install.ps1 | iex");
        return 1;
    };
    let Some(program) = agent.program() else { return 1 };
    let prompt = format!(
        "Vas a trabajar en el servidor remoto «{name}» ({target}). No estás dentro de él: ejecuta cada comando \
         con `ssh {name} '<comando>'` (ya está configurado en ~/.ssh/config, con su llave) y copia archivos con \
         `scp`. Antes de cualquier cambio que modifique el servidor, explícame qué vas a hacer y espera mi \
         confirmación. Para empezar, comprueba la conexión con `ssh {name} 'uname -a; whoami'`, dime en una línea \
         qué sistema es y espera mis instrucciones. Responde en español."
    );
    println!("{GRAY}Abriendo {} para trabajar en {name}…{RESET}", agent.label());
    let mut cmd = vec![program.display().to_string()];
    cmd.extend(agent.interactive_args(&prompt));
    crate::run_external(&cmd)
}

fn choice_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("agente"))
}

/// El asistente elegido con `agent`; si no hay elección (o ya no está instalado), el primero disponible.
fn current() -> Option<Agent> {
    let chosen = choice_file().and_then(|f| std::fs::read_to_string(f).ok());
    let chosen = chosen.as_deref().map(str::trim);
    ALL.into_iter()
        .find(|a| Some(a.id()) == chosen && a.program().is_some())
        .or_else(|| ALL.into_iter().find(|a| a.program().is_some()))
}

/// `agent`: selector de las IA y cuál está disponible; la elegida queda guardada.
pub fn agent_cmd() -> i32 {
    let active = current();
    let rows: Vec<Vec<String>> = ALL
        .iter()
        .map(|a| {
            let state = if a.program().is_some() { format!("{GREEN}disponible{RESET}") } else { format!("{GRAY}no instalada{RESET}") };
            let mark = if Some(*a) == active { format!("{GREEN}● en uso{RESET}") } else { String::new() };
            vec![a.label().to_string(), state, mark]
        })
        .collect();
    println!("{GRAY}El asistente responde a explica e ins:{RESET}");
    let Some(i) = picker::select_one(&["Asistente", "Estado", ""], &rows) else {
        if let Some(a) = active {
            println!("{GRAY}Asistente actual: {}{RESET}", a.label());
        }
        return 0;
    };
    let agent = ALL[i];
    if agent.program().is_none() {
        eprintln!("{} no está instalada.", agent.label());
        if let Some(hint) = crate::install_hint(agent.id()) {
            eprintln!("{GRAY}Para instalarla: {hint}{RESET}");
        }
        return 1;
    }
    let saved = choice_file().is_some_and(|f| {
        f.parent().map(std::fs::create_dir_all);
        std::fs::write(f, agent.id()).is_ok()
    });
    if !saved {
        eprintln!("agent: no se pudo guardar la elección en ~/.fysh/agente");
        return 1;
    }
    println!("{GREEN}✓{RESET} Asistente: {BOLD}{}{RESET}", agent.label());
    0
}

/// Muestra una animación mientras `f` trabaja (solo si la salida es una terminal).
fn with_spinner<T>(label: &str, f: impl FnOnce() -> T) -> T {
    if !std::io::stdout().is_terminal() {
        return f();
    }
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let label = label.to_string();
    let spinner = std::thread::spawn(move || {
        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let mut i = 0;
        while !flag.load(Ordering::Relaxed) {
            print!("\r{GRAY}{} {label}{RESET}", frames[i % frames.len()]);
            std::io::stdout().flush().ok();
            i += 1;
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        print!("\r\x1b[2K");
        std::io::stdout().flush().ok();
    });
    let result = f();
    done.store(true, Ordering::Relaxed);
    spinner.join().ok();
    result
}

/// Pregunta al asistente y devuelve su respuesta, o el error para mostrar.
fn ask_agent(prompt: &str) -> Result<(Agent, String), String> {
    let agent = current().ok_or("No hay ninguna IA instalada. Instala Claude Code: irm https://claude.ai/install.ps1 | iex")?;
    let program = agent.program().ok_or("El asistente ya no está instalado.")?;
    let output = with_spinner(&format!("Preguntando a {}…", agent.label()), || {
        Command::new(&program).args(agent.ask_args(prompt)).stdin(Stdio::null()).output()
    })
    .map_err(|e| format!("No se pudo ejecutar {}: {e}", agent.label()))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || text.is_empty() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("{} no respondió{}", agent.label(), if err.is_empty() { ".".into() } else { format!(": {err}") }));
    }
    Ok((agent, text))
}

fn context() -> String {
    let os = if cfg!(windows) { "Windows" } else if cfg!(target_os = "macos") { "macOS" } else { "Linux" };
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    format!("fysh (una shell propia) en {os}, carpeta actual {cwd}")
}

const FYSH_ABILITIES: &str = "fysh tiene estos comandos propios: cd, dir/ls, mkdir, rm (manda a la papelera), cp, mv, cat, \
touch, open, buscar <texto> [-file|-folder] (busca por nombre en subcarpetas), espacio, extraer, comprimir, renombrar, compartir, programar, echo, set NOMBRE=valor, env, path, alias, z <carpeta>, y git funciona normal. Ejecuta cualquier programa del PATH \
(git, npm, python, etc.). Soporta pipes (|), redirecciones (> >> < 2> 2>&1), && / || / ; y comodines (* ?), \
como bash, además de $(comando) y comando & (segundo plano). Bloques al estilo fish, no bash: \
if cond; …; else; …; end · for x in a b; …; end · while cond; …; end · function f; …; end; condiciones con \
test (-f, -d, =, -eq, -lt…), cuentas con math. No soporta comillas invertidas ni if/then/fi de bash.";

/// Busca en la respuesta la línea "COMANDO: ..." y la devuelve sin ese prefijo.
fn extract_command(answer: &str) -> Option<String> {
    answer
        .lines()
        .find_map(|l| l.trim().trim_start_matches(['*', '`']).strip_prefix("COMANDO:"))
        .map(|c| c.trim_matches(['`', '*', ' ']).to_string())
        .filter(|c| !c.is_empty())
}

/// De una respuesta de `ins:` saca solo el comando: sin bloques ``` ni "$ " delante.
fn clean_command(answer: &str) -> Option<String> {
    let line = answer
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("```"))?;
    let line = line.trim_matches('`').trim();
    let line = line.strip_prefix("$ ").or_else(|| line.strip_prefix("> ")).unwrap_or(line);
    let line = extract_command(line).unwrap_or_else(|| line.to_string());
    (!line.is_empty()).then_some(line)
}

/// `explica`: el asistente explica el último error y, si puede, deja escrito el comando que lo arregla.
pub fn explica_cmd(shell: &mut Shell) -> i32 {
    let Some(fail) = &shell.last_failure else {
        println!("{GRAY}No hay ningún error reciente para explicar.{RESET}");
        return 0;
    };
    let output = fail.output.as_deref().unwrap_or("(no disponible)");
    let prompt = format!(
        "Uso {}. Ejecuté este comando:\n{}\nTerminó con código {}. Esto es lo que se vio en pantalla:\n<salida>\n{}\n</salida>\n\
         Explícame en español, en pocas líneas y sin tecnicismos innecesarios, qué salió mal y cómo arreglarlo.\n\
         Si un comando concreto lo arregla, escríbelo al final en una sola línea que empiece exactamente con \"COMANDO: \".\n\
         {FYSH_ABILITIES}\nNo ejecutes nada ni modifiques archivos: solo responde.",
        context(),
        fail.command,
        fail.code,
        output
    );
    println!("{GRAY}Error de: {}{RESET}", fail.command);
    match ask_agent(&prompt) {
        Ok((_, answer)) => {
            let command = extract_command(&answer);
            for line in answer.lines().filter(|l| !l.trim().starts_with("COMANDO:")) {
                println!("{line}");
            }
            if let Some(cmd) = command {
                println!("\n{GRAY}Comando sugerido escrito en la línea: revísalo y presiona Enter (o bórralo).{RESET}");
                shell.pending = Some(cmd);
            }
            0
        }
        Err(e) => {
            eprintln!("{RED}{e}{RESET}");
            1
        }
    }
}

/// `ins: <tarea>`: el asistente propone el comando y queda escrito en la línea.
pub fn ins_cmd(shell: &mut Shell, task: &str) -> i32 {
    let task = task.trim();
    if task.is_empty() {
        eprintln!("{GRAY}uso: ins: <lo que quieres hacer>   ej: ins: borrar la rama remota feature{RESET}");
        return 1;
    }
    let prompt = format!(
        "Uso {}. Necesito el comando para: {task}\n{FYSH_ABILITIES}\n\
         Responde SOLO con el comando, en una sola línea, sin explicación y sin bloques de código. \
         Si hacen falta varios pasos, responde solo el primero. No ejecutes nada.",
        context()
    );
    match ask_agent(&prompt) {
        Ok((_, answer)) => match clean_command(&answer) {
            Some(cmd) => {
                println!("{GRAY}Comando sugerido escrito en la línea: revísalo y presiona Enter (o bórralo).{RESET}");
                shell.pending = Some(cmd);
                0
            }
            None => {
                println!("{answer}");
                1
            }
        },
        Err(e) => {
            eprintln!("{RED}{e}{RESET}");
            1
        }
    }
}

// ---------- leer lo que se ve en la consola (Windows) ----------

/// Fila del cursor en la consola, para saber dónde empieza la salida de un comando.
#[cfg(windows)]
pub fn console_row() -> Option<i16> {
    use windows_sys::Win32::System::Console::{GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE};
    unsafe {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) != 0).then_some(info.dwCursorPosition.Y)
    }
}

/// Texto de la consola desde la fila `start` hasta el cursor (como mucho las últimas 80 filas).
/// Si la pantalla se desplazó tanto que `start` ya no está, toma lo que se ve.
#[cfg(windows)]
pub fn console_text_since(start: i16) -> Option<String> {
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, ReadConsoleOutputCharacterW, CONSOLE_SCREEN_BUFFER_INFO, COORD,
        STD_OUTPUT_HANDLE,
    };
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut info) == 0 {
            return None;
        }
        let end = info.dwCursorPosition.Y;
        let first = if start <= end { start } else { 0 }.max(end - 80).max(0);
        let width = info.dwSize.X.max(1) as usize;
        let mut lines = Vec::new();
        for row in first..=end {
            let mut buf = vec![0u16; width];
            let mut read = 0u32;
            ReadConsoleOutputCharacterW(handle, buf.as_mut_ptr(), width as u32, COORD { X: 0, Y: row }, &mut read);
            lines.push(String::from_utf16_lossy(&buf[..read as usize]).trim_end().to_string());
        }
        let text = lines.join("\n").trim().to_string();
        let chars: Vec<char> = text.chars().collect();
        let text = if chars.len() > 4000 { chars[chars.len() - 4000..].iter().collect() } else { text };
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(not(windows))]
pub fn console_row() -> Option<i16> {
    None
}

#[cfg(not(windows))]
pub fn console_text_since(_start: i16) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saca_el_comando_sugerido() {
        let answer = "La rama no existe en el remoto.\nPrimero trae los cambios.\nCOMANDO: git fetch --prune";
        assert_eq!(extract_command(answer).as_deref(), Some("git fetch --prune"));
        assert_eq!(extract_command("**COMANDO: `npm install`**").as_deref(), Some("npm install"));
        assert_eq!(extract_command("sin comando"), None);
    }

    #[test]
    fn limpia_la_respuesta_de_ins() {
        assert_eq!(clean_command("```powershell\ngit push origin --delete feature\n```").as_deref(), Some("git push origin --delete feature"));
        assert_eq!(clean_command("$ git status").as_deref(), Some("git status"));
        assert_eq!(clean_command("`dir`").as_deref(), Some("dir"));
        assert_eq!(clean_command("   \n"), None);
    }
}
