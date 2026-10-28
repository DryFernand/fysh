//! Notificación del sistema cuando termina un comando largo (más de 30 s, o lo que diga
//! FYSH_AVISO_SEGUNDOS; 0 las apaga) y no estás mirando la ventana de fysh.
//! Ej: "✓ npm run build terminó · 2m 05s".

use std::time::Duration;

const DEFAULT_SECS: u64 = 30;

fn threshold() -> Option<Duration> {
    let secs = std::env::var("FYSH_AVISO_SEGUNDOS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Comandos interactivos: tardan porque estabas usándolos, no porque estuvieran trabajando.
fn interactive(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    ["ssh", "claude", "agy", "antigravity", "opencode", "db", "docker", "vim", "nvim", "nano", "less", "more", "python", "node", "config", "history"]
        .contains(&first)
}

/// Llamar después de cada comando.
pub fn after_command(line: &str, status: i32, took: Duration) {
    let Some(limit) = threshold() else { return };
    if took < limit || interactive(line) || window_focused() {
        return;
    }
    let (icon, verb) = if status == 0 { ("✓", "terminó") } else { ("✗", "falló") };
    let command = crate::table::truncate(line.trim(), 60);
    let body = format!("{command}\n{verb} · {}", crate::format_duration(took));
    // En otro hilo: mostrar la notificación nunca debe frenar el prompt.
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().appname("fysh").summary(&format!("{icon} fysh: comando {verb}")).body(&body).show();
    });
}

/// ¿La ventana de fysh es la que está al frente? En Windows Terminal la ventana es de
/// WindowsTerminal.exe (un proceso "padre" de fysh), así que se compara con los procesos de arriba.
#[cfg(windows)]
fn window_focused() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        if fg == GetConsoleWindow() {
            return true; // consola clásica (conhost)
        }
        let mut fg_pid = 0u32;
        GetWindowThreadProcessId(fg, &mut fg_pid);
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
        let mut pid = sysinfo::Pid::from_u32(std::process::id());
        for _ in 0..8 {
            let Some(parent) = sys.process(pid).and_then(|p| p.parent()) else { break };
            if parent.as_u32() == fg_pid {
                return true;
            }
            pid = parent;
        }
        false
    }
}

/// En Linux/Mac no se puede saber de forma fiable: siempre avisa.
#[cfg(not(windows))]
fn window_focused() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Muestra una notificación real: `cargo test -- --ignored notificacion_real`
    #[test]
    #[ignore]
    fn notificacion_real() {
        notify_rust::Notification::new()
            .appname("fysh")
            .summary("fysh: prueba de notificación")
            .body("npm run build\nterminó · 2m 05s")
            .show()
            .expect("el sistema debe aceptar la notificación");
    }

    #[test]
    fn no_avisa_de_comandos_interactivos() {
        assert!(interactive("ssh Prueba") && interactive("claude") && interactive("  db crm"));
        assert!(!interactive("npm run build") && !interactive("cargo test"));
    }
}
