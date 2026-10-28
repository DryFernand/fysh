//! Variables de entorno permanentes. `env` lista todas y permite crear, editar o borrar una;
//! `path` edita la lista de carpetas del PATH. En Windows los cambios se guardan en el registro
//! (igual que el diálogo "Variables de entorno"), se avisa al sistema y se aplican a esta sesión.

#[cfg(windows)]
pub use imp::{env_cmd, path_cmd};

#[cfg(not(windows))]
pub fn env_cmd() -> i32 {
    eprintln!("env: por ahora solo en Windows. Para esta sesión usa: set NOMBRE=valor");
    1
}

#[cfg(not(windows))]
pub fn path_cmd() -> i32 {
    env_cmd()
}

/// Reemplaza %NOMBRE% por el valor de la variable (como hace Windows). Las desconocidas quedan igual.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out += &rest[..start];
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match std::env::var(&after[..end]) {
                Ok(value) => {
                    out += &value;
                    rest = &after[end + 1..];
                }
                Err(_) => {
                    out += &rest[start..start + end + 2];
                    rest = &after[end + 1..];
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out + rest
}

/// "a;b;;c" → ["a", "b", "c"]
#[cfg_attr(not(windows), allow(dead_code))]
fn split_list(value: &str) -> Vec<String> {
    value.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

#[cfg(windows)]
mod imp {
    use super::{expand, split_list};
    use crate::picker;
    use crate::table::{self, truncate, Align};
    use crate::{BOLD, GRAY, GREEN, RED, RESET, YELLOW};
    use rustyline::DefaultEditor;
    use std::io::{self, IsTerminal, Write};
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ};
    use winreg::{RegKey, RegValue, HKEY};

    #[derive(Clone, Copy, PartialEq, Debug)]
    pub(super) enum Scope {
        User,
        System,
    }

    impl Scope {
        fn name(self) -> &'static str {
            match self {
                Scope::User => "usuario",
                Scope::System => "sistema",
            }
        }
    }

    #[derive(Clone, Debug)]
    pub(super) struct Var {
        pub name: String,
        pub value: String,
        pub scope: Scope,
        /// REG_EXPAND_SZ: el valor tiene %VARIABLES% que Windows expande (como en PATH).
        pub expand: bool,
    }

    /// Dónde viven las variables. `real()` es el registro de Windows; los tests usan una clave aparte.
    pub(super) struct Store {
        user: (HKEY, &'static str),
        system: (HKEY, &'static str),
        /// Si es la real: respaldar, avisar a Windows y actualizar esta sesión en cada cambio.
        live: bool,
    }

    impl Store {
        fn real() -> Self {
            Store {
                user: (HKEY_CURRENT_USER, "Environment"),
                system: (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
                live: true,
            }
        }

        fn key(&self, scope: Scope, write: bool) -> io::Result<RegKey> {
            let (root, path) = match scope {
                Scope::User => self.user,
                Scope::System => self.system,
            };
            let flags = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
            RegKey::predef(root).open_subkey_with_flags(path, flags)
        }

        /// Todas las variables de texto, primero las de usuario, ordenadas por nombre.
        pub(super) fn vars(&self) -> Vec<Var> {
            let mut out = Vec::new();
            for scope in [Scope::User, Scope::System] {
                let Ok(key) = self.key(scope, false) else { continue };
                for (name, raw) in key.enum_values().filter_map(Result::ok) {
                    if raw.vtype == REG_SZ || raw.vtype == REG_EXPAND_SZ {
                        out.push(Var { value: decode(&raw.bytes), expand: raw.vtype == REG_EXPAND_SZ, name, scope });
                    }
                }
            }
            out.sort_by_key(|v| (v.scope != Scope::User, v.name.to_lowercase()));
            out
        }

        pub(super) fn get(&self, scope: Scope, name: &str) -> Option<Var> {
            let raw = self.key(scope, false).ok()?.get_raw_value(name).ok()?;
            Some(Var { name: name.into(), value: decode(&raw.bytes), scope, expand: raw.vtype == REG_EXPAND_SZ })
        }

        pub(super) fn set(&self, var: &Var) -> io::Result<()> {
            self.backup(var.scope, &var.name);
            let vtype = if var.expand { REG_EXPAND_SZ } else { REG_SZ };
            self.key(var.scope, true)?.set_raw_value(&var.name, &RegValue { bytes: encode(&var.value), vtype })?;
            self.changed(&var.name);
            Ok(())
        }

        pub(super) fn remove(&self, scope: Scope, name: &str) -> io::Result<()> {
            self.backup(scope, name);
            self.key(scope, true)?.delete_value(name)?;
            self.changed(name);
            Ok(())
        }

        /// Guarda el valor anterior en ~/.fysh/respaldo-variables.txt antes de cambiarlo.
        fn backup(&self, scope: Scope, name: &str) {
            if !self.live {
                return;
            }
            let Some(home) = std::env::var_os("USERPROFILE") else { return };
            let dir = std::path::PathBuf::from(home).join(".fysh");
            let old = self.get(scope, name).map_or("(no existía)".into(), |v| v.value);
            let line = format!("{}\t{}\t{}\t{}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), scope.name(), name, old);
            let _ = std::fs::create_dir_all(&dir).and_then(|_| {
                std::fs::OpenOptions::new().create(true).append(true).open(dir.join("respaldo-variables.txt"))?.write_all(line.as_bytes())
            });
        }

        /// Avisa a Windows (las ventanas nuevas toman el cambio) y actualiza esta sesión de fysh.
        fn changed(&self, name: &str) {
            if !self.live {
                return;
            }
            broadcast();
            if name.eq_ignore_ascii_case("path") {
                // Igual que Windows: primero las carpetas del sistema, después las del usuario.
                let part = |s| self.get(s, "Path").map(|v| expand(&v.value)).unwrap_or_default();
                std::env::set_var("PATH", format!("{};{}", part(Scope::System), part(Scope::User)));
            } else {
                match self.get(Scope::User, name).or_else(|| self.get(Scope::System, name)) {
                    Some(v) => std::env::set_var(name, expand(&v.value)),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn decode(bytes: &[u8]) -> String {
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string()
    }

    fn encode(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    /// WM_SETTINGCHANGE "Environment": Explorer y Windows Terminal recargan las variables.
    fn broadcast() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };
        let text: Vec<u16> = "Environment\0".encode_utf16().collect();
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                text.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                3000,
                std::ptr::null_mut(),
            );
        }
    }

    fn report(result: io::Result<()>, ok: &str) -> i32 {
        match result {
            Ok(()) => {
                println!("{GREEN}✓ {ok}{RESET} {GRAY}(las ventanas nuevas ya lo verán; esta sesión también){RESET}");
                0
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("{RED}Acceso denegado.{RESET} Para cambiar variables del sistema abre fysh como administrador.");
                1
            }
            Err(e) => {
                eprintln!("{RED}No se pudo guardar:{RESET} {e}");
                1
            }
        }
    }

    // ---------- preguntas en la terminal ----------

    /// Pide un texto ya escrito con `initial` para editarlo. None si se cancela (Ctrl+C / vacío).
    fn ask(prompt: &str, initial: &str) -> Option<String> {
        let mut ed = DefaultEditor::new().ok()?;
        let text = ed.readline_with_initial(prompt, (initial, "")).ok()?;
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    fn confirm(question: &str) -> bool {
        ask(&format!("{question} [s/N] "), "").is_some_and(|a| a.eq_ignore_ascii_case("s") || a.eq_ignore_ascii_case("si"))
    }

    fn pick_scope() -> Option<Scope> {
        let rows = vec![
            vec!["Usuario".into(), format!("{GRAY}solo tu cuenta (recomendado){RESET}")],
            vec!["Sistema".into(), format!("{GRAY}todas las cuentas · requiere fysh como administrador{RESET}")],
        ];
        match picker::select_one(&["Guardar en", ""], &rows)? {
            0 => Some(Scope::User),
            _ => Some(Scope::System),
        }
    }

    fn scope_cell(scope: Scope) -> String {
        match scope {
            Scope::User => format!("{GREEN}usuario{RESET}"),
            Scope::System => format!("{YELLOW}sistema{RESET}"),
        }
    }

    // ---------- env ----------

    /// `env`: tabla de todas las variables permanentes; elegir una para editarla o crear otra.
    pub fn env_cmd() -> i32 {
        let store = Store::real();
        let vars = store.vars();
        let mut rows = vec![vec!["+ Nueva variable".to_string(), String::new(), String::new()]];
        rows.extend(vars.iter().map(|v| vec![v.name.clone(), scope_cell(v.scope), truncate(&v.value, 50)]));

        if !io::stdin().is_terminal() {
            table::print(&["Variable", "Ámbito", "Valor"], &[Align::Left; 3], &rows[1..]);
            return 0;
        }
        match picker::select_one(&["Variable", "Ámbito", "Valor"], &rows) {
            Some(0) => new_var(&store),
            Some(i) => var_menu(&store, &vars[i - 1]),
            None => 0,
        }
    }

    fn new_var(store: &Store) -> i32 {
        let Some(name) = ask("Nombre de la variable: ", "") else { return 0 };
        if name.contains(['=', ' ']) {
            eprintln!("El nombre no puede tener espacios ni '='.");
            return 1;
        }
        let Some(scope) = pick_scope() else { return 0 };
        let current = store.get(scope, &name);
        if current.is_some() {
            println!("{GRAY}{name} ya existe en {}: edita su valor.{RESET}", scope.name());
        }
        let initial = current.as_ref().map_or("", |v| v.value.as_str());
        let Some(value) = ask(&format!("Valor de {name}: "), initial) else { return 0 };
        let expand = value.contains('%') || current.is_some_and(|v| v.expand);
        report(store.set(&Var { name: name.clone(), value, scope, expand }), &format!("{name} guardada ({})", scope.name()))
    }

    fn var_menu(store: &Store, var: &Var) -> i32 {
        println!("{BOLD}{RED}{}{RESET} {GRAY}({}){RESET}", var.name, var.scope.name());
        let is_list = var.value.contains(';');
        if is_list {
            for item in split_list(&var.value) {
                println!("  {GRAY}•{RESET} {item}");
            }
        } else {
            println!("  {}", var.value);
        }

        let mut actions = vec!["Editar valor"];
        if is_list {
            actions.push("Editar como lista (una carpeta por fila)");
        }
        actions.push("Eliminar variable");
        let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
        let Some(i) = picker::select_one(&["¿Qué quieres hacer?"], &rows) else { return 0 };
        match actions[i] {
            "Editar valor" => {
                let Some(value) = ask(&format!("{}: ", var.name), &var.value) else { return 0 };
                if value == var.value {
                    println!("{GRAY}Sin cambios.{RESET}");
                    return 0;
                }
                let expand = var.expand || value.contains('%');
                report(store.set(&Var { value, expand, ..var.clone() }), &format!("{} actualizada", var.name))
            }
            "Eliminar variable" => {
                if !confirm(&format!("¿Eliminar {} ({})? Queda copia en ~\\.fysh\\respaldo-variables.txt.", var.name, var.scope.name())) {
                    return 0;
                }
                report(store.remove(var.scope, &var.name), &format!("{} eliminada", var.name))
            }
            _ => list_editor(store, &var.name),
        }
    }

    // ---------- path ----------

    /// `path`: carpetas del PATH (sistema y usuario) para agregar, editar, quitar u ordenar.
    pub fn path_cmd() -> i32 {
        list_editor(&Store::real(), "Path")
    }

    /// Editor de una variable tipo lista (PATH, PATHEXT...). Repite hasta que se presiona Esc.
    fn list_editor(store: &Store, name: &str) -> i32 {
        let mut code = 0;
        loop {
            // Mismo orden que usa Windows: sistema primero, después usuario.
            let mut entries: Vec<(Scope, usize, String)> = Vec::new();
            for scope in [Scope::System, Scope::User] {
                if let Some(v) = store.get(scope, name) {
                    entries.extend(split_list(&v.value).into_iter().enumerate().map(|(i, e)| (scope, i, e)));
                }
            }
            let mut rows = vec![vec!["+ Agregar carpeta".to_string(), String::new(), String::new()]];
            rows.extend(entries.iter().map(|(scope, _, entry)| {
                let state = if std::path::Path::new(&expand(entry)).is_dir() {
                    format!("{GREEN}✓{RESET}")
                } else {
                    format!("{RED}✗ no existe{RESET}")
                };
                vec![entry.clone(), scope_cell(*scope), state]
            }));

            if !io::stdin().is_terminal() {
                table::print(&["Carpeta", "Ámbito", "Estado"], &[Align::Left; 3], &rows[1..]);
                return 0;
            }
            println!("{GRAY}{name}: Enter sobre una carpeta para editarla · Esc para salir{RESET}");
            let Some(i) = picker::select_one(&["Carpeta", "Ámbito", "Estado"], &rows) else { return code };
            code = if i == 0 {
                add_entry(store, name)
            } else {
                let (scope, index, _) = &entries[i - 1];
                entry_menu(store, name, *scope, *index)
            };
        }
    }

    /// Cambia la lista de `name` en `scope` con `change` y la guarda.
    fn update_list(store: &Store, name: &str, scope: Scope, change: impl FnOnce(&mut Vec<String>)) -> io::Result<()> {
        let var = store.get(scope, name).unwrap_or(Var { name: name.into(), value: String::new(), scope, expand: true });
        let mut list = split_list(&var.value);
        change(&mut list);
        let value = list.join(";");
        let expand = var.expand || value.contains('%');
        store.set(&Var { value, expand, ..var })
    }

    fn add_entry(store: &Store, name: &str) -> i32 {
        let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
        let Some(folder) = ask("Carpeta a agregar: ", &cwd) else { return 0 };
        if !std::path::Path::new(&expand(&folder)).is_dir() && !confirm("Esa carpeta no existe. ¿Agregarla igual?") {
            return 0;
        }
        let Some(scope) = pick_scope() else { return 0 };
        let exists = store
            .get(scope, name)
            .is_some_and(|v| split_list(&v.value).iter().any(|e| e.eq_ignore_ascii_case(&folder)));
        if exists {
            println!("{GRAY}Esa carpeta ya está en el {name} de {}.{RESET}", scope.name());
            return 0;
        }
        report(update_list(store, name, scope, |l| l.push(folder.clone())), &format!("{folder} agregada"))
    }

    fn entry_menu(store: &Store, name: &str, scope: Scope, index: usize) -> i32 {
        let actions = ["Editar ruta", "Quitar del PATH", "Subir (más prioridad)", "Bajar (menos prioridad)"];
        let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
        let Some(i) = picker::select_one(&["¿Qué quieres hacer?"], &rows) else { return 0 };
        let Some(var) = store.get(scope, name) else { return 1 };
        let list = split_list(&var.value);
        let Some(entry) = list.get(index).cloned() else { return 1 };
        match i {
            0 => {
                let Some(new) = ask("Ruta: ", &entry) else { return 0 };
                if new == entry {
                    return 0;
                }
                report(update_list(store, name, scope, |l| l[index] = new.clone()), &format!("Ruta cambiada a {new}"))
            }
            1 => {
                if !confirm(&format!("¿Quitar {entry} del {name} de {}?", scope.name())) {
                    return 0;
                }
                report(update_list(store, name, scope, |l| { l.remove(index); }), &format!("{entry} quitada"))
            }
            2 if index > 0 => report(update_list(store, name, scope, |l| l.swap(index, index - 1)), "Subida"),
            3 if index + 1 < list.len() => report(update_list(store, name, scope, |l| l.swap(index, index + 1)), "Bajada"),
            _ => {
                println!("{GRAY}Ya está en el extremo de la lista de {}.{RESET}", scope.name());
                0
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Registro de prueba aparte (HKCU\Software\fysh-test): no toca tus variables reales.
        fn test_store() -> Store {
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            hkcu.create_subkey(r"Software\fysh-test\User").unwrap();
            hkcu.create_subkey(r"Software\fysh-test\System").unwrap();
            Store { user: (HKEY_CURRENT_USER, r"Software\fysh-test\User"), system: (HKEY_CURRENT_USER, r"Software\fysh-test\System"), live: false }
        }

        #[test]
        fn guarda_lee_edita_y_borra_variables() {
            let store = test_store();
            let var = Var { name: "Path".into(), value: r"%USERPROFILE%\bin;C:\tools".into(), scope: Scope::User, expand: true };
            store.set(&var).unwrap();
            store.set(&Var { name: "EDITOR".into(), value: "code".into(), scope: Scope::System, expand: false }).unwrap();

            let got = store.get(Scope::User, "Path").unwrap();
            assert_eq!(got.value, var.value);
            assert!(got.expand, "debe conservar REG_EXPAND_SZ");
            assert!(!store.get(Scope::System, "EDITOR").unwrap().expand);
            assert_eq!(store.vars().iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["Path", "EDITOR"]);

            update_list(&store, "Path", Scope::User, |l| l.swap(0, 1)).unwrap();
            assert_eq!(store.get(Scope::User, "Path").unwrap().value, r"C:\tools;%USERPROFILE%\bin");

            store.remove(Scope::System, "EDITOR").unwrap();
            assert!(store.get(Scope::System, "EDITOR").is_none());

            RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(r"Software\fysh-test").unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expande_variables_como_windows() {
        std::env::set_var("FYSH_T_HOME", r"C:\Users\x");
        assert_eq!(expand(r"%FYSH_T_HOME%\bin"), r"C:\Users\x\bin");
        assert_eq!(expand("%NO_EXISTE_FYSH%\\a"), "%NO_EXISTE_FYSH%\\a");
        assert_eq!(expand("100% seguro"), "100% seguro");
    }

    #[test]
    fn separa_listas() {
        assert_eq!(split_list(r"C:\a; C:\b;;"), [r"C:\a", r"C:\b"]);
        assert!(split_list("").is_empty());
    }
}
